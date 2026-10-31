use std::sync::LazyLock;

use crate::sanitize::sanitize;
use ego_tree::NodeRef;
use scraper::Html;
use scraper::Selector;
use scraper::node::Node;

const MAX_TREE_DEPTH: usize = 512;
const MAX_CANDIDATE_ROOTS: usize = 256;
const CONTENT_SCAN_STEPS: usize = 2_000_000;

static TITLE_SELECTOR: LazyLock<Option<Selector>> = LazyLock::new(|| Selector::parse("title").ok());
static LINK_SELECTOR: LazyLock<Option<Selector>> =
    LazyLock::new(|| Selector::parse("a[href]").ok());
static MAIN_SELECTOR: LazyLock<Option<Selector>> = LazyLock::new(|| Selector::parse("main").ok());
static ARTICLE_SELECTOR: LazyLock<Option<Selector>> =
    LazyLock::new(|| Selector::parse("article").ok());
static ROLE_MAIN_SELECTOR: LazyLock<Option<Selector>> =
    LazyLock::new(|| Selector::parse("[role=main]").ok());
static BODY_CANDIDATE_SELECTOR: LazyLock<Option<Selector>> =
    LazyLock::new(|| Selector::parse("body div, body section, body article").ok());

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Extracted {
    pub title: Option<String>,
    pub body: String,
}

pub(crate) fn from_pdf_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut paragraph = String::new();
    for raw in text.replace('\r', "\n").lines() {
        let line = sanitize(raw);
        if line.is_empty() {
            if !paragraph.is_empty() {
                out.push_str(&paragraph);
                out.push_str("\n\n");
                paragraph.clear();
            }
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(&line);
    }
    if !paragraph.is_empty() {
        out.push_str(&paragraph);
    }
    tidy(&out)
}

pub(crate) fn title_from_html(html: &str) -> Option<String> {
    title_of(&Html::parse_document(html))
}

pub(crate) fn title_of(document: &Html) -> Option<String> {
    document
        .select(TITLE_SELECTOR.as_ref()?)
        .next()
        .map(|element| sanitize(&element.text().collect::<String>()))
        .filter(|text| !text.is_empty())
}

pub(crate) fn links(html: &str, base: &url::Url) -> Vec<url::Url> {
    let document = Html::parse_document(html);
    let Some(selector) = LINK_SELECTOR.as_ref() else {
        return Vec::new();
    };
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<url::Url> = Vec::new();
    for element in document.select(selector) {
        let Some(href) = element.value().attr("href") else {
            continue;
        };
        let Ok(mut target) = base.join(href.trim()) else {
            continue;
        };
        if !matches!(target.scheme(), "http" | "https") {
            continue;
        }
        target.set_fragment(None);
        if seen.insert(target.to_string()) {
            out.push(target);
        }
    }
    out
}

pub(crate) fn to_markdown(html: &str, base: &url::Url) -> Extracted {
    let mut document = Html::parse_document(html);
    prune_boilerplate(&mut document);
    let root = content_root(&document);
    let mut out = String::new();
    walk(root, &mut out, true, base, 0);
    Extracted {
        title: title_of(&document),
        body: tidy(&out),
    }
}

pub(crate) fn to_text(html: &str, base: &url::Url) -> Extracted {
    let mut document = Html::parse_document(html);
    prune_boilerplate(&mut document);
    let root = content_root(&document);
    let mut out = String::new();
    walk(root, &mut out, false, base, 0);
    Extracted {
        title: title_of(&document),
        body: tidy(&out),
    }
}

const BOILERPLATE_TAGS: [&str; 9] = [
    "nav", "aside", "footer", "form", "dialog", "menu", "noscript", "svg", "iframe",
];
const BOILERPLATE_HINTS: [&str; 24] = [
    "nav",
    "navbar",
    "navigation",
    "menu",
    "sidebar",
    "side-bar",
    "footer",
    "comment",
    "comments",
    "cookie",
    "consent",
    "banner",
    "advert",
    "promo",
    "share",
    "social",
    "related",
    "newsletter",
    "breadcrumb",
    "pagination",
    "pager",
    "modal",
    "popup",
    "skip-link",
];

