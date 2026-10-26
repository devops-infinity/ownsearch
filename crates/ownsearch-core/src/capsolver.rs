//! Optional captcha solving through CapSolver. Runs only when a key is configured.

use std::time::{Duration, Instant};

use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use serde_json::Value;
use url::Url;

use crate::challenge::{Challenge, ChallengeKind};
use crate::error::{Error, Result};

const CREATE_TASK_URL: &str = "https://api.capsolver.com/createTask";
const GET_RESULT_URL: &str = "https://api.capsolver.com/getTaskResult";
const POLL_INTERVAL: Duration = Duration::from_secs(3);
const MAX_POLLS: u32 = 60;

#[derive(Debug)]
pub(crate) struct CapSolver {
    http: reqwest::Client,
    key: SecretString,
    deadline: Duration,
}

#[derive(Serialize)]
struct Task<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(rename = "websiteURL")]
    website_url: &'a str,
    #[serde(rename = "websiteKey")]
    website_key: &'a str,
    #[serde(rename = "pageAction", skip_serializing_if = "Option::is_none")]
    page_action: Option<&'a str>,
}

#[derive(Serialize)]
struct CreateBody<'a> {
    #[serde(rename = "clientKey")]
    client_key: &'a str,
    task: Task<'a>,
}

#[derive(Serialize)]
struct ResultBody<'a> {
    #[serde(rename = "clientKey")]
    client_key: &'a str,
    #[serde(rename = "taskId")]
    task_id: &'a str,
}

impl CapSolver {
    pub(crate) fn new(key: SecretString, timeout_secs: u64) -> Result<Self> {
        crate::client::install_crypto_provider();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("OwnSearch captcha client")
            .build()
            .map_err(|error| Error::network("building the captcha client", error))?;
        Ok(Self {
            http,
            key,
            deadline: Duration::from_secs(timeout_secs.max(10)),
        })
    }

    pub(crate) async fn solve(&self, page: &Url, challenge: &Challenge) -> Result<String> {
        let body = CreateBody {
            client_key: self.key.expose_secret(),
            task: Task {
                kind: challenge.kind.task_type(),
                website_url: page.as_str(),
                website_key: &challenge.sitekey,
                page_action: challenge.action.as_deref(),
            },
        };
        let answer = self.post(CREATE_TASK_URL, &body).await?;
        check_error(&answer)?;
        let task_id = answer
            .get("taskId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Error::Upstream {
                status: 200,
                message: "the captcha service returned no task id".into(),
            })?
            .to_owned();
        tracing::debug!(
            kind = challenge.kind.task_type(),
            task = %task_id,
            "the captcha task was created"
        );
        self.poll(&task_id).await
    }

    async fn poll(&self, task_id: &str) -> Result<String> {
        let started = Instant::now();
        for _ in 0..MAX_POLLS {
            let answer = self
                .post(
                    GET_RESULT_URL,
                    &ResultBody {
                        client_key: self.key.expose_secret(),
                        task_id,
                    },
                )
                .await?;
            check_error(&answer)?;
            if answer.get("status").and_then(Value::as_str) == Some("ready") {
                return solved_token(&answer);
            }
            if started.elapsed() >= self.deadline {
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        Err(Error::Timeout {
            deadline_ms: u64::try_from(self.deadline.as_millis()).unwrap_or(0),
        })
    }

    async fn post<B: Serialize>(&self, url: &str, body: &B) -> Result<Value> {
        let response = self
            .http
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|error| Error::network("calling the captcha service", error))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let body = body.replace(self.key.expose_secret(), "[redacted]");
            if let Ok(value) = serde_json::from_str::<Value>(&body)
                && value.get("errorId").and_then(Value::as_i64).unwrap_or(0) != 0
            {
                return Err(map_error(
                    value
                        .get("errorCode")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ));
            }
            let shown: String = crate::sanitize::sanitize(&body).chars().take(200).collect();
            return Err(Error::Upstream {
                status: status.as_u16(),
                message: format!("the captcha service refused the request: {shown}"),
            });
        }
        response
            .json::<Value>()
            .await
            .map_err(|error| Error::parser("captcha answer", error))
    }
}

fn check_error(answer: &Value) -> Result<()> {
    let id = answer.get("errorId").and_then(Value::as_i64).unwrap_or(0);
    if id == 0 {
        return Ok(());
    }
    let code = answer
        .get("errorCode")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Err(map_error(code))
}

