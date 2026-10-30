#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use std::sync::Arc;

use ownsearch_core::{
    DiscoveredInstance, InstanceStatus, InstanceType, ProbeKind, ProbeOutcome, ProbeRecord,
    SafeSearch, ScanOptions, ScanUpdate, SearchMode, SearchRequest, Store, ToolContext, run_scan,
    search,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn discovered(url: &str) -> DiscoveredInstance {
    DiscoveredInstance {
        url: url.to_owned(),
        kind: InstanceType::Normal,
        rank: Some(1),
        version: Some("2026.10.27+g1a2b3c".to_owned()),
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
    }
}

fn insert(store: &Store, source: &str, url: &str) -> i64 {
    store.ensure_source(source, source, "test").expect("source");
    let (id, _) = store
        .upsert_instance(&discovered(url), source, None, "2026-10-26T00:00:00.000Z")
        .expect("insert");
    id
}

fn activate(store: &Store, instance_id: i64, score: f64) {
    store
        .apply_scan_update(&ScanUpdate {
            instance_id,
            status: InstanceStatus::Active,
            previous_status: InstanceStatus::Candidate,
            successes: 3,
            failures: 0,
            score,
            rank: None,
            hard_gate_passed: true,
            reasons: vec!["probe.json_ok".to_owned()],
            next_scan_at: None,
            cooldown_until: None,
            last_success_at: None,
            last_failure_at: None,
            version: None,
            capability_id: None,
            tls_id: None,
        })
        .expect("scan update");
}

async fn searxng_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/config"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "categories": ["general"],
            "engines": [{"name": "duckduckgo", "enabled": true}],
            "locales": {"en": "English"},
            "plugins": [],
            "safe_search": 1,
            "version": "2026.10.27+g1a2b3c"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {"url": "https://answer.example/x", "title": "A", "content": "one", "engine": "duckduckgo", "positions": [1]}
            ]
        })))
        .mount(&server)
        .await;
    server
}

fn request() -> SearchRequest {
    SearchRequest {
        query: "rust mcp".to_owned(),
        categories: Vec::new(),
        engines: Vec::new(),
        language: None,
        page: 1,
        time_range: None,
        safe_search: SafeSearch::Moderate,
        limit: 10,
        mode: SearchMode::Balanced,
        deadline_override_secs: Some(10),
        dedupe: true,
        min_engines: None,
        instance_ids: Vec::new(),
        engine_weights: std::collections::BTreeMap::new(),
        exclude_engines: Vec::new(),
        freshness_days: None,
    }
}

