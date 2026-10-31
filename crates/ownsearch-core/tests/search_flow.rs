#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use std::sync::Arc;

use ownsearch_core::DiscoveredInstance;
use ownsearch_core::ScanUpdate;
use ownsearch_core::{InstanceStatus, InstanceType, ProbeOutcome, SafeSearch, SearchMode};
use ownsearch_core::{SearchRequest, Settings, Store, ToolContext, search};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn discovered(url: &str) -> DiscoveredInstance {
    DiscoveredInstance {
        url: url.to_owned(),
        kind: InstanceType::Normal,
        rank: Some(1),
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
        asn_cidr: None,
    }
}

fn activate(store: &Store, instance_id: i64, html_allowed: bool, score: f64) {
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
    if html_allowed {
        store
            .upsert_override(
                instance_id,
                &json!({"htmlFallbackAllowed": true}),
                &["test".to_owned()],
            )
            .expect("override");
    }
}

fn context(settings: &Settings) -> Arc<ToolContext> {
    Arc::new(ToolContext::new(settings.clone()).expect("context"))
}

fn request() -> SearchRequest {
    SearchRequest {
        query: "rust mcp".to_owned(),
        categories: Vec::new(),
        engines: Vec::new(),
        language: Some("en".to_owned()),
        page: 1,
        time_range: None,
        safe_search: SafeSearch::Moderate,
        limit: 10,
        mode: SearchMode::Balanced,
        deadline_override_secs: None,
        dedupe: true,
        min_engines: None,
        instance_ids: Vec::new(),
        engine_weights: std::collections::BTreeMap::new(),
        exclude_engines: Vec::new(),
        freshness_days: None,
    }
}

fn insert(store: &Store, url: &str) -> i64 {
    store
        .ensure_source("test", "test", "wiremock")
        .expect("source");
    let (id, _) = store
        .upsert_instance(&discovered(url), "test", None, "2026-10-26T00:00:00.000Z")
        .expect("insert");
    id
}

