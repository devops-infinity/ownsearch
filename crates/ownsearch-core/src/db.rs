use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::instances::DiscoveredInstance;
use crate::time::now_rfc3339;
use crate::types::{
    InstanceStatus, InstanceType, ProbeKind, ProbeOutcome, QueryEntity, normalize_url, stable_id,
};

pub(crate) const APPLICATION_ID: u32 = 1_330_794_835;
pub(crate) const SCORE_VERSION: &str = "ownsearch-score-1";
pub(crate) const SOURCE_ID_SEARX_SPACE: &str = "searx_space";
pub(crate) const SOURCE_ID_BOOTSTRAP: &str = "bootstrap";

const MIGRATIONS: &[(u32, &str, &str)] = &[
    (1, "initial", include_str!("db/migrations/0001_initial.sql")),
    (
        2,
        "index_expiry_reads_and_drop_unread_views",
        include_str!("db/migrations/0002_index_expiry_reads_and_drop_unread_views.sql"),
    ),
    (
        3,
        "drop_unused_hourly_probe_metrics",
        include_str!("db/migrations/0003_drop_unused_hourly_probe_metrics.sql"),
    ),
    (
        4,
        "restore_probe_metrics_read_path",
        include_str!("db/migrations/0004_restore_probe_metrics_read_path.sql"),
    ),
    (
        5,
        "trust_priority_engine_stats_and_auth",
        include_str!("db/migrations/0005_trust_priority_engine_stats_and_auth.sql"),
    ),
];

const STALE_RUN_SECS: i64 = 3_600;
const MAX_EVENT_OFFSET: usize = 50_000;
const MAX_FLEET_SNAPSHOTS: i64 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorAction {
    Deny,
    Allow,
    Retire,
    Restore,
    Reset,
}

#[derive(Debug, Clone, Default)]
pub struct InstanceFilter {
    pub status: Option<InstanceStatus>,
    pub kind: Option<InstanceType>,
    pub min_score: Option<f64>,
    pub instance_id: Option<i64>,
    pub ids: Vec<i64>,
    pub url: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone)]
pub struct InstanceRow {
    pub instance_id: i64,
    pub slug: String,
    pub url: String,
    pub host: String,
    pub kind: InstanceType,
    pub status: InstanceStatus,
    pub version: Option<String>,
    pub score: Option<f64>,
    pub rank: Option<i64>,
    pub hard_gate_passed: Option<bool>,
    pub reasons: Vec<String>,
    pub json_enabled: Option<bool>,
    pub html_enabled: Option<bool>,
    pub google_exact: bool,
    pub google_cse: bool,
    pub consecutive_successes: i64,
    pub consecutive_failures: i64,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub next_scan_at: Option<String>,
    pub cooldown_until: Option<String>,
    pub scan_enabled: bool,
    pub html_fallback_allowed: bool,
    pub discovery_rank: Option<i64>,
    pub network_group: Option<String>,
    pub pinned: bool,
    pub denied: bool,
    pub trusted: bool,
    pub source: String,
    pub json_throttled_until: Option<String>,
    pub manual_priority: i64,
    pub added_at: String,
}

pub fn serving_state(row: &InstanceRow, now: &str) -> (bool, Option<String>) {
    if row.status != InstanceStatus::Active {
        return (false, Some(format!("status is {}", row.status.as_str())));
    }
    if row.hard_gate_passed != Some(true) {
        return (false, Some("the hard gate has not passed".to_owned()));
    }
    if row.denied {
        return (false, Some("denied by an operator override".to_owned()));
    }
    if !row.scan_enabled {
        return (
            false,
            Some("scanning is disabled, so the instance leaves the fleet".to_owned()),
        );
    }
    if row.kind == InstanceType::Tor {
        return (false, Some("a Tor instance is never served".to_owned()));
    }
    if let Some(until) = row.cooldown_until.as_deref()
        && until > now
    {
        return (false, Some(format!("cooling down until {until}")));
    }
    (true, None)
}

#[derive(Debug, Clone)]
pub struct FleetRow {
    pub instance_id: i64,
    pub url: String,
    pub score: f64,
    pub rank: i64,
    pub json_enabled: bool,
    pub html_enabled: bool,
    pub google_exact: bool,
    pub google_cse: bool,
    pub network_group: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScanTarget {
    pub instance_id: i64,
    pub url: String,
    pub status: InstanceStatus,
    pub kind: InstanceType,
    pub html_fallback_allowed: bool,
    pub consecutive_successes: i64,
    pub consecutive_failures: i64,
    pub trusted: bool,
    pub json_throttled_until: Option<String>,
    pub source: String,
}

struct Upserted {
    instance_id: i64,
    is_new: bool,
    changed: bool,
}

#[derive(Debug, Clone)]
pub struct ProbeRecord {
    pub instance_id: i64,
    pub run_id: Option<String>,
    pub kind: ProbeKind,
    pub outcome: ProbeOutcome,
    pub started_at: String,
    pub completed_at: String,
    pub duration_ms: i64,
    pub http_status: Option<u16>,
    pub content_type: Option<String>,
    pub result_count: Option<i64>,
    pub response_bytes: Option<i64>,
    pub response_digest_sha256: Option<Vec<u8>>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TlsRecord {
    pub instance_id: i64,
    pub probe_id: i64,
    pub protocol: Option<String>,
    pub issuer: Option<String>,
    pub not_before: Option<i64>,
    pub not_after: Option<i64>,
    pub hostname_verified: bool,
    pub chain_valid: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CapabilityRecord {
    pub instance_id: i64,
    pub run_id: Option<String>,
    pub outcome: ProbeOutcome,
    pub http_status: Option<u16>,
    pub duration_ms: Option<i64>,
    pub response_digest_sha256: Option<Vec<u8>>,
    pub instance_name: Option<String>,
    pub categories: Vec<String>,
    pub engines: Vec<String>,
    pub locales: Vec<String>,
    pub plugins: Vec<String>,
    pub safe_search_default: Option<u8>,
    pub json_enabled: bool,
    pub csv_enabled: bool,
    pub rss_enabled: bool,
    pub html_enabled: bool,
    pub google_exact_seen: bool,
    pub google_cse_seen: bool,
}

#[derive(Debug, Clone)]
pub struct EventRecord {
    pub kind: String,
    pub instance_id: Option<i64>,
    pub run_id: Option<String>,
    pub probe_id: Option<i64>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub reasons: Vec<String>,
    pub payload: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct ScanUpdate {
    pub instance_id: i64,
    pub status: InstanceStatus,
    pub previous_status: InstanceStatus,
    pub successes: i64,
    pub failures: i64,
    pub score: f64,
    pub rank: Option<i64>,
    pub hard_gate_passed: bool,
    pub reasons: Vec<String>,
    pub next_scan_at: Option<String>,
    pub cooldown_until: Option<String>,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub version: Option<String>,
    pub capability_id: Option<i64>,
    pub tls_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct EngineStat {
    pub engine: String,
    pub successes: i64,
    pub failures: i64,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct StatusCounts {
    pub by_status: Vec<(String, i64)>,
    pub by_type: Vec<(String, i64)>,
    pub total: i64,
    pub active: i64,
    pub probes: i64,
    pub events: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct KeepWindow {
    pub probe_days: i64,
    pub tls_days: i64,
    pub capability_days: i64,
    pub metrics_days: i64,
    pub snapshot_days: i64,
    pub event_days: i64,
    pub run_days: i64,
}

impl Default for KeepWindow {
    fn default() -> Self {
        Self {
            probe_days: 90,
            tls_days: 90,
            capability_days: 90,
            metrics_days: 180,
            snapshot_days: 30,
            event_days: 365,
            run_days: 180,
        }
    }
}

#[derive(Debug)]
pub struct Store {
    writer: Mutex<Connection>,
    reader: Mutex<Connection>,
    path: PathBuf,
}

fn configure(connection: &Connection) -> Result<()> {
    connection.busy_timeout(std::time::Duration::from_millis(5_000))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

fn enable_wal(connection: &Connection) -> Result<()> {
    let mode: String = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(Error::State(format!(
            "journal mode is {mode}, not WAL; the state directory may be on a filesystem that cannot hold the sidecars"
        )));
    }
    Ok(())
}

fn ensure_migration_table(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version     INTEGER PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            checksum    TEXT NOT NULL,
            applied_at  TEXT NOT NULL
        ) STRICT;",
    )?;
    Ok(())
}

fn checksum(sql: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(sql.as_bytes()))
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            crate::fsutil::create_private_dir(parent)?;
        }
        let writer = Connection::open(path)?;
        configure(&writer)?;
        verify_owner(&writer, path)?;
        enable_wal(&writer)?;
        writer.pragma_update(None, "application_id", APPLICATION_ID)?;
        lock_down_file(path)?;
        let mut store = Self {
            writer: Mutex::new(writer),
            reader: Mutex::new(Connection::open(path)?),
            path: path.to_path_buf(),
        };
        store.migrate()?;
        {
            let reader = store.reader_conn()?;
            configure(&reader)?;
            reader.pragma_update(None, "query_only", "ON")?;
        }
        store
            .writer_conn()?
            .execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let mut tag = [0u8; 8];
        getrandom::fill(&mut tag).map_err(|error| Error::State(format!("os random: {error}")))?;
        let uri = format!(
            "file:ownsearch-{}-{}?mode=memory&cache=shared",
            std::process::id(),
            hex::encode(tag)
        );
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_URI
            | rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE;
        let writer = Connection::open_with_flags(&uri, flags)?;
        configure(&writer)?;
        let reader = Connection::open_with_flags(&uri, flags)?;
        configure(&reader)?;
        let mut store = Self {
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
            path: PathBuf::from(":memory:"),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn warm_up(&self) -> Result<(usize, usize)> {
        if self.bootstrap_imported()? || self.instance_count()? > 0 {
            return Ok((0, 0));
        }
        let snapshot = crate::instances::BootstrapSnapshot::embedded()?;
        self.import_bootstrap(&snapshot, crate::time::now_unix())
    }

    fn writer_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.writer
            .lock()
            .map_err(|_| Error::State("the writer connection lock was poisoned".into()))
    }

    pub(crate) fn write_atomic<T>(&self, work: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let mut connection = self.writer_conn()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = work(&transaction)?;
        transaction.commit()?;
        Ok(value)
    }

    fn reader_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.reader
            .lock()
            .map_err(|_| Error::State("the reader connection lock was poisoned".into()))
    }

    pub fn migrate(&mut self) -> Result<u32> {
        let connection = self.writer_conn()?;
        let preexisting: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'
                AND name NOT LIKE 'sqlite_%' AND name <> 'schema_migrations'",
            [],
            |row| row.get(0),
        )?;
        ensure_migration_table(&connection)?;
        let has_pending = MIGRATIONS.iter().any(|(version, _, sql)| {
            match connection
                .query_row(
                    "SELECT checksum FROM schema_migrations WHERE version = ?1",
                    params![version],
                    |row| row.get::<_, String>(0),
                )
                .optional()
            {
                Ok(Some(existing)) => existing != checksum(sql),
                _ => true,
            }
        });
        if has_pending && preexisting > 0 {
            backup_before_migration(&connection, &self.path)?;
        }
        let mut highest = 0;
        for (version, name, sql) in MIGRATIONS {
            let applied: Option<String> = connection
                .query_row(
                    "SELECT checksum FROM schema_migrations WHERE version = ?1",
                    params![version],
                    |row| row.get(0),
                )
                .optional()?;
            let digest = checksum(sql);
            match applied {
                Some(existing) if existing == digest => {
                    highest = *version;
                }
                Some(_) => {
                    return Err(Error::StateCorrupted(format!(
                        "migration {version} ({name}) was applied with a different checksum; the schema file changed after it ran"
                    )));
                }
                None => {
                    let transaction = rusqlite::Transaction::new_unchecked(
                        &connection,
                        TransactionBehavior::Immediate,
                    )?;
                    transaction.execute_batch(sql)?;
                    transaction.execute(
                        "INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?1, ?2, ?3, ?4)",
                        params![version, name, digest, now_rfc3339()],
                    )?;
                    transaction.pragma_update(None, "user_version", *version)?;
                    {
                        let mut statement = transaction.prepare("PRAGMA foreign_key_check")?;
                        let mut rows = statement.query([])?;
                        if rows.next()?.is_some() {
                            return Err(Error::StateCorrupted(format!(
                                "migration {version} ({name}) left a foreign key violation"
                            )));
                        }
                    }
                    transaction.commit()?;
                    highest = *version;
                }
            }
        }
        let user_version: i64 =
            connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if user_version > i64::from(highest) {
            return Err(Error::StateNewer {
                found: user_version,
                supported: highest,
            });
        }
        Ok(highest)
    }

    pub fn schema_version(&self) -> Result<i64> {
        let reader = self.reader_conn()?;
        let version: i64 = reader.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        Ok(version)
    }

    pub fn ping(&self) -> Result<()> {
        let reader = self.reader_conn()?;
        let value: i64 = reader.query_row("SELECT 1", [], |row| row.get(0))?;
        if value == 1 {
            Ok(())
        } else {
            Err(Error::StateCorrupted(format!(
                "the store answered {value} to a liveness read"
            )))
        }
    }

    pub fn quick_check(&self) -> Result<()> {
        let reader = self.reader_conn()?;
        let result: String = reader.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if result.eq_ignore_ascii_case("ok") {
            Ok(())
        } else {
            Err(Error::StateCorrupted(result))
        }
    }

    pub fn integrity_check(&self) -> Result<()> {
        let reader = self.reader_conn()?;
        let result: String = reader.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if result.eq_ignore_ascii_case("ok") {
            Ok(())
        } else {
            Err(Error::StateCorrupted(result))
        }
    }

    pub fn foreign_key_check(&self) -> Result<()> {
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare("PRAGMA foreign_key_check")?;
        let mut rows = statement.query([])?;
        if rows.next()?.is_some() {
            return Err(Error::StateCorrupted(
                "a foreign key row points at a missing parent".into(),
            ));
        }
        Ok(())
    }

    pub fn ensure_source(&self, source_id: &str, name: &str, endpoint: &str) -> Result<()> {
        self.ensure_source_with_kind(source_id, "searx_space", name, endpoint)
    }

