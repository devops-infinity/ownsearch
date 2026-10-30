#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use std::sync::Arc;

use ownsearch_core::SearchMode;
use ownsearch_core::{ScanOptions, SearchRequest, ToolContext, run_scan, search};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs live network access to searx.space and public SearXNG instances"]
async fn a_live_scan_and_search_returns_attributed_results() {
    let dir = common::temp_dir("live");
    let settings = common::settings_live(&dir);
    let context = Arc::new(ToolContext::new(settings).expect("context"));
    let cancellation = CancellationToken::new();
    let scan = run_scan(
        &context,
        &ScanOptions {
            dry_run: false,
            instance_id: None,
            limit: 12,
            force_discovery: true,
            probe_search: true,
            probe_google: false,
            force: true,
        },
        &cancellation,
    )
    .await
    .expect("scan");
    assert!(
        scan.discovered >= 60,
        "discovered {}; warnings {:?}",
        scan.discovered,
        scan.warnings
    );
    let request = SearchRequest {
        query: "rust programming language".to_owned(),
        categories: Vec::new(),
        engines: Vec::new(),
        language: Some("en".to_owned()),
        page: 1,
        time_range: None,
        safe_search: ownsearch_core::SafeSearch::Moderate,
        limit: 10,
        mode: SearchMode::Balanced,
        deadline_override_secs: None,
        dedupe: true,
        min_engines: None,
        instance_ids: Vec::new(),
        engine_weights: std::collections::BTreeMap::new(),
        exclude_engines: Vec::new(),
        freshness_days: None,
    };
    let outcome = search(&context, &request, &cancellation)
        .await
        .expect("search");
    assert!(!outcome.output.sources.selected.is_empty());
    eprintln!(
        "selected {:?}; succeeded {:?}; results {}; partial {}",
        outcome.output.sources.selected,
        outcome.output.sources.succeeded,
        outcome.output.results.len(),
        outcome.output.coverage.partial
    );
    for failure in &outcome.output.sources.failed {
        eprintln!(
            "failed: {} {} {}",
            failure.instance_id, failure.code, failure.reason
        );
    }
    if !outcome.output.sources.succeeded.is_empty() {
        assert!(
            !outcome.output.results.is_empty(),
            "a successful instance returned no usable result"
        );
    }
    for result in outcome.output.results.iter().take(3) {
        assert!(result.url.starts_with("http"));
        assert!(
            result
                .sources
                .as_ref()
                .is_some_and(|sources| !sources.is_empty())
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
