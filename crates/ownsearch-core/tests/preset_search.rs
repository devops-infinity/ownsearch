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

fn expected_stub(
    expected_categories: &'static str,
    expected_engines: &'static str,
) -> impl Fn(&Request) -> ResponseTemplate {
    move |request: &Request| {
        let categories = param(request, "categories");
        let engines = param(request, "engines");
        let categories_ok = expected_categories.is_empty() || categories == expected_categories;
        let engines_ok = expected_engines.is_empty() || engines == expected_engines;
        if !categories_ok || !engines_ok {
            return ResponseTemplate::new(400).set_body_string(format!(
                "unexpected categories {categories:?} engines {engines:?}"
            ));
        }
        ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {"url": "https://github.com/x/y", "title": "A repository", "content": "code", "engine": "github", "positions": [1]},
                {"url": "https://wiby.example/old", "title": "An old page", "content": "small web", "engine": "wiby", "positions": [1]}
            ]
        }))
    }
}

async fn context_with(
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

async fn search(
    ctx: &Arc<ToolContext>,
    args: Value,
) -> ownsearch_core::Result<Result<Value, Value>> {
    let map = args.as_object().cloned().unwrap_or_default();
    let result = ownsearch_core::call_tool(
        ctx,
        "ownsearch_search",
        Some(&map),
        &CancellationToken::new(),
    )
    .await;
    if result.is_error == Some(true) {
        let body = result
            .content
            .first()
            .and_then(|block| block.as_text())
            .map(|text| text.text.clone())
            .unwrap_or_default();
        return Ok(Err(serde_json::from_str(&body).unwrap_or(Value::Null)));
    }
    Ok(Ok(result.structured_content.unwrap_or(Value::Null)))
}

#[tokio::test]
async fn the_code_preset_asks_for_the_it_category() {
    let dir = common::temp_dir("preset-code");
    let settings = common::settings(&dir);
    let (ctx, _server) = context_with(&settings, expected_stub("it,general", "")).await;
    let structured = search(&ctx, json!({"query": "rust", "preset": "code"}))
        .await
        .expect("call")
        .expect("search");
    let results = structured
        .get("results")
        .and_then(Value::as_array)
        .expect("results");
    assert!(!results.is_empty());
    let top = results.first().expect("first");
    assert_eq!(
        top.get("url").and_then(Value::as_str),
        Some("https://github.com/x/y")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_explicit_category_argument_overrides_the_preset() {
    let dir = common::temp_dir("preset-override-categories");
    let settings = common::settings(&dir);
    let (ctx, _server) = context_with(&settings, expected_stub("news", "")).await;
    let structured = search(
        &ctx,
        json!({"query": "rust", "preset": "code", "categories": ["news"]}),
    )
    .await
    .expect("call")
    .expect("search");
    assert!(structured.get("results").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_explicit_engine_argument_overrides_the_preset() {
    let dir = common::temp_dir("preset-override-engines");
    let settings = common::settings(&dir);
    let (ctx, _server) = context_with(&settings, expected_stub("it,general", "google")).await;
    let structured = search(
        &ctx,
        json!({"query": "rust", "preset": "code", "engines": ["google"]}),
    )
    .await
    .expect("call")
    .expect("search");
    assert!(structured.get("results").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}
