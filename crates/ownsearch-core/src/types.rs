use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Preset {
    Minimal,
    #[default]
    Standard,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum TextMode {
    #[default]
    Summary,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SearchMode {
    Fast,
    #[default]
    Balanced,
    Exhaustive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum RenderMode {
    #[default]
    Auto,
    Always,
    Never,
}

impl RenderMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum FetchFormat {
    #[default]
    Markdown,
    Text,
    Html,
}

impl FetchFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Text => "text",
            Self::Html => "html",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SafeSearch {
    Off,
    #[default]
    Moderate,
    Strict,
}

impl SafeSearch {
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Moderate => 1,
            Self::Strict => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum TimeRange {
    Day,
    Week,
    Month,
    Year,
}

impl TimeRange {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum InstanceStatus {
    Candidate,
    Active,
    Probation,
    Missing,
    Quarantined,
    Retired,
}

impl InstanceStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Active => "active",
            Self::Probation => "probation",
            Self::Missing => "missing",
            Self::Quarantined => "quarantined",
            Self::Retired => "retired",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "candidate" => Some(Self::Candidate),
            "active" => Some(Self::Active),
            "probation" => Some(Self::Probation),
            "missing" => Some(Self::Missing),
            "quarantined" => Some(Self::Quarantined),
            "retired" => Some(Self::Retired),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum InstanceType {
    #[default]
    Normal,
    Tor,
    Unknown,
}

impl InstanceType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Tor => "tor",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "normal" => Self::Normal,
            "tor" => Self::Tor,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ProbeKind {
    Tls,
    Config,
    JsonSearch,
    HtmlSearch,
    Google,
}

impl ProbeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tls => "tls",
            Self::Config => "config",
            Self::JsonSearch => "json_search",
            Self::HtmlSearch => "html_search",
            Self::Google => "google",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "tls" => Some(Self::Tls),
            "config" => Some(Self::Config),
            "json_search" => Some(Self::JsonSearch),
            "html_search" => Some(Self::HtmlSearch),
            "google" => Some(Self::Google),
            _ => None,
        }
    }

    pub const ALL: [Self; 5] = [
        Self::Tls,
        Self::Config,
        Self::JsonSearch,
        Self::HtmlSearch,
        Self::Google,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ProbeOutcome {
    Success,
    HttpError,
    ParserError,
    Timeout,
    Refused,
    RateLimited,
    Blocked,
}

impl ProbeOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::HttpError => "http_error",
            Self::ParserError => "parser_error",
            Self::Timeout => "timeout",
            Self::Refused => "refused",
            Self::RateLimited => "rate_limited",
            Self::Blocked => "blocked",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "success" => Some(Self::Success),
            "http_error" => Some(Self::HttpError),
            "parser_error" => Some(Self::ParserError),
            "timeout" => Some(Self::Timeout),
            "refused" => Some(Self::Refused),
            "rate_limited" => Some(Self::RateLimited),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }

    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum AiDbMode {
    ReadOnly,
    #[default]
    ReadWrite,
}

impl AiDbMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "read-only" | "readonly" => Some(Self::ReadOnly),
            "read-write" | "readwrite" => Some(Self::ReadWrite),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::ReadWrite => "read-write",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CrudEntity {
    Instance,
    PolicyOverride,
}

impl CrudEntity {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "instance" => Some(Self::Instance),
            "policy_override" | "policy-override" => Some(Self::PolicyOverride),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Instance => "instance",
            Self::PolicyOverride => "policy_override",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum CrudOperation {
    Create,
    Update,
    Delete,
    Retire,
    Restore,
}

impl CrudOperation {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            "retire" => Some(Self::Retire),
            "restore" => Some(Self::Restore),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Retire => "retire",
            Self::Restore => "restore",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum QueryEntity {
    Instances,
    Quality,
    Overrides,
    Capabilities,
    Events,
    Snapshots,
    Runs,
}

impl QueryEntity {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "instances" => Some(Self::Instances),
            "quality" => Some(Self::Quality),
            "overrides" | "policy_overrides" | "policy-overrides" => Some(Self::Overrides),
            "capabilities" => Some(Self::Capabilities),
            "events" => Some(Self::Events),
            "snapshots" => Some(Self::Snapshots),
            "runs" | "discovery_runs" | "discovery-runs" => Some(Self::Runs),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Instances => "instances",
            Self::Quality => "quality",
            Self::Overrides => "overrides",
            Self::Capabilities => "capabilities",
            Self::Events => "events",
            Self::Snapshots => "snapshots",
            Self::Runs => "runs",
        }
    }
}

pub(crate) fn instance_slug(raw: &str) -> String {
    let host = url::Url::parse(raw)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| raw.to_owned());
    let mut slug = String::with_capacity(host.len());
    let mut last_dash = false;
    for character in host.to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "instance".to_owned()
    } else {
        slug
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]

pub(crate) struct NormalizedUrl(String);

impl TryFrom<&str> for NormalizedUrl {
    type Error = Error;

    fn try_from(raw: &str) -> Result<Self, Self::Error> {
        normalize_url(raw).map(Self)
    }
}

impl From<NormalizedUrl> for String {
    fn from(value: NormalizedUrl) -> Self {
        value.0
    }
}

pub(crate) fn normalize_url(raw: &str) -> Result<String, Error> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidArgs("url is empty".into()));
    }
    if trimmed.chars().count() > 2048 {
        return Err(Error::InvalidArgs(
            "url is longer than 2048 characters".into(),
        ));
    }
    let parsed = url::Url::parse(trimmed)
        .map_err(|error| Error::InvalidArgs(format!("url does not parse: {error}")))?;
    match parsed.scheme() {
        "https" | "http" => {}
        other => {
            return Err(Error::InvalidArgs(format!(
                "url scheme {other} is not supported; use http or https"
            )));
        }
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| Error::InvalidArgs("url has no host".into()))?;
    if host.is_empty() {
        return Err(Error::InvalidArgs("url has an empty host".into()));
    }
    let scheme = if parsed.scheme() == "http" {
        "http"
    } else {
        "https"
    };
    let mut normalized = format!("{scheme}://{}", host.to_ascii_lowercase());
    if let Some(port) = parsed.port()
        && !(scheme == "https" && port == 443)
        && !(scheme == "http" && port == 80)
    {
        normalized.push(':');
        normalized.push_str(&port.to_string());
    }
    let path = parsed.path();
    if path.is_empty() || path == "/" {
        normalized.push('/');
    } else {
        normalized.push_str(path);
    }
    if let Some(query) = parsed.query() {
        normalized.push('?');
        normalized.push_str(query);
    }
    Ok(normalized)
}

