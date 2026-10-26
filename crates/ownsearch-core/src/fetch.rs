use std::sync::Arc;
use std::time::Instant;

use tokio_util::sync::CancellationToken;
use url::Url;

use crate::context::ToolContext;
use crate::error::{Error, Result};
use crate::extract;
use crate::response::{FetchOutput, FetchPage};
use crate::sanitize::cap;
use crate::types::{FetchFormat, RenderMode};

const MAX_CRAWLED_PAGES: usize = 7;
const PDF_TEXT_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub url: String,
    pub format: FetchFormat,
    pub max_chars: usize,
    pub max_pages: usize,
    pub same_host_only: bool,
    pub render: RenderMode,
}

#[derive(Debug)]
pub struct FetchOutcome {
    pub output: FetchOutput,
    pub cache_hit: bool,
    pub bytes: u64,
}

pub async fn fetch(
    ctx: &Arc<ToolContext>,
    request: &FetchRequest,
    cancellation: &CancellationToken,
) -> Result<FetchOutcome> {
    let started = Instant::now();
    let url = Url::parse(&request.url)
        .map_err(|error| Error::InvalidArgs(format!("url does not parse: {error}")))?;
    if !matches!(url.scheme(), "https" | "http") {
        return Err(Error::InvalidArgs(
            "url must use http or https; only https is fetched from a public host".into(),
        ));
    }
    if !ctx.settings.allow_private_instances() && !crate::network_guard::url_is_public(&url).await {
        return Err(Error::PolicyRefused(
            "the url resolves to a non-public address".into(),
        ));
    }
    let canonical = serde_json::json!({
        "tool": "ownsearch_fetch",
        "url": url.as_str(),
        "format": request.format.as_str(),
        "maxChars": request.max_chars,
        "maxPages": request.max_pages,
        "sameHostOnly": request.same_host_only,
        "render": request.render.as_str(),
    });
    let cache_key = format!(
        "ownsearch_fetch:{}:{}",
        ctx.fleet_generation(),
        ctx.hash_args(&canonical)
    );
    if let Some(outcome) = cached_fetch(ctx, &cache_key)? {
        return Ok(outcome);
    }
    let budget = std::time::Duration::from_secs(ctx.settings.fetch_deadline_secs().max(5));
    let deadline = tokio::time::Instant::now() + budget;
    if request.render == RenderMode::Always {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let (rendered_html, rendered_markdown) = ctx.browser.page(&url, remaining).await?;
        let (title, content) = rendered_content(request, &rendered_html, rendered_markdown, &url);
        let (content, truncated) = cap(&content, request.max_chars);
        let output = FetchOutput {
            url: url.to_string(),
            final_url: url.to_string(),
            redirects: Vec::new(),
            status: 200,
            content_type: Some("text/html".to_owned()),
            format: request.format.as_str().to_owned(),
            fetch_mode: "browser".to_owned(),
            title,
            content,
            truncated,
            pages: Vec::new(),
            warnings: Vec::new(),
            cache_hit: false,
            cached_age_secs: None,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        return Ok(cached_outcome(ctx, cache_key, output));
    }
    let fetched = tokio::time::timeout_at(
        deadline,
        ctx.client.get(
            &url,
            "text/html,application/xhtml+xml,text/plain;q=0.9",
            cancellation,
        ),
    )
    .await
    .map_err(|_| Error::Timeout {
        deadline_ms: u64::try_from(budget.as_millis()).unwrap_or(0),
    })??;
    if fetched.status == 429 {
        return Err(Error::RateLimited {
            message: "the page host is rate limiting this client".into(),
        });
    }
    if !fetched.is_success() {
        return Err(Error::Upstream {
            status: fetched.status,
            message: format!("the page host answered HTTP {}", fetched.status),
        });
    }
    let is_pdf = fetched.is_pdf();
    let final_url = Url::parse(&fetched.final_url).unwrap_or_else(|_| url.clone());
    let redirects = fetched.redirects.clone();
    if is_pdf {
        let text = pdf_text(fetched.body.clone(), deadline).await?;
        let (content, truncated) = cap(&extract::from_pdf_text(&text), request.max_chars);
        let output = FetchOutput {
            url: url.to_string(),
            final_url: final_url.to_string(),
            redirects,
            status: fetched.status,
            content_type: fetched.content_type.clone(),
            format: request.format.as_str().to_owned(),
            fetch_mode: "http".to_owned(),
            title: None,
            content,
            truncated,
            pages: Vec::new(),
            warnings: Vec::new(),
            cache_hit: false,
            cached_age_secs: None,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        return Ok(cached_outcome(ctx, cache_key, output));
    }
    let body = fetched.text();
    let is_html = fetched.is_html() || looks_like_html(&body);
    let pages = if request.max_pages > 1 && is_html {
        crawl(ctx, &final_url, &body, request, cancellation, deadline).await
    } else {
        Vec::new()
    };
    let (title, content) = extract_content(request, &body, is_html, &url);
    let mut fetch_mode = "http".to_owned();
    let mut warnings: Vec<String> = Vec::new();
    let needs_render = match request.render {
        RenderMode::Always => true,
        RenderMode::Never => false,
        RenderMode::Auto => is_html && looks_like_javascript_shell(&body, &content),
    };
    let (title, content) = if needs_render {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match ctx.browser.page(&url, remaining).await {
            Ok((rendered_html, rendered_markdown)) => {
                fetch_mode = "browser".to_owned();
                rendered_content(request, &rendered_html, rendered_markdown, &url)
            }
            Err(error) if request.render == RenderMode::Always => return Err(error),
            Err(error) => {
                warnings.push(format!("the browser render failed: {error}"));
                (title, content)
            }
        }
    } else {
        (title, content)
    };
    let (content, truncated) = cap(&content, request.max_chars);
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let output = FetchOutput {
        url: url.to_string(),
        final_url: final_url.to_string(),
        redirects,
        status: fetched.status,
        content_type: fetched.content_type.clone(),
        format: request.format.as_str().to_owned(),
        fetch_mode,
        title,
        content,
        truncated,
        pages,
        warnings,
        cache_hit: false,
        cached_age_secs: None,
        elapsed_ms,
    };
    Ok(cached_outcome(ctx, cache_key, output))
}

fn cached_fetch(ctx: &Arc<ToolContext>, cache_key: &str) -> Result<Option<FetchOutcome>> {
    let Some((cached, age)) = ctx.cache.get(cache_key)? else {
        return Ok(None);
    };
    let Ok(mut output) = serde_json::from_value::<FetchOutput>((*cached).clone()) else {
        return Ok(None);
    };
    output.cache_hit = true;
    output.cached_age_secs = Some(age);
    ctx.metrics.count_cache_hit();
    Ok(Some(FetchOutcome {
        bytes: 0,
        cache_hit: true,
        output,
    }))
}

fn cached_outcome(ctx: &Arc<ToolContext>, cache_key: String, output: FetchOutput) -> FetchOutcome {
    if ctx.cache.enabled()
        && let Ok(value) = serde_json::to_value(&output)
        && let Err(error) = ctx.cache.put(cache_key, Arc::new(value))
    {
        tracing::warn!(%error, "the fetched page was not cached");
    }
    let bytes = serde_json::to_vec(&output)
        .map(|body| body.len() as u64)
        .unwrap_or(0);
    FetchOutcome {
        output,
        cache_hit: false,
        bytes,
    }
}

fn extract_content(
    request: &FetchRequest,
    body: &str,
    is_html: bool,
    url: &Url,
) -> (Option<String>, String) {
    match (request.format, is_html) {
        (FetchFormat::Html, true) => (
            extract::title_from_html(body),
            extract::clean_html(body, url),
        ),
        (FetchFormat::Html, false) => (None, body.to_owned()),
        (FetchFormat::Markdown, true) => {
            let extracted = extract::to_markdown(body, url);
            (extracted.title, extracted.body)
        }
        (FetchFormat::Markdown, false) => (None, body.to_owned()),
        (FetchFormat::Text, true) => {
            let extracted = extract::to_text(body, url);
            (extracted.title, extracted.body)
        }
        (FetchFormat::Text, false) => (None, body.to_owned()),
    }
}

fn rendered_content(
    request: &FetchRequest,
    html: &str,
    markdown: Option<String>,
    url: &Url,
) -> (Option<String>, String) {
    match request.format {
        FetchFormat::Html => (
            extract::title_from_html(html),
            extract::clean_html(html, url),
        ),
        FetchFormat::Markdown => match markdown.filter(|markdown| !markdown.trim().is_empty()) {
            Some(markdown) => (extract::title_from_html(html), markdown),
            None => {
                let extracted = extract::to_markdown(html, url);
                (extracted.title, extracted.body)
            }
        },
        FetchFormat::Text => {
            let extracted = extract::to_text(html, url);
            (extracted.title, extracted.body)
        }
    }
}

async fn crawl(
    ctx: &Arc<ToolContext>,
    entry: &Url,
    body: &str,
    request: &FetchRequest,
    cancellation: &CancellationToken,
    deadline: tokio::time::Instant,
) -> Vec<FetchPage> {
    let limit = request.max_pages.saturating_sub(1).min(MAX_CRAWLED_PAGES);
    if limit == 0 {
        return Vec::new();
    }
    let mut pages: Vec<FetchPage> = Vec::new();
    for link in extract::links(body, entry) {
        if pages.len() >= limit {
            break;
        }
        if link == *entry {
            continue;
        }
        if request.same_host_only && link.host_str() != entry.host_str() {
            continue;
        }
        if !ctx.settings.allow_private_instances()
            && !crate::network_guard::url_is_public(&link).await
        {
            continue;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let fetched = match tokio::time::timeout_at(
            deadline,
            ctx.client.get(
                &link,
                "text/html,application/xhtml+xml,text/plain;q=0.9",
                cancellation,
            ),
        )
        .await
        {
            Ok(Ok(fetched)) => fetched,
            _ => continue,
        };
        if !fetched.is_success() {
            continue;
        }
        let page_body = if fetched.is_pdf() {
            match pdf_text(fetched.body.clone(), deadline).await {
                Ok(text) => text,
                Err(_) => continue,
            }
        } else {
            fetched.text()
        };
        let page_is_html = fetched.is_html() || looks_like_html(&page_body);
        let (title, content) = match (request.format, page_is_html) {
            (FetchFormat::Html, true) => (
                extract::title_from_html(&page_body),
                extract::clean_html(&page_body, &link),
            ),
            (FetchFormat::Html, false) => (None, page_body),
            (FetchFormat::Markdown, true) => {
                let extracted = extract::to_markdown(&page_body, &link);
                (extracted.title, extracted.body)
            }
            (FetchFormat::Markdown, false) => (None, page_body),
            (FetchFormat::Text, true) => {
                let extracted = extract::to_text(&page_body, &link);
                (extracted.title, extracted.body)
            }
            (FetchFormat::Text, false) => (None, page_body),
        };
        let (content, truncated) = cap(&content, request.max_chars);
        pages.push(FetchPage {
            url: link.to_string(),
            title,
            content,
            truncated,
        });
    }
    pages
}

async fn pdf_text(bytes: Vec<u8>, deadline: tokio::time::Instant) -> Result<String> {
    let work = tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let document = lopdf::Document::load_mem(&bytes)?;
            let pages: Vec<u32> = document.get_pages().keys().copied().collect();
            document.extract_text_with_limit(&pages, PDF_TEXT_LIMIT)
        }))
    });
    let outcome = tokio::time::timeout_at(deadline, work)
        .await
        .map_err(|_| Error::Timeout {
            deadline_ms: u64::try_from(
                deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .as_millis(),
            )
            .unwrap_or(0),
        })?;
    let joined = outcome.map_err(|error| Error::Io {
        context: "waiting for the PDF parser",
        source: std::io::Error::other(error),
    })?;
    match joined {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(error)) => Err(Error::ParserFailed {
            format: "pdf",
            source: Box::new(std::io::Error::other(error.to_string())),
        }),
        Err(_) => Err(Error::ParserFailed {
            format: "pdf",
            source: Box::new(std::io::Error::other("the PDF parser panicked")),
        }),
    }
}

