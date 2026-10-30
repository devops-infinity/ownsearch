use std::sync::LazyLock;

use scraper::{Html, Selector};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::{FleetClient, search_url};
use crate::sanitize::sanitize;
use crate::searxng::{ProbeReport, SearchQuery, classify_outcome};

const MAX_PARSED_RESULTS: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub struct HtmlResult {
    pub title: String,
    pub url: String,
    pub snippet: Option<String>,
    pub engine: Option<String>,
    pub position: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HtmlSearch {
    pub results: Vec<HtmlResult>,
    pub containers: usize,
    pub blocked: bool,
    pub drifted: bool,
}

static CONTAINER: LazyLock<Option<Selector>> =
    LazyLock::new(|| selector("article.result, div.result, article.result-default"));
static TITLE_LINK: LazyLock<Option<Selector>> =
    LazyLock::new(|| selector("h3 a, a.url_wrapper, a"));
static SNIPPET: LazyLock<Option<Selector>> =
    LazyLock::new(|| selector("p.content, div.content, .content"));
static ENGINE_SPANS: LazyLock<Option<Selector>> = LazyLock::new(|| selector(".engines span"));
static ENGINE_SINGLE: LazyLock<Option<Selector>> = LazyLock::new(|| selector(".engine"));

impl HtmlSearch {
    pub fn parse(markup: &str, base: &Url) -> Self {
        let document = Html::parse_document(markup);
        let mut results = Vec::new();
        let mut containers = 0;
        let Some(container_selector) = &*CONTAINER else {
            return Self {
                results,
                containers,
                blocked: false,
                drifted: true,
            };
        };
        for element in document.select(container_selector) {
            containers += 1;
            if results.len() >= MAX_PARSED_RESULTS {
                break;
            }
            let anchor = TITLE_LINK
                .as_ref()
                .and_then(|selector| element.select(selector).next());
            let href = anchor.and_then(|anchor| anchor.value().attr("href"));
            let title = anchor
                .map(|anchor| sanitize(&anchor.text().collect::<String>()))
                .unwrap_or_default();
            let url = href
                .and_then(|href| base.join(href).ok())
                .map(|url| url.to_string());
            let Some(url) = url else {
                continue;
            };
            if title.is_empty() {
                continue;
            }
            let snippet_text = SNIPPET
                .as_ref()
                .and_then(|selector| element.select(selector).next())
                .map(|node| sanitize(&node.text().collect::<String>()))
                .filter(|text| !text.is_empty());
            let mut engine_names: Vec<String> = Vec::new();
            if let Some(selector) = ENGINE_SPANS.as_ref() {
                for node in element.select(selector) {
                    let text = sanitize(&node.text().collect::<String>());
                    if !text.is_empty() && !engine_names.contains(&text) {
                        engine_names.push(text);
                    }
                }
            }
            if engine_names.is_empty()
                && let Some(selector) = ENGINE_SINGLE.as_ref()
                && let Some(node) = element.select(selector).next()
            {
                let text = sanitize(&node.text().collect::<String>());
                if !text.is_empty() {
                    engine_names.push(text);
                }
            }
            let engine_text = (!engine_names.is_empty()).then(|| engine_names.join(", "));
            results.push(HtmlResult {
                title,
                url,
                snippet: snippet_text,
                engine: engine_text,
                position: u32::try_from(results.len()).unwrap_or(0).saturating_add(1),
            });
        }
        let captcha = crate::challenge::detect(markup);
        if let Some(challenge) = &captcha {
            tracing::debug!(
                kind = challenge.kind.as_str(),
                sitekey = %challenge.sitekey,
                "the page serves a captcha"
            );
        }
        let blocked =
            results.is_empty() && (captcha.is_some() || crate::searxng::is_challenge_page(markup));
        let has_results_root = markup.contains("id=\"results\"") || markup.contains("id=\"urls\"");
        Self {
            drifted: results.is_empty() && !blocked && !has_results_root,
            results,
            containers,
            blocked,
        }
    }
}

fn selector(raw: &str) -> Option<Selector> {
    Selector::parse(raw).ok()
}

pub async fn fetch(
    client: &FleetClient,
    base: &Url,
    query: &SearchQuery,
    cancellation: &CancellationToken,
    auth: Option<&crate::auth::InstanceAuth>,
) -> ProbeReport<HtmlSearch> {
    let params = query.params(false);
    let url = match search_url(base, &params) {
        Ok(url) => url,
        Err(error) => {
            return ProbeReport::failed(crate::types::ProbeOutcome::Refused, error.to_string());
        }
    };
    let fetched = match client.get(&url, cancellation, auth).await {
        Ok(fetched) => fetched,
        Err(error) => {
            let outcome = match &error {
                crate::error::Error::Timeout { .. } => crate::types::ProbeOutcome::Timeout,
                crate::error::Error::RateLimited { .. } => crate::types::ProbeOutcome::RateLimited,
                crate::error::Error::PolicyRefused(_) | crate::error::Error::Cancelled => {
                    crate::types::ProbeOutcome::Refused
                }
                _ => crate::types::ProbeOutcome::HttpError,
            };
            return ProbeReport {
                value: None,
                outcome,
                http_status: None,
                elapsed_ms: 0,
                bytes: 0,
                truncated: false,
                error_code: Some(error.code().to_owned()),
                error_message: Some(error.to_string()),
                rate_limited: matches!(error, crate::error::Error::RateLimited { .. }),
                blocked: false,
                sample: None,
                retry_after_secs: None,
            };
        }
    };
    let outcome = classify_outcome(&fetched);
    if outcome != crate::types::ProbeOutcome::Success {
        let mut report = ProbeReport::from_fetch(&fetched, outcome);
        report.error_message = Some(format!("the instance answered HTTP {}", fetched.status));
        return report;
    }
    let markup = fetched.text();
    let parsed = HtmlSearch::parse(&markup, base);
    let blocked = parsed.blocked;
    let outcome = if blocked {
        crate::types::ProbeOutcome::Blocked
    } else if parsed.drifted {
        crate::types::ProbeOutcome::ParserError
    } else {
        crate::types::ProbeOutcome::Success
    };
    ProbeReport {
        value: Some(parsed),
        outcome,
        http_status: Some(fetched.status),
        elapsed_ms: i64::try_from(fetched.elapsed_ms).unwrap_or(i64::MAX),
        bytes: fetched.body.len(),
        truncated: fetched.truncated,
        error_code: (outcome != crate::types::ProbeOutcome::Success)
            .then(|| outcome.as_str().to_owned()),
        error_message: blocked.then(|| "the instance served a captcha page".to_owned()),
        rate_limited: false,
        blocked,
        sample: (outcome == crate::types::ProbeOutcome::ParserError).then(|| fetched.body.clone()),
        retry_after_secs: fetched.retry_after_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://searx.example/").expect("base")
    }

    #[test]
    fn a_vanilla_theme_page_yields_results() {
        let markup = r#"<html><body>
            <article class="result result-default">
                <h3><a href="https://a.example/page">A title</a></h3>
                <p class="content">A snippet.</p>
                <div class="engines"><span>google</span><span>bing</span></div>
            </article>
            <article class="result">
                <h3><a href="/relative">Second</a></h3>
                <p class="content">Second snippet.</p>
                <span class="engine">duckduckgo</span>
            </article>
        </body></html>"#;
        let parsed = HtmlSearch::parse(markup, &base());
        assert_eq!(parsed.results.len(), 2);
        assert_eq!(
            parsed.results.first().map(|item| item.title.as_str()),
            Some("A title")
        );
        assert_eq!(
            parsed.results.first().map(|item| item.url.as_str()),
            Some("https://a.example/page")
        );
        assert_eq!(
            parsed
                .results
                .first()
                .and_then(|item| item.engine.as_deref()),
            Some("google, bing")
        );
        assert_eq!(
            parsed.results.get(1).map(|item| item.url.as_str()),
            Some("https://searx.example/relative")
        );
        assert!(!parsed.drifted);
    }

    #[test]
    fn a_customized_theme_with_article_only_still_parses() {
        let markup = r#"<div><article class="result">
            <a class="url_wrapper" href="https://b.example/x"><h3>B title</h3></a>
            <p class="content">B snippet</p>
        </article></div>"#;
        let parsed = HtmlSearch::parse(markup, &base());
        assert_eq!(parsed.results.len(), 1);
        assert_eq!(
            parsed.results.first().map(|item| item.title.as_str()),
            Some("B title")
        );
    }

    #[test]
    fn a_captcha_page_is_reported_as_blocked() {
        let markup = r#"<html><body><form><input name="captcha"></form></body></html>"#;
        let parsed = HtmlSearch::parse(markup, &base());
        assert!(parsed.blocked);
        assert!(parsed.results.is_empty());
    }

    #[test]
    fn a_bot_challenge_page_is_blocked_not_drifted() {
        let markup = r#"<html><head><script id="anubis_challenge" type="application/json">{}</script></head>
            <body><h1>Making sure you're not a bot!</h1></body></html>"#;
        let parsed = HtmlSearch::parse(markup, &base());
        assert!(parsed.blocked);
        assert!(!parsed.drifted);
    }

    #[test]
    fn a_page_with_no_results_container_is_drifted() {
        let markup = r#"<html><body><div class="something-else"></div></body></html>"#;
        let parsed = HtmlSearch::parse(markup, &base());
        assert!(parsed.drifted);
    }

    #[test]
    fn a_results_page_with_zero_matches_is_not_drifted() {
        let markup = r#"<html><body><div id="results"><div id="urls"></div></div></body></html>"#;
        let parsed = HtmlSearch::parse(markup, &base());
        assert!(!parsed.drifted);
        assert!(parsed.results.is_empty());
    }

    #[test]
    fn an_empty_theme_is_reported_as_drift() {
        let markup = r#"<html><body><article class="result"></article></body></html>"#;
        let parsed = HtmlSearch::parse(markup, &base());
        assert!(parsed.drifted);
    }
}
