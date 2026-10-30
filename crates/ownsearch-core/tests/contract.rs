#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use ownsearch_core::FleetClient;
use ownsearch_core::ProbeOutcome;
use ownsearch_core::{SearchQuery, config_client, search_html, search_json};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use url::Url;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn query() -> SearchQuery {
    SearchQuery::new("rust mcp")
}

#[tokio::test]
async fn a_config_answer_is_read_into_capabilities() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/config"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "categories": ["general", "news"],
            "engines": [{"name": "google", "enabled": true}, {"name": "google cse", "enabled": true}],
            "locales": {"en": "English"},
            "plugins": [],
            "safe_search": 1,
            "instance_name": "Test",
            "version": "2026.10.24+ef0ec03da"
        })))
        .mount(&server)
        .await;
    let dir = common::temp_dir("config-ok");
    let settings = common::settings(&dir);
    let client = FleetClient::new(&settings).expect("client");
    let base = Url::parse(&format!("{}/", server.uri())).expect("url");
    let report = config_client::fetch(&client, &base, &CancellationToken::new(), None).await;
    assert_eq!(report.outcome, ProbeOutcome::Success);
    let document = report.value.expect("config");
    assert!(document.exact_google());
    assert!(document.google_cse());
    assert_eq!(document.engines.len(), 2);
    assert_eq!(document.safe_search, Some(1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_config_answer_that_is_not_json_is_a_parser_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/config"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(b"<html>".to_vec(), "text/html"))
        .mount(&server)
        .await;
    let dir = common::temp_dir("config-bad");
    let settings = common::settings(&dir);
    let client = FleetClient::new(&settings).expect("client");
    let base = Url::parse(&format!("{}/", server.uri())).expect("url");
    let report = config_client::fetch(&client, &base, &CancellationToken::new(), None).await;
    assert_eq!(report.outcome, ProbeOutcome::ParserError);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_json_search_answer_yields_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("format", "json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "query": "rust mcp",
            "number_of_results": 2,
            "results": [
                {"url": "https://a.example/", "title": "A", "content": "one", "engine": "google"},
                {"url": "https://b.example/", "title": "B", "content": "two", "engine": "duckduckgo"}
            ]
        })))
        .mount(&server)
        .await;
    let dir = common::temp_dir("json-ok");
    let settings = common::settings(&dir);
    let client = FleetClient::new(&settings).expect("client");
    let base = Url::parse(&format!("{}/", server.uri())).expect("url");
    let report =
        search_json::fetch(&client, &base, &query(), &CancellationToken::new(), None).await;
    assert_eq!(report.outcome, ProbeOutcome::Success);
    assert_eq!(
        report.value.as_ref().map(|search| search.result_count()),
        Some(2)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_403_marks_json_as_refused_and_never_retries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
        .mount(&server)
        .await;
    let dir = common::temp_dir("json-403");
    let settings = common::settings(&dir);
    let client = FleetClient::new(&settings).expect("client");
    let base = Url::parse(&format!("{}/", server.uri())).expect("url");
    let report =
        search_json::fetch(&client, &base, &query(), &CancellationToken::new(), None).await;
    assert_eq!(report.outcome, ProbeOutcome::Refused);
    assert_eq!(report.http_status, Some(403));
    assert_eq!(server.received_requests().await.expect("requests").len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_200_that_is_not_json_marks_the_format_as_denied() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(b"<html></html>".to_vec(), "text/html"),
        )
        .mount(&server)
        .await;
    let dir = common::temp_dir("json-non-json");
    let settings = common::settings(&dir);
    let client = FleetClient::new(&settings).expect("client");
    let base = Url::parse(&format!("{}/", server.uri())).expect("url");
    let report =
        search_json::fetch(&client, &base, &query(), &CancellationToken::new(), None).await;
    assert_eq!(report.outcome, ProbeOutcome::Refused);
    assert_eq!(report.error_code.as_deref(), Some("format.json_disabled"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_html_fallback_parses_a_plain_theme() {
    let server = MockServer::start().await;
    let markup = r#"<html><body><article class="result">
        <h3><a href="https://a.example/page">A title</a></h3>
        <p class="content">A snippet.</p>
        <div class="engines"><span>google</span></div>
    </article></body></html>"#;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(markup.as_bytes().to_vec(), "text/html"),
        )
        .mount(&server)
        .await;
    let dir = common::temp_dir("html-ok");
    let settings = common::settings(&dir);
    let client = FleetClient::new(&settings).expect("client");
    let base = Url::parse(&format!("{}/", server.uri())).expect("url");
    let report =
        search_html::fetch(&client, &base, &query(), &CancellationToken::new(), None).await;
    assert_eq!(report.outcome, ProbeOutcome::Success);
    let parsed = report.value.expect("html");
    assert_eq!(parsed.results.len(), 1);
    assert_eq!(
        parsed.results.first().map(|item| item.engine.as_deref()),
        Some(Some("google"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_captcha_page_is_blocked() {
    let server = MockServer::start().await;
    let markup = r#"<html><body><form><input name="captcha"></form></body></html>"#;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(markup.as_bytes().to_vec(), "text/html"),
        )
        .mount(&server)
        .await;
    let dir = common::temp_dir("html-captcha");
    let settings = common::settings(&dir);
    let client = FleetClient::new(&settings).expect("client");
    let base = Url::parse(&format!("{}/", server.uri())).expect("url");
    let report =
        search_html::fetch(&client, &base, &query(), &CancellationToken::new(), None).await;
    assert_eq!(report.outcome, ProbeOutcome::Blocked);
    assert!(report.blocked);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_429_stops_after_one_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(429).set_body_string("Too many requests"))
        .mount(&server)
        .await;
    let dir = common::temp_dir("json-429");
    let settings = common::settings(&dir);
    let client = FleetClient::new(&settings).expect("client");
    let base = Url::parse(&format!("{}/", server.uri())).expect("url");
    let report =
        search_json::fetch(&client, &base, &query(), &CancellationToken::new(), None).await;
    assert_eq!(report.outcome, ProbeOutcome::RateLimited);
    assert!(report.rate_limited);
    assert_eq!(server.received_requests().await.expect("requests").len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_oversized_body_is_cut_at_the_cap() {
    let server = MockServer::start().await;
    let padding = "a".repeat(200_000);
    let body = format!(
        r#"{{"results":[{{"url":"https://a.example/","title":"A","content":"{padding}"}}]}}"#
    );
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "application/json"),
        )
        .mount(&server)
        .await;
    let dir = common::temp_dir("json-big");
    let settings = common::settings_with(&dir, &[("OWNSEARCH_MAX_UPSTREAM_BYTES", "131072")]);
    let client = FleetClient::new(&settings).expect("client");
    let base = Url::parse(&format!("{}/", server.uri())).expect("url");
    let report =
        search_json::fetch(&client, &base, &query(), &CancellationToken::new(), None).await;
    assert!(report.truncated);
    assert!(report.bytes <= 128 * 1024);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_json_answer_that_mentions_a_challenge_is_still_a_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "url": "https://a.example/",
                "title": "Too many requests and just a moment...",
                "content": "Making sure you're not a bot is a phrase in this snippet.",
                "engine": "google"
            }]
        })))
        .mount(&server)
        .await;
    let dir = common::temp_dir("json-marker");
    let settings = common::settings(&dir);
    let client = FleetClient::new(&settings).expect("client");
    let base = Url::parse(&format!("{}/", server.uri())).expect("url");
    let report =
        search_json::fetch(&client, &base, &query(), &CancellationToken::new(), None).await;
    assert_eq!(report.outcome, ProbeOutcome::Success);
    let _ = std::fs::remove_dir_all(&dir);
}