fn looks_like_javascript_shell(html: &str, content: &str) -> bool {
    let text_chars = content.chars().count();
    if text_chars > 600 {
        return false;
    }
    let lowered = html.to_ascii_lowercase();
    [
        "id=\"root\"",
        "id=\"app\"",
        "__next_data__",
        "__nuxt__",
        "ng-version",
        "enable javascript",
        "<noscript>",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
        || (text_chars < 200 && html.len() > 4_000)
}

fn looks_like_html(body: &str) -> bool {
    let head = body.trim_start();
    head.starts_with("<!doctype") || head.starts_with("<html") || head.starts_with("<!DOCTYPE")
}

pub(crate) fn summary(output: &FetchOutput) -> String {
    let title = output
        .title
        .as_deref()
        .map(|title| {
            let (capped, _) = cap(&crate::sanitize::sanitize(title), 80);
            capped
        })
        .unwrap_or_else(|| output.url.clone());
    let extra = match output.pages.len() {
        0 => String::new(),
        count => format!("; {count} more pages"),
    };
    format!(
        "{} via {} from {}; {} chars{}{}; {} ms",
        output.format,
        output.fetch_mode,
        title,
        output.content.chars().count(),
        if output.truncated { "; capped" } else { "" },
        extra,
        output.elapsed_ms
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thin_page_built_by_javascript_is_recognized() {
        let shell =
            "<html><body><div id=\"root\"></div><script src=/app.js></script></body></html>";
        assert!(looks_like_javascript_shell(shell, ""));
        let article = format!(
            "<html><body><article>{}</article></body></html>",
            "word ".repeat(300)
        );
        assert!(!looks_like_javascript_shell(&article, &"word ".repeat(300)));
    }
}