fn prune_boilerplate(document: &mut Html) {
    let Ok(selector) = scraper::Selector::parse("*") else {
        return;
    };
    let ids: Vec<_> = document
        .select(&selector)
        .filter(|element| {
            let name = element.value().name();
            if BOILERPLATE_TAGS.contains(&name) {
                return true;
            }
            let marker = format!(
                "{} {}",
                element.value().attr("class").unwrap_or_default(),
                element.value().attr("id").unwrap_or_default()
            )
            .to_ascii_lowercase();
            if marker.trim().is_empty() {
                return false;
            }
            BOILERPLATE_HINTS
                .iter()
                .any(|hint| marker.split([' ', '_']).any(|part| part == *hint))
                || marker.contains("advert")
                || marker.contains("cookie")
                || marker.contains("sidebar")
        })
        .map(|element| element.id())
        .collect();
    for id in ids {
        if let Some(mut node) = document.tree.get_mut(id) {
            node.detach();
        }
    }
}

fn content_root(document: &Html) -> NodeRef<'_, Node> {
    for selector in [&*MAIN_SELECTOR, &*ARTICLE_SELECTOR, &*ROLE_MAIN_SELECTOR] {
        if let Some(selector) = selector
            && let Some(element) = document.select(selector).next()
        {
            return *element;
        }
    }
    let Some(selector) = BODY_CANDIDATE_SELECTOR.as_ref() else {
        return document.tree.root();
    };
    let mut best: Option<(i64, NodeRef<'_, Node>)> = None;
    let mut steps = CONTENT_SCAN_STEPS;
    'candidates: for element in document.select(selector).take(MAX_CANDIDATE_ROOTS) {
        let mut text_chars = 0i64;
        let mut link_chars = 0i64;
        let mut paragraphs = 0i64;
        for descendant in element.descendants() {
            if steps == 0 {
                break 'candidates;
            }
            steps -= 1;
            if let Node::Text(text) = descendant.value() {
                let length = i64::try_from(text.trim().chars().count()).unwrap_or(0);
                let mut linked = false;
                for ancestor in descendant.ancestors().take(MAX_TREE_DEPTH) {
                    steps = steps.saturating_sub(1);
                    if matches!(ancestor.value(), Node::Element(node) if node.name() == "a") {
                        linked = true;
                        break;
                    }
                }
                text_chars = text_chars.saturating_add(length);
                if linked {
                    link_chars = link_chars.saturating_add(length);
                }
            } else if let Node::Element(node) = descendant.value()
                && node.name() == "p"
            {
                paragraphs = paragraphs.saturating_add(1);
            }
        }
        let score = text_chars
            .saturating_sub(link_chars.saturating_mul(2))
            .saturating_add(paragraphs.saturating_mul(50));
        if score > 0 && best.as_ref().is_none_or(|(current, _)| score > *current) {
            best = Some((score, *element));
        }
    }
    best.map(|(_, element)| element)
        .unwrap_or_else(|| document.tree.root())
}

const DROPPED_ELEMENTS: [&str; 12] = [
    "script", "style", "noscript", "template", "svg", "head", "iframe", "object", "embed", "form",
    "link", "meta",
];
const ALLOWED_ELEMENTS: [&str; 24] = [
    "a",
    "article",
    "aside",
    "b",
    "blockquote",
    "br",
    "code",
    "div",
    "em",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "i",
    "li",
    "main",
    "nav",
    "ol",
    "p",
];
const VOID_ELEMENTS: [&str; 3] = ["br", "hr", "img"];

pub(crate) fn clean_html(html: &str, base: &url::Url) -> String {
    let mut document = Html::parse_document(html);
    prune_boilerplate(&mut document);
    let root = content_root(&document);
    let mut out = String::new();
    clean_node(root, &mut out, base, 0);
    tidy(&out)
}

