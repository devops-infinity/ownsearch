#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ownsearch_core::{
    DiscoveredInstance, InstanceStatus, InstanceType, SafeSearch, ScanUpdate, SearchMode,
    SearchRequest, Store, ToolContext, probe_instance_health,
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

const HTML_PAGE: &str = r#"<html><body><div id="results"><div id="urls">
<article class="result"><h3><a href="https://html.example/x">HTML result</a></h3>
<p class="content">from the html path</p>
<div class="engines"><span>duckduckgo</span></div></article>
</div></div></body></html>"#;

struct Limiter {
    json_requests: Arc<AtomicUsize>,
    throttled_once: Arc<AtomicBool>,
}

fn limiter_responder(state: Limiter) -> impl Fn(&Request) -> ResponseTemplate {
    move |request: &Request| {
        let is_json = request
            .url
            .query_pairs()
            .any(|(key, value)| key == "format" && value == "json");
        let path = request.url.path();
        if path.ends_with("/config") {
            return ResponseTemplate::new(200).set_body_json(json!({
                "categories": ["general"],
                "engines": [{"name": "duckduckgo", "enabled": true}],
                "locales": {"en": "English"},
                "plugins": [],
                "safe_search": 1,
                "version": "2026.10.27+g1a2b3c"
            }));
        }
        if is_json {
            let seen = state.json_requests.fetch_add(1, Ordering::SeqCst);
            if seen >= 4 && !state.throttled_once.swap(true, Ordering::SeqCst) {
                return ResponseTemplate::new(429)
                    .insert_header("retry-after", "2")
                    .set_body_string("too many requests");
            }
            return ResponseTemplate::new(200).set_body_json(json!({
                "results": [
                    {"url": "https://json.example/x", "title": "JSON result", "content": "from json", "engine": "duckduckgo", "positions": [1]}
                ]
            }));
        }
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/html; charset=utf-8")
            .set_body_string(HTML_PAGE)
    }
}

async fn context(
    settings: &ownsearch_core::Settings,
) -> (Arc<ToolContext>, MockServer, i64, Limiter) {
    let state = Limiter {
        json_requests: Arc::new(AtomicUsize::new(0)),
        throttled_once: Arc::new(AtomicBool::new(false)),
    };
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(limiter_responder(Limiter {
            json_requests: Arc::clone(&state.json_requests),
            throttled_once: Arc::clone(&state.throttled_once),
        }))
        .mount(&server)
        .await;
    let ctx = Arc::new(ToolContext::new(settings.clone()).expect("context"));
    ctx.store
        .ensure_source("test", "test", "wiremock")
        .expect("source");
    let (id, _) = ctx
        .store
        .upsert_instance(
            &discovered(&format!("{}/", server.uri())),
            "test",
            None,
            "2026-10-26T00:00:00.000Z",
        )
        .expect("insert");
    activate(&ctx.store, id);
    (ctx, server, id, state)
}

fn activate(store: &Store, instance_id: i64) {
    store
        .apply_scan_update(&ScanUpdate {
            instance_id,
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
}

fn request(query: &str) -> SearchRequest {
    SearchRequest {
        query: query.to_owned(),
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
        freshness_days: None,
    }
}

#[tokio::test]
async fn a_json_429_switches_to_html_and_marks_the_throttle() {
    let dir = common::temp_dir("limiter-html");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_CACHE_TTL_SECS", "0")]);
    let (ctx, _server, id, state) = context(&settings).await;
    for index in 0..4 {
        let outcome = ownsearch_core::search(
            &ctx,
            &request(&format!("query {index}")),
            &CancellationToken::new(),
        )
        .await
        .expect("search");
        assert!(!outcome.output.results.is_empty(), "query {index}");
    }
    assert_eq!(state.json_requests.load(Ordering::SeqCst), 4);

    let throttled = ownsearch_core::search(
        &ctx,
        &request("query that trips the limiter"),
        &CancellationToken::new(),
    )
    .await
    .expect("search");
    assert!(!throttled.output.results.is_empty());
    assert_eq!(
        throttled
            .output
            .results
            .first()
            .and_then(|result| result.sources.as_ref())
            .and_then(|sources| sources.first())
            .map(|source| source.parser.as_str()),
        Some("html")
    );
    assert_eq!(state.json_requests.load(Ordering::SeqCst), 5);
    let after_429 = ctx.store.get_instance(id).expect("row").expect("row");
    assert!(after_429.json_throttled_until.is_some());

    let while_throttled = ownsearch_core::search(
        &ctx,
        &request("query while throttled"),
        &CancellationToken::new(),
    )
    .await
    .expect("search");
    assert!(!while_throttled.output.results.is_empty());
    assert_eq!(
        state.json_requests.load(Ordering::SeqCst),
        5,
        "no json request is sent while the marker stands"
    );

    tokio::time::sleep(std::time::Duration::from_millis(2_200)).await;
    let after_window = ownsearch_core::search(
        &ctx,
        &request("query after the window"),
        &CancellationToken::new(),
    )
    .await
    .expect("search");
    assert!(!after_window.output.results.is_empty());
    assert_eq!(
        state.json_requests.load(Ordering::SeqCst),
        6,
        "the next json attempt happens after Retry-After passes"
    );
    let cleared = ctx.store.get_instance(id).expect("row").expect("row");
    assert!(cleared.json_throttled_until.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_throttled_probe_reports_the_limiter_reason_and_still_passes() {
    let dir = common::temp_dir("limiter-probe");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_CACHE_TTL_SECS", "0")]);
    let (ctx, _server, id, state) = context(&settings).await;
    for index in 0..4 {
        let _ = ownsearch_core::search(
            &ctx,
            &request(&format!("prime {index}")),
            &CancellationToken::new(),
        )
        .await
        .expect("search");
    }
    let _ = ownsearch_core::search(&ctx, &request("prime again"), &CancellationToken::new())
        .await
        .expect("search");
    assert!(state.throttled_once.load(Ordering::SeqCst));
    let report = probe_instance_health(&ctx, id, &CancellationToken::new())
        .await
        .expect("health");
    assert!(report.ok, "reasons {:?}", report.reasons);
    assert!(
        report
            .reasons
            .contains(&"limiter.json_throttled".to_owned()),
        "reasons {:?}",
        report.reasons
    );
    let _ = std::fs::remove_dir_all(&dir);
}