pub(crate) fn stable_id(normalized: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(normalized.as_bytes());
    hex::encode(digest.get(..16).unwrap_or(digest.as_slice()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_normalization_lowercases_the_host_and_drops_the_fragment() {
        let normalized = normalize_url("HTTPS://Example.COM:443/Path?q=1#frag").expect("url");
        assert_eq!(normalized, "https://example.com/Path?q=1");
    }

    #[test]
    fn url_normalization_keeps_a_non_default_port() {
        let normalized = normalize_url("https://example.com:8443/search").expect("url");
        assert_eq!(normalized, "https://example.com:8443/search");
    }

    #[test]
    fn url_normalization_adds_a_root_path() {
        assert_eq!(
            normalize_url("https://searx.example").expect("url"),
            "https://searx.example/"
        );
    }

    #[test]
    fn url_normalization_rejects_other_schemes() {
        assert!(normalize_url("ftp://example.com").is_err());
        assert!(normalize_url("  ").is_err());
    }

    #[test]
    fn status_names_round_trip() {
        for status in [
            InstanceStatus::Candidate,
            InstanceStatus::Active,
            InstanceStatus::Probation,
            InstanceStatus::Missing,
            InstanceStatus::Quarantined,
            InstanceStatus::Retired,
        ] {
            assert_eq!(InstanceStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(InstanceStatus::parse("bogus"), None);
    }

    #[test]
    fn probe_kinds_round_trip() {
        for kind in ProbeKind::ALL {
            assert_eq!(ProbeKind::parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn ai_mode_accepts_both_spellings() {
        assert_eq!(AiDbMode::parse("READ-WRITE"), Some(AiDbMode::ReadWrite));
        assert_eq!(AiDbMode::parse("readonly"), Some(AiDbMode::ReadOnly));
        assert_eq!(AiDbMode::parse("write"), None);
    }

    #[test]
    fn a_host_turns_into_a_lowercase_dashed_slug() {
        assert_eq!(
            instance_slug("https://SearX.Example.ORG/"),
            "searx-example-org"
        );
        assert_eq!(
            instance_slug("https://sub.domain.co.uk/path"),
            "sub-domain-co-uk"
        );
        assert_eq!(instance_slug("https://127.0.0.1:8899/"), "127-0-0-1");
        assert_eq!(instance_slug("not a url!!"), "not-a-url");
    }

    #[test]
    fn every_enum_serializes_to_its_declared_string() {
        fn check<T>(value: T, expected: &str)
        where
            T: serde::Serialize + Copy + std::fmt::Debug,
        {
            let json = serde_json::to_value(value).expect("serialize");
            assert_eq!(json.as_str(), Some(expected), "{value:?}");
        }
        for (value, expected) in [
            (InstanceStatus::Candidate, "candidate"),
            (InstanceStatus::Active, "active"),
            (InstanceStatus::Probation, "probation"),
            (InstanceStatus::Missing, "missing"),
            (InstanceStatus::Quarantined, "quarantined"),
            (InstanceStatus::Retired, "retired"),
        ] {
            check(value, expected);
            assert_eq!(value.as_str(), expected);
        }
        for (value, expected) in [
            (InstanceType::Normal, "normal"),
            (InstanceType::Tor, "tor"),
            (InstanceType::Unknown, "unknown"),
        ] {
            check(value, expected);
            assert_eq!(value.as_str(), expected);
        }
        for (value, expected) in [
            (SearchMode::Fast, "fast"),
            (SearchMode::Balanced, "balanced"),
            (SearchMode::Exhaustive, "exhaustive"),
        ] {
            check(value, expected);
        }
        for (value, expected) in [
            (SafeSearch::Off, "off"),
            (SafeSearch::Moderate, "moderate"),
            (SafeSearch::Strict, "strict"),
        ] {
            check(value, expected);
        }
        check(AiDbMode::ReadOnly, "read-only");
        check(AiDbMode::ReadWrite, "read-write");
        assert_eq!(AiDbMode::ReadOnly.as_str(), "read-only");
        check(FetchFormat::Markdown, "markdown");
        check(RenderMode::Auto, "auto");
        check(RenderMode::Always, "always");
        check(RenderMode::Never, "never");
    }
}
