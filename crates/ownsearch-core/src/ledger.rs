use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{Error, Result};

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const LOCK_WAIT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CallRecord {
    pub tool: String,
    pub operation: String,
    pub status: String,
    pub duration_ms: u64,
    #[serde(default)]
    pub instances: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parser: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_class: Option<String>,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub result_count: u64,
    #[serde(default)]
    pub cache_state: String,
    pub args_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LedgerRow {
    pub seq: u64,
    pub ts_unix: i64,
    pub tool: String,
    pub operation: String,
    pub status: String,
    pub duration_ms: u64,
    #[serde(default)]
    pub instances: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parser: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_class: Option<String>,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub result_count: u64,
    #[serde(default)]
    pub cache_state: String,
    pub args_hash: String,
    pub prev: String,
    pub hash: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Totals {
    pub calls: u64,
    pub errors: u64,
    pub cache_hits: u64,
    pub per_tool: BTreeMap<String, u64>,
    pub per_tool_errors: BTreeMap<String, u64>,
}

pub struct Ledger {
    path: PathBuf,
    state: Mutex<State>,
}

struct State {
    file: File,
    offset: u64,
    seq: u64,
    head: String,
    oldest_ts: i64,
    totals: Totals,
}

impl std::fmt::Debug for Ledger {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Ledger")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Ledger {
    pub fn open(path: impl Into<PathBuf>, key: &[u8]) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            create_private_dir(parent)?;
        }
        let mut file = open_append(&path)?;
        let (mut valid_len, seq, head, oldest_ts, totals) = scan(&mut file, &path, key)?;
        let length = file.metadata()?.len();
        if length > valid_len {
            file.set_len(valid_len)?;
            tracing::warn!(
                path = %path.display(),
                removed = length.saturating_sub(valid_len),
                "truncated a partial final ledger line before appending"
            );
        }
        if valid_len > 0 && !ends_with_newline(&mut file, valid_len)? {
            file.seek(SeekFrom::Start(valid_len))?;
            file.write_all(b"\n")?;
            file.sync_data()?;
            valid_len = valid_len.saturating_add(1);
        }
        file.seek(SeekFrom::Start(valid_len))?;
        Ok(Self {
            path,
            state: Mutex::new(State {
                file,
                offset: valid_len,
                seq,
                head,
                oldest_ts,
                totals,
            }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn rotate_if_older_than(&self, cutoff_unix: i64, key: &[u8]) -> Result<bool> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::Ledger("the ledger lock was poisoned".into()))?;
        if state.oldest_ts == 0 || state.oldest_ts >= cutoff_unix {
            return Ok(false);
        }
        catch_up(&mut state, &self.path, key)?;
        state.file.sync_all()?;
        let stamp = crate::time::rfc3339(crate::time::now_unix())
            .replace([':', '.'], "-")
            .replace('T', "_");
        let archive = self.path.with_file_name(format!("calls-{stamp}.jsonl"));
        std::fs::rename(&self.path, &archive)
            .map_err(|error| Error::Ledger(format!("cannot archive the ledger: {error}")))?;
        let file = open_append(&self.path)?;
        state.file = file;
        state.offset = 0;
        state.seq = 0;
        state.head = GENESIS.to_owned();
        state.oldest_ts = 0;
        Ok(true)
    }

    pub(crate) fn totals(&self) -> Totals {
        match self.state.lock() {
            Ok(state) => state.totals.clone(),
            Err(poisoned) => poisoned.into_inner().totals.clone(),
        }
    }

    pub fn head(&self) -> (u64, String) {
        match self.state.lock() {
            Ok(state) => (state.seq, state.head.clone()),
            Err(poisoned) => {
                let state = poisoned.into_inner();
                (state.seq, state.head.clone())
            }
        }
    }

    pub(crate) fn append(&self, record: CallRecord, key: &[u8]) -> Result<LedgerRow> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::Ledger("the ledger lock was poisoned".into()))?;
        catch_up(&mut state, &self.path, key)?;
        let row = LedgerRow {
            seq: state.seq.saturating_add(1),
            ts_unix: crate::time::now_unix(),
            tool: record.tool,
            operation: record.operation,
            status: record.status,
            duration_ms: record.duration_ms,
            instances: record.instances,
            parser: record.parser,
            http_class: record.http_class,
            bytes: record.bytes,
            result_count: record.result_count,
            cache_state: record.cache_state,
            args_hash: record.args_hash,
            prev: state.head.clone(),
            hash: String::new(),
        };
        let row = sign(row, key)?;
        let mut line = serde_json::to_vec(&row)?;
        line.push(b'\n');
        state
            .file
            .lock()
            .map_err(|error| Error::Ledger(format!("cannot lock the ledger: {error}")))?;
        let written = state
            .file
            .write_all(&line)
            .and_then(|()| state.file.sync_data());
        let _ = state.file.unlock();
        written.map_err(|error| Error::Ledger(format!("cannot write the ledger row: {error}")))?;
        state.offset = state.offset.saturating_add(line.len() as u64);
        if state.oldest_ts == 0 {
            state.oldest_ts = row.ts_unix;
        }
        apply_totals(&mut state.totals, &row);
        if state.oldest_ts == 0 {
            state.oldest_ts = row.ts_unix;
        }
        state.seq = row.seq;
        state.head = row.hash.clone();
        Ok(row)
    }
}

