#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use std::sync::Arc;

use ownsearch_core::{
    DiscoveredInstance, InstanceStatus, InstanceType, ProbeOutcome, SafeSearch, ScanUpdate,
    SearchMode, SearchRequest, ToolContext, probe_instance_health,
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

async fn searxng_stub() -> MockServer {
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
            "query": "ownsearch connectivity probe",
            "results": [
                {"url": "https://a.example/", "title": "A", "content": "one", "engine": "duckduckgo", "positions": [1]}
            ]
        })))
        .mount(&server)
        .await;
    server
}

fn activate(store: &ownsearch_core::Store, instance_id: i64) {
    store
        .apply_scan_update(&ScanUpdate {
            instance_id,
            status: InstanceStatus::Active,
            previous_status: InstanceStatus::Candidate,
            successes: 3,
            failures: 0,
            score: 80.0,
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

fn request() -> SearchRequest {
    SearchRequest {
        query: "ownsearch connectivity probe".to_owned(),
        categories: Vec::new(),
        engines: Vec::new(),
        language: None,
        page: 1,
        time_range: None,
        safe_search: SafeSearch::Moderate,
        limit: 10,
        mode: SearchMode::Fast,
        deadline_override_secs: Some(5),
        dedupe: true,
        min_engines: None,
        instance_ids: Vec::new(),
        engine_weights: std::collections::BTreeMap::new(),
        exclude_engines: Vec::new(),
    }
}

#[tokio::test]
async fn a_plain_http_instance_is_refused_with_its_own_reason_code() {
    let dir = common::temp_dir("transport-refused");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ALLOW_PRIVATE_INSTANCES", "true"),
            ("OWNSEARCH_ALLOW_PLAIN_HTTP_PRIVATE", "false"),
            ("OWNSEARCH_OFFLINE", "true"),
        ],
    );
    let server = searxng_stub().await;
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    ctx.store
        .ensure_source("manual", "manual", "manual entry")
        .expect("source");
    let (id, _) = ctx
        .store
        .upsert_instance(
            &discovered(&server.uri()),
            "manual",
            None,
            "2026-10-26T00:00:00.000Z",
        )
        .expect("insert");

    let report = probe_instance_health(&ctx, id, &CancellationToken::new())
        .await
        .expect("health");
    assert!(!report.ok);
    assert!(
        report
            .reasons
            .contains(&"policy.plain_http_refused".to_owned())
    );
    assert!(!report.reasons.contains(&"gate.tls_invalid".to_owned()));
    assert!(!report.reasons.contains(&"gate.cert_unknown".to_owned()));
    assert!(!report.reasons.contains(&"probe.json_denied".to_owned()));
    assert_eq!(report.status, "quarantined");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_plain_http_instance_probes_when_both_settings_are_on() {
    let dir = common::temp_dir("transport-allowed");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ALLOW_PRIVATE_INSTANCES", "true"),
            ("OWNSEARCH_ALLOW_PLAIN_HTTP_PRIVATE", "true"),
            ("OWNSEARCH_OFFLINE", "true"),
        ],
    );
    let server = searxng_stub().await;
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    ctx.store
        .ensure_source("manual", "manual", "manual entry")
        .expect("source");
    let (id, _) = ctx
        .store
        .upsert_instance(
            &discovered(&server.uri()),
            "manual",
            None,
            "2026-10-26T00:00:00.000Z",
        )
        .expect("insert");
    let report = probe_instance_health(&ctx, id, &CancellationToken::new())
        .await
        .expect("health");
    assert!(report.ok, "reasons {:?}", report.reasons);
    assert_eq!(report.outcome, ProbeOutcome::Success.as_str());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_plain_http_search_works_when_allowed() {
    let dir = common::temp_dir("transport-search-ok");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ALLOW_PRIVATE_INSTANCES", "true"),
            ("OWNSEARCH_ALLOW_PLAIN_HTTP_PRIVATE", "true"),
            ("OWNSEARCH_OFFLINE", "true"),
        ],
    );
    let server = searxng_stub().await;
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    ctx.store
        .ensure_source("manual", "manual", "manual entry")
        .expect("source");
    let (id, _) = ctx
        .store
        .upsert_instance(
            &discovered(&server.uri()),
            "manual",
            None,
            "2026-10-26T00:00:00.000Z",
        )
        .expect("insert");
    activate(&ctx.store, id);
    let outcome = ownsearch_core::search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(outcome.output.sources.succeeded.contains(&id));
    assert_eq!(outcome.output.results.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_plain_http_search_reports_the_policy_code_when_refused() {
    let dir = common::temp_dir("transport-search-refused");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ALLOW_PRIVATE_INSTANCES", "true"),
            ("OWNSEARCH_ALLOW_PLAIN_HTTP_PRIVATE", "false"),
            ("OWNSEARCH_OFFLINE", "true"),
        ],
    );
    let server = searxng_stub().await;
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    ctx.store
        .ensure_source("manual", "manual", "manual entry")
        .expect("source");
    let (id, _) = ctx
        .store
        .upsert_instance(
            &discovered(&server.uri()),
            "manual",
            None,
            "2026-10-26T00:00:00.000Z",
        )
        .expect("insert");
    activate(&ctx.store, id);
    let outcome = ownsearch_core::search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(outcome.output.results.is_empty());
    let failure = outcome
        .output
        .errors
        .first()
        .expect("a refused source is named");
    assert_eq!(failure.code, "ownsearch.policy_refused");
    assert!(
        failure.reason.contains("allow_plain_http_private"),
        "reason {:?}",
        failure.reason
    );
    let _ = std::fs::remove_dir_all(&dir);
}