#[tokio::test]
async fn a_trusted_instance_is_tried_before_a_higher_scored_public_one() {
    let dir = common::temp_dir("fleet-trusted");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_ROUND_ROBIN", "false")]);
    let trusted_server = searxng_server().await;
    let public_server = searxng_server().await;
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let manual = insert(&ctx.store, "manual", &format!("{}/", trusted_server.uri()));
    let public = insert(&ctx.store, "test", &format!("{}/", public_server.uri()));
    activate(&ctx.store, manual, 10.0);
    activate(&ctx.store, public, 95.0);
    ctx.store
        .upsert_override(
            manual,
            &json!({"trusted": true}),
            &["operator.trusted".to_owned()],
        )
        .expect("trusted");

    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert_eq!(
        outcome.output.sources.selected.first().copied(),
        Some(manual)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn public_fleet_off_searches_only_added_instances() {
    let dir = common::temp_dir("fleet-manual-only");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_PUBLIC_FLEET", "false"),
            ("OWNSEARCH_ROUND_ROBIN", "false"),
        ],
    );
    let manual_server = searxng_server().await;
    let public_server = searxng_server().await;
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let manual = insert(&ctx.store, "manual", &format!("{}/", manual_server.uri()));
    let public = insert(&ctx.store, "test", &format!("{}/", public_server.uri()));
    activate(&ctx.store, manual, 50.0);
    activate(&ctx.store, public, 95.0);

    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert_eq!(outcome.output.sources.selected, vec![manual]);
    assert!(!outcome.output.sources.selected.contains(&public));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_parser_failure_saves_a_redacted_sample() {
    let dir = common::temp_dir("parser-sample");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_BROWSER", "false")]);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"unexpected": "not a search document"})),
        )
        .mount(&server)
        .await;
    let ctx = Arc::new(ToolContext::new(settings.clone()).expect("context"));
    let id = insert(&ctx.store, "test", &format!("{}/", server.uri()));
    activate(&ctx.store, id, 80.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(outcome.output.results.is_empty());
    let sample_dir = settings.state_dir().join("parser-samples");
    let entries: Vec<_> = std::fs::read_dir(&sample_dir)
        .map(|list| list.filter_map(Result::ok).collect())
        .unwrap_or_default();
    assert!(
        !entries.is_empty(),
        "a parser failure must leave a sample in {}",
        sample_dir.display()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_daily_probe_cap_stops_a_scan() {
    let dir = common::temp_dir("probe-cap");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_PROBE_DAILY_CAP", "1")]);
    let server = searxng_server().await;
    let ctx = Arc::new(ToolContext::new(settings.clone()).expect("context"));
    let id = insert(&ctx.store, "test", &format!("{}/", server.uri()));
    let now = ownsearch_core::now_rfc3339();
    ctx.store
        .insert_probe(&ProbeRecord {
            instance_id: id,
            run_id: None,
            kind: ProbeKind::Config,
            outcome: ProbeOutcome::Success,
            started_at: now.clone(),
            completed_at: now,
            duration_ms: 1,
            http_status: Some(200),
            content_type: None,
            result_count: None,
            response_bytes: None,
            response_digest_sha256: None,
            error_code: None,
            error_message: None,
        })
        .expect("probe row");
    let output = run_scan(
        &ctx,
        &ScanOptions {
            dry_run: false,
            instance_id: None,
            limit: 50,
            force_discovery: false,
            probe_search: true,
            probe_google: false,
            force: true,
        },
        &CancellationToken::new(),
    )
    .await
    .expect("scan");
    assert_eq!(output.probed, 0);
    assert!(
        output
            .warnings
            .iter()
            .any(|warning| warning.contains("daily probe cap")),
        "warnings {:?}",
        output.warnings
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_single_added_instance_is_the_expected_fleet_size() {
    let dir = common::temp_dir("fleet-expected-one");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_PUBLIC_FLEET", "false")]);
    let server = searxng_server().await;
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let id = insert(&ctx.store, "manual", &format!("{}/", server.uri()));
    activate(&ctx.store, id, 60.0);
    let output = run_scan(
        &ctx,
        &ScanOptions {
            dry_run: false,
            instance_id: None,
            limit: 10,
            force_discovery: false,
            probe_search: true,
            probe_google: false,
            force: true,
        },
        &CancellationToken::new(),
    )
    .await
    .expect("scan");
    assert!(
        !output
            .warnings
            .iter()
            .any(|warning| warning.contains("expected instances")),
        "warnings {:?}",
        output.warnings
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_missing_added_instance_names_the_added_count() {
    let dir = common::temp_dir("fleet-expected-two");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_PUBLIC_FLEET", "false")]);
    let server = searxng_server().await;
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let healthy = insert(&ctx.store, "manual", &format!("{}/", server.uri()));
    let down = insert(&ctx.store, "manual", "http://127.0.0.1:1/");
    activate(&ctx.store, healthy, 60.0);
    ctx.store
        .apply_scan_update(&ScanUpdate {
            instance_id: down,
            status: InstanceStatus::Quarantined,
            previous_status: InstanceStatus::Active,
            successes: 0,
            failures: 4,
            score: 10.0,
            rank: None,
            hard_gate_passed: true,
            reasons: vec!["hysteresis.quarantine".to_owned()],
            next_scan_at: None,
            cooldown_until: None,
            last_success_at: None,
            last_failure_at: None,
            version: None,
            capability_id: None,
            tls_id: None,
        })
        .expect("quarantine");
    let output = run_scan(
        &ctx,
        &ScanOptions {
            dry_run: false,
            instance_id: None,
            limit: 10,
            force_discovery: false,
            probe_search: true,
            probe_google: false,
            force: true,
        },
        &CancellationToken::new(),
    )
    .await
    .expect("scan");
    assert!(
        output
            .warnings
            .iter()
            .any(|warning| warning.contains("fewer than the 2 expected instances")),
        "warnings {:?}",
        output.warnings
    );
    let _ = std::fs::remove_dir_all(&dir);
}
