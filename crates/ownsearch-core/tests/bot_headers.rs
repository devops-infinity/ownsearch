#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use std::sync::Arc;

use ownsearch_core::{
    DiscoveredInstance, InstanceStatus, InstanceType, SafeSearch, ScanUpdate, SearchMode,
    SearchRequest, ToolContext, probe_instance_health,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

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

fn header<'a>(request: &'a Request, name: &str) -> &'a str {
    request
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
}

fn searxng_bot_check(request: &Request) -> ResponseTemplate {
    let accept = header(request, "accept");
    let encoding = header(request, "accept-encoding");
    let language = header(request, "accept-language");
    let agent = header(request, "user-agent");
    let passes = accept.contains("text/html")
        && (encoding.contains("gzip") || encoding.contains("deflate"))
        && !language.is_empty()
        && !agent.is_empty();
    if !passes {
        return ResponseTemplate::new(429).insert_header("retry-after", "60");
    }
    if request.url.path().ends_with("/config") {
        ResponseTemplate::new(200).set_body_json(json!({
            "categories": ["general"],
            "engines": [{"name": "duckduckgo", "enabled": true}],
            "locales": {"en": "English"},
            "plugins": [],
            "safe_search": 1,
            "version": "2026.10.27+g1a2b3c"
        }))
    } else {
        ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {"url": "https://a.example/", "title": "A", "content": "one", "engine": "duckduckgo", "positions": [1]}
            ]
        }))
    }
}

async fn instance(settings: &ownsearch_core::Settings) -> (Arc<ToolContext>, MockServer, i64) {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(searxng_bot_check)
        .mount(&server)
        .await;
    let ctx = Arc::new(ToolContext::new(settings.clone()).expect("context"));
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
    (ctx, server, id)
}

fn request() -> SearchRequest {
    SearchRequest {
        query: "ownsearch probe".to_owned(),
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
async fn the_default_headers_pass_the_bot_checks() {
    let dir = common::temp_dir("bot-headers-ok");
    let settings = common::settings(&dir);
    let (ctx, _server, id) = instance(&settings).await;
    let report = probe_instance_health(&ctx, id, &CancellationToken::new())
        .await
        .expect("health");
    assert!(report.ok, "reasons {:?}", report.reasons);
    ctx.store
        .apply_scan_update(&ScanUpdate {
            instance_id: id,
            status: InstanceStatus::Active,
            previous_status: InstanceStatus::Candidate,
            successes: 2,
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
        .expect("update");
    let outcome = ownsearch_core::search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert_eq!(outcome.output.results.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_json_only_accept_header_is_refused_by_the_bot_check() {
    let dir = common::temp_dir("bot-headers-old");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_ACCEPT", "application/json")]);
    let (ctx, _server, id) = instance(&settings).await;
    let report = probe_instance_health(&ctx, id, &CancellationToken::new())
        .await
        .expect("health");
    assert!(!report.ok);
    assert!(
        report.reasons.contains(&"probe.rate_limited".to_owned()),
        "reasons {:?}",
        report.reasons
    );
    ctx.store
        .apply_scan_update(&ScanUpdate {
            instance_id: id,
            status: InstanceStatus::Active,
            previous_status: InstanceStatus::Candidate,
            successes: 2,
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
        .expect("update");
    let outcome = ownsearch_core::search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(outcome.output.results.is_empty());
    assert!(
        outcome
            .output
            .errors
            .iter()
            .any(|failure| failure.code == "ownsearch.rate_limited"),
        "errors {:?}",
        outcome.output.errors
    );
    let _ = std::fs::remove_dir_all(&dir);
}
