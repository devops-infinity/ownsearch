#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use std::sync::Arc;

use ownsearch_core::{
    CapabilityRecord, DiscoveredInstance, InstanceType, ProbeOutcome, SafeSearch, ScanUpdate,
    SearchMode, SearchRequest, Settings, Store, ToolContext, result_key, search,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn discovered(url: &str) -> DiscoveredInstance {
    DiscoveredInstance {
        url: url.to_owned(),
        kind: InstanceType::Normal,
        rank: Some(1),
        version: Some("2026.10.27+g1a2b3c".to_owned()),
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

fn insert(store: &Store, url: &str) -> i64 {
    store
        .ensure_source("test", "test", "wiremock")
        .expect("source");
    let (id, _) = store
        .upsert_instance(&discovered(url), "test", None, "2026-10-26T00:00:00.000Z")
        .expect("insert");
    id
}

fn activate(store: &Store, instance_id: i64, score: f64) {
    store
        .apply_scan_update(&ScanUpdate {
            instance_id,
            status: ownsearch_core::InstanceStatus::Active,
            previous_status: ownsearch_core::InstanceStatus::Candidate,
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
}

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(path).expect("fixture");
    serde_json::from_str(&raw).expect("json")
}

async fn mount_search(server: &MockServer, body: &Value) {
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body.clone()))
        .mount(server)
        .await;
}

fn request(query: &str, dedupe: bool, limit: usize) -> SearchRequest {
    SearchRequest {
        query: query.to_owned(),
        categories: Vec::new(),
        engines: Vec::new(),
        language: Some("en".to_owned()),
        page: 1,
        time_range: None,
        safe_search: SafeSearch::Moderate,
        limit,
        mode: SearchMode::Balanced,
        deadline_override_secs: Some(10),
        dedupe,
        min_engines: None,
        instance_ids: Vec::new(),
        engine_weights: std::collections::BTreeMap::new(),
        exclude_engines: Vec::new(),
    }
}

async fn run_one(
    settings: &Settings,
    body: &Value,
    query: &str,
    dedupe: bool,
    limit: usize,
) -> (i64, ownsearch_core::SearchOutcome) {
    let server = MockServer::start().await;
    mount_search(&server, body).await;
    let ctx = Arc::new(ToolContext::new(settings.clone()).expect("context"));
    let id = insert(&ctx.store, &server.uri());
    activate(&ctx.store, id, 90.0);
    let outcome = search(
        &ctx,
        &request(query, dedupe, limit),
        &CancellationToken::new(),
    )
    .await
    .expect("search");
    (id, outcome)
}

#[tokio::test]
async fn the_nextjs_fixture_merges_dedupes_and_ranks_agreement_first() {
    let dir = common::temp_dir("merge-nextjs");
    let settings = common::settings(&dir);
    let body = fixture("searxng-nextjs.json");
    let (id, outcome) = run_one(&settings, &body, "next.js latest version", true, 25).await;
    let ctx = Arc::new(ToolContext::new(settings.clone()).expect("context"));

    let results = &outcome.output.results;
    assert_eq!(results.len(), 25, "the cap decides what the caller sees");
    assert_eq!(
        results.first().map(|result| result.url.as_str()),
        Some("https://nextjs.org/")
    );

    let mut keys: Vec<String> = results
        .iter()
        .map(|result| result_key(&result.url))
        .collect();
    keys.sort();
    let unique = keys.len();
    keys.dedup();
    assert_eq!(keys.len(), unique, "two results share a normalized url");

    for result in results {
        let engines = result.engines.as_ref().expect("engines");
        assert!(!engines.is_empty(), "{} lists no engine", result.url);
    }

    let three_engine_floor = results
        .iter()
        .filter(|result| result.engines.as_ref().is_some_and(|list| list.len() >= 3))
        .map(|result| result.score)
        .fold(f64::MAX, f64::min);
    let wiby_ceiling = results
        .iter()
        .filter(|result| {
            result.engines.as_ref().is_some_and(|list| {
                list.len() == 1 && list.first().is_some_and(|engine| engine == "wiby")
            })
        })
        .map(|result| result.score)
        .fold(0.0_f64, f64::max);
    assert!(
        three_engine_floor > wiby_ceiling,
        "three-engine floor {three_engine_floor} must beat the wiby ceiling {wiby_ceiling}"
    );

    let npm = results
        .iter()
        .find(|result| result.url == "https://www.npmjs.com/package/next")
        .expect("the npm result");
    assert!(npm.engines.as_ref().is_some_and(|list| list.len() >= 3));

    let unresponsive = &outcome.output.sources.unresponsive_engines;
    assert_eq!(unresponsive.len(), 4);
    let dogpile = unresponsive
        .iter()
        .find(|item| item.engine == "dogpile")
        .expect("dogpile");
    assert!(dogpile.reason.contains("access denied"));

    let stats = ctx
        .store
        .engine_stats_for(&[id])
        .expect("engine stats")
        .get(&id)
        .cloned()
        .unwrap_or_default();
    assert!(
        stats
            .iter()
            .any(|stat| stat.engine.eq_ignore_ascii_case("google") && stat.failures >= 1),
        "the unresponsive google engine must be counted"
    );
    assert!(
        stats
            .iter()
            .any(|stat| stat.engine.eq_ignore_ascii_case("yahoo") && stat.successes >= 1),
        "a serving engine must be counted"
    );
    assert!(outcome.output.timing.merge_ms <= outcome.output.timing.elapsed_ms);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_react_fixture_keeps_its_order_and_merges_one_duplicate() {
    let dir = common::temp_dir("merge-react");
    let settings = common::settings(&dir);
    let body = fixture("searxng-react.json");
    let (_, outcome) = run_one(&settings, &body, "react latest version", true, 25).await;
    let results = &outcome.output.results;
    assert_eq!(
        results.first().map(|result| result.url.as_str()),
        Some("https://react.dev/versions")
    );
    let wiby_top = results
        .iter()
        .filter(|result| result.url.contains("wiby"))
        .map(|result| result.score)
        .fold(0.0_f64, f64::max);
    let strong_floor = results
        .iter()
        .filter(|result| result.engines.as_ref().is_some_and(|list| list.len() >= 2))
        .map(|result| result.score)
        .fold(f64::MAX, f64::min);
    assert!(strong_floor > wiby_top);
    let shared = results
        .iter()
        .filter(|result| result.url == "https://www.youtube.com/watch?v=rwC7HY8_U_g")
        .collect::<Vec<_>>();
    assert_eq!(shared.len(), 1, "one url from two engines must merge once");
    assert_eq!(shared.first().map(|result| result.merged_from), Some(2));
    assert!(results.iter().all(|result| {
        result
            .sources
            .as_ref()
            .is_some_and(|sources| !sources.is_empty())
    }));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn dedupe_off_keeps_both_copies_of_one_url() {
    let dir = common::temp_dir("merge-nodedupe");
    let settings = common::settings(&dir);
    let body = fixture("searxng-react.json");
    let (_, outcome) = run_one(&settings, &body, "react latest version", false, 100).await;
    let shared: Vec<_> = outcome
        .output
        .results
        .iter()
        .filter(|result| result.url == "https://www.youtube.com/watch?v=rwC7HY8_U_g")
        .collect();
    assert_eq!(shared.len(), 2, "dedupe off must keep both copies");
    assert!(shared.iter().all(|result| result.merged_from == 1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn instance_info_returns_the_enabled_engines_and_categories() {
    let dir = common::temp_dir("engines-info");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let id = insert(&ctx.store, "https://searx.example/");
    activate(&ctx.store, id, 80.0);
    ctx.store
        .insert_capability(&CapabilityRecord {
            instance_id: id,
            run_id: None,
            outcome: ProbeOutcome::Success,
            http_status: Some(200),
            duration_ms: Some(12),
            response_digest_sha256: None,
            instance_name: Some("Trial".to_owned()),
            categories: vec!["general".to_owned(), "news".to_owned()],
            engines: vec!["google".to_owned(), "duckduckgo".to_owned()],
            locales: vec!["en".to_owned()],
            plugins: Vec::new(),
            safe_search_default: Some(1),
            json_enabled: true,
            csv_enabled: false,
            rss_enabled: false,
            html_enabled: true,
            google_exact_seen: true,
            google_cse_seen: true,
        })
        .expect("capability");
    let args = serde_json::json!({"instance_id": id});
    let map = args.as_object().cloned().expect("object");
    let result = ownsearch_core::call_tool(
        &ctx,
        "ownsearch_instance_info",
        Some(&map),
        &CancellationToken::new(),
    )
    .await;
    let body = result.structured_content.expect("structured");
    let instance = body
        .get("instances")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .expect("instance");
    let engines: Vec<&str> = instance
        .get("engines")
        .and_then(Value::as_array)
        .expect("engines")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(engines, vec!["duckduckgo", "google"]);
    let categories = instance
        .get("categories")
        .and_then(Value::as_array)
        .expect("categories");
    assert_eq!(categories.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_engine_yield_and_health_flag_reflect_the_serving_engines() {
    let dir = common::temp_dir("engine-yield");
    let settings = common::settings(&dir);
    let body = fixture("searxng-nextjs.json");
    let (id, _outcome) = run_one(&settings, &body, "next.js latest version", true, 25).await;
    let ctx = Arc::new(ToolContext::new(settings.clone()).expect("context"));
    let engines: Vec<String> = [
        "brave",
        "yahoo",
        "google cse",
        "bing",
        "mojeek",
        "wiby",
        "exaapi",
        "e-a",
        "e-b",
        "e-c",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    ctx.store
        .insert_capability(&CapabilityRecord {
            instance_id: id,
            run_id: None,
            outcome: ProbeOutcome::Success,
            http_status: Some(200),
            duration_ms: Some(10),
            response_digest_sha256: None,
            instance_name: None,
            categories: vec!["general".to_owned()],
            engines,
            locales: Vec::new(),
            plugins: Vec::new(),
            safe_search_default: None,
            json_enabled: true,
            csv_enabled: false,
            rss_enabled: false,
            html_enabled: false,
            google_exact_seen: false,
            google_cse_seen: true,
        })
        .expect("capability");
    let ratio = ctx
        .store
        .engine_yield_for(&[id])
        .expect("yield")
        .get(&id)
        .copied()
        .expect("a ratio");
    assert!(ratio > 0.0 && ratio < 1.0, "yield {ratio}");
    let args = json!({"instance_id": id});
    let map = args.as_object().cloned().expect("object");
    let result = ownsearch_core::call_tool(
        &ctx,
        "ownsearch_instance_health",
        Some(&map),
        &CancellationToken::new(),
    )
    .await;
    let structured = result.structured_content.expect("structured");
    let item = structured
        .get("instances")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .expect("health item");
    assert_eq!(item.get("engineYield").and_then(Value::as_f64), Some(ratio));
    assert_eq!(
        item.get("engineDegraded").and_then(Value::as_bool),
        Some(ratio < 0.5)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
