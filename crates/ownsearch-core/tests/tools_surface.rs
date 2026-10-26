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
use ownsearch_core::{InstanceStatus, InstanceType};
use ownsearch_core::{Store, ToolContext};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
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
        working_engines: vec!["google".to_owned()],
        exact_google_capability: true,
        google_cse_capability: false,
        asn_cidr: Some("192.0.2.0/24".to_owned()),
    }
}

fn activate(store: &Store, instance_id: i64) {
    store
        .apply_scan_update(&ScanUpdate {
            instance_id,
            status: InstanceStatus::Active,
            previous_status: InstanceStatus::Candidate,
            successes: 3,
            failures: 0,
            score: 91.5,
            rank: Some(1),
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

fn insert(store: &Store, url: &str) -> i64 {
    store
        .ensure_source("test", "test", "wiremock")
        .expect("source");
    let (id, _) = store
        .upsert_instance(&discovered(url), "test", None, "2026-10-26T00:00:00.000Z")
        .expect("insert");
    id
}

async fn search_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {"url": "https://a.example/one", "title": "One", "content": "first", "engine": "google", "positions": [1]},
                {"url": "https://a.example/two", "title": "Two", "content": "second", "engine": "bing", "positions": [2]}
            ]
        })))
        .mount(&server)
        .await;
    server
}

