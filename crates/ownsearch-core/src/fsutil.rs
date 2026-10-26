use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

const LOCK_WAIT: Duration = Duration::from_secs(2);

pub(crate) fn create_private_dir(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|error| Error::State(format!("cannot create {}: {error}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn open_private(path: &Path, append: bool) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    if append {
        options.append(true);
    } else {
        options.truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| Error::State(format!("cannot open {}: {error}", path.display())))
}

pub fn write_atomic(path: &Path, payload: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::State(format!("{} has no file name", path.display())))?;
    let stamp = format!("{}-{}", std::process::id(), crate::time::now_unix());
    let temporary = parent.join(format!(".{name}.{stamp}.tmp"));
    {
        use std::io::Write as _;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|error| {
            Error::State(format!("cannot write {}: {error}", temporary.display()))
        })?;
        file.write_all(payload)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        Error::State(format!("cannot put {} in place: {error}", path.display()))
    })?;
    sync_directory(parent);
    Ok(())
}

pub(crate) fn append_line(path: &Path, line: &str) -> Result<()> {
    with_file_lock(path, || {
        use std::io::Write as _;
        let mut file = open_private(path, true)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        Ok(())
    })
}

pub(crate) fn trim_lines(path: &Path, keep: impl Fn(&str) -> bool) -> Result<usize> {
    with_file_lock(path, || {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => {
                return Err(Error::State(format!(
                    "cannot read {}: {error}",
                    path.display()
                )));
            }
        };
        let mut kept: Vec<&str> = Vec::new();
        let mut dropped = 0usize;
        for line in raw.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if keep(line) {
                kept.push(line);
            } else {
                dropped = dropped.saturating_add(1);
            }
        }
        if dropped == 0 {
            return Ok(0);
        }
        let mut body = kept.join("\n");
        body.push('\n');
        write_atomic(path, body.as_bytes())?;
        Ok(dropped)
    })
}

pub(crate) fn try_file_lock(path: &Path) -> Result<Option<std::fs::File>> {
    let lock = path.with_extension("lock");
    let file = open_private(&lock, false)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(Error::State(format!(
            "cannot lock {}: {error}",
            lock.display()
        ))),
    }
}

pub(crate) fn with_file_lock<T>(path: &Path, work: impl FnOnce() -> Result<T>) -> Result<T> {
    let lock = path.with_extension("lock");
    let file = open_private(&lock, false)?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(_) if started.elapsed() < LOCK_WAIT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(Error::State(format!(
                    "another process holds {}: {error}",
                    lock.display()
                )));
            }
        }
    }
    let value = work();
    let _ = file.unlock();
    value
}

pub(crate) fn sync_directory(dir: &Path) {
    #[cfg(unix)]
    {
        if let Ok(handle) = std::fs::File::open(dir) {
            let _ = handle.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_atomic_write_replaces_the_target() {
        let dir = std::env::temp_dir().join(format!("ownsearch-fs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("snapshot.json");
        write_atomic(&path, b"{\"a\":1}").expect("write");
        write_atomic(&path, b"{\"a\":2}").expect("rewrite");
        let body = std::fs::read_to_string(&path).expect("read");
        assert_eq!(body, "{\"a\":2}");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("list")
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_appended_line_ends_with_a_newline() {
        let dir = std::env::temp_dir().join(format!("ownsearch-fs-line-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("events.jsonl");
        append_line(&path, "{\"a\":1}").expect("append");
        append_line(&path, "{\"a\":2}").expect("append");
        let body = std::fs::read_to_string(&path).expect("read");
        assert_eq!(body, "{\"a\":1}\n{\"a\":2}\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_lock_reports_busy_and_dropping_the_first_frees_it() {
        let dir = std::env::temp_dir().join(format!("ownsearch-fs-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("scan");
        let first = try_file_lock(&path)
            .expect("first lock")
            .expect("first lock is acquired");
        assert!(
            try_file_lock(&path).expect("second lock").is_none(),
            "a held lock must be reported as busy"
        );
        drop(first);
        assert!(
            try_file_lock(&path).expect("third lock").is_some(),
            "dropping the holder must free the lock"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