fn clean_node(node: NodeRef<'_, Node>, out: &mut String, base: &url::Url, depth: usize) {
    if depth > MAX_TREE_DEPTH {
        return;
    }
    match node.value() {
        Node::Text(text) => {
            let raw = text.to_string();
            let text = sanitize(&raw);
            if !text.is_empty() {
                if !out.is_empty() && !out.ends_with([' ', '\n', '>']) {
                    out.push(' ');
                }
                out.push_str(&escape(&text));
                if raw.ends_with(char::is_whitespace) && !out.ends_with(' ') {
                    out.push(' ');
                }
            }
        }
        Node::Element(element) => {
            let name = element.name();
            if DROPPED_ELEMENTS.contains(&name) {
                return;
            }
            if !ALLOWED_ELEMENTS.contains(&name) {
                for child in node.children() {
                    clean_node(child, out, base, depth + 1);
                }
                return;
            }
            if is_block(name) {
                block(out);
            }
            out.push('<');
            out.push_str(name);
            if name == "a"
                && let Some(url) = safe_link(element.attr("href"), base)
            {
                out.push_str(" href=\"");
                out.push_str(&escape(url.as_str()));
                out.push_str("\" rel=\"nofollow noopener noreferrer\"");
            }
            out.push('>');
            if VOID_ELEMENTS.contains(&name) {
                return;
            }
            for child in node.children() {
                clean_node(child, out, base, depth + 1);
            }
            out.push_str("</");
            out.push_str(name);
            out.push('>');
            if is_block(name) {
                block(out);
            }
        }
        _ => {
            for child in node.children() {
                clean_node(child, out, base, depth + 1);
            }
        }
    }
}

fn safe_link(href: Option<&str>, base: &url::Url) -> Option<url::Url> {
    let raw = href?.trim();
    if raw.is_empty() {
        return None;
    }
    let joined = base.join(raw).ok()?;
    matches!(joined.scheme(), "http" | "https").then_some(joined)
}

pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

fn walk(node: NodeRef<'_, Node>, out: &mut String, markdown: bool, base: &url::Url, depth: usize) {
    if depth > MAX_TREE_DEPTH {
        return;
    }
    match node.value() {
        Node::Text(text) => {
            let text = sanitize(text);
            if !text.is_empty() {
                if !out.is_empty() && !out.ends_with([' ', '\n']) {
                    out.push(' ');
                }
                out.push_str(&text);
            }
        }
        Node::Element(element) => {
            let name = element.name();
            if matches!(
                name,
                "script" | "style" | "noscript" | "template" | "svg" | "head" | "iframe"
            ) {
                return;
            }
            if !markdown {
                if is_block(name) {
                    block(out);
                }
                if name == "a" {
                    let href = safe_link(element.attr("href"), base);
                    let start = out.len();
                    for child in node.children() {
                        walk(child, out, false, base, depth + 1);
                    }
                    if let Some(url) = href
                        && out.len() > start
                    {
                        out.push_str(&format!(" ({url})"));
                    }
                    return;
                }
                for child in node.children() {
                    walk(child, out, false, base, depth + 1);
                }
                if is_block(name) {
                    block(out);
                }
                return;
            }
            match name {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    block(out);
                    let level = name.chars().nth(1).and_then(|digit| digit.to_digit(10));
                    out.push_str(&"#".repeat(usize::try_from(level.unwrap_or(1)).unwrap_or(1)));
                    out.push(' ');
                    for child in node.children() {
                        walk(child, out, true, base, depth + 1);
                    }
                    block(out);
                }
                "p" | "div" | "section" | "article" | "header" | "footer" | "blockquote" => {
                    block(out);
                    for child in node.children() {
                        walk(child, out, true, base, depth + 1);
                    }
                    block(out);
                }
                "ul" | "ol" | "table" | "tr" => {
                    block(out);
                    for child in node.children() {
                        walk(child, out, true, base, depth + 1);
                    }
                    block(out);
                }
                "li" => {
                    block(out);
                    out.push_str("- ");
                    for child in node.children() {
                        walk(child, out, true, base, depth + 1);
                    }
                    block(out);
                }
                "br" => block(out),
                "a" => {
                    let href = safe_link(element.attr("href"), base);
                    let mut label = String::new();
                    for child in node.children() {
                        walk(child, &mut label, true, base, depth + 1);
                    }
                    let label = sanitize(&label).replace(['[', ']'], "");
                    if label.is_empty() {
                        return;
                    }
                    match href {
                        Some(url) => out.push_str(&format!("[{label}]({url})")),
                        None => out.push_str(&label),
                    }
                }
                "strong" | "b" => {
                    out.push_str("**");
                    for child in node.children() {
                        walk(child, out, true, base, depth + 1);
                    }
                    out.push_str("**");
                }
                "em" | "i" => {
                    out.push('*');
                    for child in node.children() {
                        walk(child, out, true, base, depth + 1);
                    }
                    out.push('*');
                }
                "code" => {
                    out.push('`');
                    for child in node.children() {
                        walk(child, out, true, base, depth + 1);
                    }
                    out.push('`');
                }
                "pre" => {
                    block(out);
                    let language = node
                        .children()
                        .find_map(|child| match child.value() {
                            Node::Element(inner) if inner.name() == "code" => inner.attr("class"),
                            _ => None,
                        })
                        .and_then(|class| {
                            class.split_whitespace().find_map(|part| {
                                part.strip_prefix("language-")
                                    .or_else(|| part.strip_prefix("lang-"))
                            })
                        })
                        .filter(|name| {
                            !name.is_empty()
                                && name
                                    .chars()
                                    .all(|character| character.is_ascii_alphanumeric())
                        })
                        .unwrap_or_default();
                    out.push_str("```");
                    out.push_str(language);
                    out.push('\n');
                    let mut text = String::new();
                    for child in node.children() {
                        walk(child, &mut text, false, base, depth + 1);
                    }
                    out.push_str(text.trim_end());
                    out.push_str("\n```");
                    block(out);
                }
                _ => {
                    for child in node.children() {
                        walk(child, out, true, base, depth + 1);
                    }
                }
            }
        }
        _ => {
            for child in node.children() {
                walk(child, out, markdown, base, depth + 1);
            }
        }
    }
}

