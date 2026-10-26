use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::config::Settings;
use crate::error::Result;
use crate::fsutil;
use crate::ledger::Ledger;

#[derive(Debug, Clone, Default)]
pub struct CleanupReport {
    pub ledger_rotated: bool,
    pub ledger_archives_removed: usize,
    pub pre_migration_removed: usize,
    pub events_trimmed: usize,
    pub warnings: Vec<String>,
}

pub fn clean(settings: &Settings, ledger: &Ledger, key: &[u8]) -> Result<CleanupReport> {
    let keep = settings.keep_window_secs().max(1);
    let now = crate::time::now_unix();
    let cutoff_unix = now.saturating_sub(i64::try_from(keep).unwrap_or(i64::MAX));
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(keep))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut report = CleanupReport {
        ledger_rotated: ledger.rotate_if_older_than(cutoff_unix, key)?,
        ..CleanupReport::default()
    };

    remove_old_files(
        settings.ledger_dir(),
        "calls-",
        ".jsonl",
        cutoff,
        &mut report.ledger_archives_removed,
        &mut report.warnings,
    );
    remove_old_files(
        &settings.backups_dir(),
        "instances-",
        ".db",
        cutoff,
        &mut report.pre_migration_removed,
        &mut report.warnings,
    );
    remove_old_files(
        settings.state_dir(),
        "pre-migration-",
        ".db",
        cutoff,
        &mut report.pre_migration_removed,
        &mut report.warnings,
    );
    trim_events(&settings.events_path(), cutoff_unix, &mut report)?;
    Ok(report)
}

fn remove_old_files(
    dir: &Path,
    prefix: &str,
    suffix: &str,
    cutoff: SystemTime,
    removed: &mut usize,
    warnings: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            warnings.push(format!(
                "cannot read {} to clean old files: {error}; run `ownsearch db maintenance`",
                dir.display()
            ));
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(prefix) || !name.ends_with(suffix) {
            continue;
        }
        let path = entry.path();
        let modified = match entry.metadata().and_then(|meta| meta.modified()) {
            Ok(modified) => modified,
            Err(error) => {
                warnings.push(format!(
                    "cannot read the age of {}: {error}",
                    path.display()
                ));
                continue;
            }
        };
        if modified >= cutoff {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => *removed = removed.saturating_add(1),
            Err(error) => warnings.push(format!(
                "cannot remove the expired file {}: {error}; run `ownsearch db maintenance`",
                path.display()
            )),
        }
    }
}

const MAX_EVENTS_FILE_BYTES: u64 = 64 * 1024 * 1024;

fn trim_events(path: &Path, cutoff_unix: i64, report: &mut CleanupReport) -> Result<()> {
    if let Ok(meta) = std::fs::metadata(path)
        && meta.len() > MAX_EVENTS_FILE_BYTES
    {
        report.warnings.push(format!(
            "{} is larger than {} bytes; it was left untouched. Archive or shorten it by hand.",
            path.display(),
            MAX_EVENTS_FILE_BYTES
        ));
        return Ok(());
    }
    match fsutil::trim_lines(path, |line| {
        event_timestamp(line.trim()).is_none_or(|stamp| stamp >= cutoff_unix)
    }) {
        Ok(trimmed) => {
            report.events_trimmed = trimmed;
        }
        Err(error) => report
            .warnings
            .push(format!("cannot trim {}: {error}", path.display())),
    }
    Ok(())
}

