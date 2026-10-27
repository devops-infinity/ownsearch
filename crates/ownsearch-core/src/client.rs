use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, LOCATION};
use reqwest::redirect::Policy;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::config::Settings;
use crate::error::{Error, Result};

const MAX_REDIRECTS: u8 = 3;
const PINNED_CLIENTS: usize = 256;

fn configure(
    timeout: Duration,
    connect_timeout: Duration,
    user_agent: &str,
    proxy: Option<reqwest::Proxy>,
) -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(connect_timeout)
        .redirect(Policy::none())
        .pool_max_idle_per_host(8)
        .pool_idle_timeout(Duration::from_secs(30))
        .user_agent(user_agent)
        .https_only(false);
    match proxy {
        Some(proxy) => builder.proxy(proxy),
        None => builder.no_proxy(),
    }
}

#[derive(Debug, Clone)]
pub struct Fetched {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub elapsed_ms: u64,
    pub truncated: bool,
    pub final_url: String,
    pub redirects: Vec<String>,
}

impl Fetched {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn is_json(&self) -> bool {
        self.content_type
            .as_deref()
            .map(|value| value.to_ascii_lowercase().contains("json"))
            .unwrap_or(false)
    }

    pub fn is_pdf(&self) -> bool {
        self.content_type
            .as_deref()
            .map(|value| value.to_ascii_lowercase().contains("application/pdf"))
            .unwrap_or_else(|| self.body.starts_with(b"%PDF"))
    }

    pub fn is_html(&self) -> bool {
        self.content_type
            .as_deref()
            .map(|value| value.to_ascii_lowercase().contains("html"))
            .unwrap_or_else(|| {
                let head = self.body.get(..4096).unwrap_or(&self.body);
                let head = String::from_utf8_lossy(head).to_ascii_lowercase();
                let head = head.trim_start();
                head.starts_with("<!doctype") || head.starts_with("<html")
            })
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

#[derive(Clone, Debug)]
pub struct FleetClient {
    http: reqwest::Client,
    pinned: Arc<Mutex<HashMap<String, reqwest::Client>>>,
    user_agent: String,
    max_bytes: usize,
    timeout: Duration,
    connect_timeout: Duration,
    proxy: Option<reqwest::Proxy>,
    allow_private: bool,
}

impl FleetClient {
    pub fn new(settings: &Settings) -> Result<Self> {
        Self::build(settings, settings.request_timeout_secs())
    }

    pub fn with_probe_timeout(settings: &Settings) -> Result<Self> {
        Self::build(settings, settings.probe_timeout_secs())
    }

    fn build(settings: &Settings, timeout_secs: u64) -> Result<Self> {
        install_crypto_provider();
        let timeout = Duration::from_secs(timeout_secs);
        let connect_timeout = Duration::from_secs(settings.connect_timeout_secs());
        let proxy = match settings.proxy() {
            Some(proxy) => Some(reqwest::Proxy::all(proxy).map_err(|error| {
                Error::Config(format!("OWNSEARCH_PROXY does not parse: {error}"))
            })?),
            None => None,
        };
        let user_agent = settings.user_agent();
        let http = configure(timeout, connect_timeout, &user_agent, proxy.clone())
            .build()
            .map_err(|error| Error::network("building the HTTP client", error))?;
        Ok(Self {
            http,
            pinned: Arc::new(Mutex::new(HashMap::new())),
            user_agent,
            max_bytes: settings.max_upstream_bytes(),
            timeout,
            connect_timeout,
            proxy,
            allow_private: settings.allow_private_instances(),
        })
    }

    fn pinned_client(&self, host: &str, address: std::net::SocketAddr) -> Result<reqwest::Client> {
        if let Ok(cache) = self.pinned.lock()
            && let Some(client) = cache.get(host)
        {
            return Ok(client.clone());
        }
        let client = configure(
            self.timeout,
            self.connect_timeout,
            &self.user_agent,
            self.proxy.clone(),
        )
        .resolve(host, address)
        .build()
        .map_err(|error| Error::network("building a pinned HTTP client", error))?;
        if let Ok(mut cache) = self.pinned.lock() {
            if cache.len() >= PINNED_CLIENTS {
                cache.clear();
            }
            cache.insert(host.to_owned(), client.clone());
        }
        Ok(client)
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    pub async fn get(
        &self,
        url: &Url,
        accept: &str,
        cancellation: &CancellationToken,
    ) -> Result<Fetched> {
        let started = Instant::now();
        let mut current = url.clone();
        let mut hops = 0u8;
        let mut redirects: Vec<String> = Vec::new();
        loop {
            let client = if self.allow_private {
                self.http.clone()
            } else {
                let Some(address) = crate::network_guard::resolved_public_address(&current).await
                else {
                    return Err(Error::PolicyRefused(
                        "the url resolves to a non-public address".into(),
                    ));
                };
                match current.host() {
                    Some(url::Host::Domain(host)) => self.pinned_client(host, address)?,
                    _ => self.http.clone(),
                }
            };
            if current.scheme() == "http" && !host_is_loopback(&current) {
                return Err(Error::PolicyRefused(
                    "plain http is only allowed for a loopback test server".into(),
                ));
            }
            let request = client
                .get(current.clone())
                .timeout(self.timeout)
                .header(ACCEPT, accept)
                .header(ACCEPT_LANGUAGE, "en-US,en;q=0.8");
            let response = tokio::select! {
                result = request.send() => result,
                () = cancellation.cancelled() => return Err(Error::Cancelled),
            };
            let response = match response {
                Ok(response) => response,
                Err(error) if error.is_timeout() => {
                    return Err(Error::Timeout {
                        deadline_ms: u64::try_from(self.timeout.as_millis()).unwrap_or(0),
                    });
                }
                Err(error) if error.is_connect() => {
                    return Err(Error::network(
                        "opening a connection to the instance",
                        error,
                    ));
                }
                Err(error) => return Err(Error::network("reading an instance answer", error)),
            };
            if !self.allow_private
                && let Some(address) = response.remote_addr()
                && !crate::network_guard::is_public(address.ip())
            {
                return Err(Error::PolicyRefused(format!(
                    "the connection landed on {}, a non-public address",
                    address.ip()
                )));
            }
            let status = response.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let Some(location) = location else {
                    return Err(Error::Upstream {
                        status,
                        message: "the answer asked for a redirect without a Location header".into(),
                    });
                };
                if hops >= MAX_REDIRECTS {
                    return Err(Error::Upstream {
                        status,
                        message: format!("more than {MAX_REDIRECTS} redirects in a row"),
                    });
                }
                hops = hops.saturating_add(1);
                redirects.push(current.to_string());
                current = current.join(&location).map_err(|error| Error::Upstream {
                    status,
                    message: format!("the redirect target does not parse: {error}"),
                })?;
                if !matches!(current.scheme(), "https" | "http") {
                    return Err(Error::PolicyRefused(
                        "a redirect pointed at a scheme other than http or https".into(),
                    ));
                }
                continue;
            }
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let (body, truncated) = read_capped(response, self.max_bytes, cancellation).await?;
            return Ok(Fetched {
                status,
                content_type,
                body,
                elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                truncated,
                final_url: current.to_string(),
                redirects,
            });
        }
    }
}

async fn read_capped(
    mut response: reqwest::Response,
    max_bytes: usize,
    cancellation: &CancellationToken,
) -> Result<(Vec<u8>, bool)> {
    let mut body: Vec<u8> = Vec::new();
    loop {
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk,
            () = cancellation.cancelled() => return Err(Error::Cancelled),
        };
        match chunk {
            Ok(Some(bytes)) => {
                if body.len().saturating_add(bytes.len()) > max_bytes {
                    let room = max_bytes.saturating_sub(body.len());
                    if let Some(slice) = bytes.get(..room) {
                        body.extend_from_slice(slice);
                    }
                    return Ok((body, true));
                }
                body.extend_from_slice(&bytes);
            }
            Ok(None) => return Ok((body, false)),
            Err(error) if error.is_timeout() => {
                return Err(Error::Timeout { deadline_ms: 0 });
            }
            Err(error) => return Err(Error::network("reading an instance body", error)),
        }
    }
}

fn host_is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub(crate) fn search_url(base: &Url, params: &[(&str, String)]) -> Result<Url> {
    let mut url = base
        .join("search")
        .map_err(|error| Error::InvalidArgs(format!("cannot build the search URL: {error}")))?;
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in params {
            pairs.append_pair(key, value);
        }
    }
    Ok(url)
}

