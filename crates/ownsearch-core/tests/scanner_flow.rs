#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use std::sync::Arc;

use ownsearch_core::DiscoveredInstance;
use ownsearch_core::InstanceType;
use ownsearch_core::{InstanceStatus, ScanOptions, ScanUpdate, refresh_discovery, run_scan};
use ownsearch_core::{Settings, Store, ToolContext};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn discovered(url: &str, rank: u32) -> DiscoveredInstance {
    DiscoveredInstance {
        url: url.to_owned(),
        kind: InstanceType::Normal,
        rank: Some(rank),
        version: Some("2026.10.21+dedeb8f7d".to_owned()),
        tls_grade: Some("A+".to_owned()),
        tls_protocol: Some("TLSv1.3".to_owned()),
        certificate_not_after: None,
        certificate_days_remaining: Some(80),
        http_grade: Some("A+".to_owned()),
        csp_grade: Some("A+".to_owned()),
        html_grade: Some("C".to_owned()),
        analytics: false,
        ipv6: false,
        uptime_month: Some(100.0),
        uptime_year: Some(99.0),
        search_success: Some(100.0),
        search_median_secs: Some(0.6),
        initial_success: Some(100.0),
        initial_secs: Some(0.2),
        google_success: Some(0.0),
        google_secs: None,
        working_engines: vec!["duckduckgo".to_owned()],
        exact_google_capability: false,
        google_cse_capability: false,
        asn_cidr: Some("192.0.2.0/24".to_owned()),
    }
}

fn context(settings: &Settings, urls: &[&str]) -> Arc<ToolContext> {
    let ctx = Arc::new(ToolContext::new(settings.clone()).expect("context"));
    ctx.store
        .ensure_source("test", "test", "wiremock")
        .expect("source");
    for (index, url) in urls.iter().enumerate() {
        ctx.store
            .upsert_instance(
                &discovered(url, u32::try_from(index + 1).unwrap_or(1)),
                "test",
                None,
                "2026-10-26T00:00:00.000Z",
            )
            .expect("insert");
    }
    ctx
}

