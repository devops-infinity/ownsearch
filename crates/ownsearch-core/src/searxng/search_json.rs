use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::{FleetClient, search_url};
use crate::error::{Error, Result};
use crate::searxng::{ProbeReport, SearchQuery, classify_outcome};

const MAX_PARSED_RESULTS: usize = 200;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct JsonSearch {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(rename = "number_of_results", default)]
    pub number_of_results: Option<u64>,
    #[serde(default)]
    pub results: Vec<JsonResult>,
    #[serde(default)]
    pub answers: Vec<Value>,
    #[serde(default)]
    pub corrections: Vec<Value>,
    #[serde(default)]
    pub suggestions: Vec<Value>,
    #[serde(default)]
    pub infoboxes: Vec<Value>,
    #[serde(rename = "unresponsive_engines", default)]
    pub unresponsive_engines: Vec<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct JsonResult {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(default)]
    pub engines: Vec<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(rename = "publishedDate", default)]
    pub published_date: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(rename = "img_src", default)]
    pub img_src: Option<String>,
    #[serde(default)]
    pub positions: Vec<u32>,
}

impl JsonSearch {
    pub fn parse(body: &[u8]) -> Result<Self> {
        let answer: Value = serde_json::from_slice(body)
            .map_err(|error| Error::parser("searxng json search", error))?;
        let is_document = answer
            .as_object()
            .is_some_and(|object| object.contains_key("results"));
        if !is_document {
            return Err(Error::parser(
                "searxng json search",
                std::io::Error::other("the answer has no results list"),
            ));
        }
        let mut search: Self = serde_json::from_value(answer)
            .map_err(|error| Error::parser("searxng json search", error))?;
        search.results.truncate(MAX_PARSED_RESULTS);
        Ok(search)
    }

    pub fn result_count(&self) -> usize {
        self.results.len()
    }
}

impl JsonResult {
    pub fn engine_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        if let Some(engine) = &self.engine
            && !engine.is_empty()
        {
            names.push(engine.clone());
        }
        for engine in &self.engines {
            if !names.iter().any(|existing| existing == engine) {
                names.push(engine.clone());
            }
        }
        names
    }

    pub fn observed_rank(&self) -> Option<u32> {
        self.positions.first().copied()
    }
}

pub async fn fetch(
    client: &FleetClient,
    base: &Url,
    query: &SearchQuery,
    cancellation: &CancellationToken,
    auth: Option<&crate::auth::InstanceAuth>,
) -> ProbeReport<JsonSearch> {
    let params = query.params(true);
    let url = match search_url(base, &params) {
        Ok(url) => url,
        Err(error) => {
            return ProbeReport::failed(crate::types::ProbeOutcome::Refused, error.to_string());
        }
    };
    let fetched = match client.get(&url, cancellation, auth).await {
        Ok(fetched) => fetched,
        Err(error) => {
            return ProbeReport {
                value: None,
                outcome: outcome_for(&error),
                http_status: None,
                elapsed_ms: 0,
                bytes: 0,
                truncated: false,
                error_code: Some(error.code().to_owned()),
                error_message: Some(error.to_string()),
                rate_limited: matches!(error, Error::RateLimited { .. }),
                blocked: false,
                sample: None,
                retry_after_secs: None,
            };
        }
    };
    let outcome = classify_outcome(&fetched);
    if outcome != crate::types::ProbeOutcome::Success {
        let mut report = ProbeReport::from_fetch(&fetched, outcome);
        report.error_message = Some(format!("the instance returned HTTP {}", fetched.status));
        return report;
    }
    if !fetched.is_json() {
        let mut report = ProbeReport::from_fetch(&fetched, crate::types::ProbeOutcome::Refused);
        report.error_code = Some("format.json_disabled".to_owned());
        report.error_message = Some("the instance did not return JSON".to_owned());
        return report;
    }
    match JsonSearch::parse(&fetched.body) {
        Ok(search) => ProbeReport {
            value: Some(search),
            outcome: crate::types::ProbeOutcome::Success,
            http_status: Some(fetched.status),
            elapsed_ms: i64::try_from(fetched.elapsed_ms).unwrap_or(i64::MAX),
            bytes: fetched.body.len(),
            truncated: fetched.truncated,
            error_code: None,
            error_message: None,
            rate_limited: false,
            blocked: false,
            sample: None,
            retry_after_secs: Some(fetched.retry_after_secs).flatten(),
        },
        Err(error) => {
            if crate::searxng::is_index_page(&String::from_utf8_lossy(&fetched.body)) {
                let mut report =
                    ProbeReport::from_fetch(&fetched, crate::types::ProbeOutcome::Refused);
                report.error_message =
                    Some("the instance sent its landing page instead of results".to_owned());
                return report;
            }
            ProbeReport {
                value: None,
                outcome: crate::types::ProbeOutcome::ParserError,
                http_status: Some(fetched.status),
                elapsed_ms: i64::try_from(fetched.elapsed_ms).unwrap_or(i64::MAX),
                bytes: fetched.body.len(),
                truncated: fetched.truncated,
                error_code: Some(error.code().to_owned()),
                error_message: Some(error.to_string()),
                rate_limited: false,
                blocked: false,
                sample: Some(fetched.body.clone()),
                retry_after_secs: fetched.retry_after_secs,
            }
        }
    }
}

fn outcome_for(error: &Error) -> crate::types::ProbeOutcome {
    match error {
        Error::Timeout { .. } => crate::types::ProbeOutcome::Timeout,
        Error::RateLimited { .. } => crate::types::ProbeOutcome::RateLimited,
        Error::Cancelled | Error::PolicyRefused(_) => crate::types::ProbeOutcome::Refused,
        _ => crate::types::ProbeOutcome::HttpError,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_answer_with_extra_fields_parses() {
        let body = br#"{
            "query": "rust",
            "number_of_results": 2,
            "results": [
                {"url": "https://a.example/", "title": "A", "content": "one", "engine": "google", "engines": ["google", "bing"], "positions": [1, 2], "score": 0.9, "unknown": 1},
                {"url": "https://b.example/", "title": "B", "content": "two", "engine": "duckduckgo"}
            ],
            "answers": [],
            "suggestions": ["rust lang"],
            "unresponsive_engines": [["brave", "timeout"]]
        }"#;
        let search = JsonSearch::parse(body).expect("json");
        assert_eq!(search.result_count(), 2);
        let first = search.results.first().expect("first");
        assert_eq!(first.engine_names(), vec!["google", "bing"]);
        assert_eq!(first.observed_rank(), Some(1));
        assert_eq!(search.number_of_results, Some(2));
    }

    #[test]
    fn a_body_that_is_not_a_search_document_fails() {
        assert!(JsonSearch::parse(b"<html></html>").is_err());
        assert!(JsonSearch::parse(b"{}").is_err());
    }

    #[test]
    fn a_hostile_result_array_is_cut_at_the_cap() {
        let results: Vec<serde_json::Value> = (0..5_000)
            .map(|index| {
                serde_json::json!({
                    "url": format!("https://example.com/{index}"),
                    "title": format!("Result {index}"),
                })
            })
            .collect();
        let body = serde_json::json!({ "results": results });
        let parsed = JsonSearch::parse(body.to_string().as_bytes()).expect("parse");
        assert_eq!(parsed.results.len(), MAX_PARSED_RESULTS);
    }
}