fn args(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

async fn call(ctx: &Arc<ToolContext>, name: &str, value: Value) -> CallToolResult {
    ownsearch_core::call_tool(ctx, name, Some(&args(value)), &CancellationToken::new()).await
}

fn structured(result: &CallToolResult) -> &Value {
    result
        .structured_content
        .as_ref()
        .expect("structured content")
}

fn error_body(result: &CallToolResult) -> Value {
    let text = result
        .content
        .first()
        .and_then(|block| block.as_text())
        .map(|text| text.text.clone())
        .unwrap_or_default();
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

#[tokio::test]
async fn the_search_tool_returns_structured_and_text_results() {
    let server = search_server().await;
    let dir = common::temp_dir("tools-search");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let id = insert(&ctx.store, &format!("{}/", server.uri()));
    activate(&ctx.store, id);
    let result = call(
        &ctx,
        "ownsearch_search",
        json!({"query": "rust", "mode": "fast", "limit": 5}),
    )
    .await;
    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        structured(&result)
            .get("results")
            .and_then(|value| value.as_array())
            .map(Vec::len),
        Some(2)
    );
    let text = result
        .content
        .first()
        .and_then(|block| block.as_text())
        .map(|text| text.text.clone())
        .unwrap_or_default();
    assert!(text.contains("results from"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_search_tool_honours_the_minimal_preset() {
    let server = search_server().await;
    let dir = common::temp_dir("tools-minimal");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let id = insert(&ctx.store, &format!("{}/", server.uri()));
    activate(&ctx.store, id);
    let result = call(
        &ctx,
        "ownsearch_search",
        json!({"query": "rust", "fields": "minimal", "result_text": "full"}),
    )
    .await;
    let first = structured(&result)
        .get("results")
        .and_then(|value| value.as_array())
        .and_then(|items| items.first())
        .expect("result");
    assert!(first.get("snippet").is_none());
    assert!(first.get("sourceId").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_bad_search_argument_returns_a_stable_error() {
    let dir = common::temp_dir("tools-bad-arg");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let result = call(&ctx, "ownsearch_search", json!({"query": "   "})).await;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        error_body(&result).get("code").and_then(Value::as_str),
        Some("ownsearch.bad_request")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_unknown_tool_is_refused_with_the_tool_code() {
    let dir = common::temp_dir("tools-unknown");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let result = call(&ctx, "ownsearch_missing", json!({})).await;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        error_body(&result).get("code").and_then(Value::as_str),
        Some("ownsearch.tool_not_found")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn instance_info_and_health_read_the_stored_state() {
    let dir = common::temp_dir("tools-info");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let id = insert(&ctx.store, "https://searx.example/");
    activate(&ctx.store, id);
    let info = call(&ctx, "ownsearch_instance_info", json!({"limit": 5})).await;
    assert_eq!(
        structured(&info).get("count").and_then(Value::as_u64),
        Some(1)
    );
    let by_id = call(
        &ctx,
        "ownsearch_instance_info",
        json!({"instance_id": id, "include_capabilities": true}),
    )
    .await;
    assert_eq!(
        structured(&by_id).get("count").and_then(Value::as_u64),
        Some(1)
    );
    let health = call(&ctx, "ownsearch_instance_health", json!({"limit": 5})).await;
    assert_eq!(
        structured(&health)
            .get("instances")
            .and_then(|value| value.as_array())
            .map(Vec::len),
        Some(1)
    );
    let bad = call(
        &ctx,
        "ownsearch_instance_info",
        json!({"instance_id": id, "url": "https://searx.example/"}),
    )
    .await;
    assert_eq!(bad.is_error, Some(true));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_session_tool_reports_local_counters() {
    let dir = common::temp_dir("tools-session");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let result = call(&ctx, "ownsearch_session", json!({})).await;
    assert_eq!(result.is_error, Some(false));
    assert!(
        structured(&result)
            .get("ledgerHead")
            .and_then(Value::as_str)
            .is_some()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn every_query_entity_returns_rows() {
    let dir = common::temp_dir("tools-query");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let id = insert(&ctx.store, "https://searx.example/");
    activate(&ctx.store, id);
    for entity in [
        "instances",
        "quality",
        "overrides",
        "capabilities",
        "events",
        "snapshots",
        "runs",
    ] {
        let result = call(&ctx, "ownsearch_db_query", json!({"entity": entity})).await;
        assert_eq!(result.is_error, Some(false), "{entity}");
        assert!(
            structured(&result).get("rows").is_some(),
            "{entity} has no rows block"
        );
    }
    let bad = call(&ctx, "ownsearch_db_query", json!({"entity": "secrets"})).await;
    assert_eq!(bad.is_error, Some(true));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_crud_preview_and_apply_work_through_the_tool() {
    let dir = common::temp_dir("tools-crud");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_AI_DB_MODE", "read-write")]);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let id = insert(&ctx.store, "https://searx.example/");
    activate(&ctx.store, id);
    let preview = call(
        &ctx,
        "ownsearch_db_crud",
        json!({
            "operation": "update",
            "entity": "policy_override",
            "instance_id": id,
            "fields": {"pinned": true},
            "reason": "operator asked to pin this instance",
            "dry_run": true
        }),
    )
    .await;
    if preview.is_error == Some(true) {
        panic!("preview refused: {:?}", error_body(&preview));
    }
    let token = structured(&preview)
        .get("confirmationToken")
        .and_then(Value::as_str)
        .expect("token")
        .to_owned();
    let applied = call(
        &ctx,
        "ownsearch_db_crud",
        json!({
            "operation": "update",
            "entity": "policy_override",
            "instance_id": id,
            "fields": {"pinned": true},
            "reason": "operator asked to pin this instance",
            "dry_run": false,
            "confirmation_token": token
        }),
    )
    .await;
    assert_eq!(applied.is_error, Some(false));
    assert_eq!(
        structured(&applied).get("applied").and_then(Value::as_bool),
        Some(true)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_crud_change_is_refused_in_read_only_mode() {
    let dir = common::temp_dir("tools-crud-ro");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_AI_DB_MODE", "read-only")]);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let id = insert(&ctx.store, "https://searx.example/");
    activate(&ctx.store, id);
    let result = call(
        &ctx,
        "ownsearch_db_crud",
        json!({
            "operation": "retire",
            "entity": "instance",
            "instance_id": id,
            "reason": "no longer wanted",
            "dry_run": true
        }),
    )
    .await;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        error_body(&result).get("code").and_then(Value::as_str),
        Some("ownsearch.write_disabled")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_scan_tool_call_with_dry_run_changes_nothing() {
    let dir = common::temp_dir("tools-scan");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let result = call(
        &ctx,
        "ownsearch_scan",
        json!({"dry_run": true, "limit": 1, "probe_search": false}),
    )
    .await;
    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        structured(&result).get("dryRun").and_then(Value::as_bool),
        Some(true)
    );
    let counts = ctx.store.counts().expect("counts");
    assert_eq!(counts.probes, 0, "a dry run must not write probes");
    assert_eq!(counts.total, 0, "a dry run must not import instances");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_tool_list_matches_the_documented_surface() {
    let tools = ownsearch_core::tool_definitions().expect("definitions");
    let names: Vec<String> = tools.iter().map(|tool| tool.name.to_string()).collect();
    for expected in [
        "ownsearch_search",
        "ownsearch_instance_info",
        "ownsearch_instance_health",
        "ownsearch_scan",
        "ownsearch_session",
        "ownsearch_db_query",
        "ownsearch_db_crud",
    ] {
        assert!(names.iter().any(|name| name == expected), "{expected}");
    }
}

#[tokio::test]
async fn every_preset_conforms_to_the_advertised_output_schema() {
    let server = search_server().await;
    let dir = common::temp_dir("tools-schema");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let id = insert(&ctx.store, &format!("{}/", server.uri()));
    activate(&ctx.store, id);
    let tools = ownsearch_core::tool_definitions().expect("tools");
    let search = tools
        .iter()
        .find(|tool| tool.name == "ownsearch_search")
        .expect("search tool");
    let schema = search.output_schema.as_ref().expect("output schema");
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let item_required = schema
        .get("properties")
        .and_then(|properties| properties.get("results"))
        .and_then(|results| results.get("items"))
        .and_then(|items| items.get("required"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for preset in ["minimal", "standard", "full"] {
        let result = call(
            &ctx,
            "ownsearch_search",
            json!({"query": "rust", "fields": preset}),
        )
        .await;
        assert_eq!(result.is_error, Some(false), "{preset}");
        let body = structured(&result);
        for key in &required {
            let name = key.as_str().expect("required name");
            assert!(body.get(name).is_some(), "{preset} lacks {name}");
        }
        for item in body
            .get("results")
            .and_then(Value::as_array)
            .expect("results")
        {
            for key in &item_required {
                let name = key.as_str().expect("required name");
                assert!(
                    item.get(name).is_some(),
                    "{preset} result lacks required field {name}: {item}"
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_failed_call_is_counted_per_tool() {
    let dir = common::temp_dir("tools-errors");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let _ = call(&ctx, "ownsearch_missing", json!({})).await;
    let session = call(&ctx, "ownsearch_session", json!({})).await;
    let usage = structured(&session)
        .get("perTool")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        usage
            .iter()
            .any(|entry| entry.get("errors").and_then(Value::as_u64).unwrap_or(0) >= 1),
        "a failed call must appear with a non-zero error count: {usage:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_fetch_tool_extracts_a_page_as_markdown() {
    let server = MockServer::start().await;
    let markup = r#"<html><head><title>Example page</title></head><body>
        <h1>Heading</h1><p>Body text with a <a href="https://a.example/">link</a>.</p>
    </body></html>"#;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(markup.as_bytes().to_vec(), "text/html"),
        )
        .mount(&server)
        .await;
    let dir = common::temp_dir("tools-fetch");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let result = call(
        &ctx,
        "ownsearch_fetch",
        json!({"url": format!("{}/page", server.uri()), "format": "markdown"}),
    )
    .await;
    assert_eq!(result.is_error, Some(false), "{:?}", error_body(&result));
    let body = structured(&result);
    assert_eq!(
        body.get("title").and_then(Value::as_str),
        Some("Example page")
    );
    let content = body
        .get("content")
        .and_then(Value::as_str)
        .expect("content");
    assert!(content.contains("# Heading"));
    assert!(content.contains("[link](https://a.example/)"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_fetch_tool_follows_same_host_links() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<html><head><title>Root</title></head><body><a href=\"/child\">child</a><a href=\"https://other.example/x\">out</a></body></html>",
            "text/html",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/child"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<html><head><title>Child</title></head><body><p>Child body.</p></body></html>",
            "text/html",
        ))
        .mount(&server)
        .await;
    let dir = common::temp_dir("tools-fetch-crawl");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let result = call(
        &ctx,
        "ownsearch_fetch",
        json!({"url": format!("{}/page", server.uri()), "max_pages": 3}),
    )
    .await;
    assert_eq!(result.is_error, Some(false), "{:?}", error_body(&result));
    let body = structured(&result);
    let pages = body.get("pages").and_then(Value::as_array).expect("pages");
    assert_eq!(pages.len(), 1, "{pages:?}");
    let page = &pages[0];
    assert!(
        page.get("url")
            .and_then(Value::as_str)
            .is_some_and(|url| url.ends_with("/child"))
    );
    assert_eq!(page.get("title").and_then(Value::as_str), Some("Child"));
    let single = call(
        &ctx,
        "ownsearch_fetch",
        json!({"url": format!("{}/page", server.uri()), "max_pages": 1}),
    )
    .await;
    let single_body = structured(&single);
    assert!(single_body.get("pages").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_fetch_tool_refuses_a_private_address() {
    let dir = common::temp_dir("tools-fetch-private");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_ALLOW_PRIVATE_INSTANCES", "false")]);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    for url in ["https://127.0.0.1/", "https://10.0.0.1/page"] {
        let result = call(&ctx, "ownsearch_fetch", json!({"url": url})).await;
        assert_eq!(result.is_error, Some(true), "{url}");
        assert_eq!(
            error_body(&result).get("code").and_then(Value::as_str),
            Some("ownsearch.policy_refused"),
            "{url}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_fetch_tool_follows_a_redirect_to_a_public_page() {
    use wiremock::matchers::{method, path};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/old"))
        .respond_with(
            ResponseTemplate::new(301)
                .insert_header("location", "/new")
                .set_body_string("moved"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/new"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<html><head><title>New</title></head><body><p>Moved here.</p></body></html>",
            "text/html",
        ))
        .mount(&server)
        .await;
    let dir = common::temp_dir("tools-fetch-redirect");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let result = call(
        &ctx,
        "ownsearch_fetch",
        json!({"url": format!("{}/old", server.uri()), "max_pages": 1}),
    )
    .await;
    assert_eq!(result.is_error, Some(false), "{:?}", error_body(&result));
    let body = structured(&result);
    assert_eq!(body.get("title").and_then(Value::as_str), Some("New"));
    assert_eq!(body.get("status").and_then(Value::as_u64), Some(200));
    assert!(
        body.get("finalUrl")
            .and_then(Value::as_str)
            .is_some_and(|url| url.ends_with("/new"))
    );
    assert_eq!(
        body.get("redirects")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_redirect_loop_is_refused_after_the_hop_limit() {
    use wiremock::matchers::{method, path};
    let server = MockServer::start().await;
    for target in ["/a", "/b"] {
        let next = if target == "/a" { "/b" } else { "/a" };
        Mock::given(method("GET"))
            .and(path(target))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", next)
                    .set_body_string("loop"),
            )
            .mount(&server)
            .await;
    }
    let dir = common::temp_dir("tools-fetch-redirect-loop");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let result = call(
        &ctx,
        "ownsearch_fetch",
        json!({"url": format!("{}/a", server.uri()), "max_pages": 1}),
    )
    .await;
    assert_eq!(result.is_error, Some(true));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_crud_preview_works_with_the_built_in_defaults() {
    let dir = common::temp_dir("tools-crud-default");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let id = insert(&ctx.store, "https://searx.example/");
    activate(&ctx.store, id);
    let result = call(
        &ctx,
        "ownsearch_db_crud",
        json!({
            "operation": "retire",
            "entity": "instance",
            "instance_id": id,
            "reason": "operator asked",
            "dry_run": true
        }),
    )
    .await;
    assert_eq!(result.is_error, Some(false), "{:?}", error_body(&result));
    let body = structured(&result);
    assert!(
        body.get("confirmationToken")
            .and_then(Value::as_str)
            .is_some()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn chrome_available() -> bool {
    let candidates = [
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/google-chrome-stable",
    ];
    candidates
        .iter()
        .any(|path| std::path::Path::new(path).exists())
        || std::env::var("OWNSEARCH_BROWSER_PATH").is_ok()
}

#[tokio::test]
async fn the_fetch_tool_renders_a_javascript_page_when_chrome_is_present() {
    use wiremock::matchers::{method, path};
    if !chrome_available() {
        return;
    }
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/shell"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<html><body><div id=\"root\"></div>\
             <script>document.getElementById('root').textContent = 'Rendered by JavaScript';</script>\
             </body></html>",
            "text/html",
        ))
        .mount(&server)
        .await;
    let dir = common::temp_dir("tools-fetch-render");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let result = call(
        &ctx,
        "ownsearch_fetch",
        json!({"url": format!("{}/shell", server.uri()), "max_pages": 1, "render": "always"}),
    )
    .await;
    assert_eq!(result.is_error, Some(false), "{:?}", error_body(&result));
    let body = structured(&result);
    assert_eq!(
        body.get("fetchMode").and_then(Value::as_str),
        Some("browser")
    );
    assert!(
        body.get("content")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("Rendered by JavaScript")),
        "{}",
        body.get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