fn catch_up(state: &mut State, path: &Path, key: &[u8]) -> Result<()> {
    let length = std::fs::metadata(path)?.len();
    if length <= state.offset {
        return Ok(());
    }
    let mut file = open_read(path)?;
    file.seek(SeekFrom::Start(state.offset))?;
    let mut raw = String::new();
    file.read_to_string(&mut raw)?;
    for chunk in raw.split_inclusive('\n') {
        if !chunk.ends_with('\n') {
            state.file.set_len(state.offset)?;
            state.file.seek(SeekFrom::Start(state.offset))?;
            break;
        }
        let trimmed = chunk.trim();
        if trimmed.is_empty() {
            state.offset = state.offset.saturating_add(chunk.len() as u64);
            continue;
        }
        let row: LedgerRow = serde_json::from_str(trimmed).map_err(|error| {
            Error::Ledger(format!("a concurrent ledger row does not parse: {error}"))
        })?;
        validate(&row, key)?;
        apply_totals(&mut state.totals, &row);
        if state.oldest_ts == 0 {
            state.oldest_ts = row.ts_unix;
        }
        state.seq = row.seq;
        state.head = row.hash.clone();
        state.offset = state.offset.saturating_add(chunk.len() as u64);
    }
    Ok(())
}

fn apply_totals(totals: &mut Totals, row: &LedgerRow) {
    totals.calls = totals.calls.saturating_add(1);
    let failed = row.status.starts_with("error") || row.status == "refused";
    if failed {
        totals.errors = totals.errors.saturating_add(1);
    }
    if row.cache_state == "hit" {
        totals.cache_hits = totals.cache_hits.saturating_add(1);
    }
    let calls = totals.per_tool.entry(row.tool.clone()).or_insert(0);
    *calls = calls.saturating_add(1);
    if failed {
        let errors = totals.per_tool_errors.entry(row.tool.clone()).or_insert(0);
        *errors = errors.saturating_add(1);
    }
}

fn sign(mut row: LedgerRow, key: &[u8]) -> Result<LedgerRow> {
    let payload = serde_json::to_vec(&RowPayload {
        seq: row.seq,
        ts_unix: row.ts_unix,
        tool: &row.tool,
        operation: &row.operation,
        status: &row.status,
        duration_ms: row.duration_ms,
        instances: &row.instances,
        parser: row.parser.as_deref(),
        http_class: row.http_class.as_deref(),
        bytes: row.bytes,
        result_count: row.result_count,
        cache_state: &row.cache_state,
        args_hash: &row.args_hash,
        prev: &row.prev,
    })?;
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key)
        .map_err(|_| Error::Ledger("the arguments key is not usable for signing".into()))?;
    mac.update(&payload);
    row.hash = hex::encode(mac.finalize().into_bytes());
    Ok(row)
}

#[derive(Serialize)]
struct RowPayload<'a> {
    seq: u64,
    ts_unix: i64,
    tool: &'a str,
    operation: &'a str,
    status: &'a str,
    duration_ms: u64,
    instances: &'a [i64],
    parser: Option<&'a str>,
    http_class: Option<&'a str>,
    bytes: u64,
    result_count: u64,
    cache_state: &'a str,
    args_hash: &'a str,
    prev: &'a str,
}

fn validate(row: &LedgerRow, key: &[u8]) -> Result<()> {
    let expected = sign(
        LedgerRow {
            hash: String::new(),
            ..row.clone()
        },
        key,
    )?;
    if expected.hash != row.hash {
        return Err(Error::Ledger(format!(
            "row {} does not match its own hash",
            row.seq
        )));
    }
    Ok(())
}

