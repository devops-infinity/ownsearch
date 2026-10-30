use std::path::PathBuf;

use crate::config::Settings;

const MAX_SAMPLE_BYTES: usize = 8 * 1024;
const MIN_TOKEN_RUN: usize = 32;

pub(crate) fn save(
    settings: &Settings,
    instance_id: i64,
    version: Option<&str>,
    body: &[u8],
) -> Option<PathBuf> {
    if body.is_empty() {
        return None;
    }
    let directory = settings.state_dir().join("parser-samples");
    if let Err(error) = crate::fsutil::create_private_dir(&directory) {
        tracing::warn!(%error, "the parser sample directory could not be created");
        return None;
    }
    let head = body.get(..body.len().min(MAX_SAMPLE_BYTES)).unwrap_or(body);
    let text = String::from_utf8_lossy(head);
    let mut name = format!("{instance_id}-{}", crate::time::now_unix());
    if let Some(version) = version {
        let label = version_label(version);
        if !label.is_empty() {
            name.push('-');
            name.push_str(&label);
        }
    }
    name.push_str(".html");
    let path = directory.join(name);
    if let Err(error) = crate::fsutil::write_atomic(&path, redact(&text).as_bytes()) {
        tracing::warn!(%error, "the parser sample could not be written");
        return None;
    }
    tracing::warn!(instance_id, path = %path.display(), "a parser failure sample was saved");
    Some(path)
}

fn version_label(version: &str) -> String {
    version
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '+')
        })
        .take(32)
        .collect()
}

fn redact(text: &str) -> String {
    let cleaned = crate::sanitize::sanitize(text);
    let mut out = String::with_capacity(cleaned.len());
    let mut run = String::new();
    for character in cleaned.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
            run.push(character);
            continue;
        }
        flush_run(&mut out, &mut run);
        out.push(character);
    }
    flush_run(&mut out, &mut run);
    out
}

fn flush_run(out: &mut String, run: &mut String) {
    if run.len() >= MIN_TOKEN_RUN {
        out.push_str("[redacted]");
    } else {
        out.push_str(run);
    }
    run.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_token_run_is_replaced() {
        let token = "a".repeat(40);
        assert_eq!(redact(&format!("<p>{token}</p>")), "<p>[redacted]</p>");
    }

    #[test]
    fn ordinary_markup_survives() {
        let markup = "<article class=\"result\"><h3>Title</h3></article>";
        assert_eq!(redact(markup), markup);
    }

    #[test]
    fn a_version_label_keeps_only_safe_characters() {
        assert_eq!(version_label("2026.10.27+g1a2b3c"), "2026.10.27+g1a2b3c");
        assert_eq!(version_label("x/../../etc/passwd"), "x....etcpasswd");
        assert_eq!(version_label(&"9".repeat(60)).len(), 32);
    }
}