fn event_timestamp(line: &str) -> Option<i64> {
    let entry: Value = serde_json::from_str(line).ok()?;
    for key in ["at", "occurredAt", "generatedAt"] {
        if let Some(stamp) = entry.get(key).and_then(Value::as_str)
            && let Some(unix) = crate::time::parse_rfc3339(stamp)
        {
            return Some(unix);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{CallRecord, Ledger};

    const KEY: [u8; 32] = [4u8; 32];

    fn settings(dir: &Path) -> Settings {
        let map: std::collections::BTreeMap<String, String> = [
            ("HOME".to_owned(), "/home/tester".to_owned()),
            ("OWNSEARCH_STATE_DIR".to_owned(), dir.display().to_string()),
            (
                "OWNSEARCH_LEDGER_DIR".to_owned(),
                dir.join("ledger").display().to_string(),
            ),
        ]
        .into_iter()
        .collect();
        Settings::load_with(
            crate::config::Overrides {
                state_dir: Some(dir.to_path_buf()),
                ledger_dir: Some(dir.join("ledger")),
                ..crate::config::Overrides::default()
            },
            |name| map.get(name).cloned(),
        )
        .expect("settings")
    }

    fn record() -> CallRecord {
        CallRecord {
            tool: "ownsearch_search".to_owned(),
            operation: "search".to_owned(),
            status: "ok".to_owned(),
            duration_ms: 1,
            instances: Vec::new(),
            parser: None,
            http_class: None,
            bytes: 0,
            result_count: 0,
            cache_state: "miss".to_owned(),
            args_hash: "x".to_owned(),
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ownsearch-cleanup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn an_expired_archive_and_a_pre_migration_backup_are_removed() {
        let dir = temp_dir("files");
        let settings = settings(&dir);
        let ledger = Ledger::open(settings.ledger_path(), &KEY).expect("ledger");
        let archive = settings
            .ledger_dir()
            .join("calls-2000-01-01_00-00-00-000Z.jsonl");
        std::fs::write(&archive, "{}\n").expect("archive");
        let backup = settings
            .state_dir()
            .join("pre-migration-2000-01-01_00-00-00-000Z.db");
        std::fs::write(&backup, "x").expect("backup");
        let old = SystemTime::now() - Duration::from_secs(30 * 86_400);
        set_modified(&archive, old);
        set_modified(&backup, old);
        let report = clean(&settings, &ledger, &KEY).expect("clean");
        assert_eq!(report.ledger_archives_removed, 1);
        assert_eq!(report.pre_migration_removed, 1);
        assert!(!archive.exists());
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fresh_archive_is_kept() {
        let dir = temp_dir("fresh");
        let settings = settings(&dir);
        let ledger = Ledger::open(settings.ledger_path(), &KEY).expect("ledger");
        let archive = settings
            .ledger_dir()
            .join("calls-2000-01-01_00-00-00-000Z.jsonl");
        std::fs::write(&archive, "{}\n").expect("archive");
        let report = clean(&settings, &ledger, &KEY).expect("clean");
        assert_eq!(report.ledger_archives_removed, 0);
        assert!(archive.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_event_lines_are_trimmed_and_new_ones_kept() {
        let dir = temp_dir("events");
        let settings = settings(&dir);
        let ledger = Ledger::open(settings.ledger_path(), &KEY).expect("ledger");
        let events = settings.events_path();
        std::fs::write(
            &events,
            "{\"event\":\"discovery_run\",\"at\":\"2000-01-01T00:00:00.000Z\"}\n\
             {\"event\":\"discovery_run\",\"at\":\"2026-10-26T00:00:00.000Z\"}\n",
        )
        .expect("events");
        let report = clean(&settings, &ledger, &KEY).expect("clean");
        assert_eq!(report.events_trimmed, 1);
        let body = std::fs::read_to_string(&events).expect("read");
        assert!(!body.contains("2000-01-01"));
        assert!(body.contains("2026-10-26"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_expired_live_ledger_is_rotated_and_the_next_row_lands_in_the_new_file() {
        let dir = temp_dir("rotate");
        let settings = settings(&dir);
        let ledger = Ledger::open(settings.ledger_path(), &KEY).expect("ledger");
        ledger.append(record(), &KEY).expect("append");
        let rotated = ledger
            .rotate_if_older_than(crate::time::now_unix().saturating_add(1), &KEY)
            .expect("rotate");
        assert!(rotated);
        ledger.append(record(), &KEY).expect("append new");
        let body = std::fs::read_to_string(settings.ledger_path()).expect("read");
        assert_eq!(body.lines().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn set_modified(path: &Path, when: SystemTime) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open");
        file.set_modified(when).expect("mtime");
    }
}