pub(crate) fn config_url(base: &Url) -> Result<Url> {
    base.join("config")
        .map_err(|error| Error::InvalidArgs(format!("cannot build the config URL: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_url_carries_every_parameter_once() {
        let base = Url::parse("https://searx.example/").expect("url");
        let url = search_url(
            &base,
            &[("q", "rust mcp".to_owned()), ("format", "json".to_owned())],
        )
        .expect("search url");
        assert_eq!(
            url.as_str(),
            "https://searx.example/search?q=rust+mcp&format=json"
        );
    }

    #[test]
    fn a_json_content_type_is_recognized() {
        let fetched = Fetched {
            status: 200,
            content_type: Some("application/json; charset=utf-8".to_owned()),
            body: Vec::new(),
            elapsed_ms: 1,
            truncated: false,
            final_url: "https://searx.example/".to_owned(),
            redirects: Vec::new(),
        };
        assert!(fetched.is_json());
        assert!(!fetched.is_html());
    }

    #[test]
    fn a_pdf_content_type_is_recognized() {
        let fetched = Fetched {
            status: 200,
            content_type: Some("application/pdf".to_owned()),
            body: Vec::new(),
            elapsed_ms: 1,
            truncated: false,
            final_url: "https://searx.example/paper.pdf".to_owned(),
            redirects: Vec::new(),
        };
        assert!(fetched.is_pdf());
    }

    #[test]
    fn an_html_body_without_a_content_type_is_still_html() {
        let fetched = Fetched {
            status: 200,
            content_type: None,
            body: b"<!DOCTYPE html><html></html>".to_vec(),
            elapsed_ms: 1,
            truncated: false,
            final_url: "https://searx.example/".to_owned(),
            redirects: Vec::new(),
        };
        assert!(fetched.is_html());
    }
}
