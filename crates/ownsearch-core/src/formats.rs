use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::FleetClient;
use crate::error::{Error, Result};

pub(crate) async fn fetch(
    client: &FleetClient,
    base: &Url,
    query: &crate::searxng::SearchQuery,
    format: &str,
    cancellation: &CancellationToken,
    auth: Option<&crate::auth::InstanceAuth>,
) -> Result<Vec<Entry>> {
    let params = [("q", query.query.clone()), ("format", format.to_owned())];
    let url = crate::client::search_url(base, &params)?;
    let fetched = client.get(&url, cancellation, auth).await?;
    if !fetched.is_success() {
        return Err(Error::Upstream {
            status: fetched.status,
            message: format!("the instance returned HTTP {}", fetched.status),
        });
    }
    parse(format, &fetched.text())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub engine: Option<String>,
}

pub(crate) fn parse(format: &str, body: &str) -> Result<Vec<Entry>> {
    match format {
        "rss" => parse_rss(body),
        "csv" => parse_csv(body),
        other => Err(Error::InvalidArgs(format!(
            "unknown search format: {other}"
        ))),
    }
}

pub(crate) fn parse_rss(body: &str) -> Result<Vec<Entry>> {
    if !body.contains("<item") {
        if body.contains("<rss") || body.contains("<feed") {
            return Ok(Vec::new());
        }
        return Err(Error::parser(
            "searxng rss",
            std::io::Error::other("the answer is not an RSS document"),
        ));
    }
    let mut entries = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("<item") {
        let after = &rest[start..];
        let Some(end) = after.find("</item>") else {
            break;
        };
        let block = &after[..end];
        rest = &after[end + "</item>".len()..];
        let title = tag(block, "title").unwrap_or_default();
        let url = tag(block, "link").unwrap_or_default();
        if title.is_empty() || url.is_empty() {
            continue;
        }
        entries.push(Entry {
            title,
            url,
            snippet: tag(block, "description").unwrap_or_default(),
            engine: None,
        });
    }
    Ok(entries)
}

pub(crate) fn parse_csv(body: &str) -> Result<Vec<Entry>> {
    let mut lines = body.lines().filter(|line| !line.trim().is_empty());
    let Some(header) = lines.next() else {
        return Err(Error::parser(
            "searxng csv",
            std::io::Error::other("the answer had no content"),
        ));
    };
    let names: Vec<String> = split_csv(header)
        .into_iter()
        .map(|name| name.trim().to_ascii_lowercase())
        .collect();
    let column = |wanted: &str| names.iter().position(|name| name == wanted);
    let (Some(title_at), Some(url_at)) = (column("title"), column("url")) else {
        return Err(Error::parser(
            "searxng csv",
            std::io::Error::other("the header has no title and url columns"),
        ));
    };
    let snippet_at = column("content").or_else(|| column("snippet"));
    let engine_at = column("engine");
    let mut entries = Vec::new();
    for line in lines {
        let fields = split_csv(line);
        let field = |at: Option<usize>| {
            at.and_then(|index| fields.get(index))
                .map(|value| value.trim().to_owned())
                .unwrap_or_default()
        };
        let title = field(Some(title_at));
        let url = field(Some(url_at));
        if title.is_empty() || url.is_empty() {
            continue;
        }
        let engine = field(engine_at);
        entries.push(Entry {
            title,
            url,
            snippet: field(snippet_at),
            engine: (!engine.is_empty()).then_some(engine),
        });
    }
    Ok(entries)
}

fn tag(block: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)? + start;
    Some(unescape(block[start..end].trim()))
}

fn unescape(raw: &str) -> String {
    raw.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn split_csv(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in line.chars() {
        match character {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                fields.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    fields.push(current);
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_rss_document_yields_its_items() {
        let body = r#"<rss version="2.0"><channel>
<item><title>Rust &amp; Cargo</title><link>https://a.example/x</link><description>one &lt;b&gt;</description></item>
<item><title>Second</title><link>https://b.example/y</link><description>two</description></item>
</channel></rss>"#;
        let entries = parse_rss(body).expect("rss");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "Rust & Cargo");
        assert_eq!(entries[0].url, "https://a.example/x");
        assert_eq!(entries[0].snippet, "one <b>");
        assert_eq!(entries[1].title, "Second");
    }

    #[test]
    fn an_empty_rss_channel_is_not_an_error() {
        assert!(
            parse_rss("<rss><channel></channel></rss>")
                .expect("rss")
                .is_empty()
        );
        assert!(parse_rss("<html>nope</html>").is_err());
    }

    #[test]
    fn csv_maps_by_header_name_in_any_order() {
        let first = "title,url,content,engine\nA,https://a.example,\"snip, with comma\",google\n";
        let second = "url,title,content,engine\nhttps://a.example,A,\"snip, with comma\",google\n";
        for body in [first, second] {
            let entries = parse_csv(body).expect("csv");
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].title, "A");
            assert_eq!(entries[0].url, "https://a.example");
            assert_eq!(entries[0].snippet, "snip, with comma");
            assert_eq!(entries[0].engine.as_deref(), Some("google"));
        }
    }

    #[test]
    fn a_header_without_title_or_url_is_an_error() {
        assert!(parse_csv("host,engine\na.example,google\n").is_err());
        assert!(parse_csv("").is_err());
    }

    #[test]
    fn the_dispatcher_refuses_an_unknown_format() {
        assert!(parse("toml", "x").is_err());
    }
}
