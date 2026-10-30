#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use ownsearch_core::{AuthKind, AuthRecord, FleetClient, InstanceAuth};
use tokio_util::sync::CancellationToken;
use url::Url;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn record(kind: AuthKind) -> AuthRecord {
    AuthRecord {
        kind,
        header_name: Some("x-api-key".to_owned()),
        user_name: Some("user".to_owned()),
        secret_env: "OWNSEARCH_TEST_KEY".to_owned(),
    }
}

fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
}

#[tokio::test]
async fn a_header_credential_is_sent_and_dropped_on_a_cross_origin_redirect() {
    let elsewhere = MockServer::start().await;
    Mock::given(any())
        .respond_with(|request: &Request| {
            if header(request, "x-api-key").is_some() {
                ResponseTemplate::new(400).set_body_string("the credential leaked")
            } else {
                ResponseTemplate::new(200).set_body_string("clean")
            }
        })
        .mount(&elsewhere)
        .await;
    let target = format!("{}/final", elsewhere.uri());
    let home = MockServer::start().await;
    Mock::given(any())
        .respond_with(move |request: &Request| {
            if header(request, "x-api-key") == Some("s3cret") {
                ResponseTemplate::new(302).insert_header("location", target.clone())
            } else {
                ResponseTemplate::new(403).set_body_string("no credential")
            }
        })
        .mount(&home)
        .await;

    let dir = common::temp_dir("auth-redirect");
    let settings = common::settings(&dir);
    let client = FleetClient::new(&settings).expect("client");
    let url = Url::parse(&format!("{}/search", home.uri())).expect("url");
    let auth = InstanceAuth::with_secret(
        &format!("{}/", home.uri()),
        record(AuthKind::Header),
        "s3cret",
    )
    .expect("auth");
    let fetched = client
        .get(&url, &CancellationToken::new(), Some(&auth))
        .await
        .expect("fetch");
    assert_eq!(fetched.status, 200, "{}", fetched.text());
    assert_eq!(fetched.text(), "clean");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_basic_credential_is_sent_as_a_basic_authorization_header() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|request: &Request| {
            if header(request, "authorization") == Some("Basic dXNlcjpwYXNz") {
                ResponseTemplate::new(200).set_body_string("clean")
            } else {
                ResponseTemplate::new(401).set_body_string("no basic credential")
            }
        })
        .mount(&server)
        .await;
    let dir = common::temp_dir("auth-basic");
    let settings = common::settings(&dir);
    let client = FleetClient::new(&settings).expect("client");
    let url = Url::parse(&format!("{}/config", server.uri())).expect("url");
    let auth = InstanceAuth::with_secret(
        &format!("{}/", server.uri()),
        AuthRecord {
            kind: AuthKind::Basic,
            header_name: None,
            user_name: Some("user".to_owned()),
            secret_env: "OWNSEARCH_TEST_KEY".to_owned(),
        },
        "pass",
    )
    .expect("auth");
    let fetched = client
        .get(&url, &CancellationToken::new(), Some(&auth))
        .await
        .expect("fetch");
    assert_eq!(fetched.status, 200, "{}", fetched.text());
    let _ = std::fs::remove_dir_all(&dir);
}
