pub mod config_client;
pub mod search_html;
pub mod search_json;

use crate::client::Fetched;
use crate::types::{ProbeOutcome, SafeSearch, TimeRange};

#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub query: String,
    pub categories: Vec<String>,
    pub engines: Vec<String>,
    pub language: Option<String>,
    pub page: u32,
    pub time_range: Option<TimeRange>,
    pub safe_search: SafeSearch,
}

impl SearchQuery {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            categories: Vec::new(),
            engines: Vec::new(),
            language: None,
            page: 1,
            time_range: None,
            safe_search: SafeSearch::Moderate,
        }
    }

    pub fn params(&self, json: bool) -> Vec<(&'static str, String)> {
        let mut params: Vec<(&'static str, String)> = vec![("q", self.query.clone())];
        if json {
            params.push(("format", "json".to_owned()));
        }
        if self.page > 1 {
            params.push(("pageno", self.page.to_string()));
        }
        if !self.categories.is_empty() {
            params.push(("categories", self.categories.join(",")));
        }
        if !self.engines.is_empty() {
            params.push(("engines", self.engines.join(",")));
        }
        if let Some(language) = &self.language {
            params.push(("language", language.clone()));
        }
        if let Some(range) = self.time_range {
            params.push(("time_range", range.as_str().to_owned()));
        }
        params.push(("safesearch", self.safe_search.as_u8().to_string()));
        params
    }
}

#[derive(Debug, Clone)]
pub struct ProbeReport<T> {
    pub value: Option<T>,
    pub outcome: ProbeOutcome,
    pub http_status: Option<u16>,
    pub elapsed_ms: i64,
    pub bytes: usize,
    pub truncated: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub rate_limited: bool,
    pub blocked: bool,
    pub sample: Option<Vec<u8>>,
    pub retry_after_secs: Option<u64>,
}

impl<T> ProbeReport<T> {
    pub fn failed(outcome: ProbeOutcome, message: impl Into<String>) -> Self {
        Self {
            value: None,
            outcome,
            http_status: None,
            elapsed_ms: 0,
            bytes: 0,
            truncated: false,
            error_code: Some(outcome.as_str().to_owned()),
            error_message: Some(message.into()),
            rate_limited: false,
            blocked: false,
            sample: None,
            retry_after_secs: None,
        }
    }

    pub fn from_fetch(fetched: &Fetched, outcome: ProbeOutcome) -> Self {
        Self {
            value: None,
            outcome,
            http_status: Some(fetched.status),
            elapsed_ms: i64::try_from(fetched.elapsed_ms).unwrap_or(i64::MAX),
            bytes: fetched.body.len(),
            truncated: fetched.truncated,
            error_code: Some(outcome.as_str().to_owned()),
            error_message: None,
            rate_limited: fetched.status == 429,
            blocked: outcome == ProbeOutcome::Blocked,
            sample: None,
            retry_after_secs: fetched.retry_after_secs,
        }
    }

    pub fn is_success(&self) -> bool {
        self.outcome.is_success()
    }
}

const CHALLENGE_MARKERS: [&str; 16] = [
    "anubis_challenge",
    "making sure you're not a bot",
    "verifying your browser",
    "checking your browser",
    "security check",
    "substation",
    "attention required",
    "verify you are human",
    "enable javascript and cookies",
    "name=\"captcha",
    "antibot/captcha",
    "cf-chl",
    "just a moment...",
    "ddos-guard",
    "cap-widget",
    "security verification",
];

pub(crate) fn is_rate_limit_body(text: &str) -> bool {
    if text.len() > 4096 {
        return false;
    }
    let lowered = text.to_ascii_lowercase();
    lowered.contains("too many requests")
        || lowered.contains("rate limit exceeded")
        || lowered.contains("rate-limit exceeded")
}

pub(crate) fn is_challenge_page(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    CHALLENGE_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

const INDEX_PAGE_MARKERS: [&str; 3] = [
    "class=\"index_endpoint\"",
    "id=\"main_index\"",
    "name=\"endpoint\" content=\"index\"",
];

pub(crate) fn is_index_page(text: &str) -> bool {
    INDEX_PAGE_MARKERS
        .iter()
        .any(|marker| text.contains(marker))
}

pub(crate) fn classify(fetched: &Fetched) -> Option<ProbeOutcome> {
    if fetched.status == 429 {
        return Some(ProbeOutcome::RateLimited);
    }
    if fetched.status == 403 {
        return Some(ProbeOutcome::Refused);
    }
    if fetched.status == 401 {
        return Some(ProbeOutcome::Blocked);
    }
    if fetched.status >= 400 {
        return Some(ProbeOutcome::HttpError);
    }
    if fetched.is_json() {
        return None;
    }
    let text = String::from_utf8_lossy(&fetched.body);
    if is_challenge_page(&text) {
        return Some(ProbeOutcome::Blocked);
    }
    if is_rate_limit_body(&text) {
        return Some(ProbeOutcome::RateLimited);
    }
    None
}

pub(crate) fn classify_outcome(fetched: &Fetched) -> ProbeOutcome {
    classify(fetched).unwrap_or(ProbeOutcome::Success)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_rate_limit_body_is_recognized() {
        assert!(is_rate_limit_body("Too Many Requests"));
        assert!(is_rate_limit_body("Rate limit exceeded, try later"));
        assert!(!is_rate_limit_body(&"x".repeat(5000)));
        assert!(!is_rate_limit_body("<html>results</html>"));
    }

    #[test]
    fn a_challenge_marker_is_recognized() {
        assert!(is_challenge_page("Making sure you're not a bot!"));
        assert!(is_challenge_page("<script id=\"anubis_challenge\">"));
        assert!(is_challenge_page(
            "<title>Security check - Substation</title><p>Verify you are human</p>"
        ));
        assert!(is_challenge_page(
            "<h1>Checking your browser before accessing</h1>"
        ));
        assert!(is_challenge_page(
            r#"<cap-widget data-cap-api-endpoint="/__cap/"></cap-widget>"#
        ));
        assert!(is_challenge_page("<title>Security Verification</title>"));
        assert!(!is_challenge_page("normal results page"));
    }

    #[test]
    fn an_instance_index_page_is_recognized() {
        assert!(is_index_page(
            r#"<body class="index_endpoint"><main id="main_index"></main></body>"#
        ));
        assert!(is_index_page(
            r#"<meta name="endpoint" content="index"><div class="index"></div>"#
        ));
        assert!(!is_index_page(
            r#"<meta name="endpoint" content="results"><div id="results"></div>"#
        ));
        assert!(!is_index_page(
            "<html><body>normal results page</body></html>"
        ));
    }
}