fn is_block(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "main"
            | "aside"
            | "nav"
            | "li"
            | "ul"
            | "ol"
            | "table"
            | "tr"
            | "td"
            | "th"
            | "blockquote"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "pre"
            | "br"
            | "hr"
    )
}

fn block(out: &mut String) {
    while out.ends_with(' ') {
        out.pop();
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

pub(crate) const MIN_USEFUL_CHARS: usize = 200;
const MAX_HEADINGS: usize = 200;
const MAX_DOC_LINKS: usize = 200;
const MAX_JSON_LD_BLOCKS: usize = 16;
const MAX_JSON_LD_BYTES: usize = 65_536;
const MAX_META_CHARS: usize = 512;
const MAX_HEADING_CHARS: usize = 200;
const MAX_LINK_TEXT_CHARS: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtractionAttempt {
    pub parser: &'static str,
    pub characters: usize,
    pub used: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chained {
    pub title: Option<String>,
    pub body: String,
    pub parser: &'static str,
    pub attempts: Vec<ExtractionAttempt>,
}

pub(crate) fn chained(html: &str, base: &url::Url, markdown: bool) -> Chained {
    let mut attempts: Vec<ExtractionAttempt> = Vec::with_capacity(3);
    let mut parsed = Html::parse_document(html);
    prune_boilerplate(&mut parsed);
    let title = title_of(&parsed);
    let mut best: Option<(&'static str, String, usize)> = None;
    for (name, root) in [
        ("main", content_root(&parsed)),
        ("body", parsed.tree.root()),
    ] {
        let body = render(root, markdown, base);
        let characters = body.chars().count();
        attempts.push(ExtractionAttempt {
            parser: name,
            characters,
            used: false,
        });
        if characters >= MIN_USEFUL_CHARS {
            if let Some(last) = attempts.last_mut() {
                last.used = true;
            }
            return Chained {
                title,
                body,
                parser: name,
                attempts,
            };
        }
        if best
            .as_ref()
            .is_none_or(|(_, _, current)| characters > *current)
        {
            best = Some((name, body, characters));
        }
    }
    let raw = Html::parse_document(html);
    let body = render(raw.tree.root(), markdown, base);
    let characters = body.chars().count();
    attempts.push(ExtractionAttempt {
        parser: "raw",
        characters,
        used: false,
    });
    if characters >= MIN_USEFUL_CHARS
        || best
            .as_ref()
            .is_none_or(|(_, _, current)| characters > *current)
    {
        if let Some(last) = attempts.last_mut() {
            last.used = true;
        }
        return Chained {
            title,
            body,
            parser: "raw",
            attempts,
        };
    }
    let (parser, body) = match best {
        Some((name, body, characters)) if characters > 0 => (name, body),
        _ => ("none", String::new()),
    };
    if let Some(attempt) = attempts.iter_mut().find(|attempt| attempt.parser == parser) {
        attempt.used = true;
    }
    Chained {
        title,
        body,
        parser,
        attempts,
    }
}

fn render(root: NodeRef<'_, Node>, markdown: bool, base: &url::Url) -> String {
    let mut out = String::new();
    walk(root, &mut out, markdown, base, 0);
    tidy(&out)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Metadata {
    pub description: Option<String>,
    pub canonical: Option<String>,
    pub language: Option<String>,
    pub published_at: Option<String>,
    pub author: Option<String>,
    pub site_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Heading {
    pub level: u8,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DocLink {
    pub url: String,
    pub text: String,
}

pub(crate) fn metadata(html: &str, base: &url::Url) -> Metadata {
    let document = Html::parse_document(html);
    Metadata {
        description: meta_content(&document, "name", "description")
            .or_else(|| meta_content(&document, "property", "og:description")),
        canonical: attr_of(&document, "link[rel=canonical]", "href")
            .and_then(|href| base.join(href.trim()).ok())
            .filter(|target| matches!(target.scheme(), "http" | "https"))
            .map(|target| target.to_string()),
        language: attr_of(&document, "html", "lang"),
        published_at: meta_content(&document, "property", "article:published_time")
            .or_else(|| meta_content(&document, "name", "date")),
        author: meta_content(&document, "name", "author")
            .or_else(|| meta_content(&document, "property", "article:author")),
        site_name: meta_content(&document, "property", "og:site_name"),
    }
}

fn meta_content(document: &Html, attribute: &str, value: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[{attribute}=\"{value}\"]")).ok()?;
    document
        .select(&selector)
        .next()
        .and_then(|element| element.value().attr("content"))
        .map(|content| short_text(&sanitize(content), MAX_META_CHARS))
        .filter(|content| !content.is_empty())
}

fn attr_of(document: &Html, selector: &str, attribute: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    document
        .select(&selector)
        .next()
        .and_then(|element| element.value().attr(attribute))
        .map(|value| short_text(&sanitize(value), MAX_META_CHARS))
        .filter(|value| !value.is_empty())
}

fn short_text(value: &str, limit: usize) -> String {
    let (capped, _) = crate::sanitize::cap(value, limit);
    capped.trim().to_owned()
}

pub(crate) fn headings(html: &str) -> Vec<Heading> {
    let document = Html::parse_document(html);
    let Ok(selector) = Selector::parse("h1, h2, h3, h4, h5, h6") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for element in document.select(&selector) {
        if out.len() >= MAX_HEADINGS {
            break;
        }
        let text = short_text(
            &sanitize(&element.text().collect::<String>()),
            MAX_HEADING_CHARS,
        );
        if text.is_empty() {
            continue;
        }
        let level = match element.value().name() {
            "h1" => 1,
            "h2" => 2,
            "h3" => 3,
            "h4" => 4,
            "h5" => 5,
            _ => 6,
        };
        out.push(Heading { level, text });
    }
    out
}

pub(crate) fn doc_links(html: &str, base: &url::Url) -> Vec<DocLink> {
    let document = Html::parse_document(html);
    let Some(selector) = LINK_SELECTOR.as_ref() else {
        return Vec::new();
    };
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for element in document.select(selector) {
        if out.len() >= MAX_DOC_LINKS {
            break;
        }
        let Some(href) = element.value().attr("href") else {
            continue;
        };
        let Ok(target) = base.join(href.trim()) else {
            continue;
        };
        if !matches!(target.scheme(), "http" | "https") {
            continue;
        }
        let url = target.to_string();
        if !seen.insert(url.clone()) {
            continue;
        }
        let text = short_text(
            &sanitize(&element.text().collect::<String>()),
            MAX_LINK_TEXT_CHARS,
        );
        out.push(DocLink { url, text });
    }
    out
}

pub(crate) fn json_ld(html: &str) -> Vec<serde_json::Value> {
    let document = Html::parse_document(html);
    let Ok(selector) = Selector::parse("script[type=\"application/ld+json\"]") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut total = 0usize;
    for element in document.select(&selector) {
        if out.len() >= MAX_JSON_LD_BLOCKS {
            break;
        }
        let text = element.text().collect::<String>();
        total = total.saturating_add(text.len());
        if total > MAX_JSON_LD_BYTES {
            break;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim())
            && matches!(
                value,
                serde_json::Value::Object(_) | serde_json::Value::Array(_)
            )
        {
            out.push(value);
        }
    }
    out
}

fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0usize;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank = blank.saturating_add(1);
            if blank > 1 {
                continue;
            }
            out.push('\n');
            continue;
        }
        blank = 0;
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> url::Url {
        url::Url::parse("https://searx.example/").expect("base")
    }

    #[test]
    fn deeply_nested_markup_is_cut_at_the_depth_cap() {
        let mut html = String::from("<html><body><div>shallow</div>");
        for _ in 0..MAX_TREE_DEPTH + 200 {
            html.push_str("<div>");
        }
        html.push_str("beyond");
        for _ in 0..MAX_TREE_DEPTH + 200 {
            html.push_str("</div>");
        }
        html.push_str("</body></html>");
        let text = to_text(&html, &base());
        assert!(text.body.contains("shallow"));
        assert!(!text.body.contains("beyond"));
        let markdown = to_markdown(&html, &base());
        assert!(markdown.body.contains("shallow"));
        assert!(!markdown.body.contains("beyond"));
        let cleaned = clean_html(&html, &base());
        assert!(cleaned.contains("shallow"));
    }

    #[test]
    fn a_page_with_thousands_of_blocks_is_extracted_in_one_pass() {
        let mut html = String::from("<html><body>");
        for _ in 0..5_000 {
            html.push_str("<div><p>paragraph</p></div>");
        }
        html.push_str("<div><p>last block</p></div></body></html>");
        let extracted = to_markdown(&html, &base());
        assert!(extracted.body.contains("paragraph"));
    }

    #[test]
    fn the_chain_falls_back_to_the_body_when_the_main_root_is_empty() {
        let filler = "word ".repeat(80);
        let html = format!(
            r#"<html><body><main></main><div class="content"><p>{filler}</p></div></body></html>"#
        );
        let chained = chained(&html, &base(), true);
        assert_eq!(chained.parser, "body");
        assert!(chained.body.contains("word"));
        let attempts: Vec<(&str, bool)> = chained
            .attempts
            .iter()
            .map(|attempt| (attempt.parser, attempt.used))
            .collect();
        assert_eq!(attempts, vec![("main", false), ("body", true)]);
    }

    #[test]
    fn the_chain_reports_none_when_a_page_holds_no_text() {
        let html = "<html><body><script>let a = 1;</script></body></html>";
        let chained = chained(html, &base(), true);
        assert_eq!(chained.parser, "none");
        assert!(chained.body.is_empty());
        assert!(
            chained
                .attempts
                .iter()
                .any(|attempt| attempt.parser == "raw")
        );
    }

    #[test]
    fn a_short_article_still_uses_the_main_root() {
        let html = "<html><body><main><p>tiny page</p></main></body></html>";
        let chained = chained(html, &base(), true);
        assert_eq!(chained.parser, "main");
        assert!(chained.body.contains("tiny page"));
    }

    #[test]
    fn metadata_headings_links_and_json_ld_are_extracted() {
        let html = r#"<html lang="en"><head>
            <title>Doc title</title>
            <meta name="description" content="A page about comets.">
            <meta property="article:published_time" content="2026-10-01T10:00:00Z">
            <meta name="author" content="A. Writer">
            <link rel="canonical" href="/canonical">
            <script type="application/ld+json">{"@type":"Article","headline":"Comets"}</script>
            </head><body>
            <h1>Comets</h1><h2>Orbits</h2>
            <p>See <a href="https://a.example/x">the orbit page</a>.</p>
            <a href="javascript:alert(1)">bad</a>
            </body></html>"#;
        let meta = metadata(html, &base());
        assert_eq!(meta.description.as_deref(), Some("A page about comets."));
        assert_eq!(
            meta.canonical.as_deref(),
            Some("https://searx.example/canonical")
        );
        assert_eq!(meta.language.as_deref(), Some("en"));
        assert_eq!(meta.author.as_deref(), Some("A. Writer"));
        assert_eq!(meta.published_at.as_deref(), Some("2026-10-01T10:00:00Z"));
        let heads = headings(html);
        assert_eq!(
            heads
                .iter()
                .map(|heading| (heading.level, heading.text.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "Comets"), (2, "Orbits")]
        );
        let anchors = doc_links(html, &base());
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].url, "https://a.example/x");
        assert_eq!(anchors[0].text, "the orbit page");
        let blocks = json_ld(html);
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn headings_links_and_paragraphs_become_markdown() {
        let html = r#"<html><head><title>Doc</title></head><body>
            <h1>Main</h1>
            <p>Hello <a href="https://a.example/">world</a>.</p>
            <ul><li>one</li><li>two</li></ul>
        </body></html>"#;
        let extracted = to_markdown(html, &base());
        assert_eq!(extracted.title.as_deref(), Some("Doc"));
        assert!(extracted.body.contains("# Main"));
        assert!(extracted.body.contains("[world](https://a.example/)"));
        assert!(extracted.body.contains("- one"));
        assert!(extracted.body.contains("- two"));
    }

    #[test]
    fn scripts_and_styles_are_dropped() {
        let html =
            r#"<html><body><script>alert(1)</script><style>.a{}</style><p>keep</p></body></html>"#;
        let extracted = to_markdown(html, &base());
        assert!(extracted.body.contains("keep"));
        assert!(!extracted.body.contains("alert"));
        assert!(!extracted.body.contains(".a{"));
    }

    #[test]
    fn plain_text_keeps_lines_and_links() {
        let html = r#"<html><body><h2>Head</h2><p>Body <a href="https://b.example/">link</a></p></body></html>"#;
        let extracted = to_text(html, &base());
        assert!(extracted.body.contains("Head"));
        assert!(extracted.body.contains("link (https://b.example/)"));
    }

    #[test]
    fn a_hidden_character_is_removed() {
        let html = "<html><body><p>a\u{200B}b</p></body></html>";
        assert!(to_text(html, &base()).body.contains("ab"));
    }

    #[test]
    fn the_clean_html_serializer_drops_scripts_handlers_and_unsafe_links() {
        let html = r##"<html><head><title>T</title></head><body onload="steal()">
            <script>alert(1)</script>
            <p onclick="steal()">Keep <b>this</b></p>
            <a href="javascript:alert(1)">bad</a>
            <a href="https://ok.example/x" target="_blank">good</a>
            <iframe src="https://evil.example/"></iframe>
        </body></html>"##;
        let cleaned = clean_html(html, &base());
        assert!(cleaned.contains("Keep <b>this</b>"));
        assert!(cleaned.contains(
            "<a href=\"https://ok.example/x\" rel=\"nofollow noopener noreferrer\">good</a>"
        ));
        assert!(!cleaned.contains("script"));
        assert!(!cleaned.contains("onload"));
        assert!(!cleaned.contains("onclick"));
        assert!(!cleaned.contains("javascript:"));
        assert!(!cleaned.contains("iframe"));
        assert!(!cleaned.contains("target="));
    }

    #[test]
    fn an_unsafe_link_scheme_keeps_its_label_and_loses_the_target() {
        let html = r#"<p><a href="javascript:alert(1)">label</a></p>"#;
        let markdown = to_markdown(html, &base());
        assert!(markdown.body.contains("label"));
        assert!(!markdown.body.contains("javascript:"));
    }

    #[test]
    fn navigation_and_footer_text_are_pruned_from_the_markdown() {
        let html = r#"<html><body>
            <nav><a href="/">Home</a><a href="/docs">Docs</a></nav>
            <div class="sidebar-promo">Subscribe to the newsletter</div>
            <main><h1>Real article</h1><p>This is the content a reader wants.</p>
            <pre><code class="language-rust">fn main() {}</code></pre></main>
            <div id="cookie-consent">We use cookies</div>
            <footer>Copyright 2026</footer>
        </body></html>"#;
        let extracted = to_markdown(html, &base());
        assert!(extracted.body.contains("Real article"));
        assert!(extracted.body.contains("This is the content"));
        assert!(extracted.body.contains("```rust"));
        assert!(!extracted.body.contains("Home"));
        assert!(!extracted.body.contains("newsletter"));
        assert!(!extracted.body.contains("cookies"));
        assert!(!extracted.body.contains("Copyright"));
    }

    #[test]
    fn the_article_root_wins_over_a_link_list_next_to_it() {
        let html = r#"<html><body>
            <div class="links"><a href="https://a.example/">one</a><a href="https://b.example/">two</a></div>
            <article><p>Article body sentence with real substance.</p><p>Second paragraph.</p></article>
        </body></html>"#;
        let extracted = to_markdown(html, &base());
        assert!(extracted.body.contains("Article body"));
        assert!(!extracted.body.contains("one"));
    }
}
