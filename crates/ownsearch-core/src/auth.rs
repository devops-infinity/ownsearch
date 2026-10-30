use secrecy::{ExposeSecret, SecretString};
use url::Url;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    Header,
    Basic,
}

impl AuthKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Header => "header",
            Self::Basic => "basic",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "header" => Some(Self::Header),
            "basic" => Some(Self::Basic),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRecord {
    pub kind: AuthKind,
    pub header_name: Option<String>,
    pub user_name: Option<String>,
    pub secret_env: String,
}

#[derive(Debug, Clone)]
pub struct InstanceAuth {
    record: AuthRecord,
    origin: String,
    secret: SecretString,
}

impl InstanceAuth {
    pub fn resolve(instance_url: &str, record: AuthRecord) -> Result<Self> {
        let name = record.secret_env.clone();
        let secret = std::env::var(&name)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                Error::Config(format!(
                    "the instance auth needs a value in the environment variable {name}; set it for the server and the CLI before this call"
                ))
            })?;
        Self::with_secret(instance_url, record, &secret)
    }

    pub fn with_secret(instance_url: &str, record: AuthRecord, secret: &str) -> Result<Self> {
        let parsed = Url::parse(instance_url)
            .map_err(|error| Error::Config(format!("instance url {instance_url:?}: {error}")))?;
        if secret.is_empty() {
            return Err(Error::InvalidArgs(
                "the instance auth secret is empty".into(),
            ));
        }
        Ok(Self {
            origin: origin_of(&parsed),
            record,
            secret: SecretString::from(secret.to_owned()),
        })
    }

    pub fn applies_to(&self, url: &Url) -> bool {
        origin_of(url) == self.origin
    }

    pub fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.record.kind {
            AuthKind::Header => {
                let name = self
                    .record
                    .header_name
                    .as_deref()
                    .unwrap_or("authorization");
                request.header(name, self.secret.expose_secret())
            }
            AuthKind::Basic => request.basic_auth(
                self.record.user_name.as_deref().unwrap_or("ownsearch"),
                Some(self.secret.expose_secret()),
            ),
        }
    }

    pub fn describe(&self) -> String {
        match self.record.kind {
            AuthKind::Header => format!(
                "header {} from {}",
                self.record
                    .header_name
                    .as_deref()
                    .unwrap_or("authorization"),
                self.record.secret_env
            ),
            AuthKind::Basic => format!(
                "basic user {} from {}",
                self.record.user_name.as_deref().unwrap_or("-"),
                self.record.secret_env
            ),
        }
    }

    pub fn kind(&self) -> AuthKind {
        self.record.kind
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn browser_header(&self) -> (String, String) {
        match self.record.kind {
            AuthKind::Header => (
                self.record
                    .header_name
                    .clone()
                    .unwrap_or_else(|| "authorization".to_owned()),
                self.secret.expose_secret().to_owned(),
            ),
            AuthKind::Basic => (
                "authorization".to_owned(),
                format!(
                    "Basic {}",
                    base64(
                        format!(
                            "{}:{}",
                            self.record.user_name.as_deref().unwrap_or("ownsearch"),
                            self.secret.expose_secret()
                        )
                        .as_bytes()
                    )
                ),
            ),
        }
    }
}

fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk.first().copied().unwrap_or(0);
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        let combined = (u32::from(first) << 16) | (u32::from(second) << 8) | u32::from(third);
        let indices = [
            (combined >> 18) & 0x3f,
            (combined >> 12) & 0x3f,
            (combined >> 6) & 0x3f,
            combined & 0x3f,
        ];
        for (position, index) in indices.iter().enumerate() {
            if position > chunk.len() {
                out.push('=');
            } else if let Some(character) = ALPHABET.get(*index as usize) {
                out.push(char::from(*character));
            }
        }
    }
    out
}

pub fn origin_of(url: &Url) -> String {
    let mut origin = format!(
        "{}://{}",
        url.scheme(),
        url.host_str().unwrap_or_default().to_ascii_lowercase()
    );
    if let Some(port) = url.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    origin
}

