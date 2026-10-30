#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use std::sync::Arc;

use ownsearch_core::{DiscoveredInstance, InstanceStatus, InstanceType, ScanUpdate, ToolContext};
use serde_json::{Value, json};
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

fn param(request: &Request, name: &str) -> String {
    request
        .url
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

fn results_stub(expected_range: &'static str) -> impl Fn(&Request) -> ResponseTemplate {
    move |request: &Request| {
        if param(request, "time_range") != expected_range {
            return ResponseTemplate::new(400).set_body_string(format!(
                "unexpected time_range {:?}",
                param(request, "time_range")
            ));
        }
        ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {"url": "https://old.example/", "title": "Old page", "content": "old", "engine": "google", "positions": [1], "publishedDate": "2020-01-01T00:00:00"},
                {"url": "https://fresh.example/", "title": "Fresh page", "content": "new", "engine": "bing", "positions": [1], "publishedDate": "2026-10-01T00:00:00"},
                {"url": "https://undated.example/", "title": "Undated page", "content": "no date", "engine": "brave", "positions": [1]}
            ]
        }))
    }
}

async fn context(
    settings: &ownsearch_core::Settings,
    responder: impl wiremock::Respond + 'static,
) -> (Arc<ToolContext>, MockServer) {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(responder)
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
    (ctx, server)
}

async fn search(ctx: &Arc<ToolContext>, args: Value) -> Value {
    let map = args.as_object().cloned().unwrap_or_default();
    let result = ownsearch_core::call_tool(
        ctx,
        "ownsearch_search",
        Some(&map),
        &CancellationToken::new(),
    )
    .await;
    assert_ne!(result.is_error, Some(true), "{:?}", result.content);
    result.structured_content.unwrap_or(Value::Null)
}

fn urls(body: &Value) -> Vec<String> {
    body.get("results")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("url").and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn the_default_window_drops_a_known_old_result_and_asks_upstream_for_a_year() {
    let dir = common::temp_dir("freshness-default");
    let settings = common::settings(&dir);
    let (ctx, _server) = context(&settings, results_stub("year")).await;
    let body = search(
        &ctx,
        json!({"query": "sqlite wal mode", "limit": 10, "fields": "full"}),
    )
    .await;
    let found = urls(&body);
    assert!(found.contains(&"https://fresh.example/".to_owned()));
    assert!(found.contains(&"https://undated.example/".to_owned()));
    assert!(
        !found.contains(&"https://old.example/".to_owned()),
        "{found:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_zero_window_turns_the_filter_off() {
    let dir = common::temp_dir("freshness-off");
    let settings = common::settings(&dir);
    let (ctx, _server) = context(&settings, results_stub("")).await;
    let body = search(
        &ctx,
        json!({"query": "sqlite wal mode", "limit": 10, "freshness_days": 0}),
    )
    .await;
    let found = urls(&body);
    assert!(
        found.contains(&"https://old.example/".to_owned()),
        "{found:?}"
    );
    assert_eq!(found.len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_explicit_time_range_wins_over_the_window() {
    let dir = common::temp_dir("freshness-range");
    let settings = common::settings(&dir);
    let (ctx, _server) = context(&settings, results_stub("month")).await;
    let body = search(
        &ctx,
        json!({"query": "sqlite wal mode", "limit": 10, "time_range": "month"}),
    )
    .await;
    let found = urls(&body);
    assert!(
        !found.contains(&"https://old.example/".to_owned()),
        "{found:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_profile_can_widen_the_window() {
    let dir = common::temp_dir("freshness-wider");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_FRESHNESS_DAYS", "0")]);
    let (ctx, _server) = context(&settings, results_stub("")).await;
    let body = search(&ctx, json!({"query": "sqlite wal mode", "limit": 10})).await;
    assert_eq!(urls(&body).len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}
