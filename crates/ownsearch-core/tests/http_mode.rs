#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use std::net::TcpListener;
use std::time::Duration;

use ownsearch_core::{
    DiscoveredInstance, InstanceStatus, InstanceType, ScanUpdate, Settings, Store, serve_http,
};
use serde_json::{Value, json};

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("addr").port()
}

fn token() -> String {
    "t".repeat(40)
}

async fn wait_ready(client: &reqwest::Client, base: &str) {
    for _ in 0..600 {
        if let Ok(response) = client.get(format!("{base}/healthz")).send().await
            && response.status().is_success()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the server never answered on {base}");
}

struct Server {
    handle: tokio::task::JoinHandle<ownsearch_core::Result<()>>,
    base: String,
    client: reqwest::Client,
    dir: std::path::PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.handle.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start(name: &str, extra: &[(&str, &str)]) -> Server {
    ownsearch_core::install_crypto_provider();
    let dir = common::temp_dir(name);
    let port = free_port();
    let bind = format!("127.0.0.1:{port}");
    let mut pairs: Vec<(&str, &str)> = vec![
        ("OWNSEARCH_BIND", bind.as_str()),
        ("OWNSEARCH_HTTP", "true"),
    ];
    pairs.extend_from_slice(extra);
    let settings: Settings = common::settings_with(&dir, &pairs);
    let handle = tokio::spawn(async move { serve_http(settings).await });
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("client");
    wait_ready(&client, &base).await;
    Server {
        handle,
        base,
        client,
        dir,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_startup_route_reports_the_store_is_open() {
    let server = start("http-startup", &[]).await;
    let response = server
        .client
        .get(format!("{}/startupz", server.base))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.expect("body");
    assert_eq!(body.get("status").and_then(Value::as_str), Some("started"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_health_and_readiness_routes_answer_without_a_token() {
    let server = start("http-health", &[]).await;
    for path in ["healthz", "readyz", "metrics"] {
        let response = server
            .client
            .get(format!("{}/{}", server.base, path))
            .send()
            .await
            .expect("request");
        assert!(response.status().is_success(), "{path}");
        assert_eq!(
            response
                .headers()
                .get("x-content-type-options")
                .and_then(|value| value.to_str().ok()),
            Some("nosniff")
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_protected_route_refuses_a_missing_token() {
    let server = start("http-token", &[("OWNSEARCH_BEARER_TOKENS", &token())]).await;
    let response = server
        .client
        .post(format!("{}/mcp", server.base))
        .header("content-type", "application/json")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 401);
    assert_eq!(
        response
            .headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer realm=\"ownsearch\"")
    );
    let body: Value = response.json().await.expect("body");
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("ownsearch.auth_failed")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_valid_token_reaches_the_mcp_endpoint() {
    let server = start("http-mcp", &[("OWNSEARCH_BEARER_TOKENS", &token())]).await;
    let response = server
        .client
        .post(format!("{}/mcp", server.base))
        .header("authorization", format!("Bearer {}", token()))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "0.1"}
                }
            })
            .to_string(),
        )
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.expect("body");
    assert_eq!(
        body.get("result")
            .and_then(|result| result.get("serverInfo"))
            .and_then(|info| info.get("name"))
            .and_then(Value::as_str),
        Some("OwnSearch")
    );
}

fn seeded_instance(url: &str) -> DiscoveredInstance {
    DiscoveredInstance {
        url: url.to_owned(),
        kind: InstanceType::Normal,
        rank: Some(1),
        version: Some("2026.10.26+a9308ed5e".to_owned()),
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

fn seed_active(store: &Store, url: &str, score: f64) -> i64 {
    store
        .ensure_source("test", "test", "wiremock")
        .expect("source");
    let (instance_id, _) = store
        .upsert_instance(
            &seeded_instance(url),
            "test",
            None,
            "2026-10-26T00:00:00.000Z",
        )
        .expect("insert");
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
    instance_id
}

#[tokio::test(flavor = "multi_thread")]
async fn the_rest_search_route_walks_the_fleet_until_an_instance_answers() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let mut failing = Vec::new();
    for _ in 0..3 {
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
            "results": [{
                "url": "https://answer.example/page",
                "title": "The answer",
                "content": "A real result.",
                "engine": "google"
            }]
        })))
        .mount(&good)
        .await;
    let server = start(
        "http-rest-failover",
        &[
            ("OWNSEARCH_ROUND_ROBIN", "false"),
            ("OWNSEARCH_SEARCH_ATTEMPTS", "1"),
            ("OWNSEARCH_HTML_FALLBACK", "false"),
            ("OWNSEARCH_ALLOW_PRIVATE_INSTANCES", "true"),
        ],
    )
    .await;
    let store = Store::open(&server.dir.join("instances.db")).expect("store");
    let mut failing_ids = Vec::new();
    for (index, mock) in failing.iter().enumerate() {
        failing_ids.push(seed_active(
            &store,
            &format!("{}/", mock.uri()),
            95.0 - index as f64,
        ));
    }
    let good_id = seed_active(&store, &format!("{}/", good.uri()), 80.0);
    let response = server
        .client
        .post(format!("{}/v1/search", server.base))
        .header("content-type", "application/json")
        .body(json!({"query": "rust mcp", "mode": "balanced", "limit": 5}).to_string())
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.expect("body");
    let results = body
        .get("results")
        .and_then(Value::as_array)
        .expect("results");
    assert_eq!(results.len(), 1, "{body}");
    assert_eq!(
        results
            .first()
            .and_then(|item| item.get("url"))
            .and_then(Value::as_str),
        Some("https://answer.example/page")
    );
    let sources = body.get("sources").expect("sources");
    let failed = sources
        .get("failed")
        .and_then(Value::as_array)
        .expect("failed");
    assert_eq!(failed.len(), 3, "{body}");
    assert_eq!(
        sources.get("succeeded").and_then(Value::as_array).cloned(),
        Some(vec![Value::from(good_id)])
    );
    for instance_id in failing_ids {
        let row = store.get_instance(instance_id).expect("read").expect("row");
        assert!(
            row.cooldown_until.is_some(),
            "instance {instance_id} must cool down"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rest_call_is_counted_in_the_metrics_route() {
    let server = start("http-rest-metrics", &[]).await;
    let before: Value = server
        .client
        .get(format!("{}/metrics", server.base))
        .send()
        .await
        .expect("metrics")
        .json()
        .await
        .expect("body");
    let before_calls = before.get("calls").and_then(Value::as_u64).unwrap_or(0);
    server
        .client
        .post(format!("{}/v1/search", server.base))
        .header("content-type", "application/json")
        .body(r#"{"query":"   "}"#)
        .send()
        .await
        .expect("request");
    let after: Value = server
        .client
        .get(format!("{}/metrics", server.base))
        .send()
        .await
        .expect("metrics")
        .json()
        .await
        .expect("body");
    assert_eq!(
        after.get("calls").and_then(Value::as_u64),
        Some(before_calls + 1)
    );
    assert_eq!(
        after
            .get("perTool")
            .and_then(|map| map.get("ownsearch_search"))
            .and_then(Value::as_u64),
        Some(1)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_rest_routes_require_the_bearer_token_when_one_is_configured() {
    let server = start("http-rest-token", &[("OWNSEARCH_BEARER_TOKENS", &token())]).await;
    for path in ["v1/search", "v1/fetch"] {
        let response = server
            .client
            .post(format!("{}/{}", server.base, path))
            .header("content-type", "application/json")
            .body(r#"{"query":"rust"}"#)
            .send()
            .await
            .expect("request");
        assert_eq!(response.status().as_u16(), 401, "{path}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_rest_search_route_reports_a_bad_argument_as_400() {
    let server = start("http-rest-bad", &[]).await;
    let response = server
        .client
        .post(format!("{}/v1/search", server.base))
        .header("content-type", "application/json")
        .body(r#"{"query":"   "}"#)
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.expect("body");
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("ownsearch.bad_request")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_rest_fetch_route_returns_the_page_as_json() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let page = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/doc"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<html><head><title>Doc</title></head><body><p>Body text.</p></body></html>",
            "text/html",
        ))
        .mount(&page)
        .await;
    let server = start(
        "http-rest-fetch",
        &[("OWNSEARCH_ALLOW_PRIVATE_INSTANCES", "true")],
    )
    .await;
    let response = server
        .client
        .post(format!("{}/v1/fetch", server.base))
        .header("content-type", "application/json")
        .body(json!({"url": format!("{}/doc", page.uri()), "max_pages": 1}).to_string())
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.expect("body");
    assert_eq!(body.get("title").and_then(Value::as_str), Some("Doc"));
    assert!(
        body.get("content")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("Body text."))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_token_is_refused() {
    let server = start("http-wrong-token", &[("OWNSEARCH_BEARER_TOKENS", &token())]).await;
    let response = server
        .client
        .post(format!("{}/mcp", server.base))
        .header("authorization", format!("Bearer {}", "x".repeat(40)))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disallowed_origin_is_refused() {
    let server = start(
        "http-origin",
        &[
            ("OWNSEARCH_BEARER_TOKENS", &token()),
            ("OWNSEARCH_ALLOWED_ORIGINS", "https://allowed.example"),
        ],
    )
    .await;
    let refused = server
        .client
        .post(format!("{}/mcp", server.base))
        .header("origin", "https://evil.example")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("request");
    assert_eq!(refused.status().as_u16(), 403);
    let allowed = server
        .client
        .post(format!("{}/mcp", server.base))
        .header("origin", "https://allowed.example")
        .send()
        .await
        .expect("request");
    assert_ne!(allowed.status().as_u16(), 403);
    assert_eq!(
        allowed
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("https://allowed.example")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_allowed_origin_gets_an_answered_preflight() {
    let server = start(
        "http-preflight",
        &[("OWNSEARCH_ALLOWED_ORIGINS", "https://allowed.example")],
    )
    .await;
    let response = server
        .client
        .request(
            reqwest::Method::OPTIONS,
            format!("{}/v1/search", server.base),
        )
        .header("origin", "https://allowed.example")
        .header("access-control-request-method", "POST")
        .send()
        .await
        .expect("request");
    let status = response.status().as_u16();
    let methods = response
        .headers()
        .get("access-control-allow-methods")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    assert_eq!(status, 204);
    assert_eq!(methods.as_deref(), Some("GET, POST, DELETE, OPTIONS"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rest_call_without_a_json_content_type_is_refused() {
    let server = start("http-media", &[("OWNSEARCH_BEARER_TOKENS", &token())]).await;
    let response = server
        .client
        .post(format!("{}/v1/search", server.base))
        .header("authorization", format!("Bearer {}", token()))
        .header("content-type", "text/plain")
        .body("{}")
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 415);
    let body: Value = response.json().await.expect("body");
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("ownsearch.bad_request")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disallowed_host_is_refused() {
    let server = start("http-host", &[("OWNSEARCH_BEARER_TOKENS", &token())]).await;
    let response = server
        .client
        .post(format!("{}/mcp", server.base))
        .header("host", "evil.example")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 403);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_body_is_refused() {
    let server = start("http-big", &[("OWNSEARCH_BEARER_TOKENS", &token())]).await;
    let huge = "a".repeat(2 * 1024 * 1024);
    let outcome = server
        .client
        .post(format!("{}/mcp", server.base))
        .header("content-type", "application/json")
        .body(huge)
        .send()
        .await;
    match outcome {
        Ok(response) => assert!(
            response.status().as_u16() >= 400,
            "status {}",
            response.status()
        ),
        Err(error) => assert!(
            error.is_request() || error.is_body(),
            "unexpected error: {error}"
        ),
    }
}

#[tokio::test]
async fn a_public_bind_without_a_token_is_refused_at_startup() {
    let dir = common::temp_dir("http-public");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_BIND", "0.0.0.0:8774"),
            ("OWNSEARCH_HTTP", "true"),
        ],
    );
    let error = serve_http(settings).await.expect_err("public bind");
    assert!(
        error.to_string().contains("OWNSEARCH_BEARER_TOKENS"),
        "unexpected error: {error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_short_token_is_refused_at_startup() {
    let dir = common::temp_dir("http-short");
    let settings = common::settings_with(
        &dir,
        &[
            ("OWNSEARCH_BIND", "127.0.0.1:0"),
            ("OWNSEARCH_HTTP", "true"),
            ("OWNSEARCH_BEARER_TOKENS", "short"),
        ],
    );
    let error = serve_http(settings).await.expect_err("short token");
    assert!(error.to_string().contains("32"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_wrong_tokens_lock_the_client_out() {
    let server = start("http-lockout", &[("OWNSEARCH_BEARER_TOKENS", &token())]).await;
    let mut statuses = Vec::new();
    for _ in 0..8 {
        let response = server
            .client
            .post(format!("{}/mcp", server.base))
            .header("authorization", format!("Bearer {}", "x".repeat(40)))
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .expect("request");
        statuses.push(response.status().as_u16());
    }
    assert!(
        statuses.contains(&429),
        "a burst of wrong tokens must lock the address out: {statuses:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rest_refusal_is_problem_details() {
    let server = start("http-problem", &[]).await;
    let response = server
        .client
        .post(format!("{}/v1/search", server.base))
        .header("content-type", "application/json")
        .body(r#"{"query":"   "}"#)
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 400);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json")
    );
    let body: Value = response.json().await.expect("body");
    assert_eq!(body.get("status").and_then(Value::as_u64), Some(400));
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("ownsearch.bad_request")
    );
    assert!(body.get("type").and_then(Value::as_str).is_some());
    assert!(body.get("detail").and_then(Value::as_str).is_some());
    assert!(body.get("instance").and_then(Value::as_str).is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_metrics_route_reports_per_route_latency() {
    let server = start("http-metrics-route", &[]).await;
    server
        .client
        .get(format!("{}/readyz", server.base))
        .send()
        .await
        .expect("request");
    let body: Value = server
        .client
        .get(format!("{}/metrics", server.base))
        .send()
        .await
        .expect("metrics")
        .json()
        .await
        .expect("body");
    let latency = body
        .get("httpLatency")
        .and_then(Value::as_object)
        .expect("latency");
    assert!(
        latency.keys().any(|key| key.contains("/readyz")),
        "readyz missing from {latency:?}"
    );
}
