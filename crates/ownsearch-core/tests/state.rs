#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use ownsearch_core::BootstrapSnapshot;
use ownsearch_core::{CapabilityRecord, ProbeOutcome, Store, now_rfc3339, now_unix};
use ownsearch_core::{InstanceFilter, ScanUpdate};
use ownsearch_core::{InstanceStatus, QueryEntity};

fn imported(name: &str) -> (Store, std::path::PathBuf) {
    let dir = common::temp_dir(name);
    let store = Store::open(&dir.join("instances.db")).expect("store");
    (store, dir)
}

#[test]
fn a_fresh_database_lands_on_schema_one_with_wal_and_foreign_keys() {
    let (store, dir) = imported("schema");
    assert_eq!(store.schema_version().expect("version"), 1);
    store.quick_check().expect("quick check");
    store.foreign_key_check().expect("foreign keys");
    assert!(dir.join("instances.db").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_bootstrap_import_creates_every_candidate_once() {
    let (store, dir) = imported("bootstrap");
    let snapshot = BootstrapSnapshot::embedded().expect("snapshot");
    let now = now_unix();
    let (added, total) = store.import_bootstrap(&snapshot, now).expect("import");
    assert_eq!(added, 67);
    assert_eq!(total, 67);
    let (again, _) = store.import_bootstrap(&snapshot, now).expect("import");
    assert_eq!(again, 0);
    let counts = store.counts().expect("counts");
    assert_eq!(counts.total, 67);
    assert_eq!(counts.active, 0);
    assert_eq!(
        counts
            .by_status
            .iter()
            .find(|(status, _)| status == "candidate")
            .map(|(_, count)| *count),
        Some(67)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_status_change_persists_and_lands_in_the_event_history() {
    let (store, dir) = imported("status");
    let snapshot = BootstrapSnapshot::embedded().expect("snapshot");
    store
        .import_bootstrap(&snapshot, now_unix())
        .expect("import");
    let row = store
        .list_instances(&InstanceFilter {
            limit: 1,
            ..InstanceFilter::default()
        })
        .expect("rows")
        .into_iter()
        .next()
        .expect("row");
    store
        .set_status(
            row.instance_id,
            InstanceStatus::Retired,
            &["operator.retire".to_owned()],
        )
        .expect("status");
    let after = store
        .get_instance(row.instance_id)
        .expect("read")
        .expect("row");
    assert_eq!(after.status, InstanceStatus::Retired);
    let events = store
        .events_for(Some(row.instance_id), 5, 0)
        .expect("events");
    assert!(
        events
            .iter()
            .any(|event| event.get("newValue").and_then(|value| value.as_str()) == Some("retired"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_scan_update_sets_the_score_and_rank_used_by_the_fleet_query() {
    let (store, dir) = imported("quality");
    let snapshot = BootstrapSnapshot::embedded().expect("snapshot");
    store
        .import_bootstrap(&snapshot, now_unix())
        .expect("import");
    let row = store
        .list_instances(&InstanceFilter {
            limit: 1,
            ..InstanceFilter::default()
        })
        .expect("rows")
        .into_iter()
        .next()
        .expect("row");
    store
        .apply_scan_update(&ScanUpdate {
            instance_id: row.instance_id,
            status: InstanceStatus::Active,
            previous_status: row.status,
            successes: 3,
            failures: 0,
            score: 91.5,
            rank: Some(1),
            hard_gate_passed: true,
            reasons: vec!["probe.json_ok".to_owned()],
            next_scan_at: None,
            cooldown_until: None,
            last_success_at: Some(now_rfc3339()),
            last_failure_at: None,
            version: None,
            capability_id: None,
            tls_id: None,
        })
        .expect("update");
    let fleet = store.fleet(10).expect("fleet");
    assert_eq!(fleet.len(), 1);
    assert_eq!(
        fleet.first().map(|item| item.instance_id),
        Some(row.instance_id)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_snapshot_export_is_best_first_and_keeps_every_instance() {
    let (store, dir) = imported("snapshot");
    let snapshot = BootstrapSnapshot::embedded().expect("snapshot");
    store
        .import_bootstrap(&snapshot, now_unix())
        .expect("import");
    let target = dir.join("instances.snapshot.json");
    let (count, bytes) = store.write_snapshot(&target).expect("snapshot");
    assert_eq!(count, 67);
    assert!(bytes > 1_000);
    let exported: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&target).expect("read")).expect("json");
    assert_eq!(
        exported.get("schemaVersion").and_then(|item| item.as_i64()),
        Some(1)
    );
    assert_eq!(
        exported
            .get("instances")
            .and_then(|item| item.as_array())
            .map(Vec::len),
        Some(67)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_backup_is_a_verified_copy() {
    let (store, dir) = imported("backup");
    let snapshot = BootstrapSnapshot::embedded().expect("snapshot");
    store
        .import_bootstrap(&snapshot, now_unix())
        .expect("import");
    let target = dir.join("backups").join("instances-test.db");
    store.backup(&target).expect("backup");
    let copy = Store::open(&target).expect("open copy");
    copy.quick_check().expect("copy check");
    assert_eq!(copy.instance_count().expect("count"), 67);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn maintenance_reports_the_wal_and_expires_nothing_young() {
    let (store, dir) = imported("maintenance");
    let snapshot = BootstrapSnapshot::embedded().expect("snapshot");
    store
        .import_bootstrap(&snapshot, now_unix())
        .expect("import");
    let checkpoint = store.maintenance().expect("maintenance");
    assert!(checkpoint.get("checkpointBusy").is_some());
    let expired = store
        .expire(ownsearch_core::KeepWindow::default())
        .expect("expire");
    assert_eq!(
        expired.get("events").and_then(|item| item.as_u64()),
        Some(0)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_query_returns_bounded_rows_without_sql() {
    let (store, dir) = imported("query");
    let snapshot = BootstrapSnapshot::embedded().expect("snapshot");
    store
        .import_bootstrap(&snapshot, now_unix())
        .expect("import");
    let page = store
        .query_entity(
            QueryEntity::Instances,
            &InstanceFilter {
                limit: 5,
                ..InstanceFilter::default()
            },
            5,
            0,
        )
        .expect("query");
    assert_eq!(page.get("count").and_then(|item| item.as_u64()), Some(5));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_candidate_imports_as_an_https_url() {
    let (store, dir) = imported("https");
    let snapshot = BootstrapSnapshot::embedded().expect("snapshot");
    store
        .import_bootstrap(&snapshot, now_unix())
        .expect("import");
    let rows = store
        .list_instances(&InstanceFilter {
            limit: 500,
            ..InstanceFilter::default()
        })
        .expect("rows");
    assert_eq!(rows.len(), 67);
    assert!(rows.iter().all(|row| row.url.starts_with("https://")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn expiry_removes_old_probes_with_their_tls_rows_and_event_links() {
    let (store, dir) = imported("expire");
    let snapshot = BootstrapSnapshot::embedded().expect("snapshot");
    store
        .import_bootstrap(&snapshot, now_unix())
        .expect("import");
    let row = store
        .list_instances(&InstanceFilter {
            limit: 1,
            ..InstanceFilter::default()
        })
        .expect("rows")
        .into_iter()
        .next()
        .expect("row");
    let probe_id = store
        .insert_probe(&ownsearch_core::ProbeRecord {
            instance_id: row.instance_id,
            run_id: None,
            kind: ownsearch_core::ProbeKind::Tls,
            outcome: ownsearch_core::ProbeOutcome::Success,
            started_at: "2000-01-01T00:00:00.000Z".to_owned(),
            completed_at: "2000-01-01T00:00:01.000Z".to_owned(),
            duration_ms: 10,
            http_status: None,
            content_type: None,
            result_count: None,
            response_bytes: None,
            response_digest_sha256: None,
            error_code: None,
            error_message: None,
        })
        .expect("probe");
    store
        .insert_tls(&ownsearch_core::TlsRecord {
            instance_id: row.instance_id,
            probe_id,
            protocol: Some("TLSv1.3".to_owned()),
            issuer: None,
            not_before: None,
            not_after: None,
            hostname_verified: true,
            chain_valid: true,
            error_code: None,
            error_message: None,
        })
        .expect("tls");
    store
        .append_event(&ownsearch_core::EventRecord {
            kind: "probe_completed".to_owned(),
            instance_id: Some(row.instance_id),
            run_id: None,
            probe_id: Some(probe_id),
            old_value: None,
            new_value: None,
            reasons: vec!["test".to_owned()],
            payload: None,
        })
        .expect("event");
    let expired = store
        .expire(ownsearch_core::KeepWindow::default())
        .expect("expire");
    assert_eq!(
        expired.get("probes").and_then(|item| item.as_u64()),
        Some(1)
    );
    assert_eq!(expired.get("tls").and_then(|item| item.as_u64()), Some(1));
    let counts = store.counts().expect("counts");
    assert_eq!(counts.probes, 0);
    let events = store
        .events_for(Some(row.instance_id), 10, 0)
        .expect("events");
    assert!(
        events.iter().all(|event| event.get("probeId").is_none()
            || event.get("probeId") == Some(&serde_json::Value::Null)),
        "an event kept a pointer to a deleted probe: {events:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn deleting_an_override_restores_the_instance_policy_flags() {
    let (store, dir) = imported("override-reset");
    let snapshot = BootstrapSnapshot::embedded().expect("snapshot");
    store
        .import_bootstrap(&snapshot, now_unix())
        .expect("import");
    let row = store
        .list_instances(&InstanceFilter {
            limit: 1,
            ..InstanceFilter::default()
        })
        .expect("rows")
        .into_iter()
        .next()
        .expect("row");
    store
        .upsert_override(
            row.instance_id,
            &serde_json::json!({"scanEnabled": false, "denied": true}),
            &["operator.deny".to_owned()],
        )
        .expect("override");
    let denied = store
        .get_instance(row.instance_id)
        .expect("read")
        .expect("row");
    assert!(!denied.scan_enabled);
    store.delete_override(row.instance_id).expect("delete");
    let restored = store
        .get_instance(row.instance_id)
        .expect("read")
        .expect("row");
    assert!(restored.scan_enabled);
    assert!(!restored.html_fallback_allowed);
    let targets = store.scan_targets(500, &now_rfc3339()).expect("targets");
    assert!(
        targets
            .iter()
            .any(|item| item.instance_id == row.instance_id)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_bootstrap_import_is_recorded_once() {
    let dir = common::temp_dir("bootstrap-marker");
    let store = Store::open(&dir.join("instances.db")).expect("store");
    let snapshot = BootstrapSnapshot::embedded().expect("snapshot");
    assert!(!store.bootstrap_imported().expect("marker"));
    let (added, _) = store
        .import_bootstrap(&snapshot, now_unix())
        .expect("import");
    assert_eq!(added, 67);
    assert!(store.bootstrap_imported().expect("marker"));
    let (again, _) = store
        .import_bootstrap(&snapshot, now_unix())
        .expect("import");
    assert_eq!(again, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_foreign_sqlite_file_is_refused() {
    let dir = common::temp_dir("foreign-db");
    let path = dir.join("instances.db");
    let connection = rusqlite::Connection::open(&path).expect("open");
    connection
        .execute_batch("CREATE TABLE someone_elses (id INTEGER PRIMARY KEY) STRICT;")
        .expect("table");
    drop(connection);
    let error = match Store::open(&path) {
        Ok(_) => panic!("a foreign file must be refused"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("did not create"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_empty_database_answers_none_for_metrics_and_runs() {
    let (store, dir) = imported("empty-read");
    assert!(store.discovery_metrics(42).expect("metrics").is_none());
    assert!(store.last_run_at("searx_space").expect("runs").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn expiry_clears_an_event_pointer_before_deleting_an_old_run() {
    let (store, dir) = imported("expire-run");
    let snapshot = BootstrapSnapshot::embedded().expect("snapshot");
    store
        .import_bootstrap(&snapshot, now_unix())
        .expect("import");
    store
        .ensure_source("test", "test", "wiremock")
        .expect("source");
    let run = store.begin_run("test", "0.1.0").expect("run");
    let row = store
        .list_instances(&InstanceFilter {
            limit: 1,
            ..InstanceFilter::default()
        })
        .expect("rows")
        .into_iter()
        .next()
        .expect("row");
    store
        .append_event(&ownsearch_core::EventRecord {
            kind: "instance_added".to_owned(),
            instance_id: Some(row.instance_id),
            run_id: Some(run.clone()),
            probe_id: None,
            old_value: None,
            new_value: None,
            reasons: vec!["test".to_owned()],
            payload: None,
        })
        .expect("event");
    let path = dir.join("instances.db");
    let connection = rusqlite::Connection::open(&path).expect("open");
    connection
        .execute(
            "UPDATE discovery_runs SET started_at = '2000-01-01T00:00:00.000Z' WHERE run_id = ?1",
            [&run],
        )
        .expect("age run");
    drop(connection);
    let keep_window = ownsearch_core::KeepWindow {
        run_days: 180,
        ..ownsearch_core::KeepWindow::default()
    };
    let report = store.expire(keep_window).expect("expire");
    assert!(
        report
            .get("runs")
            .and_then(|item| item.as_u64())
            .unwrap_or(0)
            >= 1
    );
    let connection = rusqlite::Connection::open(&path).expect("open");
    let remaining: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM instance_events WHERE discovery_run_id = ?1",
            [&run],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(remaining, 0, "an event kept a pointer to a deleted run");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_expiry_sweep_removes_capability_children_with_their_parent() {
    let (store, dir) = imported("expire-capability");
    let snapshot = BootstrapSnapshot::embedded().expect("snapshot");
    store
        .import_bootstrap(&snapshot, now_unix())
        .expect("import");
    let capability_id = store
        .insert_capability(&CapabilityRecord {
            instance_id: 1,
            run_id: None,
            outcome: ProbeOutcome::Success,
            http_status: Some(200),
            duration_ms: Some(5),
            response_digest_sha256: None,
            instance_name: Some("Example".to_owned()),
            categories: vec!["general".to_owned()],
            engines: vec!["google".to_owned(), "bing".to_owned()],
            locales: vec!["en".to_owned()],
            plugins: Vec::new(),
            safe_search_default: Some(0),
            json_enabled: true,
            csv_enabled: false,
            rss_enabled: false,
            html_enabled: true,
            google_exact_seen: false,
            google_cse_seen: false,
        })
        .expect("capability");
    let path = dir.join("instances.db");
    let connection = rusqlite::Connection::open(&path).expect("open");
    connection
        .execute(
            "UPDATE capability_snapshots SET observed_at = '2000-01-01T00:00:00.000Z'
              WHERE capability_id = ?1",
            [capability_id],
        )
        .expect("age capability");
    drop(connection);
    let report = store
        .expire(ownsearch_core::KeepWindow::default())
        .expect("expire");
    assert!(
        report
            .get("capabilities")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            >= 1,
        "{report}"
    );
    let connection = rusqlite::Connection::open(&path).expect("open");
    let engines: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM capability_engines WHERE capability_id = ?1",
            [capability_id],
            |row| row.get(0),
        )
        .expect("count engines");
    assert_eq!(engines, 0, "an engine row outlived its capability");
    let current: Option<i64> = connection
        .query_row(
            "SELECT current_capability_id FROM instances WHERE instance_id = 1",
            [],
            |row| row.get(0),
        )
        .expect("count instances");
    assert_eq!(current, None);
    drop(connection);
    store.foreign_key_check().expect("foreign keys");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_hosts_that_produce_the_same_slug_stay_unique() {
    use ownsearch_core::InstanceType;
    let (store, dir) = imported("slug-unique");
    store
        .ensure_source("test", "test", "wiremock")
        .expect("source");
    let build = |url: &str| ownsearch_core::DiscoveredInstance {
        url: url.to_owned(),
        kind: InstanceType::Normal,
        rank: Some(1),
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
    let (first, _) = store
        .upsert_instance(
            &build("https://a.b.example/"),
            "test",
            None,
            "2026-10-26T00:00:00.000Z",
        )
        .expect("first");
    let (second, _) = store
        .upsert_instance(
            &build("https://a-b.example/"),
            "test",
            None,
            "2026-10-26T00:00:00.000Z",
        )
        .expect("second");
    let one = store.get_instance(first).expect("read").expect("row");
    let two = store.get_instance(second).expect("read").expect("row");
    assert_eq!(one.slug, "a-b-example");
    assert_eq!(two.slug, "a-b-example-2");
    assert_ne!(one.slug, two.slug);
    let _ = std::fs::remove_dir_all(&dir);
}