    fn ensure_source_with_kind(
        &self,
        source_id: &str,
        kind: &str,
        name: &str,
        endpoint: &str,
    ) -> Result<()> {
        let connection = self.writer_conn()?;
        connection.execute(
            "INSERT INTO discovery_sources (source_id, kind, name, endpoint, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(source_id) DO UPDATE SET name = excluded.name, endpoint = excluded.endpoint",
            params![source_id, kind, name, endpoint, now_rfc3339()],
        )?;
        Ok(())
    }

    pub fn begin_run(&self, source_id: &str, scanner_version: &str) -> Result<String> {
        let now = now_rfc3339();
        let stale = crate::time::rfc3339(crate::time::now_unix().saturating_sub(STALE_RUN_SECS));
        let run_id = format!("run-{}-{}", crate::time::now_unix(), stable_id(&now));
        let inserted = run_id.clone();
        self.write_atomic(move |connection| {
            connection.execute(
                "UPDATE discovery_runs SET outcome = 'error', completed_at = ?2,
                    error_code = 'ownsearch.stale_run',
                    error_message = 'the process stopped before this run finished'
                  WHERE outcome = 'running' AND started_at < ?1",
                params![stale, now],
            )?;
            connection.execute(
                "INSERT INTO discovery_runs (run_id, discovery_source_id, started_at, outcome, scanner_version)
                 VALUES (?1, ?2, ?3, 'running', ?4)",
                params![inserted, source_id, now, scanner_version],
            )?;
            Ok(())
        })?;
        Ok(run_id)
    }

    pub fn finish_run(&self, run_id: &str, outcome: &str, counts: (u32, u32, u32)) -> Result<()> {
        let connection = self.writer_conn()?;
        let changed = connection.execute(
            "UPDATE discovery_runs SET completed_at = ?2, outcome = ?3, discovered_count = ?4,
                changed_count = ?5, missing_count = ?6 WHERE run_id = ?1",
            params![run_id, now_rfc3339(), outcome, counts.0, counts.1, counts.2],
        )?;
        if changed == 0 {
            return Err(Error::State(format!(
                "discovery run {run_id} is not in the state database"
            )));
        }
        Ok(())
    }

    pub fn record_run_failure(&self, run_id: &str, code: &str, message: &str) -> Result<()> {
        let connection = self.writer_conn()?;
        connection.execute(
            "UPDATE discovery_runs SET error_code = ?2, error_message = ?3 WHERE run_id = ?1",
            params![run_id, code, crate::sanitize::cap(message, 500).0],
        )?;
        Ok(())
    }

    pub fn record_run_source(
        &self,
        run_id: &str,
        snapshot_at: Option<i64>,
        digest: &[u8],
    ) -> Result<()> {
        let connection = self.writer_conn()?;
        let stamp = snapshot_at.map(crate::time::rfc3339);
        connection.execute(
            "UPDATE discovery_runs SET source_published_at = ?2, source_digest_sha256 = ?3 WHERE run_id = ?1",
            params![run_id, stamp, digest],
        )?;
        Ok(())
    }

    pub fn upsert_instance(
        &self,
        record: &DiscoveredInstance,
        source_id: &str,
        run_id: Option<&str>,
        now: &str,
    ) -> Result<(i64, bool)> {
        self.write_atomic(|connection| {
            let outcome = self.upsert_instance_on(connection, record, source_id, run_id, now)?;
            Ok((outcome.instance_id, outcome.is_new))
        })
    }

    pub fn import_discovered(
        &self,
        records: &[crate::instances::DiscoveredInstance],
        source_id: &str,
        run_id: Option<&str>,
        now: &str,
    ) -> Result<(usize, usize, Vec<String>)> {
        self.write_atomic(|connection| {
            let mut added = 0usize;
            let mut changed = 0usize;
            let mut present = Vec::with_capacity(records.len());
            for record in records {
                let outcome =
                    self.upsert_instance_on(connection, record, source_id, run_id, now)?;
                if outcome.is_new {
                    added = added.saturating_add(1);
                } else if outcome.changed {
                    changed = changed.saturating_add(1);
                }
                present.push(record.url.clone());
            }
            Ok((added, changed, present))
        })
    }

    pub fn add_manual_instance(&self, url: &str, pinned: bool, now: &str) -> Result<i64> {
        let normalized = crate::types::normalize_url(url)?;
        let record = crate::instances::DiscoveredInstance {
            url: normalized.clone(),
            kind: crate::types::InstanceType::Normal,
            rank: None,
            version: None,
            tls_grade: None,
            tls_protocol: None,
            certificate_not_after: None,
            certificate_days_remaining: None,
            http_grade: None,
            csp_grade: None,
            html_grade: None,
            analytics: false,
            ipv6: false,
            uptime_month: None,
            uptime_year: None,
            search_success: None,
            search_median_secs: None,
            initial_success: None,
            initial_secs: None,
            google_success: None,
            google_secs: None,
            working_engines: Vec::new(),
            exact_google_capability: false,
            google_cse_capability: false,
            asn_cidr: None,
        };
        self.ensure_source_with_kind("manual", "manual", "manual entry", "manual")?;
        self.import_discovered(std::slice::from_ref(&record), "manual", None, now)?;
        let existing = self
            .find_instance_by_url(&normalized)?
            .ok_or_else(|| Error::State("the manual instance was not stored".into()))?;
        let instance_id = existing.instance_id;
        if existing.denied {
            self.apply_operator_action(
                instance_id,
                OperatorAction::Allow,
                &["operator.add_revived".to_owned()],
            )?;
        } else if matches!(
            existing.status,
            InstanceStatus::Retired | InstanceStatus::Missing
        ) {
            self.apply_operator_action(
                instance_id,
                OperatorAction::Restore,
                &["operator.add_revived".to_owned()],
            )?;
        }
        if pinned {
            self.apply_policy_change(
                instance_id,
                None,
                None,
                Some(&json!({"pinned": true})),
                &["operator.pinned".to_owned()],
            )?;
        }
        Ok(instance_id)
    }

    fn upsert_instance_on(
        &self,
        connection: &Connection,
        record: &DiscoveredInstance,
        source_id: &str,
        run_id: Option<&str>,
        now: &str,
    ) -> Result<Upserted> {
        let digest = {
            use sha2::{Digest, Sha256};
            Sha256::digest(record.url.as_bytes()).to_vec()
        };
        let existing: Option<(i64, String, Option<String>)> = connection
            .query_row(
                "SELECT instance_id, instance_kind, discovery_metrics_json FROM instances WHERE normalized_url = ?1",
                params![record.url],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((instance_id, previous_type, previous_metrics)) = existing {
            let metrics = serde_json::to_string(record)?;
            let changed = previous_type != record.kind.as_str()
                || previous_metrics.as_deref() != Some(metrics.as_str());
            connection.execute(
                "UPDATE instances SET last_announced_at = ?2, updated_at = ?2, discovery_rank = COALESCE(?3, discovery_rank),
                    network_group = COALESCE(?4, network_group), current_discovery_run_id = ?5,
                    searxng_version = COALESCE(?6, searxng_version),
                    searxng_version_observed_at = CASE WHEN ?6 IS NULL THEN searxng_version_observed_at ELSE ?2 END,
                    discovery_metrics_json = ?7
                 WHERE instance_id = ?1",
                params![
                    instance_id,
                    now,
                    record.rank,
                    record.network_group(),
                    run_id,
                    record.version,
                    metrics
                ],
            )?;
            if previous_type != record.kind.as_str() {
                connection.execute(
                    "INSERT INTO instance_events (occurred_at, event_kind, instance_id, discovery_run_id, previous_value, current_value, reason_codes_json)
                     VALUES (?1, 'type_changed', ?2, ?3, ?4, ?5, ?6)",
                    params![
                        now,
                        instance_id,
                        run_id,
                        previous_type,
                        record.kind.as_str(),
                        json!(["type.changed"]).to_string()
                    ],
                )?;
                connection.execute(
                    "UPDATE instances SET instance_kind = ?2 WHERE instance_id = ?1",
                    params![instance_id, record.kind.as_str()],
                )?;
            }
            let stored_status: String = connection.query_row(
                "SELECT lifecycle_status FROM instances WHERE instance_id = ?1",
                params![instance_id],
                |row| row.get(0),
            )?;
            let target = if record.kind == InstanceType::Tor {
                (stored_status != InstanceStatus::Excluded.as_str())
                    .then_some(InstanceStatus::Excluded)
            } else if stored_status == InstanceStatus::Excluded.as_str()
                || stored_status == "missing"
                || stored_status == "unlisted"
            {
                Some(InstanceStatus::Candidate)
            } else {
                None
            };
            if let Some(target) = target {
                connection.execute(
                    "UPDATE instances SET lifecycle_status = ?2, updated_at = ?3 WHERE instance_id = ?1",
                    params![instance_id, target.as_str(), now],
                )?;
                connection.execute(
                    "INSERT INTO instance_events (occurred_at, event_kind, instance_id, discovery_run_id, previous_value, current_value, reason_codes_json)
                     VALUES (?1, 'status_changed', ?2, ?3, ?4, ?5, ?6)",
                    params![
                        now,
                        instance_id,
                        run_id,
                        stored_status,
                        target.as_str(),
                        json!([if record.kind == InstanceType::Tor {
                            "discovery.tor_excluded"
                        } else {
                            "discovery.announced"
                        }])
                        .to_string()
                    ],
                )?;
            }
            Ok(Upserted {
                instance_id,
                is_new: false,
                changed,
            })
        } else {
            let metrics = serde_json::to_string(record)?;
            let slug = unique_slug(connection, &record.url)?;
            let initial_status = if record.kind == InstanceType::Tor {
                InstanceStatus::Excluded
            } else {
                InstanceStatus::Candidate
            };
            connection.execute(
                "INSERT INTO instances (instance_slug, normalized_url, url_digest_sha256, host, instance_kind, lifecycle_status,
                    discovery_source_id, discovered_at, last_announced_at, next_scan_at, discovery_rank, network_group,
                    discovery_metrics_json, current_discovery_run_id, updated_at)
                 VALUES (?11, ?1, ?2, ?3, ?4, ?12, ?5, ?6, ?6, ?6, ?7, ?8, ?9, ?10, ?6)",
                params![
                    record.url,
                    digest,
                    record.host(),
                    record.kind.as_str(),
                    source_id,
                    now,
                    record.rank,
                    record.network_group(),
                    metrics,
                    run_id,
                    slug,
                    initial_status.as_str()
                ],
            )?;
            let instance_id = connection.last_insert_rowid();
            connection.execute(
                "INSERT INTO instance_events (occurred_at, event_kind, instance_id, discovery_run_id, current_value, reason_codes_json)
                 VALUES (?1, 'instance_added', ?2, ?3, ?4, ?5)",
                params![
                    now,
                    instance_id,
                    run_id,
                    initial_status.as_str(),
                    json!([if record.kind == InstanceType::Tor {
                        "discovery.tor_excluded"
                    } else {
                        "discovery.added"
                    }])
                    .to_string()
                ],
            )?;
            Ok(Upserted {
                instance_id,
                is_new: true,
                changed: true,
            })
        }
    }

    pub fn mark_missing(&self, present: &[String], run_id: &str, now: &str) -> Result<u32> {
        let mut connection = self.writer_conn()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut statement = transaction.prepare(
            "SELECT i.instance_id, i.normalized_url, i.lifecycle_status, i.discovery_source_id
               FROM instances AS i
               LEFT JOIN instance_policy_overrides AS o ON o.instance_id = i.instance_id
              WHERE COALESCE(o.is_denied, 0) = 0
                AND i.lifecycle_status NOT IN ('missing', 'unlisted', 'retired', 'excluded', 'quarantined')",
        )?;
        let rows: Vec<(i64, String, String, String)> = statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        let present_urls: std::collections::HashSet<&str> =
            present.iter().map(String::as_str).collect();
        let mut changed = 0;
        for (instance_id, url, status, source_id) in rows {
            if present_urls.contains(url.as_str()) {
                continue;
            }
            let manual = source_id == "manual";
            let target = if manual {
                InstanceStatus::Unlisted
            } else {
                InstanceStatus::Missing
            };
            transaction.execute(
                "UPDATE instances SET lifecycle_status = ?3, updated_at = ?2 WHERE instance_id = ?1",
                params![instance_id, now, target.as_str()],
            )?;
            transaction.execute(
                "INSERT INTO instance_events (occurred_at, event_kind, instance_id, discovery_run_id, previous_value, current_value, reason_codes_json)
                 VALUES (?1, 'status_changed', ?2, ?3, ?4, ?5, ?6)",
                params![
                    now,
                    instance_id,
                    run_id,
                    status,
                    target.as_str(),
                    json!([if manual {
                        "discovery.unlisted"
                    } else {
                        "discovery.missing"
                    }])
                    .to_string()
                ],
            )?;
            changed += 1;
        }
        transaction.commit()?;
        Ok(changed)
    }

    pub fn insert_probe(&self, probe: &ProbeRecord) -> Result<i64> {
        let connection = self.writer_conn()?;
        Self::insert_probe_on(&connection, probe)
    }