async fn config_and_json(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/config"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "categories": ["general"],
            "engines": [{"name": "duckduckgo", "enabled": true}],
            "locales": {"en": "English"},
            "plugins": [],
            "safe_search": 1,
            "version": "2026.10.21+dedeb8f7d"
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"url": "https://a.example/", "title": "A", "content": "one", "engine": "duckduckgo"}]
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_scan_probes_persists_and_records_status_changes() {
    let server = MockServer::start().await;
    config_and_json(&server).await;
    let dir = common::temp_dir("scan-flow");
    let settings = common::settings(&dir);
    let ctx = context(&settings, &[&format!("{}/", server.uri())]);
    let output = run_scan(
        &ctx,
        &ScanOptions {
            dry_run: false,
            instance_id: None,
            limit: 10,
            force_discovery: false,
            probe_search: true,
            force: false,
            probe_google: false,
        },
        &CancellationToken::new(),
    )
    .await
    .expect("scan");
    assert_eq!(output.probed, 1);
    assert!(!output.dry_run);
    assert_eq!(output.status_changes.len(), 0);
    let counts = ctx.store.counts().expect("counts");
    assert!(counts.probes >= 2, "probes {}", counts.probes);
    assert!(counts.events > 0);
    let row = ctx.store.get_instance(1).expect("read").expect("row");
    assert_eq!(row.status.as_str(), "candidate");
    assert!(row.hard_gate_passed == Some(true));
    assert!(row.json_enabled == Some(true));
    assert!(row.score.is_some());
    let second = run_scan(
        &ctx,
        &ScanOptions {
            dry_run: false,
            instance_id: None,
            limit: 10,
            force_discovery: false,
            probe_search: true,
            force: true,
            probe_google: false,
        },
        &CancellationToken::new(),
    )
    .await
    .expect("second scan");
    assert_eq!(second.status_changes.len(), 1);
    let row = ctx.store.get_instance(1).expect("read").expect("row");
    assert_eq!(row.status.as_str(), "active");
    let page = ctx
        .store
        .query_entity(
            ownsearch_core::QueryEntity::Capabilities,
            &ownsearch_core::InstanceFilter::default(),
            10,
            0,
        )
        .expect("capabilities");
    assert!(page.get("rows").and_then(|rows| rows.as_array()).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_dry_run_scan_writes_nothing() {
    let server = MockServer::start().await;
    config_and_json(&server).await;
    let dir = common::temp_dir("scan-dry");
    let settings = common::settings(&dir);
    let ctx = context(&settings, &[&format!("{}/", server.uri())]);
    let output = run_scan(
        &ctx,
        &ScanOptions {
            dry_run: true,
            instance_id: None,
            limit: 10,
            force_discovery: false,
            probe_search: true,
            force: false,
            probe_google: false,
        },
        &CancellationToken::new(),
    )
    .await
    .expect("scan");
    assert!(output.dry_run);
    assert_eq!(output.probed, 1);
    let counts = ctx.store.counts().expect("counts");
    assert_eq!(counts.probes, 0);
    let row = ctx.store.get_instance(1).expect("read").expect("row");
    assert_eq!(row.status.as_str(), "candidate");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_rate_limited_probe_cools_down_and_keeps_the_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/config"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"engines": [], "categories": []})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .mount(&server)
        .await;
    let dir = common::temp_dir("scan-429");
    let settings = common::settings(&dir);
    let ctx = context(&settings, &[&format!("{}/", server.uri())]);
    let output = run_scan(
        &ctx,
        &ScanOptions {
            dry_run: false,
            instance_id: None,
            limit: 10,
            force_discovery: false,
            probe_search: true,
            force: false,
            probe_google: false,
        },
        &CancellationToken::new(),
    )
    .await
    .expect("scan");
    let row = ctx.store.get_instance(1).expect("read").expect("row");
    assert_eq!(row.status.as_str(), "candidate");
    assert!(row.cooldown_until.is_some());
    assert!(
        output
            .status_changes
            .iter()
            .all(|change| change.next_status == "candidate")
    );
    assert_eq!(server.received_requests().await.expect("requests").len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_challenge_page_quarantines_the_instance() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/config"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"engines": [], "categories": []})),
        )
        .mount(&server)
        .await;
    let markup = r#"<html><head><script id="anubis_challenge">{}</script></head>
        <body><h1>Making sure you're not a bot!</h1></body></html>"#;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(markup.as_bytes().to_vec(), "text/html"),
        )
        .mount(&server)
        .await;
    let dir = common::temp_dir("scan-challenge");
    let settings = common::settings(&dir);
    let ctx = context(&settings, &[&format!("{}/", server.uri())]);
    run_scan(
        &ctx,
        &ScanOptions {
            dry_run: false,
            instance_id: None,
            limit: 10,
            force_discovery: false,
            probe_search: true,
            force: false,
            probe_google: false,
        },
        &CancellationToken::new(),
    )
    .await
    .expect("scan");
    let row = ctx.store.get_instance(1).expect("read").expect("row");
    assert_eq!(row.status.as_str(), "quarantined");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn discovery_adds_new_instances_and_marks_absent_ones_missing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/instances.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadata": {"timestamp": 1793000584},
            "instances": {
                "https://one.example/": {"main": true, "network_type": "normal", "tls": {"certificate": {"notAfter": "Jan  5 19:32:34 2027 GMT"}}},
                "https://two.example/": {"main": true, "network_type": "normal", "tls": {"certificate": {"notAfter": "Jan  5 19:32:34 2027 GMT"}}},
                "https://tor.example/": {"main": true, "network_type": "tor"}
            }
        })))
        .mount(&server)
        .await;
    let dir = common::temp_dir("scan-discovery");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_OFFLINE", "false"),
            (
                "OWNSEARCH_DISCOVERY_URL",
                &format!("{}/instances.json", server.uri()),
            ),
        ],
    );
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let outcome = refresh_discovery(&ctx, true, &CancellationToken::new())
        .await
        .expect("refresh");
    assert_eq!(outcome.discovered, 3);
    assert_eq!(outcome.added, 3);
    assert!(outcome.error.is_none());
    let rows = ctx
        .store
        .list_instances(&ownsearch_core::InstanceFilter {
            limit: 10,
            ..ownsearch_core::InstanceFilter::default()
        })
        .expect("rows");
    assert_eq!(rows.len(), 3);
    let again = refresh_discovery(&ctx, false, &CancellationToken::new())
        .await
        .expect("second refresh");
    assert!(again.skipped);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_failed_discovery_refresh_is_reported_and_recorded() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/instances.json"))
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .mount(&server)
        .await;
    let dir = common::temp_dir("scan-discovery-fail");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_OFFLINE", "false"),
            (
                "OWNSEARCH_DISCOVERY_URL",
                &format!("{}/instances.json", server.uri()),
            ),
        ],
    );
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let outcome = refresh_discovery(&ctx, true, &CancellationToken::new())
        .await
        .expect("refresh");
    assert!(outcome.error.is_some());
    assert_eq!(outcome.discovered, 0);
    let runs = ctx
        .store
        .query_entity(
            ownsearch_core::QueryEntity::Runs,
            &ownsearch_core::InstanceFilter::default(),
            5,
            0,
        )
        .expect("runs");
    let rows = runs
        .get("rows")
        .and_then(|value| value.as_array())
        .expect("rows");
    assert_eq!(
        rows.first()
            .and_then(|row| row.get("outcome"))
            .and_then(|value| value.as_str()),
        Some("error")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_scan_of_a_single_instance_touches_only_that_instance() {
    let server = MockServer::start().await;
    config_and_json(&server).await;
    let dir = common::temp_dir("scan-single");
    let settings = common::settings(&dir);
    let ctx = context(
        &settings,
        &[
            &format!("{}/", server.uri()),
            "https://unreachable.example/",
        ],
    );
    let output = run_scan(
        &ctx,
        &ScanOptions {
            dry_run: true,
            instance_id: Some(1),
            limit: 10,
            force_discovery: false,
            probe_search: true,
            force: false,
            probe_google: false,
        },
        &CancellationToken::new(),
    )
    .await
    .expect("scan");
    assert_eq!(output.probed, 1);
    assert_eq!(server.received_requests().await.expect("requests").len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_store_is_reachable_from_the_test_harness() {
    let dir = common::temp_dir("scan-store");
    let store = Store::open(&dir.join("instances.db")).expect("store");
    assert_eq!(store.schema_version().expect("version"), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_dry_run_on_an_empty_store_writes_nothing_at_all() {
    let dir = common::temp_dir("scan-dry-empty");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let output = run_scan(
        &ctx,
        &ScanOptions {
            dry_run: true,
            instance_id: None,
            limit: 0,
            force_discovery: false,
            probe_search: false,
            force: false,
            probe_google: false,
        },
        &CancellationToken::new(),
    )
    .await
    .expect("scan");
    assert!(output.dry_run);
    assert_eq!(output.probed, 0);
    let counts = ctx.store.counts().expect("counts");
    assert_eq!(counts.total, 0, "a dry run must not import instances");
    assert_eq!(counts.probes, 0, "a dry run must not write probes");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_discovery_record_that_resolves_to_loopback_is_skipped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/instances.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadata": {"timestamp": 1793000584},
            "instances": {
                "https://127.0.0.1/": {"main": true, "network_type": "normal"}
            }
        })))
        .mount(&server)
        .await;
    let dir = common::temp_dir("scan-ssrf");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_OFFLINE", "false"),
            (
                "OWNSEARCH_DISCOVERY_URL",
                &format!("{}/instances.json", server.uri()),
            ),
            ("OWNSEARCH_ALLOW_PRIVATE_INSTANCES", "false"),
        ],
    );
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let outcome = refresh_discovery(&ctx, true, &CancellationToken::new())
        .await
        .expect("refresh");
    assert_eq!(outcome.added, 0);
    assert_eq!(ctx.store.instance_count().expect("count"), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_explicit_scan_of_a_denied_instance_is_refused() {
    let server = MockServer::start().await;
    config_and_json(&server).await;
    let dir = common::temp_dir("scan-denied");
    let settings = common::settings(&dir);
    let ctx = context(&settings, &[&format!("{}/", server.uri())]);
    ctx.store
        .upsert_override(
            1,
            &json!({"denied": true, "scanEnabled": false}),
            &["test".to_owned()],
        )
        .expect("deny");
    let error = run_scan(
        &ctx,
        &ScanOptions {
            dry_run: false,
            instance_id: Some(1),
            limit: 10,
            force_discovery: false,
            probe_search: false,
            force: false,
            probe_google: false,
        },
        &CancellationToken::new(),
    )
    .await
    .expect_err("a denied instance must be refused");
    assert_eq!(error.code(), "ownsearch.policy_refused");
    assert_eq!(server.received_requests().await.expect("requests").len(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_explicit_scan_of_a_cooling_instance_is_refused() {
    let server = MockServer::start().await;
    config_and_json(&server).await;
    let dir = common::temp_dir("scan-cooling");
    let settings = common::settings(&dir);
    let ctx = context(&settings, &[&format!("{}/", server.uri())]);
    ctx.store
        .mark_search_failure(1, "2999-01-01T00:00:00.000Z", "ownsearch.upstream_failed")
        .expect("cooldown");
    let error = run_scan(
        &ctx,
        &ScanOptions {
            dry_run: true,
            instance_id: Some(1),
            limit: 10,
            force_discovery: false,
            probe_search: false,
            force: false,
            probe_google: false,
        },
        &CancellationToken::new(),
    )
    .await
    .expect_err("a cooling instance must be refused");
    assert_eq!(error.code(), "ownsearch.policy_refused");
    assert_eq!(server.received_requests().await.expect("requests").len(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_discovery_record_with_a_ported_loopback_host_is_skipped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/instances.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadata": {"timestamp": 1793000584},
            "instances": {
                "https://127.0.0.1:8443/": {"main": true, "network_type": "normal"},
                "https://[::1]:8443/": {"main": true, "network_type": "normal"}
            }
        })))
        .mount(&server)
        .await;
    let dir = common::temp_dir("scan-ssrf-port");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_OFFLINE", "false"),
            (
                "OWNSEARCH_DISCOVERY_URL",
                &format!("{}/instances.json", server.uri()),
            ),
            ("OWNSEARCH_ALLOW_PRIVATE_INSTANCES", "false"),
        ],
    );
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let outcome = refresh_discovery(&ctx, true, &CancellationToken::new())
        .await
        .expect("refresh");
    assert_eq!(outcome.added, 0);
    assert_eq!(ctx.store.instance_count().expect("count"), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_second_writing_scan_waits_for_the_interval() {
    let server = MockServer::start().await;
    config_and_json(&server).await;
    let dir = common::temp_dir("scan-interval");
    let settings = common::settings(&dir);
    let ctx = context(&settings, &[&format!("{}/", server.uri())]);
    let options = ScanOptions {
        dry_run: false,
        instance_id: None,
        limit: 10,
        force_discovery: false,
        probe_search: false,
        probe_google: false,
        force: false,
    };
    run_scan(&ctx, &options, &CancellationToken::new())
        .await
        .expect("first scan");
    let error = run_scan(&ctx, &options, &CancellationToken::new())
        .await
        .expect_err("the second scan must wait");
    assert_eq!(error.code(), "ownsearch.policy_refused");
    let mut preview = options.clone();
    preview.dry_run = true;
    let error = run_scan(&ctx, &preview, &CancellationToken::new())
        .await
        .expect_err("a dry run probes too, so it must wait");
    assert_eq!(error.code(), "ownsearch.policy_refused");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_explicit_scan_waits_for_the_next_scan_time_unless_forced() {
    let server = MockServer::start().await;
    config_and_json(&server).await;
    let dir = common::temp_dir("scan-schedule");
    let settings = common::settings(&dir);
    let ctx = context(&settings, &[&format!("{}/", server.uri())]);
    ctx.store
        .apply_scan_update(&ScanUpdate {
            instance_id: 1,
            status: InstanceStatus::Candidate,
            previous_status: InstanceStatus::Candidate,
            successes: 0,
            failures: 0,
            score: 50.0,
            rank: None,
            hard_gate_passed: true,
            reasons: vec!["test".to_owned()],
            next_scan_at: Some("2999-01-01T00:00:00.000Z".to_owned()),
            cooldown_until: None,
            last_success_at: None,
            last_failure_at: None,
            version: None,
            capability_id: None,
            tls_id: None,
        })
        .expect("schedule");
    let mut options = ScanOptions {
        dry_run: false,
        instance_id: Some(1),
        limit: 10,
        force_discovery: false,
        probe_search: false,
        probe_google: false,
        force: false,
    };
    let error = run_scan(&ctx, &options, &CancellationToken::new())
        .await
        .expect_err("a scheduled instance must be refused");
    assert_eq!(error.code(), "ownsearch.policy_refused");
    options.force = true;
    run_scan(&ctx, &options, &CancellationToken::new())
        .await
        .expect("the refusal must not arm the interval, so the forced scan runs");
    let _ = std::fs::remove_dir_all(&dir);
}