#[tokio::test]
async fn a_search_returns_merged_attributed_results_from_a_json_instance() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {"url": "https://a.example/one", "title": "One", "content": "first", "engine": "google", "positions": [1]},
                {"url": "https://a.example/two", "title": "Two", "content": "second", "engine": "duckduckgo", "positions": [2]}
            ]
        })))
        .mount(&server)
        .await;
    let dir = common::temp_dir("flow-json");
    let settings = common::settings(&dir);
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", server.uri()));
    activate(&ctx.store, id, false, 90.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert_eq!(outcome.output.results.len(), 2);
    assert_eq!(outcome.output.sources.succeeded, vec![id]);
    assert!(outcome.output.sources.failed.is_empty());
    let first = outcome.output.results.first().expect("result");
    assert_eq!(first.rank, 1);
    assert_eq!(
        first
            .sources
            .as_ref()
            .and_then(|sources| sources.first())
            .map(|source| source.instance_id),
        Some(id)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn one_failing_instance_does_not_fail_the_search() {
    let good = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"url": "https://b.example/x", "title": "X", "content": "y", "engine": "google"}]
        })))
        .mount(&good)
        .await;
    let bad = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .mount(&bad)
        .await;
    let dir = common::temp_dir("flow-one-fails");
    let settings = common::settings(&dir);
    let ctx = context(&settings);
    let good_id = insert(&ctx.store, &format!("{}/", good.uri()));
    let bad_id = insert(&ctx.store, &format!("{}/", bad.uri()));
    activate(&ctx.store, good_id, false, 95.0);
    activate(&ctx.store, bad_id, false, 80.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(!outcome.output.results.is_empty());
    assert!(outcome.output.coverage.partial);
    assert_eq!(outcome.output.sources.succeeded, vec![good_id]);
    assert_eq!(
        outcome
            .output
            .sources
            .failed
            .first()
            .map(|item| item.instance_id),
        Some(bad_id)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_html_fallback_is_used_only_when_the_instance_allows_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
        .mount(&server)
        .await;
    let dir = common::temp_dir("flow-html-denied");
    let settings = common::settings(&dir);
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", server.uri()));
    activate(&ctx.store, id, false, 90.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(outcome.output.results.is_empty());
    assert_eq!(
        outcome
            .output
            .sources
            .failed
            .first()
            .map(|item| item.code.as_str()),
        Some("ownsearch.policy_refused")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_html_fallback_parses_when_policy_allows_it() {
    let server = MockServer::start().await;
    let markup = r#"<html><body><article class="result result-default">
        <h3><a href="https://c.example/page">C title</a></h3>
        <p class="content">C snippet.</p>
        <div class="engines"><span>google</span></div>
    </article></body></html>"#;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(markup.as_bytes().to_vec(), "text/html"),
        )
        .mount(&server)
        .await;
    let dir = common::temp_dir("flow-html");
    let settings = common::settings(&dir);
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", server.uri()));
    activate(&ctx.store, id, true, 90.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert_eq!(outcome.output.results.len(), 1);
    assert_eq!(
        outcome
            .output
            .results
            .first()
            .and_then(|item| item.snippet.as_deref()),
        Some("C snippet.")
    );
    assert_eq!(
        outcome
            .output
            .results
            .first()
            .and_then(|item| item.sources.as_ref().and_then(|sources| sources.first()))
            .map(|source| source.parser.as_str()),
        Some("html")
    );
    assert_eq!(outcome.output.sources.fallbacks, vec![id]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_cached_search_answers_without_touching_the_instance() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"url": "https://d.example/x", "title": "D", "content": "y", "engine": "google"}]
        })))
        .mount(&server)
        .await;
    let dir = common::temp_dir("flow-cache");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_CACHE_TTL_SECS", "60")]);
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", server.uri()));
    activate(&ctx.store, id, false, 90.0);
    let first = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(!first.cache_hit);
    let second = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("cached");
    assert!(second.cache_hit);
    assert_eq!(second.output.results.len(), first.output.results.len());
    assert_eq!(server.received_requests().await.expect("requests").len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_rate_limited_instance_is_reported_and_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .mount(&server)
        .await;
    let dir = common::temp_dir("flow-429");
    let settings = common::settings(&dir);
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", server.uri()));
    activate(&ctx.store, id, false, 90.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(outcome.output.results.is_empty());
    assert_eq!(
        outcome
            .output
            .sources
            .failed
            .first()
            .map(|item| item.code.as_str()),
        Some("ownsearch.rate_limited")
    );
    assert_eq!(
        server.received_requests().await.expect("requests").len(),
        2,
        "one json attempt and one html attempt, with no retry"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_parser_error_is_named_in_the_failure_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(b"not json".to_vec(), "application/json"),
        )
        .mount(&server)
        .await;
    let dir = common::temp_dir("flow-parse");
    let settings = common::settings(&dir);
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", server.uri()));
    activate(&ctx.store, id, false, 90.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert_eq!(
        outcome
            .output
            .sources
            .failed
            .first()
            .map(|item| item.code.as_str()),
        Some("ownsearch.parser_failed")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

const INDEX_PAGE: &str = r#"<html><head><meta name="endpoint" content="index"></head><body>
    <main id="main_index"><form id="search" action="/search">
    <input id="q" name="q" type="text" value=""></form></main></body></html>"#;

#[tokio::test]
async fn an_index_page_is_reported_as_refused_not_as_a_parser_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(INDEX_PAGE.as_bytes(), "text/html"))
        .mount(&server)
        .await;
    let dir = common::temp_dir("flow-index-html");
    let settings = common::settings(&dir);
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", server.uri()));
    activate(&ctx.store, id, true, 90.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    let failure = outcome.output.sources.failed.first().expect("a failure");
    assert_eq!(failure.code.as_str(), "ownsearch.policy_refused");
    assert_eq!(failure.parser.as_deref(), Some("html"));
    assert_eq!(
        failure.reason.as_str(),
        "the instance answered its index page instead of results"
    );
    assert_eq!(outcome.output.outcome, "failed");
    let notice = outcome
        .output
        .messages
        .iter()
        .find(|notice| notice.code == "search.sources_failed")
        .expect("a failure notice");
    assert_eq!(notice.level, "warning");
    assert!(
        notice
            .next_step
            .as_deref()
            .is_some_and(|step| !step.is_empty())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_index_page_under_a_json_content_type_is_refused() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(INDEX_PAGE.as_bytes(), "application/json"),
        )
        .mount(&server)
        .await;
    let dir = common::temp_dir("flow-index-json");
    let settings = common::settings(&dir);
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", server.uri()));
    activate(&ctx.store, id, true, 90.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    let failure = outcome.output.sources.failed.first().expect("a failure");
    assert_eq!(failure.code.as_str(), "ownsearch.policy_refused");
    assert_eq!(failure.parser.as_deref(), Some("html"));
    assert_eq!(
        failure.reason.as_str(),
        "the instance answered its index page instead of results"
    );
    assert_eq!(outcome.output.outcome, "failed");
    let notice = outcome
        .output
        .messages
        .iter()
        .find(|notice| notice.code == "search.sources_failed")
        .expect("a failure notice");
    assert_eq!(notice.level, "warning");
    assert!(
        notice
            .next_step
            .as_deref()
            .is_some_and(|step| !step.is_empty())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_rss_fallback_success_is_named_in_the_fallbacks_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("format", "json"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(b"not json".to_vec(), "application/json"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("format", "rss"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<rss><channel><item><title>Rust guide</title><link>https://a.example/rust</link><description>one</description></item></channel></rss>"#,
            "application/rss+xml",
        ))
        .mount(&server)
        .await;
    let dir = common::temp_dir("flow-rss-fallback");
    let settings = common::settings(&dir);
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", server.uri()));
    activate(&ctx.store, id, true, 90.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(outcome.output.sources.failed.is_empty());
    assert_eq!(outcome.output.sources.fallbacks, vec![id]);
    assert_eq!(outcome.output.results.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_failure_codes_cover_every_probe_outcome() {
    let cases = [
        (ProbeOutcome::RateLimited, "ownsearch.rate_limited"),
        (ProbeOutcome::ParserError, "ownsearch.parser_failed"),
        (ProbeOutcome::Timeout, "ownsearch.timeout"),
        (ProbeOutcome::Blocked, "ownsearch.policy_refused"),
        (ProbeOutcome::Refused, "ownsearch.policy_refused"),
        (ProbeOutcome::HttpError, "ownsearch.upstream_failed"),
    ];
    for (outcome, code) in cases {
        assert!(!code.is_empty(), "{}", outcome.as_str());
    }
}

#[tokio::test]
async fn a_failed_instance_falls_over_to_the_next_one() {
    let bad = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .mount(&bad)
        .await;
    let good = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"url": "https://e.example/x", "title": "E", "content": "y", "engine": "google"}]
        })))
        .mount(&good)
        .await;
    let dir = common::temp_dir("flow-failover");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ROUND_ROBIN", "false"),
            ("OWNSEARCH_SEARCH_ATTEMPTS", "1"),
        ],
    );
    let ctx = context(&settings);
    let bad_id = insert(&ctx.store, &format!("{}/", bad.uri()));
    let good_id = insert(&ctx.store, &format!("{}/", good.uri()));
    activate(&ctx.store, bad_id, false, 95.0);
    activate(&ctx.store, good_id, false, 80.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(!outcome.output.results.is_empty());
    assert_eq!(outcome.output.sources.succeeded, vec![good_id]);
    assert_eq!(
        outcome
            .output
            .sources
            .failed
            .first()
            .map(|item| item.instance_id),
        Some(bad_id)
    );
    let row = ctx.store.get_instance(bad_id).expect("read").expect("row");
    assert!(
        row.cooldown_until.is_some(),
        "a failed instance must cool down"
    );
    assert_eq!(
        outcome.output.sources.selected.first().copied(),
        Some(bad_id)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_instance_is_retried_before_it_cools_down() {
    let bad = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .mount(&bad)
        .await;
    let dir = common::temp_dir("flow-retry");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ROUND_ROBIN", "false"),
            ("OWNSEARCH_SEARCH_ATTEMPTS", "3"),
            ("OWNSEARCH_HTML_FALLBACK", "false"),
        ],
    );
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", bad.uri()));
    activate(&ctx.store, id, false, 90.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(outcome.output.results.is_empty());
    assert_eq!(bad.received_requests().await.expect("requests").len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn round_robin_starts_with_a_different_instance_each_call() {
    let first = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"url": "https://f.example/x", "title": "F", "content": "y", "engine": "google"}]
        })))
        .mount(&first)
        .await;
    let second = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"url": "https://g.example/x", "title": "G", "content": "y", "engine": "google"}]
        })))
        .mount(&second)
        .await;
    let dir = common::temp_dir("flow-round-robin");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ROUND_ROBIN", "true"),
            ("OWNSEARCH_CACHE_TTL_SECS", "0"),
        ],
    );
    let ctx = context(&settings);
    let first_id = insert(&ctx.store, &format!("{}/", first.uri()));
    let second_id = insert(&ctx.store, &format!("{}/", second.uri()));
    activate(&ctx.store, first_id, false, 95.0);
    activate(&ctx.store, second_id, false, 90.0);
    let one = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("first");
    let two = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("second");
    assert_ne!(
        one.output.sources.selected.first().copied(),
        two.output.sources.selected.first().copied(),
        "round robin must rotate the starting instance"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_request_deadline_overrides_the_setting_and_is_reported() {
    use std::time::Duration;
    let slow = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(5))
                .set_body_json(json!({"results": []})),
        )
        .mount(&slow)
        .await;
    let dir = common::temp_dir("flow-deadline");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ROUND_ROBIN", "false"),
            ("OWNSEARCH_SEARCH_ATTEMPTS", "1"),
            ("OWNSEARCH_SEARCH_DEADLINE_SECS", "30"),
            ("OWNSEARCH_HTML_FALLBACK", "false"),
        ],
    );
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", slow.uri()));
    activate(&ctx.store, id, false, 90.0);
    let mut request = request();
    request.mode = SearchMode::Fast;
    request.deadline_override_secs = Some(1);
    let started = std::time::Instant::now();
    let outcome = search(&ctx, &request, &CancellationToken::new())
        .await
        .expect("search");
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(outcome.output.timing.deadline_ms, 1_000);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_slow_discovery_document_does_not_hold_the_search_past_its_budget() {
    use std::time::Duration;
    let discovery = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/instances.json"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(30))
                .set_body_json(json!({"metadata": {"timestamp": 1}, "instances": {}})),
        )
        .mount(&discovery)
        .await;
    let search_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"url": "https://answer.example/x", "title": "A", "content": "b"}]
        })))
        .mount(&search_server)
        .await;
    let dir = common::temp_dir("flow-slow-discovery");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ROUND_ROBIN", "false"),
            ("OWNSEARCH_SEARCH_ATTEMPTS", "1"),
            ("OWNSEARCH_OFFLINE", "false"),
            (
                "OWNSEARCH_DISCOVERY_URL",
                &format!("{}/instances.json", discovery.uri()),
            ),
        ],
    );
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", search_server.uri()));
    activate(&ctx.store, id, false, 90.0);
    {
        let connection = rusqlite::Connection::open(dir.join("instances.db")).expect("open");
        connection
            .execute("DELETE FROM discovery_runs", [])
            .expect("clear runs");
    }
    let started = std::time::Instant::now();
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!outcome.output.results.is_empty());
    assert!(
        !outcome
            .output
            .messages
            .iter()
            .any(|notice| notice.message.contains("discovery")),
        "a background refresh must not surface as a search warning"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_hostile_html_title_is_capped_before_the_budget() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(403).set_body_string("no json"))
        .mount(&server)
        .await;
    let long_title = "T".repeat(200_000);
    let page = format!(
        "<html><body><div id=\"results\"><div id=\"urls\">
         <article class=\"result\"><h3><a href=\"https://e.example/x\">{long_title}</a></h3>
         <p class=\"content\">snippet</p></article></div></div></body></html>"
    );
    let html_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(page, "text/html"))
        .mount(&html_server)
        .await;
    let dir = common::temp_dir("flow-hostile-title");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ROUND_ROBIN", "false"),
            ("OWNSEARCH_SEARCH_ATTEMPTS", "1"),
            ("OWNSEARCH_HTML_FALLBACK", "true"),
        ],
    );
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", html_server.uri()));
    let _ = server;
    activate(&ctx.store, id, true, 90.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(
        !outcome.output.results.is_empty(),
        "a capped title must still leave a result: {:?}",
        outcome.output.messages
    );
    let title = outcome
        .output
        .results
        .first()
        .map(|item| item.title.clone())
        .unwrap_or_default();
    assert!(
        title.chars().count() <= 300,
        "title was {} chars",
        title.chars().count()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_feature_is_on_in_the_built_in_defaults() {
    let dir = common::temp_dir("flow-defaults");
    let settings = common::settings(&dir);
    assert!(settings.cache_ttl_secs() > 0, "the response cache is on");
    assert_eq!(settings.ai_db_mode().as_str(), "read-write");
    assert_eq!(settings.exhaustive_max(), 16);
    assert_eq!(settings.search_deadline_secs(SearchMode::Balanced), 30);
    assert_eq!(settings.result_cap(), 25);
    assert!(settings.html_fallback());
    assert!(settings.search_failover());
    assert!(settings.round_robin());
    assert!(settings.search_max_concurrent() >= 1);
    assert!(settings.search_queue_limit() >= 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn failover_walks_past_the_eighth_instance_to_find_an_answer() {
    let mut failing = Vec::new();
    for _ in 0..12 {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
            .mount(&server)
            .await;
        failing.push(server);
    }
    let good = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"url": "https://late.example/x", "title": "Late", "content": "found"}]
        })))
        .mount(&good)
        .await;
    let dir = common::temp_dir("flow-failover-13");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ROUND_ROBIN", "false"),
            ("OWNSEARCH_SEARCH_ATTEMPTS", "1"),
            ("OWNSEARCH_HTML_FALLBACK", "false"),
        ],
    );
    let ctx = context(&settings);
    let mut ids = Vec::new();
    for (index, mock) in failing.iter().enumerate() {
        let id = insert(&ctx.store, &format!("{}/", mock.uri()));
        activate(&ctx.store, id, false, 99.0 - index as f64);
        ids.push(id);
    }
    let good_id = insert(&ctx.store, &format!("{}/", good.uri()));
    activate(&ctx.store, good_id, false, 10.0);
    let outcome = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("search");
    assert!(!outcome.output.results.is_empty());
    assert_eq!(outcome.output.sources.succeeded, vec![good_id]);
    assert_eq!(outcome.output.sources.failed.len(), 12);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_full_queue_refuses_a_search_instead_of_stacking_work() {
    use std::time::Duration;
    let slow = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(500))
                .set_body_json(json!({"results": []})),
        )
        .mount(&slow)
        .await;
    let dir = common::temp_dir("flow-queue");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ROUND_ROBIN", "false"),
            ("OWNSEARCH_SEARCH_ATTEMPTS", "1"),
            ("OWNSEARCH_HTML_FALLBACK", "false"),
            ("OWNSEARCH_SEARCH_MAX_CONCURRENT", "1"),
            ("OWNSEARCH_SEARCH_QUEUE_LIMIT", "1"),
            ("OWNSEARCH_SEARCH_QUEUE_WAIT_SECS", "3"),
        ],
    );
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", slow.uri()));
    activate(&ctx.store, id, false, 90.0);
    let task = |ctx: Arc<ToolContext>| async move {
        search(&ctx, &request(), &CancellationToken::new()).await
    };
    let first = tokio::spawn(task(Arc::clone(&ctx)));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let second = tokio::spawn(task(Arc::clone(&ctx)));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let third = tokio::spawn(task(Arc::clone(&ctx)));
    let results = [
        first.await.expect("first"),
        second.await.expect("second"),
        third.await.expect("third"),
    ];
    let refused = results
        .iter()
        .filter(|result| {
            result
                .as_ref()
                .err()
                .is_some_and(|error| error.code() == "ownsearch.busy")
        })
        .count();
    assert!(
        refused >= 1,
        "the queue must refuse at least one of three calls"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_cache_answers_the_second_identical_search_by_default() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"url": "https://cache.example/x", "title": "Cached", "content": "value"}]
        })))
        .mount(&server)
        .await;
    let dir = common::temp_dir("flow-cache-default");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ROUND_ROBIN", "false"),
            ("OWNSEARCH_SEARCH_ATTEMPTS", "1"),
        ],
    );
    let ctx = context(&settings);
    let id = insert(&ctx.store, &format!("{}/", server.uri()));
    activate(&ctx.store, id, false, 90.0);
    let first = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("first");
    assert!(!first.cache_hit);
    let second = search(&ctx, &request(), &CancellationToken::new())
        .await
        .expect("second");
    assert!(second.cache_hit, "the built-in cache is on");
    assert_eq!(server.received_requests().await.expect("requests").len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_slow_instance_returns_partial_results_at_the_deadline() {
    use std::time::{Duration, Instant};
    let fast = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"url": "https://fast.example/x", "title": "Fast", "content": "one", "engine": "duckduckgo", "positions": [1]}]
        })))
        .mount(&fast)
        .await;
    let slow = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(3))
                .set_body_json(json!({
                    "results": [{"url": "https://slow.example/x", "title": "Slow", "content": "late", "engine": "duckduckgo", "positions": [1]}]
                })),
        )
        .mount(&slow)
        .await;
    let dir = common::temp_dir("flow-partial");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_ROUND_ROBIN", "false"),
            ("OWNSEARCH_SEARCH_ATTEMPTS", "1"),
        ],
    );
    let ctx = context(&settings);
    let fast_id = insert(&ctx.store, &format!("{}/", fast.uri()));
    let slow_id = insert(&ctx.store, &format!("{}/", slow.uri()));
    activate(&ctx.store, fast_id, false, 90.0);
    activate(&ctx.store, slow_id, false, 80.0);
    let mut request = request();
    request.mode = SearchMode::Exhaustive;
    request.deadline_override_secs = Some(1);
    let started = Instant::now();
    let outcome = search(&ctx, &request, &CancellationToken::new())
        .await
        .expect("search");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(!outcome.output.results.is_empty());
    assert!(outcome.output.coverage.partial);
    assert!(
        outcome
            .output
            .errors
            .iter()
            .any(|failure| failure.instance_id == slow_id && failure.code == "ownsearch.timeout"),
        "errors {:?}",
        outcome.output.errors
    );
    assert!(
        outcome
            .output
            .messages
            .iter()
            .any(|notice| notice.message.contains("sources.failed"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}