fn map_error(code: &str) -> Error {
    match code {
        "ERROR_KEY_DENIED_ACCESS" | "ERROR_KEY_TEMP_BLOCKED" => {
            Error::Config("the captcha key was refused; check OWNSEARCH_CAPSOLVER_KEY".to_owned())
        }
        "ERROR_ZERO_BALANCE" | "ERROR_SETTLEMENT_FAILED" => {
            Error::PolicyRefused("the captcha service balance is empty".to_owned())
        }
        "ERROR_RATE_LIMIT" => Error::RateLimited {
            message: "the captcha service rate limited the request".to_owned(),
        },
        "ERROR_IP_BANNED" => {
            Error::PolicyRefused("the captcha service banned this address".to_owned())
        }
        "ERROR_TASK_TIMEOUT" => Error::Timeout { deadline_ms: 0 },
        "ERROR_CAPTCHA_UNSOLVABLE" => {
            Error::PolicyRefused("the captcha could not be solved".to_owned())
        }
        "ERROR_TASK_NOT_SUPPORTED" => {
            Error::PolicyRefused("the captcha type is not supported by the service".to_owned())
        }
        "ERROR_INVALID_TASK_DATA" => Error::PolicyRefused(
            "the site key, the page url, and the task type did not match; the service refused the task"
                .to_owned(),
        ),
        other => Error::PolicyRefused(format!("the captcha service refused the task: {other}")),
    }
}

fn solved_token(answer: &Value) -> Result<String> {
    answer
        .get("solution")
        .and_then(|solution| {
            solution
                .get("token")
                .or_else(|| solution.get("gRecaptchaResponse"))
        })
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Error::Upstream {
            status: 200,
            message: "the captcha service returned no token".into(),
        })
}

pub(crate) fn injection_script(kind: ChallengeKind, token: &str) -> String {
    let field = kind.response_field();
    let token = serde_json::to_string(token).unwrap_or_else(|_| "\"\"".to_owned());
    format!(
        "(()=>{{const f=document.querySelector('input[name={field}],textarea[name={field}]');\
if(!f){{return false;}}f.value={token};\
const form=f.closest('form');if(form){{form.submit();return true;}}return false;}})()"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(body: &str) -> Value {
        serde_json::from_str(body).expect("json")
    }

    #[test]
    fn a_clean_answer_carries_no_error() {
        assert!(check_error(&answer(r#"{"errorId":0,"taskId":"abc"}"#)).is_ok());
    }

    #[test]
    fn each_error_code_maps_to_its_own_message() {
        let cases = [
            ("ERROR_KEY_DENIED_ACCESS", "captcha key"),
            ("ERROR_KEY_TEMP_BLOCKED", "captcha key"),
            ("ERROR_ZERO_BALANCE", "balance"),
            ("ERROR_RATE_LIMIT", "rate limited"),
            ("ERROR_IP_BANNED", "banned"),
            ("ERROR_TASK_TIMEOUT", "deadline"),
            ("ERROR_CAPTCHA_UNSOLVABLE", "could not be solved"),
            ("ERROR_TASK_NOT_SUPPORTED", "not supported"),
            ("ERROR_INVALID_TASK_DATA", "did not match"),
        ];
        for (code, needle) in cases {
            let body = format!(r#"{{"errorId":12,"errorCode":"{code}"}}"#);
            let error = check_error(&answer(&body)).expect_err(code);
            assert!(
                error.to_string().contains(needle),
                "{code} should mention {needle}, got {error}"
            );
        }
    }

    #[test]
    fn a_ready_answer_yields_the_token_in_either_shape() {
        assert_eq!(
            solved_token(&answer(r#"{"solution":{"token":"t1"}}"#)).expect("token"),
            "t1"
        );
        assert_eq!(
            solved_token(&answer(r#"{"solution":{"gRecaptchaResponse":"t2"}}"#)).expect("token"),
            "t2"
        );
        assert!(solved_token(&answer(r#"{"solution":{}}"#)).is_err());
        assert!(solved_token(&answer(r#"{"solution":{"token":""}}"#)).is_err());
    }

    #[test]
    fn the_injection_script_names_the_field_and_escapes_the_token() {
        let script = injection_script(ChallengeKind::Turnstile, "a\"b");
        assert!(script.contains("cf-turnstile-response"));
        assert!(script.contains(r#""a\"b""#));
        assert!(injection_script(ChallengeKind::HCaptcha, "t").contains("h-captcha-response"));
        assert!(injection_script(ChallengeKind::RecaptchaV2, "t").contains("g-recaptcha-response"));
    }
}