fn scan(file: &mut File, path: &Path, key: &[u8]) -> Result<(u64, u64, String, i64, Totals)> {
    file.seek(SeekFrom::Start(0))?;
    let mut raw = String::new();
    file.read_to_string(&mut raw)?;
    let mut seq = 0u64;
    let mut head = GENESIS.to_owned();
    let mut oldest_ts = 0i64;
    let mut totals = Totals::default();
    let mut valid_len = 0u64;
    for line in raw.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let complete = line.ends_with('\n');
        let row: LedgerRow = match serde_json::from_str(trimmed) {
            Ok(row) => row,
            Err(_) if !complete => break,
            Err(error) => {
                return Err(Error::Ledger(format!(
                    "{} holds a row that does not parse at byte {valid_len}: {error}",
                    path.display()
                )));
            }
        };
        if row.prev != head {
            return Err(Error::Ledger(format!(
                "the hash chain breaks at row {} in {}",
                row.seq,
                path.display()
            )));
        }
        validate(&row, key)?;
        if row.seq != seq.saturating_add(1) {
            return Err(Error::Ledger(format!(
                "row {} follows row {} in {}",
                row.seq,
                seq,
                path.display()
            )));
        }
        if oldest_ts == 0 {
            oldest_ts = row.ts_unix;
        }
        seq = row.seq;
        head = row.hash.clone();
        apply_totals(&mut totals, &row);
        valid_len = valid_len.saturating_add(line.len() as u64);
    }
    Ok((valid_len, seq, head, oldest_ts, totals))
}

pub fn verify(path: &Path, key: &[u8]) -> Result<(u64, String)> {
    let mut file = open_read(path)?;
    let (_, seq, head, _, _) = scan(&mut file, path, key)?;
    Ok((seq, head))
}

pub fn existing_key(path: &Path) -> Option<[u8; 32]> {
    let bytes = std::fs::read(path).ok()?;
    <[u8; 32]>::try_from(bytes.as_slice()).ok()
}

pub fn rotate(path: &Path, key: &[u8]) -> Result<(u64, PathBuf)> {
    let (entries, _) = verify(path, key)?;
    let stamp = crate::time::now_rfc3339()
        .replace([':', '.'], "-")
        .replace('T', "_");
    let archive = path.with_file_name(format!("calls-{stamp}.jsonl"));
    std::fs::rename(path, &archive)
        .map_err(|error| Error::Ledger(format!("cannot archive the ledger: {error}")))?;
    Ok((entries, archive))
}

pub(crate) fn args_hash(key: &[u8], value: &serde_json::Value) -> String {
    let mut mac = match <Hmac<Sha256> as KeyInit>::new_from_slice(key) {
        Ok(mac) => mac,
        Err(_) => return String::from("key-error"),
    };
    mac.update(value.to_string().as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn load_or_create_key(path: &Path) -> Result<[u8; 32]> {
    match std::fs::read(path) {
        Ok(bytes) => <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| {
            Error::Ledger(format!(
                "{} is not the 32-byte arguments key; restore it or remove it on purpose to start a new ledger chain",
                path.display()
            ))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut key = [0u8; 32];
            getrandom::fill(&mut key)
                .map_err(|error| Error::Ledger(format!("os random: {error}")))?;
            if let Some(parent) = path.parent() {
                create_private_dir(parent)?;
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(path) {
                Ok(mut file) => {
                    file.write_all(&key)?;
                    file.sync_all()?;
                    Ok(key)
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let bytes = std::fs::read(path)?;
                    <[u8; 32]>::try_from(bytes.as_slice())
                        .map_err(|_| Error::Ledger("the arguments key changed size".into()))
                }
                Err(error) => Err(Error::Ledger(format!(
                    "cannot create {}: {error}",
                    path.display()
                ))),
            }
        }
        Err(error) => Err(Error::Ledger(format!("cannot read {}: {error}", path.display()))),
    }
}

fn ends_with_newline(file: &mut File, length: u64) -> Result<bool> {
    if length == 0 {
        return Ok(true);
    }
    file.seek(SeekFrom::Start(length.saturating_sub(1)))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] == b'\n')
}

fn open_append(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .map_err(|error| Error::Ledger(format!("cannot open {}: {error}", path.display())))?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(_) if started.elapsed() < LOCK_WAIT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(Error::Ledger(format!(
                    "another ownsearch process holds {}: {error}",
                    path.display()
                )));
            }
        }
    }
    let _ = file.unlock();
    Ok(file)
}