    pub(crate) fn insert_probe_on(connection: &Connection, probe: &ProbeRecord) -> Result<i64> {
        connection.execute(
            "INSERT INTO probes (instance_id, discovery_run_id, probe_kind, outcome, attempt, started_at,
                completed_at, duration_ms, http_status, content_type, result_count, response_bytes,
                response_digest_sha256, error_code, error_message)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                probe.instance_id,
                probe.run_id,
                probe.kind.as_str(),
                probe.outcome.as_str(),
                probe.started_at,
                probe.completed_at,
                probe.duration_ms,
                probe.http_status,
                probe.content_type,
                probe.result_count,
                probe.response_bytes,
                probe.response_digest_sha256,
                probe.error_code,
                probe.error_message
            ],
        )?;
        Ok(connection.last_insert_rowid())
    }

    pub fn insert_tls(&self, tls: &TlsRecord) -> Result<i64> {
        self.write_atomic(|connection| Self::insert_tls_on(connection, tls))
    }

    pub(crate) fn insert_tls_on(connection: &Connection, tls: &TlsRecord) -> Result<i64> {
        let not_before = tls.not_before.map(crate::time::rfc3339);
        let not_after = tls.not_after.map(crate::time::rfc3339);
        connection.execute(
            "INSERT INTO tls_observations (instance_id, probe_id, observed_at, protocol, issuer,
                not_before, not_after, is_hostname_verified, is_chain_valid, error_code, error_message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                tls.instance_id,
                tls.probe_id,
                now_rfc3339(),
                tls.protocol,
                tls.issuer,
                not_before,
                not_after,
                i64::from(tls.hostname_verified),
                i64::from(tls.chain_valid),
                tls.error_code,
                tls.error_message
            ],
        )?;
        let tls_id = connection.last_insert_rowid();
        connection.execute(
            "UPDATE instances SET current_tls_observation_id = ?2 WHERE instance_id = ?1",
            params![tls.instance_id, tls_id],
        )?;
        Ok(tls_id)
    }

    pub fn insert_capability(&self, capability: &CapabilityRecord) -> Result<i64> {
        self.write_atomic(|connection| Self::insert_capability_on(connection, capability))
    }

    pub(crate) fn insert_capability_on(
        connection: &Connection,
        capability: &CapabilityRecord,
    ) -> Result<i64> {
        connection.execute(
            "INSERT INTO capability_snapshots (instance_id, observed_at, discovery_run_id, outcome,
                http_status, duration_ms, response_digest_sha256, categories_json, engines_json, locales_json,
                plugins_json, default_safe_search, is_json_enabled, is_csv_enabled, is_rss_enabled, is_html_enabled,
                is_google_exact_seen, is_google_cse_seen, instance_name)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                capability.instance_id,
                now_rfc3339(),
                capability.run_id,
                capability.outcome.as_str(),
                capability.http_status,
                capability.duration_ms,
                capability.response_digest_sha256,
                json!(capability.categories).to_string(),
                json!(capability.engines).to_string(),
                json!(capability.locales).to_string(),
                json!(capability.plugins).to_string(),
                capability.safe_search_default,
                i64::from(capability.json_enabled),
                i64::from(capability.csv_enabled),
                i64::from(capability.rss_enabled),
                i64::from(capability.html_enabled),
                i64::from(capability.google_exact_seen),
                i64::from(capability.google_cse_seen),
                capability.instance_name
            ],
        )?;
        let capability_id = connection.last_insert_rowid();
        for engine in &capability.engines {
            let engine_id = upsert_name(connection, "engines", "engine_id", "engine_name", engine)?;
            connection.execute(
                "INSERT OR IGNORE INTO capability_engines (capability_id, engine_id) VALUES (?1, ?2)",
                params![capability_id, engine_id],
            )?;
        }
        for category in &capability.categories {
            let category_id = upsert_name(
                connection,
                "categories",
                "category_id",
                "category_name",
                category,
            )?;
            connection.execute(
                "INSERT OR IGNORE INTO capability_categories (capability_id, category_id) VALUES (?1, ?2)",
                params![capability_id, category_id],
            )?;
        }
        connection.execute(
            "UPDATE instances SET current_capability_id = ?2 WHERE instance_id = ?1",
            params![capability.instance_id, capability_id],
        )?;
        Ok(capability_id)
    }

    pub fn apply_scan_update(&self, update: &ScanUpdate) -> Result<()> {
        self.write_atomic(|connection| Self::apply_scan_update_on(connection, update))
    }

    pub(crate) fn apply_scan_update_on(connection: &Connection, update: &ScanUpdate) -> Result<()> {
        let now = now_rfc3339();
        let current: String = connection.query_row(
            "SELECT lifecycle_status FROM instances WHERE instance_id = ?1",
            params![update.instance_id],
            |row| row.get(0),
        )?;
        let status = if current == update.previous_status.as_str() {
            update.status.as_str().to_owned()
        } else {
            current.clone()
        };
        connection.execute(
            "UPDATE instances SET lifecycle_status = ?2, consecutive_successes = ?3, consecutive_failures = ?4,
                last_success_at = COALESCE(?5, last_success_at),
                last_failure_at = COALESCE(?6, last_failure_at),
                next_scan_at = ?7, cooldown_until = ?8,
                searxng_version = COALESCE(?9, searxng_version),
                updated_at = ?10
             WHERE instance_id = ?1",
            params![
                update.instance_id,
                status,
                update.successes,
                update.failures,
                update.last_success_at,
                update.last_failure_at,
                update.next_scan_at,
                update.cooldown_until,
                update.version,
                now
            ],
        )?;
        connection.execute(
            "INSERT INTO instance_quality (instance_id, score_model_version, score, fleet_rank, is_hard_gate_passed, reason_codes_json, computed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(instance_id) DO UPDATE SET score_model_version = excluded.score_model_version,
                score = excluded.score, fleet_rank = COALESCE(excluded.fleet_rank, instance_quality.fleet_rank),
                is_hard_gate_passed = excluded.is_hard_gate_passed, reason_codes_json = excluded.reason_codes_json,
                computed_at = excluded.computed_at",
            params![
                update.instance_id,
                SCORE_VERSION,
                update.score,
                update.rank,
                i64::from(update.hard_gate_passed),
                json!(update.reasons).to_string(),
                now
            ],
        )?;
        if status != current {
            connection.execute(
                "INSERT INTO instance_events (occurred_at, event_kind, instance_id, previous_value, current_value, reason_codes_json)
                 VALUES (?1, 'status_changed', ?2, ?3, ?4, ?5)",
                params![
                    now,
                    update.instance_id,
                    current,
                    status,
                    json!(update.reasons).to_string()
                ],
            )?;
        }
        connection.execute(
            "INSERT INTO instance_events (occurred_at, event_kind, instance_id, previous_value, current_value, reason_codes_json)
             VALUES (?1, 'score_updated', ?2, NULL, ?3, ?4)",
            params![
                now,
                update.instance_id,
                format!("{:.2}", update.score),
                json!(update.reasons).to_string()
            ],
        )?;
        if let Some(capability_id) = update.capability_id {
            connection.execute(
                "UPDATE instances SET current_capability_id = ?2 WHERE instance_id = ?1",
                params![update.instance_id, capability_id],
            )?;
        }
        if let Some(tls_id) = update.tls_id {
            connection.execute(
                "UPDATE instances SET current_tls_observation_id = ?2 WHERE instance_id = ?1",
                params![update.instance_id, tls_id],
            )?;
        }
        Ok(())
    }

    pub fn update_fleet_ranks(&self, ranks: &[(i64, i64)]) -> Result<()> {
        let mut connection = self.writer_conn()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("UPDATE instance_quality SET fleet_rank = NULL", [])?;
        for (instance_id, rank) in ranks {
            transaction.execute(
                "UPDATE instance_quality SET fleet_rank = ?2 WHERE instance_id = ?1",
                params![instance_id, rank],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn append_event(&self, event: &EventRecord) -> Result<i64> {
        let connection = self.writer_conn()?;
        Self::append_event_on(&connection, event)
    }

    pub(crate) fn append_event_on(connection: &Connection, event: &EventRecord) -> Result<i64> {
        connection.execute(
            "INSERT INTO instance_events (occurred_at, event_kind, instance_id, discovery_run_id, probe_id,
                previous_value, current_value, reason_codes_json, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                now_rfc3339(),
                event.kind,
                event.instance_id,
                event.run_id,
                event.probe_id,
                event.old_value,
                event.new_value,
                json!(event.reasons).to_string(),
                event.payload.as_ref().map(Value::to_string)
            ],
        )?;
        Ok(connection.last_insert_rowid())
    }

    pub fn discovery_metrics(&self, instance_id: i64) -> Result<Option<DiscoveredInstance>> {
        let reader = self.reader_conn()?;
        let raw: Option<Option<String>> = reader
            .query_row(
                "SELECT discovery_metrics_json FROM instances WHERE instance_id = ?1",
                params![instance_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(raw
            .flatten()
            .and_then(|text| serde_json::from_str(&text).ok()))
    }

    pub fn discovery_metrics_for(
        &self,
        ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, DiscoveredInstance>> {
        let mut out = std::collections::HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT instance_id, discovery_metrics_json FROM instances
              WHERE instance_id IN ({placeholders}) AND discovery_metrics_json IS NOT NULL"
        );
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare(&sql)?;
        let values: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = statement.query_map(values.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (instance_id, raw) = row?;
            match serde_json::from_str(&raw) {
                Ok(metrics) => {
                    out.insert(instance_id, metrics);
                }
                Err(error) => {
                    tracing::warn!(%error, instance_id, "the stored discovery metrics do not parse")
                }
            }
        }
        Ok(out)
    }

    pub fn last_run_at(&self, source_id: &str) -> Result<Option<i64>> {
        let reader = self.reader_conn()?;
        let stamp: Option<String> = reader
            .query_row(
                "SELECT completed_at FROM discovery_runs
                  WHERE discovery_source_id = ?1
                    AND outcome = 'success'
                    AND completed_at IS NOT NULL
                  ORDER BY completed_at DESC LIMIT 1",
                params![source_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(stamp.as_deref().and_then(crate::time::parse_rfc3339))
    }

    pub fn known_urls(&self) -> Result<Vec<String>> {
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare("SELECT normalized_url FROM instances")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let out = collect_rows(rows)?;
        Ok(out)
    }

    pub fn instance_count(&self) -> Result<i64> {
        let reader = self.reader_conn()?;
        let count: i64 =
            reader.query_row("SELECT COUNT(*) FROM instances", [], |row| row.get(0))?;
        Ok(count)
    }

    pub fn bootstrap_imported(&self) -> Result<bool> {
        let reader = self.reader_conn()?;
        let present: i64 = reader.query_row(
            "SELECT COUNT(*) FROM application_metadata WHERE metadata_key = 'bootstrap_imported'",
            [],
            |row| row.get(0),
        )?;
        Ok(present > 0)
    }

    pub fn import_bootstrap(
        &self,
        snapshot: &crate::instances::BootstrapSnapshot,
        now: i64,
    ) -> Result<(usize, usize)> {
        if self.bootstrap_imported()? {
            return Ok((0, snapshot.instances.len()));
        }
        self.ensure_source(
            SOURCE_ID_BOOTSTRAP,
            "SearXNG bootstrap snapshot",
            "embedded:instances.bootstrap.json",
        )?;
        let stamp = now_rfc3339();
        let discovered = snapshot.to_discovered(now);
        let total = discovered.len();
        let mut connection = self.writer_conn()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut added = 0;
        for record in &discovered {
            let outcome =
                self.upsert_instance_on(&transaction, record, SOURCE_ID_BOOTSTRAP, None, &stamp)?;
            if outcome.is_new {
                added += 1;
            }
        }
        transaction.execute(
            "INSERT INTO application_metadata (metadata_key, metadata_value) VALUES ('bootstrap_imported', ?1)
             ON CONFLICT(metadata_key) DO UPDATE SET metadata_value = excluded.metadata_value",
            params![added.to_string()],
        )?;
        transaction.commit()?;
        Ok((added, total))
    }

    pub fn list_instances(&self, filter: &InstanceFilter) -> Result<Vec<InstanceRow>> {
        self.query_instances(filter, false)
    }

    pub(crate) fn list_instances_unbounded(
        &self,
        filter: &InstanceFilter,
    ) -> Result<Vec<InstanceRow>> {
        self.query_instances(filter, true)
    }

    fn query_instances(
        &self,
        filter: &InstanceFilter,
        unbounded: bool,
    ) -> Result<Vec<InstanceRow>> {
        let reader = self.reader_conn()?;
        let limit = if filter.limit == 0 {
            100
        } else {
            filter.limit.min(500)
        };
        let mut clauses: Vec<String> = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(status) = filter.status {
            clauses.push("i.lifecycle_status = ?".to_owned());
            values.push(Box::new(status.as_str().to_owned()));
        }
        if let Some(kind) = filter.kind {
            clauses.push("i.instance_kind = ?".to_owned());
            values.push(Box::new(kind.as_str().to_owned()));
        }
        if let Some(min_score) = filter.min_score {
            clauses.push("COALESCE(q.score, 0) >= ?".to_owned());
            values.push(Box::new(min_score));
        }
        if let Some(instance_id) = filter.instance_id {
            clauses.push("i.instance_id = ?".to_owned());
            values.push(Box::new(instance_id));
        }
        if !filter.ids.is_empty() {
            let placeholders = vec!["?"; filter.ids.len()].join(",");
            clauses.push(format!("i.instance_id IN ({placeholders})"));
            for id in &filter.ids {
                values.push(Box::new(*id));
            }
        }
        if let Some(url) = &filter.url {
            let normalized = normalize_url(url)?;
            clauses.push("i.normalized_url = ?".to_owned());
            values.push(Box::new(normalized));
        }
        let where_clause = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        let sql = format!(
            "SELECT i.instance_id, i.normalized_url, i.host, i.instance_kind, i.lifecycle_status,
                    i.searxng_version, q.score, q.fleet_rank, q.is_hard_gate_passed, q.reason_codes_json,
                    c.is_json_enabled, c.is_html_enabled, c.is_google_exact_seen, c.is_google_cse_seen,
                    i.consecutive_successes, i.consecutive_failures, i.last_success_at, i.last_failure_at,
                    i.next_scan_at, i.cooldown_until, i.is_scan_enabled, i.is_html_fallback_allowed,
                    i.discovery_rank, i.network_group, i.discovered_at,
                    COALESCE(o.is_pinned, 0), COALESCE(o.is_denied, 0), i.instance_slug,
                    COALESCE(o.is_trusted, 0), i.discovery_source_id, i.json_throttled_until,
                    i.manual_priority
               FROM instances AS i
               LEFT JOIN instance_quality AS q ON q.instance_id = i.instance_id
               LEFT JOIN capability_snapshots AS c ON c.capability_id = i.current_capability_id
               LEFT JOIN instance_policy_overrides AS o ON o.instance_id = i.instance_id
               {where_clause}
              ORDER BY CASE WHEN q.is_hard_gate_passed = 1 THEN 0 ELSE 1 END,
                       COALESCE(q.score, 0) DESC, i.instance_id
              LIMIT ? OFFSET ?"
        );
        let mut statement = reader.prepare(&sql)?;
        let mut params_vec: Vec<&dyn rusqlite::ToSql> =
            values.iter().map(|value| value.as_ref()).collect();
        let limit_box = Box::new(if unbounded {
            -1
        } else {
            i64::try_from(limit).unwrap_or(100)
        });
        let offset_box = Box::new(i64::try_from(filter.offset).unwrap_or(0));
        params_vec.push(limit_box.as_ref());
        params_vec.push(offset_box.as_ref());
        let rows = statement.query_map(params_vec.as_slice(), map_instance_row)?;
        let out = collect_rows(rows)?;
        Ok(out)
    }

    pub fn get_instance(&self, instance_id: i64) -> Result<Option<InstanceRow>> {
        let filter = InstanceFilter {
            instance_id: Some(instance_id),
            limit: 1,
            ..InstanceFilter::default()
        };
        Ok(self.list_instances(&filter)?.into_iter().next())
    }

    pub fn find_instance_by_url(&self, url: &str) -> Result<Option<InstanceRow>> {
        let filter = InstanceFilter {
            url: Some(url.to_owned()),
            limit: 1,
            ..InstanceFilter::default()
        };
        Ok(self.list_instances(&filter)?.into_iter().next())
    }

    pub fn scan_targets(
        &self,
        limit: usize,
        now: &str,
        force: bool,
        manual_only: bool,
    ) -> Result<Vec<ScanTarget>> {
        let reader = self.reader_conn()?;
        let schedule_clause = if force {
            ""
        } else {
            "AND (i.next_scan_at IS NULL OR i.next_scan_at <= ?2)"
        };
        let source_clause = if manual_only {
            "AND i.discovery_source_id = 'manual'"
        } else {
            ""
        };
        let sql = format!(
            "SELECT i.instance_id, i.normalized_url, i.lifecycle_status, i.instance_kind,
                    i.is_html_fallback_allowed, i.consecutive_successes, i.consecutive_failures,
                    COALESCE(o.is_trusted, 0), i.json_throttled_until, i.discovery_source_id
               FROM instances AS i
               LEFT JOIN instance_policy_overrides AS o ON o.instance_id = i.instance_id
              WHERE COALESCE(o.is_denied, 0) = 0
                AND i.is_scan_enabled <> 0
                AND i.instance_kind <> 'tor'
                AND i.lifecycle_status NOT IN ('retired', 'excluded')
                AND (i.cooldown_until IS NULL OR i.cooldown_until <= ?2)
                {source_clause}
                {schedule_clause}
              ORDER BY CASE i.lifecycle_status
                         WHEN 'active' THEN 0
                         WHEN 'probation' THEN 1
                         WHEN 'candidate' THEN 2
                         WHEN 'missing' THEN 3
                         WHEN 'unlisted' THEN 3
                         ELSE 4
                       END,
                       COALESCE(i.discovery_rank, 2147483647), i.instance_id
              LIMIT ?1"
        );
        let mut statement = reader.prepare(&sql)?;
        let rows =
            statement.query_map(params![i64::try_from(limit).unwrap_or(200), now], |row| {
                let status_text: String = row.get(2)?;
                let kind: String = row.get(3)?;
                let status = InstanceStatus::parse(&status_text).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::other(format!(
                            "unknown lifecycle status {status_text:?}"
                        ))),
                    )
                })?;
                Ok(ScanTarget {
                    instance_id: row.get(0)?,
                    url: row.get(1)?,
                    status,
                    kind: InstanceType::parse(&kind),
                    html_fallback_allowed: row.get::<_, i64>(4)? != 0,
                    consecutive_successes: row.get(5)?,
                    consecutive_failures: row.get(6)?,
                    trusted: row.get::<_, i64>(7)? != 0,
                    json_throttled_until: row.get(8)?,
                    source: row.get(9)?,
                })
            })?;
        let out = collect_rows(rows)?;
        Ok(out)
    }

    pub fn fleet(&self, limit: usize, now: &str) -> Result<Vec<FleetRow>> {
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare(
            "SELECT i.instance_id, i.normalized_url, COALESCE(q.score, 0), COALESCE(q.fleet_rank, 0),
                    COALESCE(c.is_json_enabled, 0), COALESCE(c.is_html_enabled, 0),
                    COALESCE(c.is_google_exact_seen, 0), COALESCE(c.is_google_cse_seen, 0), i.network_group
               FROM instances AS i
               JOIN instance_quality AS q ON q.instance_id = i.instance_id
               LEFT JOIN capability_snapshots AS c ON c.capability_id = i.current_capability_id
               LEFT JOIN instance_policy_overrides AS o ON o.instance_id = i.instance_id
              WHERE i.lifecycle_status = 'active'
                AND i.is_scan_enabled <> 0
                AND COALESCE(o.is_denied, 0) = 0
                AND i.instance_kind <> 'tor'
                AND q.is_hard_gate_passed = 1
                AND (i.cooldown_until IS NULL OR i.cooldown_until <= ?2)
              ORDER BY q.fleet_rank IS NULL, q.fleet_rank, q.score DESC, i.instance_id
              LIMIT ?1",
        )?;
        let rows =
            statement.query_map(params![i64::try_from(limit).unwrap_or(64), now], |row| {
                Ok(FleetRow {
                    instance_id: row.get(0)?,
                    url: row.get(1)?,
                    score: row.get(2)?,
                    rank: row.get(3)?,
                    json_enabled: row.get::<_, i64>(4)? != 0,
                    html_enabled: row.get::<_, i64>(5)? != 0,
                    google_exact: row.get::<_, i64>(6)? != 0,
                    google_cse: row.get::<_, i64>(7)? != 0,
                    network_group: row.get(8)?,
                })
            })?;
        let out = collect_rows(rows)?;
        Ok(out)
    }

    pub fn engine_names(&self, instance_id: i64) -> Result<Vec<String>> {
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare(
            "SELECT e.engine_name FROM instances AS i
               JOIN capability_engines AS ce ON ce.capability_id = i.current_capability_id
               JOIN engines AS e ON e.engine_id = ce.engine_id
              WHERE i.instance_id = ?1 ORDER BY e.engine_name",
        )?;
        let rows = statement.query_map(params![instance_id], |row| row.get::<_, String>(0))?;
        let out = collect_rows(rows)?;
        Ok(out)
    }

    pub fn category_names(&self, instance_id: i64) -> Result<Vec<String>> {
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare(
            "SELECT c.category_name FROM instances AS i
               JOIN capability_categories AS cc ON cc.capability_id = i.current_capability_id
               JOIN categories AS c ON c.category_id = cc.category_id
              WHERE i.instance_id = ?1 ORDER BY c.category_name",
        )?;
        let rows = statement.query_map(params![instance_id], |row| row.get::<_, String>(0))?;
        let out = collect_rows(rows)?;
        Ok(out)
    }

    pub fn engine_names_for(
        &self,
        ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, Vec<String>>> {
        let mut out: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT i.instance_id, e.engine_name
               FROM instances AS i
               JOIN capability_engines AS ce ON ce.capability_id = i.current_capability_id
               JOIN engines AS e ON e.engine_id = ce.engine_id
              WHERE i.instance_id IN ({placeholders})
              ORDER BY e.engine_name"
        );
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare(&sql)?;
        let values: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = statement.query_map(values.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (instance_id, name) = row?;
            out.entry(instance_id).or_default().push(name);
        }
        Ok(out)
    }

    pub fn category_names_for(
        &self,
        ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, Vec<String>>> {
        let mut out: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT i.instance_id, c.category_name
               FROM instances AS i
               JOIN capability_categories AS cc ON cc.capability_id = i.current_capability_id
               JOIN categories AS c ON c.category_id = cc.category_id
              WHERE i.instance_id IN ({placeholders})
              ORDER BY c.category_name"
        );
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare(&sql)?;
        let values: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = statement.query_map(values.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (instance_id, name) = row?;
            out.entry(instance_id).or_default().push(name);
        }
        Ok(out)
    }

    pub fn events_for_many(
        &self,
        ids: &[i64],
        per_instance: usize,
    ) -> Result<std::collections::HashMap<i64, Vec<Value>>> {
        let mut out: std::collections::HashMap<i64, Vec<Value>> = std::collections::HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT instance_id, event_id, occurred_at, event_kind, previous_value, current_value,
                    reason_codes_json, position
               FROM (
                    SELECT instance_id, event_id, occurred_at, event_kind, previous_value, current_value,
                           reason_codes_json,
                           ROW_NUMBER() OVER (PARTITION BY instance_id ORDER BY event_id DESC) AS position
                      FROM instance_events
                     WHERE instance_id IN ({placeholders})
               )
              WHERE position <= ?
              ORDER BY instance_id, event_id DESC"
        );
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare(&sql)?;
        let limit = i64::try_from(per_instance.max(1)).unwrap_or(1);
        let mut values: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        values.push(&limit);
        let rows = statement.query_map(values.as_slice(), |row| {
            let reasons: String = row.get(6)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(7)?,
                json!({
                    "eventId": row.get::<_, i64>(1)?,
                    "occurredAt": row.get::<_, String>(2)?,
                    "kind": row.get::<_, String>(3)?,
                    "oldValue": row.get::<_, Option<String>>(4)?,
                    "newValue": row.get::<_, Option<String>>(5)?,
                    "reasons": serde_json::from_str::<Value>(&reasons).unwrap_or(Value::Null),
                }),
            ))
        })?;
        for row in rows {
            let (instance_id, _position, value) = row?;
            out.entry(instance_id).or_default().push(value);
        }
        Ok(out)
    }

    pub fn engine_stats_for(
        &self,
        ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, Vec<EngineStat>>> {
        let mut out: std::collections::HashMap<i64, Vec<EngineStat>> =
            std::collections::HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT instance_id, engine_name, success_count, failure_count, last_seen_at
               FROM engine_stats
              WHERE instance_id IN ({placeholders})
              ORDER BY instance_id, engine_name"
        );
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare(&sql)?;
        let values: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = statement.query_map(values.as_slice(), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                EngineStat {
                    engine: row.get(1)?,
                    successes: row.get(2)?,
                    failures: row.get(3)?,
                    last_seen_at: row.get(4)?,
                },
            ))
        })?;
        for row in rows {
            let (instance_id, stat) = row?;
            out.entry(instance_id).or_default().push(stat);
        }
        Ok(out)
    }

    pub fn record_engine_stats(
        &self,
        instance_id: i64,
        successes: &[String],
        failures: &[String],
    ) -> Result<()> {
        if successes.is_empty() && failures.is_empty() {
            return Ok(());
        }
        let now = now_rfc3339();
        self.write_atomic(|connection| {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for engine in successes {
                let name = engine.trim().to_ascii_lowercase();
                if seen.insert(name.clone()) {
                    bump_engine(connection, instance_id, &name, true, &now)?;
                }
            }
            for engine in failures {
                let name = engine.trim().to_ascii_lowercase();
                if seen.insert(name.clone()) {
                    bump_engine(connection, instance_id, &name, false, &now)?;
                }
            }
            Ok(())
        })
    }

    pub fn engine_yield_for(&self, ids: &[i64]) -> Result<std::collections::HashMap<i64, f64>> {
        let mut out: std::collections::HashMap<i64, f64> = std::collections::HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT i.instance_id,
                    (SELECT COUNT(*) FROM capability_engines AS ce
                      WHERE ce.capability_id = i.current_capability_id),
                    (SELECT COUNT(*) FROM engine_stats AS es
                      WHERE es.instance_id = i.instance_id AND es.success_count > 0)
               FROM instances AS i
              WHERE i.instance_id IN ({placeholders})"
        );
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare(&sql)?;
        let values: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = statement.query_map(values.as_slice(), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (instance_id, enabled, contributing) = row?;
            if enabled > 0 {
                let contributing = f64::from(i32::try_from(contributing.max(0)).unwrap_or(0));
                let enabled = f64::from(i32::try_from(enabled).unwrap_or(1));
                out.insert(instance_id, (contributing / enabled).clamp(0.0, 1.0));
            }
        }
        Ok(out)
    }

    pub fn update_instance_url(&self, instance_id: i64, url: &str) -> Result<String> {
        let normalized = crate::types::normalize_url(url)?;
        let host = url::Url::parse(&normalized)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_owned))
            .ok_or_else(|| Error::InvalidArgs(format!("{normalized} has no host")))?;
        let digest = {
            use sha2::{Digest, Sha256};
            Sha256::digest(normalized.as_bytes()).to_vec()
        };
        let mut connection = self.writer_conn()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: String = transaction.query_row(
            "SELECT normalized_url FROM instances WHERE instance_id = ?1",
            params![instance_id],
            |row| row.get(0),
        )?;
        if previous == normalized {
            return Ok(normalized);
        }
        let taken: Option<i64> = transaction
            .query_row(
                "SELECT instance_id FROM instances WHERE normalized_url = ?1 AND instance_id <> ?2",
                params![normalized, instance_id],
                |row| row.get(0),
            )
            .optional()?;
        if taken.is_some() {
            return Err(Error::InvalidArgs(format!(
                "another instance already holds {normalized}"
            )));
        }
        let slug = unique_slug(&transaction, &normalized)?;
        let now = now_rfc3339();
        transaction.execute(
            "UPDATE instances SET normalized_url = ?2, url_digest_sha256 = ?3, host = ?4,
                instance_slug = ?5, lifecycle_status = 'candidate',
                current_capability_id = NULL, current_tls_observation_id = NULL,
                json_throttled_until = NULL, consecutive_successes = 0, consecutive_failures = 0,
                last_success_at = NULL, last_failure_at = NULL, cooldown_until = NULL,
                next_scan_at = ?6, updated_at = ?6
             WHERE instance_id = ?1",
            params![instance_id, normalized, digest, host, slug, now],
        )?;
        transaction.execute(
            "DELETE FROM instance_quality WHERE instance_id = ?1",
            params![instance_id],
        )?;
        transaction.execute(
            "INSERT INTO instance_events (occurred_at, event_kind, instance_id, previous_value, current_value, reason_codes_json)
             VALUES (?1, 'instance_url_changed', ?2, ?3, ?4, ?5)",
            params![
                now,
                instance_id,
                previous,
                normalized,
                json!(["operator.url_changed"]).to_string()
            ],
        )?;
        transaction.commit()?;
        Ok(normalized)
    }

    pub fn set_manual_priority(&self, instance_id: i64, priority: i64) -> Result<()> {
        let now = now_rfc3339();
        self.write_atomic(|connection| {
            connection.execute(
                "UPDATE instances SET manual_priority = ?2, updated_at = ?3 WHERE instance_id = ?1",
                params![instance_id, priority, now],
            )?;
            Store::append_event_on(
                connection,
                &EventRecord {
                    kind: "priority_changed".to_owned(),
                    instance_id: Some(instance_id),
                    run_id: None,
                    probe_id: None,
                    old_value: None,
                    new_value: Some(priority.to_string()),
                    reasons: vec!["operator.priority".to_owned()],
                    payload: None,
                },
            )?;
            Ok(())
        })
    }

    pub fn delete_instance(&self, instance_id: i64) -> Result<()> {
        let mut connection = self.writer_conn()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let url: String = transaction.query_row(
            "SELECT normalized_url FROM instances WHERE instance_id = ?1",
            params![instance_id],
            |row| row.get(0),
        )?;
        transaction.execute(
            "UPDATE instances SET current_capability_id = NULL, current_tls_observation_id = NULL WHERE instance_id = ?1",
            params![instance_id],
        )?;
        transaction.execute(
            "DELETE FROM engine_stats WHERE instance_id = ?1",
            params![instance_id],
        )?;
        transaction.execute(
            "DELETE FROM instance_auth WHERE instance_id = ?1",
            params![instance_id],
        )?;
        transaction.execute(
            "DELETE FROM instance_policy_overrides WHERE instance_id = ?1",
            params![instance_id],
        )?;
        transaction.execute(
            "DELETE FROM instance_quality WHERE instance_id = ?1",
            params![instance_id],
        )?;
        transaction.execute(
            "DELETE FROM capability_engines WHERE capability_id IN (SELECT capability_id FROM capability_snapshots WHERE instance_id = ?1)",
            params![instance_id],
        )?;
        transaction.execute(
            "DELETE FROM capability_categories WHERE capability_id IN (SELECT capability_id FROM capability_snapshots WHERE instance_id = ?1)",
            params![instance_id],
        )?;
        transaction.execute(
            "DELETE FROM capability_snapshots WHERE instance_id = ?1",
            params![instance_id],
        )?;
        transaction.execute(
            "DELETE FROM tls_observations WHERE instance_id = ?1",
            params![instance_id],
        )?;
        transaction.execute(
            "DELETE FROM probes WHERE instance_id = ?1",
            params![instance_id],
        )?;
        transaction.execute(
            "DELETE FROM instance_events WHERE instance_id = ?1",
            params![instance_id],
        )?;
        transaction.execute(
            "DELETE FROM instances WHERE instance_id = ?1",
            params![instance_id],
        )?;
        transaction.execute(
            "INSERT INTO instance_events (occurred_at, event_kind, instance_id, previous_value, reason_codes_json)
             VALUES (?1, 'instance_deleted', NULL, ?2, ?3)",
            params![now_rfc3339(), url, json!(["operator.deleted"]).to_string()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn set_instance_auth(
        &self,
        instance_id: i64,
        record: &crate::auth::AuthRecord,
    ) -> Result<()> {
        let now = now_rfc3339();
        self.write_atomic(|connection| {
            connection.execute(
                "INSERT INTO instance_auth (instance_id, auth_kind, header_name, user_name, secret_env, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                 ON CONFLICT(instance_id) DO UPDATE SET
                    auth_kind = excluded.auth_kind,
                    header_name = excluded.header_name,
                    user_name = excluded.user_name,
                    secret_env = excluded.secret_env,
                    updated_at = excluded.updated_at",
                params![
                    instance_id,
                    record.kind.as_str(),
                    record.header_name,
                    record.user_name,
                    record.secret_env,
                    now
                ],
            )?;
            Store::append_event_on(
                connection,
                &EventRecord {
                    kind: "auth_changed".to_owned(),
                    instance_id: Some(instance_id),
                    run_id: None,
                    probe_id: None,
                    old_value: None,
                    new_value: Some(record.kind.as_str().to_owned()),
                    reasons: vec!["operator.auth_changed".to_owned()],
                    payload: Some(json!({
                        "header": record.header_name,
                        "user": record.user_name,
                        "secretEnv": record.secret_env,
                    })),
                },
            )?;
            Ok(())
        })
    }

    pub fn clear_instance_auth(&self, instance_id: i64) -> Result<bool> {
        self.write_atomic(|connection| {
            let removed = connection.execute(
                "DELETE FROM instance_auth WHERE instance_id = ?1",
                params![instance_id],
            )?;
            if removed > 0 {
                Store::append_event_on(
                    connection,
                    &EventRecord {
                        kind: "auth_cleared".to_owned(),
                        instance_id: Some(instance_id),
                        run_id: None,
                        probe_id: None,
                        old_value: None,
                        new_value: None,
                        reasons: vec!["operator.auth_cleared".to_owned()],
                        payload: None,
                    },
                )?;
            }
            Ok(removed > 0)
        })
    }

    pub fn auth_for(
        &self,
        ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, crate::auth::AuthRecord>> {
        let mut out: std::collections::HashMap<i64, crate::auth::AuthRecord> =
            std::collections::HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT instance_id, auth_kind, header_name, user_name, secret_env
               FROM instance_auth WHERE instance_id IN ({placeholders})"
        );
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare(&sql)?;
        let values: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = statement.query_map(values.as_slice(), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                crate::auth::AuthRecord {
                    kind: row
                        .get::<_, String>(1)
                        .ok()
                        .and_then(|raw| crate::auth::AuthKind::parse(&raw))
                        .unwrap_or(crate::auth::AuthKind::Header),
                    header_name: row.get(2)?,
                    user_name: row.get(3)?,
                    secret_env: row.get(4)?,
                },
            ))
        })?;
        for row in rows {
            let (instance_id, record) = row?;
            out.insert(instance_id, record);
        }
        Ok(out)
    }

    pub fn mark_json_throttled(&self, instance_id: i64, until: &str) -> Result<()> {
        let now = now_rfc3339();
        self.write_atomic(|connection| {
            connection.execute(
                "UPDATE instances SET json_throttled_until = ?2, updated_at = ?3 WHERE instance_id = ?1",
                params![instance_id, until, now],
            )?;
            Store::append_event_on(
                connection,
                &EventRecord {
                    kind: "json_throttled".to_owned(),
                    instance_id: Some(instance_id),
                    run_id: None,
                    probe_id: None,
                    old_value: None,
                    new_value: Some(until.to_owned()),
                    reasons: vec!["limiter.json_throttled".to_owned()],
                    payload: None,
                },
            )?;
            Ok(())
        })
    }

    pub fn clear_json_throttled(&self, instance_id: i64) -> Result<()> {
        let now = now_rfc3339();
        self.write_atomic(|connection| {
            connection.execute(
                "UPDATE instances SET json_throttled_until = NULL, updated_at = ?2 WHERE instance_id = ?1",
                params![instance_id, now],
            )?;
            Ok(())
        })
    }

    pub fn probe_kind_count_since(&self, instance_id: i64, kind: &str, since: &str) -> Result<i64> {
        let reader = self.reader_conn()?;
        let count = reader.query_row(
            "SELECT COUNT(*) FROM probes WHERE instance_id = ?1 AND probe_kind = ?2 AND started_at >= ?3",
            params![instance_id, kind, since],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn probe_count_since(&self, instance_id: i64, since: &str) -> Result<i64> {
        let reader = self.reader_conn()?;
        let count = reader.query_row(
            "SELECT COUNT(*) FROM probes WHERE instance_id = ?1 AND started_at >= ?2",
            params![instance_id, since],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn count_instances(&self, filter: &InstanceFilter) -> Result<usize> {
        Ok(self.list_instances_unbounded(filter)?.len())
    }

    pub fn counts(&self) -> Result<StatusCounts> {
        let reader = self.reader_conn()?;
        let by_status = collect_counts(
            &reader,
            "SELECT lifecycle_status, COUNT(*) FROM instances GROUP BY lifecycle_status",
        )?;
        let by_type = collect_counts(
            &reader,
            "SELECT instance_kind, COUNT(*) FROM instances GROUP BY instance_kind",
        )?;
        let total: i64 =
            reader.query_row("SELECT COUNT(*) FROM instances", [], |row| row.get(0))?;
        let active: i64 = reader.query_row(
            "SELECT COUNT(*) FROM instances WHERE lifecycle_status = 'active'",
            [],
            |row| row.get(0),
        )?;
        let probes: i64 = reader.query_row("SELECT COUNT(*) FROM probes", [], |row| row.get(0))?;
        let events: i64 =
            reader.query_row("SELECT COUNT(*) FROM instance_events", [], |row| row.get(0))?;
        Ok(StatusCounts {
            by_status,
            by_type,
            total,
            active,
            probes,
            events,
        })
    }

    pub fn events_for(
        &self,
        instance_id: Option<i64>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Value>> {
        if offset > MAX_EVENT_OFFSET {
            return Err(Error::InvalidArgs(format!(
                "offset {offset} is above the {MAX_EVENT_OFFSET} row cap; page with a smaller offset"
            )));
        }
        let reader = self.reader_conn()?;
        let sql = match instance_id {
            Some(_) => {
                "SELECT event_id, occurred_at, event_kind, previous_value, current_value, reason_codes_json
                   FROM instance_events WHERE instance_id = ?1 ORDER BY event_id DESC LIMIT ?2 OFFSET ?3"
            }
            None => {
                "SELECT event_id, occurred_at, event_kind, previous_value, current_value, reason_codes_json
                   FROM instance_events ORDER BY event_id DESC LIMIT ?2 OFFSET ?1"
            }
        };
        let mut statement = reader.prepare(sql)?;
        let limit = i64::try_from(limit.min(500)).unwrap_or(100);
        let mapper = |row: &rusqlite::Row<'_>| {
            let reasons: String = row.get(5)?;
            Ok(json!({
                "eventId": row.get::<_, i64>(0)?,
                "occurredAt": row.get::<_, String>(1)?,
                "kind": row.get::<_, String>(2)?,
                "oldValue": row.get::<_, Option<String>>(3)?,
                "newValue": row.get::<_, Option<String>>(4)?,
                "reasons": serde_json::from_str::<Value>(&reasons).unwrap_or(Value::Null),
            }))
        };
        let offset = i64::try_from(offset.min(50_000)).unwrap_or(0);
        let rows = match instance_id {
            Some(id) => statement
                .query_map(params![id, limit, offset], mapper)?
                .collect::<Vec<_>>(),
            None => statement
                .query_map(params![offset, limit], mapper)?
                .collect::<Vec<_>>(),
        };
        let out = collect_rows(rows.into_iter())?;
        Ok(out)
    }

    pub fn export_snapshot(&self) -> Result<Value> {
        let now = now_rfc3339();
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare(
            "SELECT i.instance_id, i.normalized_url, i.instance_kind, i.lifecycle_status, i.searxng_version,
                    COALESCE(q.score, 0), q.fleet_rank, COALESCE(q.is_hard_gate_passed, 0), COALESCE(q.reason_codes_json, '[]'),
                    COALESCE(c.is_json_enabled, 0), COALESCE(c.is_html_enabled, 0),
                    COALESCE(c.is_google_exact_seen, 0), COALESCE(c.is_google_cse_seen, 0),
                    i.discovery_rank, i.network_group, i.consecutive_successes, i.consecutive_failures,
                    COALESCE(o.is_denied, 0), i.is_scan_enabled, i.cooldown_until
               FROM instances AS i
               LEFT JOIN instance_quality AS q ON q.instance_id = i.instance_id
               LEFT JOIN capability_snapshots AS c ON c.capability_id = i.current_capability_id
               LEFT JOIN instance_policy_overrides AS o ON o.instance_id = i.instance_id
              ORDER BY CASE WHEN i.lifecycle_status = 'active' THEN 0 ELSE 1 END,
                       COALESCE(q.fleet_rank, 2147483647), COALESCE(q.score, 0) DESC, i.instance_id",
        )?;
        let rows = statement.query_map([], |row| {
            let reasons: String = row.get(8)?;
            Ok(json!({
                "id": row.get::<_, i64>(0)?,
                "url": row.get::<_, String>(1)?,
                "type": row.get::<_, String>(2)?,
                "status": row.get::<_, String>(3)?,
                "version": row.get::<_, Option<String>>(4)?,
                "score": row.get::<_, f64>(5)?,
                "rank": row.get::<_, Option<i64>>(6)?,
                "hardGatePassed": row.get::<_, i64>(7)? != 0,
                "reasons": serde_json::from_str::<Value>(&reasons).unwrap_or(Value::Null),
                "jsonEnabled": row.get::<_, i64>(9)? != 0,
                "htmlEnabled": row.get::<_, i64>(10)? != 0,
                "googleExact": row.get::<_, i64>(11)? != 0,
                "googleCse": row.get::<_, i64>(12)? != 0,
                "discoveryRank": row.get::<_, Option<i64>>(13)?,
                "networkGroup": row.get::<_, Option<String>>(14)?,
                "consecutiveSuccesses": row.get::<_, i64>(15)?,
                "consecutiveFailures": row.get::<_, i64>(16)?,
                "denied": row.get::<_, i64>(17)? != 0,
                "scanEnabled": row.get::<_, i64>(18)? != 0,
                "cooldownUntil": row.get::<_, Option<String>>(19)?,
            }))
        })?;
        let mut instances = Vec::new();
        for row in rows {
            instances.push(row?);
        }
        let fleet: Vec<Value> = instances
            .iter()
            .filter(|item| {
                item.get("status").and_then(Value::as_str) == Some("active")
                    && item.get("hardGatePassed").and_then(Value::as_bool) == Some(true)
                    && item.get("denied").and_then(Value::as_bool) == Some(false)
                    && item.get("scanEnabled").and_then(Value::as_bool) == Some(true)
                    && item.get("type").and_then(Value::as_str) != Some("tor")
                    && item
                        .get("cooldownUntil")
                        .and_then(Value::as_str)
                        .is_none_or(|until| until <= now.as_str())
            })
            .cloned()
            .collect();
        Ok(json!({
            "schemaVersion": 1,
            "generatedAt": now,
            "fleet": fleet,
            "instances": instances,
        }))
    }

    pub fn write_snapshot(&self, path: &Path) -> Result<(usize, usize)> {
        let snapshot = self.export_snapshot()?;
        let payload = serde_json::to_vec_pretty(&snapshot)?;
        if let Some(parent) = path.parent() {
            crate::fsutil::create_private_dir(parent)?;
        }
        crate::fsutil::write_atomic(path, &payload)?;
        let count = snapshot
            .get("instances")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        Ok((count, payload.len()))
    }

    pub fn backup(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            crate::fsutil::create_private_dir(parent)?;
        }
        if path.exists() {
            return Err(Error::State(format!(
                "backup target {} already exists; choose a new name",
                path.display()
            )));
        }
        let connection = self.writer_conn()?;
        connection.execute("VACUUM INTO ?1", params![utf8_path(path)?])?;
        lock_down_file(path)?;
        drop(connection);
        let check = Connection::open(path)?;
        let result: String = check.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if !result.eq_ignore_ascii_case("ok") {
            return Err(Error::StateCorrupted(format!(
                "the backup failed its quick check: {result}"
            )));
        }
        {
            let mut statement = check.prepare("PRAGMA foreign_key_check")?;
            let mut rows = statement.query([])?;
            if rows.next()?.is_some() {
                return Err(Error::StateCorrupted(
                    "the backup holds a foreign key violation".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn maintenance(&self) -> Result<Value> {
        let connection = self.writer_conn()?;
        let checkpoint: (i64, i64, i64) =
            connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
        connection.execute_batch("VACUUM; PRAGMA optimize;")?;
        Ok(json!({
            "checkpointBusy": checkpoint.0,
            "logFrames": checkpoint.1,
            "checkpointedFrames": checkpoint.2,
        }))
    }

    pub fn expire(&self, window: KeepWindow) -> Result<Value> {
        let mut connection = self.writer_conn()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = crate::time::now_unix();
        let cutoff = |days: i64| {
            crate::time::rfc3339(now.saturating_sub(days.max(0).saturating_mul(86_400)))
        };
        let probe_cutoff = cutoff(window.probe_days);
        let events = transaction.execute(
            "DELETE FROM instance_events WHERE occurred_at < ?1",
            params![cutoff(window.event_days)],
        )?;
        let tls = transaction.execute(
            "DELETE FROM tls_observations WHERE observed_at < ?1
                OR probe_id IN (SELECT probe_id FROM probes WHERE started_at < ?2)",
            params![cutoff(window.tls_days), probe_cutoff],
        )?;
        transaction.execute(
            "UPDATE instance_events SET probe_id = NULL
              WHERE probe_id IN (SELECT probe_id FROM probes WHERE started_at < ?1)",
            params![probe_cutoff],
        )?;
        let probes = transaction.execute(
            "DELETE FROM probes WHERE started_at < ?1",
            params![probe_cutoff],
        )?;
        let capability_cutoff = cutoff(window.capability_days);
        transaction.execute(
            "UPDATE instances SET current_capability_id = NULL
              WHERE current_capability_id IN (
                SELECT capability_id FROM capability_snapshots WHERE observed_at < ?1)",
            params![capability_cutoff],
        )?;
        transaction.execute(
            "DELETE FROM capability_engines WHERE capability_id IN (
                SELECT capability_id FROM capability_snapshots WHERE observed_at < ?1)",
            params![capability_cutoff],
        )?;
        transaction.execute(
            "DELETE FROM capability_categories WHERE capability_id IN (
                SELECT capability_id FROM capability_snapshots WHERE observed_at < ?1)",
            params![capability_cutoff],
        )?;
        let capabilities = transaction.execute(
            "DELETE FROM capability_snapshots WHERE observed_at < ?1",
            params![capability_cutoff],
        )?;
        let metrics = transaction.execute(
            "DELETE FROM hourly_probe_metrics WHERE window_start_utc < ?1",
            params![cutoff(window.metrics_days)],
        )?;
        let snapshots = transaction.execute(
            "DELETE FROM fleet_snapshots WHERE generated_at < ?1",
            params![cutoff(window.snapshot_days)],
        )?;
        let run_cutoff = cutoff(window.run_days);
        transaction.execute(
            "UPDATE instance_events SET discovery_run_id = NULL
              WHERE discovery_run_id IN (SELECT run_id FROM discovery_runs WHERE started_at < ?1)",
            params![run_cutoff],
        )?;
        transaction.execute(
            "UPDATE instances SET current_discovery_run_id = NULL
              WHERE current_discovery_run_id IN (SELECT run_id FROM discovery_runs WHERE started_at < ?1)",
            params![run_cutoff],
        )?;
        let runs = transaction.execute(
            "DELETE FROM discovery_runs WHERE started_at < ?1",
            params![run_cutoff],
        )?;
        transaction.execute(
            "UPDATE instances SET current_capability_id = NULL
              WHERE current_capability_id IS NOT NULL
                AND current_capability_id NOT IN (SELECT capability_id FROM capability_snapshots)",
            [],
        )?;
        transaction.execute(
            "UPDATE instances SET current_tls_observation_id = NULL
              WHERE current_tls_observation_id IS NOT NULL
                AND current_tls_observation_id NOT IN (SELECT tls_observation_id FROM tls_observations)",
            [],
        )?;
        transaction.commit()?;
        Ok(json!({
            "probes": probes,
            "tls": tls,
            "capabilities": capabilities,
            "metrics": metrics,
            "snapshots": snapshots,
            "events": events,
            "runs": runs,
        }))
    }

    pub fn record_metrics(
        &self,
        instance_id: i64,
        kind: ProbeKind,
        duration_ms: i64,
        success: bool,
        result_count: i64,
        response_bytes: i64,
    ) -> Result<()> {
        let connection = self.writer_conn()?;
        Self::record_metrics_on(
            &connection,
            instance_id,
            kind,
            duration_ms,
            success,
            result_count,
            response_bytes,
        )
    }

    pub(crate) fn record_metrics_on(
        connection: &Connection,
        instance_id: i64,
        kind: ProbeKind,
        duration_ms: i64,
        success: bool,
        result_count: i64,
        response_bytes: i64,
    ) -> Result<()> {
        let now = crate::time::now_unix();
        let window = crate::time::rfc3339(now - now.rem_euclid(3_600));
        let successes = i64::from(success);
        let failures = 1 - successes;
        connection.execute(
            "INSERT INTO hourly_probe_metrics (instance_id, probe_kind, window_start_utc, attempt_count, success_count,
                failure_count, success_rate, duration_last_ms, duration_min_ms, duration_max_ms,
                total_result_count, response_bytes, updated_at)
             VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7, ?7, ?7, ?8, ?9, ?10)
             ON CONFLICT(instance_id, probe_kind, window_start_utc) DO UPDATE SET
                attempt_count = hourly_probe_metrics.attempt_count + 1,
                success_count = hourly_probe_metrics.success_count + excluded.success_count,
                failure_count = hourly_probe_metrics.failure_count + excluded.failure_count,
                success_rate = CAST(hourly_probe_metrics.success_count + excluded.success_count AS REAL)
                    / (hourly_probe_metrics.attempt_count + 1),
                duration_last_ms = excluded.duration_last_ms,
                duration_min_ms = MIN(hourly_probe_metrics.duration_min_ms, excluded.duration_min_ms),
                duration_max_ms = MAX(hourly_probe_metrics.duration_max_ms, excluded.duration_max_ms),
                total_result_count = hourly_probe_metrics.total_result_count + excluded.total_result_count,
                response_bytes = hourly_probe_metrics.response_bytes + excluded.response_bytes,
                updated_at = excluded.updated_at",
            params![
                instance_id,
                kind.as_str(),
                window,
                successes,
                failures,
                if success { 1.0 } else { 0.0 },
                duration_ms,
                result_count,
                response_bytes,
                crate::time::rfc3339(now)
            ],
        )?;
        Ok(())
    }

    pub fn record_fleet_snapshot(&self, kind: &str, payload: &Value) -> Result<()> {
        let count = payload
            .get("instances")
            .and_then(Value::as_array)
            .map(|items| i64::try_from(items.len()).unwrap_or(0))
            .unwrap_or(0);
        let body = payload.to_string();
        let digest = {
            use sha2::{Digest, Sha256};
            Sha256::digest(body.as_bytes()).to_vec()
        };
        self.write_atomic(|connection| {
            connection.execute(
                "INSERT INTO fleet_snapshots (generated_at, snapshot_kind, payload_format, instance_count, byte_size, payload_digest_sha256, payload_json)
                 VALUES (?1, ?2, 'json', ?3, ?4, ?5, ?6)",
                params![
                    now_rfc3339(),
                    kind,
                    count,
                    i64::try_from(body.len()).unwrap_or(0),
                    digest,
                    body
                ],
            )?;
            connection.execute(
                "DELETE FROM fleet_snapshots WHERE snapshot_id NOT IN (
                    SELECT snapshot_id FROM fleet_snapshots ORDER BY snapshot_id DESC LIMIT ?1)",
                params![MAX_FLEET_SNAPSHOTS],
            )?;
            Ok(())
        })
    }

    pub fn record_maintenance(&self, kind: &str, outcome: &str, detail: Value) -> Result<()> {
        let connection = self.writer_conn()?;
        connection.execute(
            "INSERT INTO maintenance_runs (maintenance_kind, started_at, completed_at, outcome, detail_json)
             VALUES (?1, ?2, ?2, ?3, ?4)",
            params![kind, now_rfc3339(), outcome, detail.to_string()],
        )?;
        Ok(())
    }

    pub fn policy_overrides(&self) -> Result<Vec<Value>> {
        let reader = self.reader_conn()?;
        let mut statement = reader.prepare(
            "SELECT o.instance_id, i.normalized_url, o.is_scan_enabled, o.is_html_fallback_allowed,
                    o.is_pinned, o.is_denied, o.reason_codes_json, o.created_at, o.updated_at,
                    o.is_trusted
               FROM instance_policy_overrides AS o
               JOIN instances AS i ON i.instance_id = o.instance_id
              ORDER BY o.instance_id",
        )?;
        let rows = statement.query_map([], |row| {
            let reasons: String = row.get(6)?;
            Ok(json!({
                "instanceId": row.get::<_, i64>(0)?,
                "url": row.get::<_, String>(1)?,
                "scanEnabled": row.get::<_, Option<i64>>(2)?.map(|value| value != 0),
                "htmlFallbackAllowed": row.get::<_, Option<i64>>(3)?.map(|value| value != 0),
                "pinned": row.get::<_, i64>(4)? != 0,
                "denied": row.get::<_, i64>(5)? != 0,
                "trusted": row.get::<_, i64>(9)? != 0,
                "reasons": serde_json::from_str::<Value>(&reasons).unwrap_or(Value::Null),
                "createdAt": row.get::<_, String>(7)?,
                "updatedAt": row.get::<_, String>(8)?,
            }))
        })?;
        let out = collect_rows(rows)?;
        Ok(out)
    }

    pub fn get_override(&self, instance_id: i64) -> Result<Option<Value>> {
        Ok(self
            .policy_overrides()?
            .into_iter()
            .find(|value| value.get("instanceId").and_then(Value::as_i64) == Some(instance_id)))
    }

    pub fn apply_policy_change(
        &self,
        instance_id: i64,
        status: Option<InstanceStatus>,
        kind: Option<InstanceType>,
        override_fields: Option<&Value>,
        reasons: &[String],
    ) -> Result<()> {
        let mut connection = self.writer_conn()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = now_rfc3339();
        let previous: String = transaction.query_row(
            "SELECT lifecycle_status FROM instances WHERE instance_id = ?1",
            params![instance_id],
            |row| row.get(0),
        )?;
        if let Some(status) = status
            && status == InstanceStatus::Active
        {
            let hard_gate: Option<i64> = transaction
                .query_row(
                    "SELECT is_hard_gate_passed FROM instance_quality WHERE instance_id = ?1",
                    params![instance_id],
                    |row| row.get(0),
                )
                .optional()?;
            if !matches!(hard_gate, Some(value) if value != 0) {
                return Err(Error::PolicyRefused(
                    "an instance cannot be made active until it passes the hard gate".into(),
                ));
            }
        }
        if let Some(status) = status {
            transaction.execute(
                "UPDATE instances SET lifecycle_status = ?2, updated_at = ?3 WHERE instance_id = ?1",
                params![instance_id, status.as_str(), now],
            )?;
            if previous != status.as_str() {
                transaction.execute(
                    "INSERT INTO instance_events (occurred_at, event_kind, instance_id, previous_value, current_value, reason_codes_json)
                     VALUES (?1, 'status_changed', ?2, ?3, ?4, ?5)",
                    params![
                        now,
                        instance_id,
                        previous,
                        status.as_str(),
                        json!(reasons).to_string()
                    ],
                )?;
            }
        }
        if let Some(kind) = kind {
            let previous_type: String = transaction.query_row(
                "SELECT instance_kind FROM instances WHERE instance_id = ?1",
                params![instance_id],
                |row| row.get(0),
            )?;
            transaction.execute(
                "UPDATE instances SET instance_kind = ?2, updated_at = ?3 WHERE instance_id = ?1",
                params![instance_id, kind.as_str(), now],
            )?;
            if previous_type != kind.as_str() {
                transaction.execute(
                    "INSERT INTO instance_events (occurred_at, event_kind, instance_id, previous_value, current_value, reason_codes_json)
                     VALUES (?1, 'type_changed', ?2, ?3, ?4, ?5)",
                    params![
                        now,
                        instance_id,
                        previous_type,
                        kind.as_str(),
                        json!(reasons).to_string()
                    ],
                )?;
            }
            let stored: String = transaction.query_row(
                "SELECT lifecycle_status FROM instances WHERE instance_id = ?1",
                params![instance_id],
                |row| row.get(0),
            )?;
            let target = if kind == InstanceType::Tor {
                (stored != InstanceStatus::Excluded.as_str()).then_some(InstanceStatus::Excluded)
            } else if stored == InstanceStatus::Excluded.as_str() {
                Some(InstanceStatus::Candidate)
            } else {
                None
            };
            if let Some(target) = target {
                transaction.execute(
                    "UPDATE instances SET lifecycle_status = ?2, updated_at = ?3 WHERE instance_id = ?1",
                    params![instance_id, target.as_str(), now],
                )?;
                transaction.execute(
                    "INSERT INTO instance_events (occurred_at, event_kind, instance_id, previous_value, current_value, reason_codes_json)
                     VALUES (?1, 'status_changed', ?2, ?3, ?4, ?5)",
                    params![
                        now,
                        instance_id,
                        stored,
                        target.as_str(),
                        json!(reasons).to_string()
                    ],
                )?;
            }
        }
        if let Some(fields) = override_fields {
            let Some(fields) = fields.as_object() else {
                return Err(Error::InvalidArgs(
                    "the override fields must be a JSON object".into(),
                ));
            };
            if let Some(unknown) = fields
                .keys()
                .find(|key| !crate::crud::OVERRIDE_FIELDS.contains(&key.as_str()))
            {
                return Err(Error::InvalidArgs(format!(
                    "unknown override field {unknown}"
                )));
            }
            if fields.contains_key("denied") {
                return Err(Error::InvalidArgs(
                    "denied is an operator intent; use the deny or allow command, not a policy field"
                        .into(),
                ));
            }
            let scan_enabled = fields.get("scanEnabled").and_then(Value::as_bool);
            let html_allowed = fields.get("htmlFallbackAllowed").and_then(Value::as_bool);
            let pinned = fields.get("pinned").and_then(Value::as_bool);
            let denied = fields.get("denied").and_then(Value::as_bool);
            let trusted = fields.get("trusted").and_then(Value::as_bool);
            transaction.execute(
                "INSERT INTO instance_policy_overrides (instance_id, is_scan_enabled, is_html_fallback_allowed, is_pinned, is_denied, reason_codes_json, created_at, updated_at, is_trusted)
                 VALUES (?1, ?2, ?3, COALESCE(?4, 0), COALESCE(?5, 0), ?6, ?7, ?7, COALESCE(?8, 0))
                 ON CONFLICT(instance_id) DO UPDATE SET
                    is_scan_enabled = COALESCE(excluded.is_scan_enabled, instance_policy_overrides.is_scan_enabled),
                    is_html_fallback_allowed = COALESCE(excluded.is_html_fallback_allowed, instance_policy_overrides.is_html_fallback_allowed),
                    is_pinned = COALESCE(?4, instance_policy_overrides.is_pinned),
                    is_denied = COALESCE(?5, instance_policy_overrides.is_denied),
                    is_trusted = COALESCE(?8, instance_policy_overrides.is_trusted),
                    reason_codes_json = excluded.reason_codes_json,
                    updated_at = excluded.updated_at",
                params![
                    instance_id,
                    scan_enabled.map(i64::from),
                    html_allowed.map(i64::from),
                    pinned.map(i64::from),
                    denied.map(i64::from),
                    json!(reasons).to_string(),
                    now,
                    trusted.map(i64::from)
                ],
            )?;
            if let Some(html) = html_allowed {
                transaction.execute(
                    "UPDATE instances SET is_html_fallback_allowed = ?2 WHERE instance_id = ?1",
                    params![instance_id, i64::from(html)],
                )?;
            }
            if let Some(scan) = scan_enabled {
                transaction.execute(
                    "UPDATE instances SET is_scan_enabled = ?2 WHERE instance_id = ?1",
                    params![instance_id, i64::from(scan)],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn upsert_override(
        &self,
        instance_id: i64,
        fields: &Value,
        reasons: &[String],
    ) -> Result<Value> {
        self.apply_policy_change(instance_id, None, None, Some(fields), reasons)?;
        self.get_override(instance_id)?
            .ok_or_else(|| Error::State("the policy override was not written".into()))
    }

    pub fn mark_search_failure(
        &self,
        instance_id: i64,
        cooldown_until: &str,
        code: &str,
    ) -> Result<()> {
        let mut connection = self.writer_conn()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = now_rfc3339();
        let previous: String = transaction.query_row(
            "SELECT lifecycle_status FROM instances WHERE instance_id = ?1",
            params![instance_id],
            |row| row.get(0),
        )?;
        transaction.execute(
            "UPDATE instances SET cooldown_until = ?2, updated_at = ?3
              WHERE instance_id = ?1",
            params![instance_id, cooldown_until, now],
        )?;
        transaction.execute(
            "INSERT INTO instance_events (occurred_at, event_kind, instance_id, previous_value, current_value, reason_codes_json)
             VALUES (?1, 'search_failed', ?2, ?3, ?3, ?4)",
            params![now, instance_id, previous, json!([code]).to_string()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn delete_override(&self, instance_id: i64) -> Result<bool> {
        let mut connection = self.writer_conn()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "DELETE FROM instance_policy_overrides WHERE instance_id = ?1",
            params![instance_id],
        )?;
        if changed == 0 {
            transaction.commit()?;
            return Ok(false);
        }
        transaction.execute(
            "UPDATE instances SET is_scan_enabled = 1, is_html_fallback_allowed = 0, updated_at = ?2
              WHERE instance_id = ?1",
            params![instance_id, now_rfc3339()],
        )?;
        transaction.commit()?;
        Ok(changed > 0)
    }

    pub fn set_status(
        &self,
        instance_id: i64,
        status: InstanceStatus,
        reasons: &[String],
    ) -> Result<()> {
        self.apply_policy_change(instance_id, Some(status), None, None, reasons)
    }

    pub fn set_type(&self, instance_id: i64, kind: InstanceType, reasons: &[String]) -> Result<()> {
        self.apply_policy_change(instance_id, None, Some(kind), None, reasons)
    }

    pub fn hard_gate_passed(&self, instance_id: i64) -> Result<bool> {
        let reader = self.reader_conn()?;
        let gate: Option<i64> = reader
            .query_row(
                "SELECT is_hard_gate_passed FROM instance_quality WHERE instance_id = ?1",
                params![instance_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(matches!(gate, Some(value) if value != 0))
    }

    pub fn apply_operator_action(
        &self,
        instance_id: i64,
        action: OperatorAction,
        reasons: &[String],
    ) -> Result<InstanceStatus> {
        self.write_atomic(|connection| {
            let now = now_rfc3339();
            let current_text: String = connection.query_row(
                "SELECT lifecycle_status FROM instances WHERE instance_id = ?1",
                params![instance_id],
                |row| row.get(0),
            )?;
            let current = InstanceStatus::parse(&current_text).ok_or_else(|| {
                Error::State(format!(
                    "stored lifecycle status {current_text:?} is not recognised"
                ))
            })?;
            let kind_text: String = connection.query_row(
                "SELECT instance_kind FROM instances WHERE instance_id = ?1",
                params![instance_id],
                |row| row.get(0),
            )?;
            let kind = InstanceType::parse(&kind_text);
            if kind == InstanceType::Tor
                && matches!(action, OperatorAction::Restore | OperatorAction::Reset)
            {
                return Err(Error::PolicyRefused(format!(
                    "instance {instance_id} is a Tor instance, which is never served; run `ownsearch instances set-type {instance_id} normal` first, then restore it"
                )));
            }
            let mut next = match action {
                OperatorAction::Deny => InstanceStatus::Quarantined,
                OperatorAction::Retire => InstanceStatus::Retired,
                OperatorAction::Allow => {
                    if matches!(current, InstanceStatus::Quarantined) {
                        InstanceStatus::Candidate
                    } else {
                        current
                    }
                }
                OperatorAction::Restore => match current {
                    InstanceStatus::Active | InstanceStatus::Probation => current,
                    _ => InstanceStatus::Candidate,
                },
                OperatorAction::Reset => InstanceStatus::Candidate,
            };
            if kind == InstanceType::Tor && action != OperatorAction::Retire {
                next = InstanceStatus::Excluded;
            }
            match action {
                OperatorAction::Deny => {
                    Self::write_override_on(connection, instance_id, Some(1), Some(0), None, &now, reasons)?;
                    connection.execute(
                        "UPDATE instances SET is_scan_enabled = 0, next_scan_at = NULL, cooldown_until = NULL, updated_at = ?2
                          WHERE instance_id = ?1",
                        params![instance_id, now],
                    )?;
                }
                OperatorAction::Allow | OperatorAction::Restore => {
                    Self::write_override_on(connection, instance_id, Some(0), Some(1), None, &now, reasons)?;
                    connection.execute(
                        "UPDATE instances SET is_scan_enabled = 1, next_scan_at = ?2, cooldown_until = NULL, updated_at = ?2
                          WHERE instance_id = ?1",
                        params![instance_id, now],
                    )?;
                }
                OperatorAction::Reset => {
                    connection.execute(
                        "DELETE FROM instance_policy_overrides WHERE instance_id = ?1",
                        params![instance_id],
                    )?;
                    connection.execute(
                        "UPDATE instances SET is_scan_enabled = 1, is_html_fallback_allowed = 0, next_scan_at = ?2,
                            cooldown_until = NULL, updated_at = ?2
                          WHERE instance_id = ?1",
                        params![instance_id, now],
                    )?;
                }
                OperatorAction::Retire => {
                    connection.execute(
                        "UPDATE instances SET next_scan_at = NULL, cooldown_until = NULL, updated_at = ?2
                          WHERE instance_id = ?1",
                        params![instance_id, now],
                    )?;
                }
            }
            if next != current {
                connection.execute(
                    "UPDATE instances SET lifecycle_status = ?2, updated_at = ?3 WHERE instance_id = ?1",
                    params![instance_id, next.as_str(), now],
                )?;
            }
            connection.execute(
                "INSERT INTO instance_events (occurred_at, event_kind, instance_id, previous_value, current_value, reason_codes_json)
                 VALUES (?1, 'policy_changed', ?2, ?3, ?4, ?5)",
                params![
                    now,
                    instance_id,
                    current.as_str(),
                    next.as_str(),
                    json!(reasons).to_string()
                ],
            )?;
            Ok(next)
        })
    }

    pub fn clear_cooldown(&self, instance_id: i64, reasons: &[String]) -> Result<bool> {
        self.write_atomic(|connection| {
            let now = now_rfc3339();
            let changed = connection.execute(
                "UPDATE instances SET cooldown_until = NULL, next_scan_at = ?2, updated_at = ?2
                  WHERE instance_id = ?1 AND cooldown_until IS NOT NULL",
                params![instance_id, now],
            )?;
            if changed > 0 {
                connection.execute(
                    "INSERT INTO instance_events (occurred_at, event_kind, instance_id, current_value, reason_codes_json)
                     VALUES (?1, 'cooldown_cleared', ?2, 'clear', ?3)",
                    params![now, instance_id, json!(reasons).to_string()],
                )?;
            }
            Ok(changed > 0)
        })
    }

    fn write_override_on(
        connection: &Connection,
        instance_id: i64,
        denied: Option<i64>,
        scan: Option<i64>,
        pinned: Option<i64>,
        now: &str,
        reasons: &[String],
    ) -> Result<()> {
        connection.execute(
            "INSERT INTO instance_policy_overrides (instance_id, is_scan_enabled, is_html_fallback_allowed, is_pinned, is_denied, reason_codes_json, created_at, updated_at)
             VALUES (?1, ?2, NULL, COALESCE(?4, 0), COALESCE(?3, 0), ?5, ?6, ?6)
             ON CONFLICT(instance_id) DO UPDATE SET
                is_scan_enabled = COALESCE(?2, instance_policy_overrides.is_scan_enabled),
                is_pinned = COALESCE(?4, instance_policy_overrides.is_pinned),
                is_denied = COALESCE(?3, instance_policy_overrides.is_denied),
                reason_codes_json = excluded.reason_codes_json,
                updated_at = excluded.updated_at",
            params![
                instance_id,
                scan,
                denied,
                pinned,
                json!(reasons).to_string(),
                now
            ],
        )?;
        Ok(())
    }

    pub fn query_entity(
        &self,
        entity: QueryEntity,
        filters: &InstanceFilter,
        limit: usize,
        offset: usize,
    ) -> Result<Value> {
        let limit = limit.clamp(1, 200);
        match entity {
            QueryEntity::Instances => {
                let rows = self.list_instances(&InstanceFilter {
                    limit,
                    offset,
                    ..filters.clone()
                })?;
                Ok(json!({
                    "entity": entity.as_str(),
                    "count": rows.len(),
                    "rows": rows.iter().map(instance_to_json).collect::<Vec<_>>(),
                }))
            }
            QueryEntity::Quality => {
                let reader = self.reader_conn()?;
                let mut statement = reader.prepare(
                    "SELECT q.instance_id, i.normalized_url, q.score_model_version, q.score,
                            q.fleet_rank, q.is_hard_gate_passed, q.reason_codes_json, q.computed_at
                       FROM instance_quality AS q
                       JOIN instances AS i ON i.instance_id = q.instance_id
                      WHERE (?1 IS NULL OR q.instance_id = ?1)
                      ORDER BY q.fleet_rank, q.score DESC, q.instance_id
                      LIMIT ?2 OFFSET ?3",
                )?;
                let rows = statement.query_map(
                    params![
                        filters.instance_id,
                        i64::try_from(limit).unwrap_or(50),
                        i64::try_from(offset).unwrap_or(0)
                    ],
                    |row| {
                        let reasons: String = row.get(6)?;
                        Ok(json!({
                            "instanceId": row.get::<_, i64>(0)?,
                            "url": row.get::<_, String>(1)?,
                            "scoreModelVersion": row.get::<_, String>(2)?,
                            "score": row.get::<_, f64>(3)?,
                            "fleetRank": row.get::<_, Option<i64>>(4)?,
                            "hardGatePassed": row.get::<_, i64>(5)? != 0,
                            "reasons": serde_json::from_str::<Value>(&reasons).unwrap_or(Value::Null),
                            "computedAt": row.get::<_, String>(7)?,
                        }))
                    },
                )?;
                let out = collect_rows(rows)?;
                Ok(json!({"entity": entity.as_str(), "count": out.len(), "rows": out}))
            }
            QueryEntity::Overrides => {
                let all = self.policy_overrides()?;
                let filtered: Vec<Value> = all
                    .into_iter()
                    .filter(|row| match filters.instance_id {
                        Some(id) => row.get("instanceId").and_then(Value::as_i64) == Some(id),
                        None => true,
                    })
                    .skip(offset)
                    .take(limit)
                    .collect();
                Ok(json!({"entity": entity.as_str(), "count": filtered.len(), "rows": filtered}))
            }
            QueryEntity::Events => {
                let rows = self.events_for(filters.instance_id, limit, offset)?;
                Ok(json!({"entity": entity.as_str(), "count": rows.len(), "rows": rows}))
            }
            QueryEntity::Capabilities => {
                let reader = self.reader_conn()?;
                let mut statement = reader.prepare(
                    "SELECT c.capability_id, c.instance_id, i.normalized_url, c.observed_at, c.outcome,
                            c.http_status, c.duration_ms, c.categories_json, c.engines_json,
                            c.locales_json, c.plugins_json, c.default_safe_search, c.is_json_enabled,
                            c.is_csv_enabled, c.is_rss_enabled, c.is_html_enabled, c.is_google_exact_seen, c.is_google_cse_seen,
                            c.instance_name
                       FROM capability_snapshots AS c
                       JOIN instances AS i ON i.instance_id = c.instance_id
                      WHERE (?1 IS NULL OR c.instance_id = ?1)
                      ORDER BY c.capability_id DESC LIMIT ?2 OFFSET ?3",
                )?;
                let rows = statement.query_map(
                    params![
                        filters.instance_id,
                        i64::try_from(limit).unwrap_or(50),
                        i64::try_from(offset).unwrap_or(0)
                    ],
                    |row| {
                        let parse = |raw: String| {
                            serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null)
                        };
                        Ok(json!({
                            "capabilityId": row.get::<_, i64>(0)?,
                            "instanceId": row.get::<_, i64>(1)?,
                            "url": row.get::<_, String>(2)?,
                            "observedAt": row.get::<_, String>(3)?,
                            "outcome": row.get::<_, String>(4)?,
                            "httpStatus": row.get::<_, Option<i64>>(5)?,
                            "durationMs": row.get::<_, Option<i64>>(6)?,
                            "categories": parse(row.get::<_, String>(7)?),
                            "engines": parse(row.get::<_, String>(8)?),
                            "locales": parse(row.get::<_, String>(9)?),
                            "plugins": parse(row.get::<_, String>(10)?),
                            "instanceName": row.get::<_, Option<String>>(18)?,
                            "safeSearchDefault": row.get::<_, Option<i64>>(11)?,
                            "jsonEnabled": row.get::<_, i64>(12)? != 0,
                            "csvEnabled": row.get::<_, i64>(13)? != 0,
                            "rssEnabled": row.get::<_, i64>(14)? != 0,
                            "htmlEnabled": row.get::<_, i64>(15)? != 0,
                            "googleExact": row.get::<_, i64>(16)? != 0,
                            "googleCse": row.get::<_, i64>(17)? != 0,
                        }))
                    },
                )?;
                let out = collect_rows(rows)?;
                Ok(json!({"entity": entity.as_str(), "count": out.len(), "rows": out}))
            }
            QueryEntity::Metrics => {
                let reader = self.reader_conn()?;
                let sql = match filters.instance_id {
                    Some(_) => {
                        "SELECT m.instance_id, i.normalized_url, m.probe_kind, m.window_start_utc,
                                m.attempt_count, m.success_count, m.failure_count, m.success_rate,
                                m.duration_last_ms, m.duration_min_ms, m.duration_max_ms,
                                m.total_result_count, m.response_bytes, m.updated_at
                           FROM hourly_probe_metrics AS m
                           JOIN instances AS i ON i.instance_id = m.instance_id
                          WHERE m.instance_id = ?1
                          ORDER BY m.window_start_utc DESC, m.probe_kind
                          LIMIT ?2 OFFSET ?3"
                    }
                    None => {
                        "SELECT m.instance_id, i.normalized_url, m.probe_kind, m.window_start_utc,
                                m.attempt_count, m.success_count, m.failure_count, m.success_rate,
                                m.duration_last_ms, m.duration_min_ms, m.duration_max_ms,
                                m.total_result_count, m.response_bytes, m.updated_at
                           FROM hourly_probe_metrics AS m
                           JOIN instances AS i ON i.instance_id = m.instance_id
                          ORDER BY m.window_start_utc DESC, m.instance_id, m.probe_kind
                          LIMIT ?2 OFFSET ?1"
                    }
                };
                let mut statement = reader.prepare(sql)?;
                let mapper = |row: &rusqlite::Row<'_>| {
                    Ok(json!({
                        "instanceId": row.get::<_, i64>(0)?,
                        "url": row.get::<_, String>(1)?,
                        "probeKind": row.get::<_, String>(2)?,
                        "windowStartUtc": row.get::<_, String>(3)?,
                        "attemptCount": row.get::<_, i64>(4)?,
                        "successCount": row.get::<_, i64>(5)?,
                        "failureCount": row.get::<_, i64>(6)?,
                        "successRate": row.get::<_, f64>(7)?,
                        "durationLastMs": row.get::<_, Option<i64>>(8)?,
                        "durationMinMs": row.get::<_, Option<i64>>(9)?,
                        "durationMaxMs": row.get::<_, Option<i64>>(10)?,
                        "totalResultCount": row.get::<_, i64>(11)?,
                        "responseBytes": row.get::<_, i64>(12)?,
                        "updatedAt": row.get::<_, String>(13)?,
                    }))
                };
                let rows = match filters.instance_id {
                    Some(id) => statement.query_map(
                        params![
                            id,
                            i64::try_from(limit).unwrap_or(20),
                            i64::try_from(offset).unwrap_or(0)
                        ],
                        mapper,
                    )?,
                    None => statement.query_map(
                        params![
                            i64::try_from(offset).unwrap_or(0),
                            i64::try_from(limit).unwrap_or(20)
                        ],
                        mapper,
                    )?,
                };
                let out: Vec<Value> = rows.collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(json!({"entity": entity.as_str(), "count": out.len(), "rows": out}))
            }
            QueryEntity::Snapshots => {
                let reader = self.reader_conn()?;
                let mut statement = reader.prepare(
                    "SELECT snapshot_id, generated_at, snapshot_kind, payload_format, instance_count, byte_size, payload_digest_sha256
                       FROM fleet_snapshots ORDER BY snapshot_id DESC LIMIT ?1 OFFSET ?2",
                )?;
                let rows = statement.query_map(
                    params![
                        i64::try_from(limit).unwrap_or(20),
                        i64::try_from(offset).unwrap_or(0)
                    ],
                    |row| {
                        Ok(json!({
                            "snapshotId": row.get::<_, i64>(0)?,
                            "generatedAt": row.get::<_, String>(1)?,
                            "kind": row.get::<_, String>(2)?,
                            "format": row.get::<_, String>(3)?,
                            "instanceCount": row.get::<_, i64>(4)?,
                            "byteSize": row.get::<_, i64>(5)?,
                        }))
                    },
                )?;
                let out = collect_rows(rows)?;
                Ok(json!({"entity": entity.as_str(), "count": out.len(), "rows": out}))
            }
            QueryEntity::Runs => {
                let reader = self.reader_conn()?;
                let mut statement = reader.prepare(
                    "SELECT run_id, started_at, completed_at, outcome, discovered_count, changed_count,
                            missing_count, scanner_version, error_code
                       FROM discovery_runs ORDER BY started_at DESC, run_id DESC LIMIT ?1 OFFSET ?2",
                )?;
                let rows = statement.query_map(
                    params![
                        i64::try_from(limit).unwrap_or(20),
                        i64::try_from(offset).unwrap_or(0)
                    ],
                    |row| {
                        Ok(json!({
                            "runId": row.get::<_, String>(0)?,
                            "startedAt": row.get::<_, String>(1)?,
                            "completedAt": row.get::<_, Option<String>>(2)?,
                            "outcome": row.get::<_, String>(3)?,
                            "discoveredCount": row.get::<_, i64>(4)?,
                            "changedCount": row.get::<_, i64>(5)?,
                            "missingCount": row.get::<_, i64>(6)?,
                            "scannerVersion": row.get::<_, String>(7)?,
                            "errorCode": row.get::<_, Option<String>>(8)?,
                        }))
                    },
                )?;
                let out = collect_rows(rows)?;
                Ok(json!({"entity": entity.as_str(), "count": out.len(), "rows": out}))
            }
        }
    }
}

fn verify_owner(connection: &Connection, path: &Path) -> Result<()> {
    let owner: i64 = connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    if owner != 0 && owner != i64::from(APPLICATION_ID) {
        return Err(Error::StateCorrupted(format!(
            "{} belongs to another application (application_id {owner}); choose another state directory",
            path.display()
        )));
    }
    if owner == 0 {
        let tables: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;
        if tables > 0 {
            let ledger: i64 = connection.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
                [],
                |row| row.get(0),
            )?;
            if ledger == 0 {
                return Err(Error::StateCorrupted(format!(
                    "{} holds tables that OwnSearch did not create; choose another state directory",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

fn backup_before_migration(connection: &Connection, path: &Path) -> Result<()> {
    if path == Path::new(":memory:") {
        return Ok(());
    }
    let tables: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    if tables == 0 {
        return Ok(());
    }
    let stamp = now_rfc3339().replace([':', '.'], "-").replace('T', "_");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    let target = path.with_file_name(format!("pre-migration-{stamp}-{nanos}.db"));
    connection.execute("VACUUM INTO ?1", params![utf8_path(&target)?])?;
    let check = Connection::open(&target)?;
    let result: String = check.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if !result.eq_ignore_ascii_case("ok") {
        return Err(Error::StateCorrupted(format!(
            "the pre-migration backup {} did not pass the quick check",
            target.display()
        )));
    }
    let orphans: i64 =
        check.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if orphans > 0 {
        return Err(Error::StateCorrupted(format!(
            "the pre-migration backup {} holds {orphans} orphan rows",
            target.display()
        )));
    }
    Ok(())
}

fn unique_slug(connection: &Connection, url: &str) -> Result<String> {
    let base = crate::types::instance_slug(url);
    let mut candidate = base.clone();
    let mut suffix = 2u32;
    loop {
        let taken: Option<i64> = connection
            .query_row(
                "SELECT instance_id FROM instances WHERE instance_slug = ?1 AND normalized_url <> ?2",
                params![candidate, url],
                |row| row.get(0),
            )
            .optional()?;
        if taken.is_none() {
            return Ok(candidate);
        }
        candidate = format!("{base}-{suffix}");
        suffix = suffix.saturating_add(1);
    }
}

fn utf8_path(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        Error::State(format!(
            "{} is not valid UTF-8, and SQLite text arguments need a UTF-8 path",
            path.display()
        ))
    })
}

fn collect_counts(connection: &Connection, sql: &str) -> Result<Vec<(String, i64)>> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let out = collect_rows(rows)?;
    Ok(out)
}

fn bump_engine(
    connection: &Connection,
    instance_id: i64,
    engine: &str,
    success: bool,
    now: &str,
) -> Result<()> {
    if engine.is_empty() || engine.chars().count() > 64 {
        return Ok(());
    }
    connection.execute(
        "INSERT INTO engine_stats (instance_id, engine_name, success_count, failure_count, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(instance_id, engine_name) DO UPDATE SET
            success_count = engine_stats.success_count + excluded.success_count,
            failure_count = engine_stats.failure_count + excluded.failure_count,
            last_seen_at = excluded.last_seen_at",
        params![
            instance_id,
            engine,
            i64::from(success),
            i64::from(!success),
            now
        ],
    )?;
    Ok(())
}

fn upsert_name(
    connection: &Connection,
    table: &str,
    id_column: &str,
    name_column: &str,
    value: &str,
) -> Result<i64> {
    connection.execute(
        &format!("INSERT OR IGNORE INTO {table} ({name_column}) VALUES (?1)"),
        params![value],
    )?;
    let id: i64 = connection.query_row(
        &format!("SELECT {id_column} FROM {table} WHERE {name_column} = ?1"),
        params![value],
        |row| row.get(0),
    )?;
    Ok(id)
}

fn collect_rows<T>(rows: impl Iterator<Item = rusqlite::Result<T>>) -> Result<Vec<T>> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn map_instance_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InstanceRow> {
    let status: String = row.get(4)?;
    let kind: String = row.get(3)?;
    let reasons: Option<String> = row.get(9)?;
    Ok(InstanceRow {
        instance_id: row.get(0)?,
        slug: row.get(27)?,
        url: row.get(1)?,
        host: row.get(2)?,
        kind: InstanceType::parse(&kind),
        status: InstanceStatus::parse(&status).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other(format!(
                    "unknown lifecycle status {status:?}"
                ))),
            )
        })?,
        version: row.get(5)?,
        score: row.get(6)?,
        rank: row.get(7)?,
        hard_gate_passed: row.get::<_, Option<i64>>(8)?.map(|value| value != 0),
        reasons: reasons
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .unwrap_or_default(),
        json_enabled: row.get::<_, Option<i64>>(10)?.map(|value| value != 0),
        html_enabled: row.get::<_, Option<i64>>(11)?.map(|value| value != 0),
        google_exact: row
            .get::<_, Option<i64>>(12)?
            .map(|value| value != 0)
            .unwrap_or(false),
        google_cse: row
            .get::<_, Option<i64>>(13)?
            .map(|value| value != 0)
            .unwrap_or(false),
        consecutive_successes: row.get(14)?,
        consecutive_failures: row.get(15)?,
        last_success_at: row.get(16)?,
        last_failure_at: row.get(17)?,
        next_scan_at: row.get(18)?,
        cooldown_until: row.get(19)?,
        scan_enabled: row.get::<_, i64>(20)? != 0,
        html_fallback_allowed: row.get::<_, i64>(21)? != 0,
        discovery_rank: row.get(22)?,
        network_group: row.get(23)?,
        added_at: row.get(24)?,
        pinned: row.get::<_, i64>(25)? != 0,
        denied: row.get::<_, i64>(26)? != 0,
        trusted: row.get::<_, i64>(28)? != 0,
        source: row.get(29)?,
        json_throttled_until: row.get(30)?,
        manual_priority: row.get(31)?,
    })
}

pub fn instance_to_json(row: &InstanceRow) -> Value {
    json!({
        "instanceId": row.instance_id,
        "slug": row.slug,
        "url": row.url,
        "host": row.host,
        "type": row.kind.as_str(),
        "status": row.status.as_str(),
        "version": row.version,
        "score": row.score,
        "rank": row.rank,
        "hardGatePassed": row.hard_gate_passed,
        "reasons": row.reasons,
        "jsonEnabled": row.json_enabled,
        "htmlEnabled": row.html_enabled,
        "googleExact": row.google_exact,
        "googleCse": row.google_cse,
        "consecutiveSuccesses": row.consecutive_successes,
        "consecutiveFailures": row.consecutive_failures,
        "lastSuccessAt": row.last_success_at,
        "lastFailureAt": row.last_failure_at,
        "nextScanAt": row.next_scan_at,
        "cooldownUntil": row.cooldown_until,
        "scanEnabled": row.scan_enabled,
        "htmlFallbackAllowed": row.html_fallback_allowed,
        "discoveryRank": row.discovery_rank,
        "networkGroup": row.network_group,
        "pinned": row.pinned,
        "denied": row.denied,
        "trusted": row.trusted,
        "source": row.source,
        "jsonThrottledUntil": row.json_throttled_until,
        "priority": row.manual_priority,
        "addedAt": row.added_at,
    })
}

fn lock_down_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(
            |error| {
                Error::State(format!(
                    "cannot restrict {} to its owner: {error}",
                    path.display()
                ))
            },
        )?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
