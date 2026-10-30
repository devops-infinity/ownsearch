use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::{Fetched, FleetClient, config_url};
use crate::error::{Error, Result};
use crate::searxng::{ProbeReport, classify_outcome};

#[derive(Debug, Clone, Default)]
pub struct ConfigDocument {
    pub categories: Vec<String>,
    pub engines: Vec<String>,
    pub locales: Vec<String>,
    pub plugins: Vec<String>,
    pub safe_search: Option<u8>,
    pub instance_name: Option<String>,
    pub version: Option<String>,
    pub default_locale: Option<String>,
}

impl ConfigDocument {
    pub fn parse(body: &[u8]) -> Result<Self> {
        let answer: Value =
            serde_json::from_slice(body).map_err(|error| Error::parser("searxng config", error))?;
        if !answer.is_object() {
            return Err(Error::parser(
                "searxng config",
                std::io::Error::other("the answer is not a JSON object"),
            ));
        }
        Ok(Self {
            categories: string_list(answer.get("categories")),
            engines: engine_names(answer.get("engines")),
            locales: locale_names(answer.get("locales")),
            plugins: string_list(answer.get("plugins")),
            safe_search: answer
                .get("safe_search")
                .and_then(Value::as_u64)
                .and_then(|raw| u8::try_from(raw).ok()),
            instance_name: answer
                .get("instance_name")
                .and_then(Value::as_str)
                .and_then(bounded_name),
            version: answer
                .get("version")
                .and_then(Value::as_str)
                .and_then(bounded_name),
            default_locale: answer
                .get("default_locale")
                .and_then(Value::as_str)
                .and_then(bounded_name),
        })
    }

    pub fn has_engine(&self, name: &str) -> bool {
        self.engines
            .iter()
            .any(|engine| engine.eq_ignore_ascii_case(name))
    }

    pub fn exact_google(&self) -> bool {
        self.has_engine("google")
    }

    pub fn google_cse(&self) -> bool {
        self.has_engine("google cse")
    }
}

const MAX_NAMES: usize = 256;
const MAX_NAME_CHARS: usize = 64;

fn bounded_name(raw: &str) -> Option<String> {
    let name = crate::sanitize::sanitize(raw);
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return None;
    }
    Some(name)
}

fn string_list(answer: Option<&Value>) -> Vec<String> {
    let Some(items) = answer.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let name = match item {
            Value::String(text) => Some(text.clone()),
            Value::Object(map) => map.get("name").and_then(Value::as_str).map(str::to_owned),
            _ => None,
        };
        if let Some(name) = name.and_then(|name| bounded_name(&name))
            && !out.iter().any(|existing| existing == &name)
            && out.len() < MAX_NAMES
        {
            out.push(name);
        }
    }
    out.sort();
    out
}

fn engine_names(answer: Option<&Value>) -> Vec<String> {
    let Some(items) = answer.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let name = match item {
            Value::String(text) => Some(text.clone()),
            Value::Object(map) => {
                let enabled = map.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                if !enabled {
                    None
                } else {
                    map.get("name").and_then(Value::as_str).map(str::to_owned)
                }
            }
            _ => None,
        };
        if let Some(name) = name.and_then(|name| bounded_name(&name))
            && !out.iter().any(|existing| existing == &name)
            && out.len() < MAX_NAMES
        {
            out.push(name);
        }
    }
    out.sort();
    out
}

fn locale_names(answer: Option<&Value>) -> Vec<String> {
    match answer {
        Some(Value::Object(map)) => {
            let mut out: Vec<String> = map
                .keys()
                .filter_map(|name| bounded_name(name))
                .take(MAX_NAMES)
                .collect();
            out.sort();
            out
        }
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .filter_map(bounded_name)
            .take(MAX_NAMES)
            .collect(),
        _ => Vec::new(),
    }
}

pub async fn fetch(
    client: &FleetClient,
    base: &Url,
    cancellation: &CancellationToken,
    auth: Option<&crate::auth::InstanceAuth>,
) -> ProbeReport<ConfigDocument> {
    let url = match config_url(base) {
        Ok(url) => url,
        Err(error) => {
            return ProbeReport::failed(crate::types::ProbeOutcome::Refused, error.to_string());
        }
    };
    let fetched: Fetched = match client.get(&url, cancellation, auth).await {
        Ok(fetched) => fetched,
        Err(error) => return from_error(&error),
    };
    let outcome = classify_outcome(&fetched);
    if outcome != crate::types::ProbeOutcome::Success {
        let mut report = ProbeReport::from_fetch(&fetched, outcome);
        report.error_message = Some(format!("the instance answered HTTP {}", fetched.status));
        return report;
    }
    match ConfigDocument::parse(&fetched.body) {
        Ok(document) => ProbeReport {
            value: Some(document),
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
        Err(error) => ProbeReport {
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
        },
    }
}

fn from_error<T>(error: &Error) -> ProbeReport<T> {
    let outcome = match error {
        Error::Timeout { .. } => crate::types::ProbeOutcome::Timeout,
        Error::RateLimited { .. } => crate::types::ProbeOutcome::RateLimited,
        Error::Cancelled | Error::PolicyRefused(_) => crate::types::ProbeOutcome::Refused,
        _ => crate::types::ProbeOutcome::HttpError,
    };
    ProbeReport {
        value: None,
        outcome,
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_document_is_read_tolerantly() {
        let body = br#"{
            "categories": ["general", "news"],
            "engines": [{"name": "google", "enabled": true}, {"name": "disabled one", "enabled": false}, "duckduckgo"],
            "locales": {"en": "English", "bn": "Bengali"},
            "plugins": ["Hash plugin"],
            "safe_search": 1,
            "instance_name": "Example",
            "version": "2026.10.24+ef0ec03da",
            "default_locale": "en"
        }"#;
        let document = ConfigDocument::parse(body).expect("config");
        assert_eq!(document.engines, vec!["duckduckgo", "google"]);
        assert!(document.exact_google());
        assert!(!document.google_cse());
        assert_eq!(document.safe_search, Some(1));
        assert_eq!(document.instance_name.as_deref(), Some("Example"));
        assert_eq!(document.locales, vec!["bn", "en"]);
    }

    #[test]
    fn a_non_object_config_is_a_parser_error() {
        assert!(ConfigDocument::parse(b"[]").is_err());
        assert!(ConfigDocument::parse(b"<html>").is_err());
    }

    #[test]
    fn a_hostile_config_list_is_cut_at_the_cap() {
        let engines: Vec<String> = (0..100_000)
            .map(|index| format!("engine-{index}"))
            .collect();
        let answer = serde_json::json!({ "engines": engines });
        let document = ConfigDocument::parse(answer.to_string().as_bytes()).expect("config");
        assert_eq!(document.engines.len(), MAX_NAMES);
        let long = "x".repeat(200);
        let answer = serde_json::json!({ "categories": [long] });
        let document = ConfigDocument::parse(answer.to_string().as_bytes()).expect("config");
        assert!(document.categories.is_empty());
    }
}