fn open_read(path: &Path) -> Result<File> {
    File::open(path)
        .map_err(|error| Error::Ledger(format!("cannot read {}: {error}", path.display())))
}

fn create_private_dir(path: &Path) -> Result<()> {
    let existed = path.exists();
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|error| Error::Ledger(format!("cannot create {}: {error}", path.display())))?;
    if !existed {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(tool: &str) -> CallRecord {
        CallRecord {
            tool: tool.to_owned(),
            operation: "search".to_owned(),
            status: "ok".to_owned(),
            duration_ms: 12,
            instances: vec![1, 2],
            parser: Some("json".to_owned()),
            http_class: Some("2xx".to_owned()),
            bytes: 100,
            result_count: 3,
            cache_state: "miss".to_owned(),
            args_hash: "abc".to_owned(),
        }
    }

    const KEY: [u8; 32] = [9u8; 32];

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ownsearch-ledger-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn appended_rows_form_a_verifiable_chain() {
        let dir = temp_dir("chain");
        let path = dir.join("calls.jsonl");
        let ledger = Ledger::open(&path, &KEY).expect("ledger");
        ledger
            .append(record("ownsearch_search"), &KEY)
            .expect("append");
        ledger
            .append(record("ownsearch_scan"), &KEY)
            .expect("append");
        let (entries, head) = verify(&path, &KEY).expect("verify");
        assert_eq!(entries, 2);
        assert_eq!(head, ledger.head().1);
        assert_eq!(ledger.totals().calls, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tampered_row_breaks_verification() {
        let dir = temp_dir("tamper");
        let path = dir.join("calls.jsonl");
        let ledger = Ledger::open(&path, &KEY).expect("ledger");
        ledger
            .append(record("ownsearch_search"), &KEY)
            .expect("append");
        let raw = std::fs::read_to_string(&path).expect("read");
        let tampered = raw.replace("\"bytes\":100", "\"bytes\":999");
        std::fs::write(&path, tampered).expect("write");
        assert!(verify(&path, &KEY).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_partial_final_line_is_removed_on_open() {
        let dir = temp_dir("partial");
        let path = dir.join("calls.jsonl");
        let ledger = Ledger::open(&path, &KEY).expect("ledger");
        ledger
            .append(record("ownsearch_search"), &KEY)
            .expect("append");
        let mut raw = std::fs::read_to_string(&path).expect("read");
        raw.push_str("{\"seq\":2");
        std::fs::write(&path, raw).expect("write");
        let reopened = Ledger::open(&path, &KEY).expect("reopen");
        let totals = reopened.totals();
        assert_eq!(totals.calls, 1);
        let body = std::fs::read_to_string(&path).expect("read");
        assert!(body.ends_with('\n'));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_partial_concurrent_line_is_truncated_before_the_next_append() {
        let dir = temp_dir("catch-up");
        let path = dir.join("calls.jsonl");
        let ledger = Ledger::open(&path, &KEY).expect("ledger");
        ledger
            .append(record("ownsearch_search"), &KEY)
            .expect("append");
        {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open");
            file.write_all(b"{\"seq\":2").expect("partial");
        }
        ledger
            .append(record("ownsearch_scan"), &KEY)
            .expect("append after partial");
        let (entries, _) = verify(&path, &KEY).expect("verify");
        assert_eq!(entries, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_archives_the_chain() {
        let dir = temp_dir("rotate");
        let path = dir.join("calls.jsonl");
        let ledger = Ledger::open(&path, &KEY).expect("ledger");
        ledger
            .append(record("ownsearch_search"), &KEY)
            .expect("append");
        let (entries, archive) = rotate(&path, &KEY).expect("rotate");
        assert_eq!(entries, 1);
        assert!(archive.exists());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_arguments_hash_hides_the_query() {
        let key = [7u8; 32];
        let first = args_hash(&key, &json!({"query": "confidential"}));
        let second = args_hash(&key, &json!({"query": "confidential"}));
        let other = args_hash(&key, &json!({"query": "different"}));
        assert_eq!(first, second);
        assert_ne!(first, other);
        assert!(!first.contains("confidential"));
    }

    #[test]
    fn the_key_file_is_created_once() {
        let dir = temp_dir("key");
        let path = dir.join("args.key");
        let first = load_or_create_key(&path).expect("key");
        let second = load_or_create_key(&path).expect("key");
        assert_eq!(first, second);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
