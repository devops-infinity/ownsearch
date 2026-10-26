#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
use ownsearch_core::sanitize;
use ownsearch_core::search_html::HtmlSearch;
use ownsearch_core::search_json::JsonSearch;
use ownsearch_core::{Observation, merge};
use proptest::prelude::*;
use url::Url;

fn observation() -> impl Strategy<Value = Observation> {
    (
        1i64..4,
        prop_oneof![
            Just("https://a.example/page".to_owned()),
            Just("https://b.example/page".to_owned()),
            Just("http://c.example/page".to_owned()),
        ],
        "[a-z ]{1,30}",
        prop::option::of("[a-z ]{1,60}"),
    )
        .prop_map(|(instance_id, url, title, snippet)| Observation {
            instance_id,
            instance_url: format!("https://searx{instance_id}.example/"),
            instance_score: 50.0,
            parser: "json".to_owned(),
            engine: Some("google".to_owned()),
            rank: Some(1),
            title,
            url,
            snippet,
            published_date: None,
        })
}

proptest! {
    #[test]
    fn the_sanitizer_never_emits_a_control_character(text in ".{0,400}") {
        let clean = sanitize(&text);
        prop_assert!(clean.chars().all(|character| !character.is_control()));
    }

    #[test]
    fn the_sanitizer_never_returns_surrounding_whitespace(text in ".{0,400}") {
        let clean = sanitize(&text);
        prop_assert_eq!(clean.trim(), clean.as_str());
    }

    #[test]
    fn merging_never_invents_a_url(observations in prop::collection::vec(observation(), 0..8)) {
        let known: Vec<String> = observations.iter().map(|item| item.url.clone()).collect();
        let output = merge(observations, 20, 300);
        for result in &output.results {
            let matches = known.iter().any(|url| {
                let stripped = url.trim_end_matches('/');
                result.url.trim_end_matches('/') == stripped
                    || result.url.replacen("https://", "http://", 1) == *url
            });
            prop_assert!(matches, "the merger produced an unknown url {}", result.url);
        }
    }

    #[test]
    fn merging_is_order_independent(observations in prop::collection::vec(observation(), 0..8)) {
        let mut reversed = observations.clone();
        reversed.reverse();
        let forward = merge(observations, 20, 300);
        let backward = merge(reversed, 20, 300);
        let mut left: Vec<String> = forward.results.iter().map(|item| item.url.clone()).collect();
        let mut right: Vec<String> = backward.results.iter().map(|item| item.url.clone()).collect();
        left.sort();
        right.sort();
        prop_assert_eq!(left, right);
    }

    #[test]
    fn merging_is_idempotent(observations in prop::collection::vec(observation(), 0..8)) {
        let first = merge(observations, 20, 300);
        let again: Vec<Observation> = first
            .results
            .iter()
            .flat_map(|result| result.sources.clone())
            .collect();
        let second = merge(again, 20, 300);
        prop_assert_eq!(first.results.len(), second.results.len());
    }

    #[test]
    fn a_merged_result_never_has_more_sources_than_observations(
        observations in prop::collection::vec(observation(), 0..8)
    ) {
        let total = observations.len();
        let output = merge(observations, 20, 300);
        let sources: usize = output.results.iter().map(|result| result.sources.len()).sum();
        prop_assert!(sources <= total);
    }
}

proptest! {
    #[test]
    fn the_json_parser_never_panics_on_any_bytes(body in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = JsonSearch::parse(&body);
    }

    #[test]
    fn the_html_parser_never_panics_on_any_bytes(body in prop::collection::vec(any::<u8>(), 0..2048)) {
        let text = String::from_utf8_lossy(&body);
        let base = Url::parse("https://searx.example/").unwrap();
        let _ = HtmlSearch::parse(&text, &base);
    }

    #[test]
    fn the_html_parser_keeps_its_counts_consistent(body in ".{0,2048}") {
        let base = Url::parse("https://searx.example/").unwrap();
        let parsed = HtmlSearch::parse(&body, &base);
        prop_assert!(parsed.results.len() <= parsed.containers);
        prop_assert!(parsed.results.iter().all(|item| !item.url.is_empty()));
    }
}
