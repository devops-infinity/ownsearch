use std::error::Error as StdError;

use serde_json::{Value, json};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("configuration: {0}")]
    Config(String),
    #[error("policy refused the request: {0}")]
    PolicyRefused(String),
    #[error("upstream returned {status}: {message}")]
    Upstream { status: u16, message: String },
    #[error("upstream rate limit: {message}")]
    RateLimited { message: String },
    #[error("the service is busy: {message}")]
    Busy { message: String },
    #[error("upstream answer could not be parsed as {format}")]
    ParserFailed {
        format: &'static str,
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
    #[error("network failure while {context}")]
    Network {
        context: &'static str,
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
    #[error("local state failure: {0}")]
    State(String),
    #[error("database failure while {context}")]
    Database {
        context: &'static str,
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
    #[error("local state is corrupt: {0}")]
    StateCorrupted(String),
    #[error("the database records schema {found}, newer than this binary reads ({supported})")]
    StateNewer { found: i64, supported: u32 },
    #[error("ledger: {0}")]
    Ledger(String),
    #[error("input or output failure while {context}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("json failure while {context}")]
    Json {
        context: &'static str,
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
    #[error("tool {name} is not available in this configuration")]
    UnknownTool { name: String },
    #[error("database writes are disabled in read-only mode")]
    WriteDisabled,
    #[error("the confirmation token is missing, expired, or already used")]
    ConfirmationExpired,
    #[error("request cancelled by the client")]
    Cancelled,
    #[error("the search deadline of {deadline_ms} ms passed")]
    Timeout { deadline_ms: u64 },
    #[error("the search found no usable instance")]
    NoFleet,
    #[error("update: {0}")]
    Update(String),
}

impl Error {
    pub fn network(context: &'static str, source: impl StdError + Send + Sync + 'static) -> Self {
        Self::Network {
            context,
            source: Box::new(source),
        }
    }

    pub fn parser(format: &'static str, source: impl StdError + Send + Sync + 'static) -> Self {
        Self::ParserFailed {
            format,
            source: Box::new(source),
        }
    }

    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidArgs(_) => "ownsearch.bad_request",
            Self::Config(_) => "ownsearch.config_error",
            Self::PolicyRefused(_) => "ownsearch.policy_refused",
            Self::Upstream { status, .. } => match status {
                400 | 422 => "ownsearch.bad_request",
                401 | 403 => "ownsearch.policy_refused",
                404 => "ownsearch.not_found",
                429 => "ownsearch.rate_limited",
                _ => "ownsearch.upstream_failed",
            },
            Self::RateLimited { .. } => "ownsearch.rate_limited",
            Self::Busy { .. } => "ownsearch.busy",
            Self::ParserFailed { .. } => "ownsearch.parser_failed",
            Self::Network { .. } => "ownsearch.upstream_failed",
            Self::State(_) | Self::Database { .. } => "ownsearch.state_error",
            Self::StateCorrupted(_) => "ownsearch.state_corrupted",
            Self::StateNewer { .. } => "ownsearch.state_newer",
            Self::Ledger(_) | Self::Io { .. } | Self::Json { .. } => "ownsearch.internal_error",
            Self::UnknownTool { .. } => "ownsearch.tool_not_found",
            Self::WriteDisabled => "ownsearch.write_disabled",
            Self::ConfirmationExpired => "ownsearch.confirmation_expired",
            Self::Cancelled => "ownsearch.cancelled",
            Self::Timeout { .. } => "ownsearch.timeout",
            Self::NoFleet => "ownsearch.no_fleet",
            Self::Update(_) => "ownsearch.update_failed",
        }
    }

    pub const fn remedy(&self) -> &'static str {
        match self {
            Self::InvalidArgs(_) => {
                "Fix the argument named in the message and call the tool again."
            }
            Self::Config(_) => {
                "Fix the setting named in the message; `ownsearch doctor` prints every resolved value."
            }
            Self::PolicyRefused(_) => {
                "Honor the operator policy named in the message; OwnSearch will not bypass it."
            }
            Self::Upstream { status, .. } => match status {
                400 | 422 => "Fix the request the instance named, then try again.",
                401 | 403 => {
                    "The instance refused this format or this request. Enable HTML fallback only if the operator policy allows it."
                }
                404 => "The instance does not serve this path; the scan will mark it and move on.",
                429 => {
                    "Stop for this instance. Wait for the operator's rate window before scanning again."
                }
                _ => {
                    "Retry later; if every instance fails, run `ownsearch scan` to refresh the fleet."
                }
            },
            Self::RateLimited { .. } => {
                "Wait for the instance rate window to pass; OwnSearch never retries a 429 in the same run."
            }
            Self::Busy { .. } => {
                "Retry shortly; the local state database or the search queue is saturated."
            }
            Self::ParserFailed { .. } => {
                "The answer was read but not understood. Scan again to refresh capability state, and report the instance."
            }
            Self::Network { .. } => "Check network reachability, then retry.",
            Self::State(_) | Self::Database { .. } => {
                "Run `ownsearch db verify` and restore the newest backup if it reports damage."
            }
            Self::StateCorrupted(_) => {
                "Restore the newest verified backup with `ownsearch db backup` output beside it, then scan again."
            }
            Self::StateNewer { .. } => {
                "This state database was written by a newer OwnSearch; run the newer release against it, or point OWNSEARCH_STATE_DIR at a different directory."
            }
            Self::Ledger(_) => {
                "Run `ownsearch audit verify` to see which row failed. Restore args.key when you have it, or archive the chain with `ownsearch audit rotate --force` and let the next call start a fresh one."
            }
            Self::Io { .. } => "Check the state and ledger directory permissions and free space.",
            Self::Json { .. } => "Retry; if it repeats, run `ownsearch db verify`.",
            Self::UnknownTool { .. } => {
                "Call tools/list and use only the names it returns for this configuration."
            }
            Self::WriteDisabled => {
                "Database writes are disabled by the operator. Ask the operator to set OWNSEARCH_AI_DB_MODE=read-write and restart, or run the change through the ownsearch CLI."
            }
            Self::ConfirmationExpired => {
                "Call the tool again with dry_run true to receive a fresh preview and token."
            }
            Self::Cancelled => "Call the tool again if the work is still needed.",
            Self::Timeout { .. } => {
                "Raise the deadline with timeout_secs (up to the transport limit), or use mode=fast to query fewer instances."
            }
            Self::NoFleet => {
                "Call the ownsearch_scan tool, or run `ownsearch scan`, to refresh the fleet. If every instance is cooling down, `ownsearch instances list` shows the status and an operator can clear it with `ownsearch scan --force --instance <id>`."
            }
            Self::Update(_) => {
                "Follow the command named in the message: the install channel owns this update. For the install script, rerun `ownsearch update`."
            }
        }
    }

    pub fn to_json(&self) -> Value {
        let message =
            crate::sanitize::cap(&crate::sanitize::sanitize(&self.public_message()), 500).0;
        let mut body = serde_json::Map::new();
        body.insert("code".to_owned(), json!(self.code()));
        body.insert("message".to_owned(), json!(message));
        body.insert("remedy".to_owned(), json!(self.remedy()));
        Value::Object(body)
    }

    fn public_message(&self) -> String {
        match self {
            Self::Database { context, .. } => format!("database failure while {context}"),
            Self::Io { context, .. } => format!("input or output failure while {context}"),
            Self::Json { context, .. } => format!("internal data failure while {context}"),
            Self::Ledger(_) => "the call ledger could not be read or written".to_owned(),
            Self::State(_) | Self::StateCorrupted(_) => {
                "the local state could not be used".to_owned()
            }
            other => redact_url_queries(&other.to_string()),
        }
    }

    pub fn is_internal(&self) -> bool {
        matches!(
            self,
            Self::State(_)
                | Self::Database { .. }
                | Self::StateCorrupted(_)
                | Self::Ledger(_)
                | Self::Io { .. }
                | Self::Json { .. }
        )
    }

    pub fn causes(&self) -> Vec<String> {
        let mut chain = Vec::new();
        let mut current: Option<&(dyn StdError + 'static)> = self.source();
        while let Some(error) = current {
            chain.push(redact_url_queries(&error.to_string()));
            current = error.source();
        }
        chain
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Self::Io {
            context: "file access",
            source,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(source: serde_json::Error) -> Self {
        Self::Json {
            context: "serializing protocol data",
            source: Box::new(source),
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(source: rusqlite::Error) -> Self {
        use rusqlite::ErrorCode;
        match source {
            rusqlite::Error::SqliteFailure(failure, _) => match failure.code {
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => Self::Busy {
                    message: "the state database is busy; retry the call".into(),
                },
                ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => {
                    Self::StateCorrupted("the state database failed its own checks".into())
                }
                ErrorCode::DiskFull => {
                    Self::State("the state database ran out of disk space".into())
                }
                ErrorCode::ConstraintViolation => {
                    Self::State("a uniqueness or reference rule refused the write".into())
                }
                ErrorCode::ReadOnly => Self::State("the state database is read-only".into()),
                _ => Self::Database {
                    context: "running a state query",
                    source: Box::new(rusqlite::Error::SqliteFailure(failure, None)),
                },
            },
            other => Self::Database {
                context: "running a state query",
                source: Box::new(other),
            },
        }
    }
}

impl From<url::ParseError> for Error {
    fn from(source: url::ParseError) -> Self {
        Self::InvalidArgs(format!("url: {source}"))
    }
}

fn redact_url_queries(text: &str) -> String {
    let mut redacted_text = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(query) = rest.find('?') {
        if !rest.get(..query).is_some_and(|head| head.contains("://")) {
            redacted_text.push_str(rest.get(..=query).unwrap_or_default());
            rest = rest.get(query.saturating_add(1)..).unwrap_or_default();
            continue;
        }
        redacted_text.push_str(rest.get(..query).unwrap_or_default());
        redacted_text.push_str("?<redacted>");
        let tail = rest.get(query.saturating_add(1)..).unwrap_or_default();
        let end = tail
            .find(|character: char| character.is_whitespace() || character == ')')
            .unwrap_or(tail.len());
        rest = tail.get(end..).unwrap_or_default();
    }
    redacted_text.push_str(rest);
    redacted_text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_text_redacts_a_url_query() {
        let text = redact_url_queries(
            "error sending request for url (https://searx.example/search?q=secret)",
        );
        assert!(text.contains("?<redacted>)"));
        assert!(!text.contains("secret"));
    }

    #[test]
    fn a_wrapped_error_keeps_its_cause_off_the_display_line() {
        let error = Error::network(
            "probing an instance",
            std::io::Error::other("connection reset"),
        );
        assert_eq!(
            error.to_string(),
            "network failure while probing an instance"
        );
        assert_eq!(error.causes(), vec!["connection reset".to_owned()]);
        assert_eq!(error.code(), "ownsearch.upstream_failed");
    }

    #[test]
    fn every_error_carries_a_code_and_a_remedy() {
        let errors = [
            Error::InvalidArgs("q".to_owned()),
            Error::Config("profile".to_owned()),
            Error::PolicyRefused("html".to_owned()),
            Error::RateLimited {
                message: "429".to_owned(),
            },
            Error::Busy {
                message: "queue".to_owned(),
            },
            Error::Timeout { deadline_ms: 12000 },
            Error::NoFleet,
        ];
        for error in errors {
            let body = error.to_json();
            assert!(body.get("code").and_then(Value::as_str).is_some());
            assert!(body.get("remedy").and_then(Value::as_str).is_some());
        }
    }

    #[test]
    fn a_busy_error_does_not_borrow_the_upstream_rate_limit_words() {
        let error = Error::Busy {
            message: "queue".to_owned(),
        };
        assert_eq!(error.code(), "ownsearch.busy");
        assert!(!error.remedy().contains("instance"));
    }

    #[test]
    fn a_403_is_reported_as_a_policy_refusal() {
        let error = Error::Upstream {
            status: 403,
            message: String::new(),
        };
        assert_eq!(error.code(), "ownsearch.policy_refused");
    }
}
