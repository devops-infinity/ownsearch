use std::sync::Arc;
use std::time::Instant;

use tokio_util::sync::CancellationToken;
use url::Url;

use crate::context::ToolContext;
use crate::error::{Error, Result};
use crate::extract;
use crate::response::{
    FetchAttempt, FetchDocument, FetchHeading, FetchLink, FetchOutput, FetchPage, Notice,
};
use crate::sanitize::cap;
use crate::types::{FetchFormat, RenderMode};

const MAX_CRAWLED_PAGES: usize = 7;
const PDF_TEXT_LIMIT: usize = 8 * 1024 * 1024;

struct Content {
    title: Option<String>,
    body: String,
    parser: String,
    attempts: Vec<FetchAttempt>,
}

impl Content {
    fn plain(body: String) -> Self {
        Self {
            title: None,
            body,
            parser: "raw".to_owned(),
            attempts: Vec::new(),
        }
    }
}

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
        let content = rendered_content(request, &rendered_html, rendered_markdown, &url);
        let (body, truncated) = cap(&content.body, request.max_chars);
        let mut messages = content_messages(&content, truncated);
        messages.push(Notice::info(
            "fetch.status_unobserved",
            "the page was rendered in a browser, so the HTTP status and redirect chain were not observed",
        ));
        let outcome = outcome_for(&body, &messages);
        let document = build_document(
            request.format,
            DocumentInput {
                url: &url,
                final_url: &url,
                status: 200,
                content_type: Some("text/html".to_owned()),
                fetch_mode: "browser",
                parser: &content.parser,
                title: content.title.as_ref(),
                attempts: &content.attempts,
                html: Some(&rendered_html),
                content: &body,
                content_format: content_format(request.format),
                truncated,
                messages: &messages,
            },
        );
        let output = FetchOutput {
            url: url.to_string(),
            final_url: url.to_string(),
            redirects: Vec::new(),
            status: 200,
            content_type: Some("text/html".to_owned()),
            format: request.format.as_str().to_owned(),
            fetch_mode: "browser".to_owned(),
            parser: content.parser.clone(),
            title: content.title.clone(),
            content: body,
            truncated,
            outcome,
            pages: Vec::new(),
            messages,
            document,
            cache_hit: false,
            cached_age_secs: None,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        return Ok(cached_outcome(ctx, cache_key, output));
    }
    let fetched = tokio::time::timeout_at(deadline, ctx.client.get(&url, cancellation, None))
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
        let text = pdf_text(fetched.body, deadline).await?;
        let (content, truncated) = cap(&extract::from_pdf_text(&text), request.max_chars);
        let mut messages = vec![Notice::info(
            "fetch.parser",
            "the text came from the pdf parser",
        )];
        if truncated {
            messages.push(Notice::warning(
                "fetch.truncated",
                "the content was capped at max_chars",
                "raise max_chars to read the rest",
            ));
        }
        let outcome = outcome_for(&content, &messages);
        let document = build_document(
            request.format,
            DocumentInput {
                url: &url,
                final_url: &final_url,
                status: fetched.status,
                content_type: fetched.content_type.clone(),
                fetch_mode: "http",
                parser: "pdf",
                title: None,
                attempts: &[],
                html: None,
                content: &content,
                content_format: "text",
                truncated,
                messages: &messages,
            },
        );
        let output = FetchOutput {
            url: url.to_string(),
            final_url: final_url.to_string(),
            redirects,
            status: fetched.status,
            content_type: fetched.content_type.clone(),
            format: request.format.as_str().to_owned(),
            fetch_mode: "http".to_owned(),
            parser: "pdf".to_owned(),
            title: None,
            content,
            truncated,
            outcome,
            pages: Vec::new(),
            messages,
            document,
            cache_hit: false,
            cached_age_secs: None,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        return Ok(cached_outcome(ctx, cache_key, output));
    }
    let body = fetched.text();
    let is_html = fetched.is_html() || looks_like_html(&body);
    let CrawlResult {
        pages,
        skipped,
        limit_reached,
    } = if request.max_pages > 1 && is_html {
        crawl(ctx, &final_url, &body, request, cancellation, deadline).await
    } else {
        CrawlResult::default()
    };
    let content = extract_content(request, &body, is_html, &url);
    let mut fetch_mode = "http".to_owned();
    let mut messages: Vec<Notice> = Vec::new();
    if skipped > 0 {
        messages.push(Notice::warning(
            "fetch.links_skipped",
            format!(
                "{skipped} linked pages were skipped because they did not answer or did not parse"
            ),
            "fetch the lost pages directly",
        ));
    }
    if limit_reached {
        messages.push(Notice::warning(
            "fetch.links_capped",
            "the link walk stopped before it visited every link",
            "raise max_pages to follow more links",
        ));
    }
    let thin = content.body.trim().is_empty();
    let needs_render = match request.render {
        RenderMode::Always => true,
        RenderMode::Never => false,
        RenderMode::Auto => is_html && (thin || looks_like_javascript_shell(&body, &content.body)),
    };
    let mut rendered_html: Option<String> = None;
    let content = if needs_render {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match ctx.browser.page(&url, remaining).await {
            Ok((html, markdown)) => {
                fetch_mode = "browser".to_owned();
                let rendered = rendered_content(request, &html, markdown, &url);
                rendered_html = Some(html);
                rendered
            }
            Err(error) if request.render == RenderMode::Always => return Err(error),
            Err(error) => {
                messages.push(Notice::warning(
                    "fetch.render_failed",
                    format!("the browser render failed: {error}"),
                    "retry with render always, or fetch the page again later",
                ));
                content
            }
        }
    } else {
        content
    };
    if fetch_mode == "browser" {
        messages.push(Notice::info(
            "fetch.status_unobserved",
            "the page was rendered in a browser, so the HTTP status and redirect chain were not observed",
        ));
    }
    let (text, truncated) = cap(&content.body, request.max_chars);
    messages.extend(content_messages(&content, truncated));
    let outcome = outcome_for(&text, &messages);
    let parser = content.parser.clone();
    let document = build_document(
        request.format,
        DocumentInput {
            url: &url,
            final_url: &final_url,
            status: fetched.status,
            content_type: fetched.content_type.clone(),
            fetch_mode: &fetch_mode,
            parser: &parser,
            title: content.title.as_ref(),
            attempts: &content.attempts,
            html: Some(rendered_html.as_deref().unwrap_or(body.as_str())),
            content: &text,
            content_format: content_format(request.format),
            truncated,
            messages: &messages,
        },
    );
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let output = FetchOutput {
        url: url.to_string(),
        final_url: final_url.to_string(),
        redirects,
        status: fetched.status,
        content_type: fetched.content_type.clone(),
        format: request.format.as_str().to_owned(),
        fetch_mode,
        parser,
        title: content.title,
        content: text,
        truncated,
        outcome,
        pages,
        messages,
        document,
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

fn extract_content(request: &FetchRequest, body: &str, is_html: bool, url: &Url) -> Content {
    match (request.format, is_html) {
        (FetchFormat::Html, true) => Content {
            title: extract::title_from_html(body),
            body: extract::clean_html(body, url),
            parser: "html".to_owned(),
            attempts: Vec::new(),
        },
        (FetchFormat::Markdown, true) | (FetchFormat::Json, true) => {
            chained_content(body, url, true)
        }
        (FetchFormat::Text, true) => chained_content(body, url, false),
        (_, false) => Content::plain(body.to_owned()),
    }
}

fn chained_content(html: &str, url: &Url, markdown: bool) -> Content {
    let chained = extract::chained(html, url, markdown);
    Content {
        title: chained.title,
        body: chained.body,
        parser: chained.parser.to_owned(),
        attempts: chained
            .attempts
            .iter()
            .map(|attempt| FetchAttempt {
                parser: attempt.parser.to_owned(),
                characters: attempt.characters,
                used: attempt.used,
            })
            .collect(),
    }
}

fn rendered_content(
    request: &FetchRequest,
    html: &str,
    markdown: Option<String>,
    url: &Url,
) -> Content {
    match request.format {
        FetchFormat::Html => Content {
            title: extract::title_from_html(html),
            body: extract::clean_html(html, url),
            parser: "html".to_owned(),
            attempts: Vec::new(),
        },
        FetchFormat::Markdown | FetchFormat::Json => {
            match markdown.filter(|markdown| !markdown.trim().is_empty()) {
                Some(markdown) => Content {
                    title: extract::title_from_html(html),
                    body: markdown,
                    parser: "browser".to_owned(),
                    attempts: Vec::new(),
                },
                None => chained_content(html, url, true),
            }
        }
        FetchFormat::Text => chained_content(html, url, false),
    }
}

fn content_format(format: FetchFormat) -> &'static str {
    match format {
        FetchFormat::Html => "html",
        FetchFormat::Markdown | FetchFormat::Json => "markdown",
        FetchFormat::Text => "text",
    }
}