pub fn validate_header_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    let valid = !name.is_empty()
        && name.chars().count() <= 64
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    if valid {
        Ok(name.to_owned())
    } else {
        Err(Error::InvalidArgs(
            "a header name uses letters, digits, hyphen, and underscore only".into(),
        ))
    }
}

pub fn validate_env_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    let mut characters = name.chars();
    let first_ok = characters
        .next()
        .is_some_and(|character| character.is_ascii_uppercase() || character == '_');
    let rest_ok = name.chars().all(|character| {
        character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
    });
    if !name.is_empty() && name.chars().count() <= 128 && first_ok && rest_ok {
        Ok(name.to_owned())
    } else {
        Err(Error::InvalidArgs(
            "an environment variable name is upper case letters, digits, and underscore".into(),
        ))
    }
}

pub fn validate_user_name(raw: &str) -> Result<String> {
    let user = raw.trim();
    if user.is_empty()
        || user.chars().count() > 128
        || user
            .chars()
            .any(|character| character.is_control() || character == ':')
    {
        return Err(Error::InvalidArgs(
            "a basic user name is 1 to 128 characters and holds no colon".into(),
        ));
    }
    Ok(user.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: AuthKind) -> AuthRecord {
        AuthRecord {
            kind,
            header_name: Some("x-api-key".to_owned()),
            user_name: Some("alice".to_owned()),
            secret_env: "OWNSEARCH_TEST_SECRET".to_owned(),
        }
    }

    fn auth(url: &str, kind: AuthKind) -> InstanceAuth {
        InstanceAuth::with_secret(url, record(kind), "secret-value").expect("auth")
    }

    #[test]
    fn the_origin_matches_scheme_host_and_port() {
        let first = auth("https://searx.example/", AuthKind::Header);
        assert!(first.applies_to(&Url::parse("https://searx.example/search").expect("url")));
        assert!(first.applies_to(&Url::parse("https://SEARX.EXAMPLE/x").expect("url")));
        assert!(!first.applies_to(&Url::parse("http://searx.example/x").expect("url")));
        assert!(!first.applies_to(&Url::parse("https://searx.example:8443/x").expect("url")));
        assert!(!first.applies_to(&Url::parse("https://other.example/x").expect("url")));
        let ported = auth("https://searx.example:8443/", AuthKind::Basic);
        assert!(ported.applies_to(&Url::parse("https://searx.example:8443/x").expect("url")));
        assert!(!ported.applies_to(&Url::parse("https://searx.example/x").expect("url")));
    }

    #[test]
    fn a_missing_variable_names_itself_and_nothing_else() {
        let error = InstanceAuth::resolve("https://searx.example/", record(AuthKind::Header))
            .expect_err("missing");
        assert!(error.to_string().contains("OWNSEARCH_TEST_SECRET"));
    }

    #[test]
    fn the_description_never_holds_the_value() {
        let auth = auth("https://searx.example/", AuthKind::Basic);
        let described = auth.describe();
        assert!(described.contains("alice"));
        assert!(described.contains("OWNSEARCH_TEST_SECRET"));
        assert!(!described.contains("secret-value"));
    }

    #[test]
    fn base64_matches_the_standard_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
    }

    #[test]
    fn a_basic_browser_header_is_well_formed() {
        let auth = InstanceAuth::with_secret(
            "https://searx.example/",
            AuthRecord {
                kind: AuthKind::Basic,
                header_name: None,
                user_name: Some("user".to_owned()),
                secret_env: "OWNSEARCH_TEST_BASIC".to_owned(),
            },
            "pass",
        )
        .expect("auth");
        let (name, value) = auth.browser_header();
        assert_eq!(name, "authorization");
        assert_eq!(value, "Basic dXNlcjpwYXNz");
    }

    #[test]
    fn the_validators_refuse_a_bad_shape() {
        assert!(validate_header_name("x-api-key").is_ok());
        assert!(validate_header_name("bad header").is_err());
        assert!(validate_env_name("OWNSEARCH_KEY").is_ok());
        assert!(validate_env_name("lower_case").is_err());
        assert!(validate_user_name("alice").is_ok());
        assert!(validate_user_name("alice:bob").is_err());
    }
}