fn content_messages(content: &Content, truncated: bool) -> Vec<Notice> {
    let mut messages = vec![Notice::info(
        "fetch.parser",
        format!("the text came from the {} parser", content.parser),
    )];
    if content.parser == "none" {
        messages.push(Notice::warning(
            "fetch.empty",
            "the page answered but no text could be extracted",
            "retry with render always, another format, or another url",
        ));
    }
    if truncated {
        messages.push(Notice::warning(
            "fetch.truncated",
            "the content was capped at max_chars",
            "raise max_chars to read the rest",
        ));
    }
    messages
}

fn outcome_for(content: &str, messages: &[Notice]) -> String {
    if content.trim().is_empty() {
        return "empty".to_owned();
    }
    if messages.iter().any(|notice| notice.level == "warning") {
        return "partial".to_owned();
    }
    "success".to_owned()
}

struct DocumentInput<'a> {
    url: &'a Url,
    final_url: &'a Url,
    status: u16,
    content_type: Option<String>,
    fetch_mode: &'a str,
    parser: &'a str,
    title: Option<&'a String>,
    attempts: &'a [FetchAttempt],
    html: Option<&'a str>,
    content: &'a str,
    content_format: &'a str,
    truncated: bool,
    messages: &'a [Notice],
}

fn build_document(format: FetchFormat, input: DocumentInput<'_>) -> Option<FetchDocument> {
    if format != FetchFormat::Json {
        return None;
    }
    let metadata = input
        .html
        .map(|html| extract::metadata(html, input.url))
        .unwrap_or_default();
    let headings = input
        .html
        .map(|html| {
            extract::headings(html)
                .into_iter()
                .map(|heading| FetchHeading {
                    level: heading.level,
                    text: heading.text,
                })
                .collect()
        })
        .unwrap_or_default();
    let links = input
        .html
        .map(|html| {
            extract::doc_links(html, input.url)
                .into_iter()
                .map(|link| FetchLink {
                    url: link.url,
                    text: link.text,
                })
                .collect()
        })
        .unwrap_or_default();
    let json_ld = input.html.map(extract::json_ld).unwrap_or_default();
    Some(FetchDocument {
        url: input.url.to_string(),
        final_url: input.final_url.to_string(),
        status: input.status,
        content_type: input.content_type,
        fetch_mode: input.fetch_mode.to_owned(),
        parser: input.parser.to_owned(),
        title: input.title.cloned(),
        description: metadata.description,
        canonical_url: metadata.canonical,
        language: metadata.language,
        published_at: metadata.published_at,
        author: metadata.author,
        site_name: metadata.site_name,
        content: input.content.to_owned(),
        content_format: input.content_format.to_owned(),
        characters: input.content.chars().count(),
        truncated: input.truncated,
        attempts: input.attempts.to_vec(),
        headings,
        links,
        json_ld,
        messages: input.messages.to_vec(),
    })
}

#[derive(Debug, Default)]
struct CrawlResult {
    pages: Vec<FetchPage>,
    skipped: usize,
    limit_reached: bool,
}

async fn crawl(
    ctx: &Arc<ToolContext>,
    entry: &Url,
    body: &str,
    request: &FetchRequest,
    cancellation: &CancellationToken,
    deadline: tokio::time::Instant,
) -> CrawlResult {
    let limit = request.max_pages.saturating_sub(1).min(MAX_CRAWLED_PAGES);
    if limit == 0 {
        return CrawlResult::default();
    }
    let links = extract::links(body, entry);
    let mut result = CrawlResult::default();
    let mut pages: Vec<FetchPage> = Vec::new();
    for (index, link) in links.iter().enumerate() {
        if pages.len() >= limit || tokio::time::Instant::now() >= deadline {
            result.limit_reached = index < links.len();
            break;
        }
        if link == entry {
            continue;
        }
        if request.same_host_only && link.host_str() != entry.host_str() {
            continue;
        }
        if !ctx.settings.allow_private_instances()
            && !crate::network_guard::url_is_public(link).await
        {
            continue;
        }
        let fetched =
            match tokio::time::timeout_at(deadline, ctx.client.get(link, cancellation, None)).await
            {
                Ok(Ok(fetched)) => fetched,
                _ => {
                    result.skipped = result.skipped.saturating_add(1);
                    continue;
                }
            };
        if !fetched.is_success() {
            result.skipped = result.skipped.saturating_add(1);
            continue;
        }
        let page_content_is_html = fetched.is_html();
        let page_body = if fetched.is_pdf() {
            let body = fetched.body;
            match pdf_text(body, deadline).await {
                Ok(text) => text,
                Err(_) => {
                    result.skipped = result.skipped.saturating_add(1);
                    continue;
                }
            }
        } else {
            fetched.text()
        };
        let page_is_html = page_content_is_html || looks_like_html(&page_body);
        let (title, content) = match (request.format, page_is_html) {
            (FetchFormat::Html, true) => (
                extract::title_from_html(&page_body),
                extract::clean_html(&page_body, link),
            ),
            (FetchFormat::Html, false) => (None, page_body),
            (FetchFormat::Markdown, true) | (FetchFormat::Json, true) => {
                let extracted = extract::to_markdown(&page_body, link);
                (extracted.title, extracted.body)
            }
            (FetchFormat::Markdown, false) | (FetchFormat::Json, false) => (None, page_body),
            (FetchFormat::Text, true) => {
                let extracted = extract::to_text(&page_body, link);
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
    result.pages = pages;
    result
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
        "{} by the {} parser via {} from {}; {} chars{}{}; {} ms",
        output.format,
        output.parser,
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
