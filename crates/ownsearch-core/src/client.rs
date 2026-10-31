use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, LOCATION, USER_AGENT};
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
    tls: rustls::ClientConfig,
) -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(connect_timeout)
        .redirect(Policy::none())
        .pool_max_idle_per_host(8)
        .pool_idle_timeout(Duration::from_secs(30))
        .user_agent(user_agent)
        .https_only(false)
        .tls_backend_preconfigured(tls);
    match proxy {
        Some(proxy) => builder.proxy(proxy),
        None => builder.no_proxy(),
    }
}

pub(crate) fn tls_client_config() -> Result<rustls::ClientConfig> {
    install_crypto_provider();
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let native = rustls_native_certs::load_native_certs();
    let _ = roots.add_parsable_certificates(native.certs);
    if roots.is_empty() {
        return Err(Error::Config(
            "no TLS root certificates could be loaded; install ca-certificates or replace the binary".into(),
        ));
    }
    let mut config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
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
    pub retry_after_secs: Option<u64>,
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

#[derive(Clone, Debug, Default)]
struct UserAgentRotation {
    cursor: Arc<AtomicUsize>,
}

impl UserAgentRotation {
    fn pick(&self) -> &'static str {
        let pool = crate::config::BROWSER_USER_AGENTS;
        let Some(fallback) = pool.first().copied() else {
            return "";
        };
        let mut byte = [0u8; 1];
        let index = if getrandom::fill(&mut byte).is_ok() {
            usize::from(byte[0])
        } else {
            self.cursor.fetch_add(1, Ordering::Relaxed)
        };
        pool.get(index % pool.len()).copied().unwrap_or(fallback)
    }
}

const PROXY_FAILURE_LIMIT: u32 = 3;
const PROXY_COOLDOWN: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Default)]
struct ProxySlot {
    failures: u32,
    cooldown_until: Option<Instant>,
}

impl ProxySlot {
    fn usable_at(&self, now: Instant) -> bool {
        self.cooldown_until.is_none_or(|until| until <= now)
    }
}

#[derive(Clone, Debug)]
pub struct FleetClient {
    http: reqwest::Client,
    pinned: Arc<Mutex<HashMap<String, reqwest::Client>>>,
    user_agent: String,
    user_agents: UserAgentRotation,
    accept: String,
    max_bytes: usize,
    timeout: Duration,
    connect_timeout: Duration,
    proxy_clients: Arc<Vec<reqwest::Client>>,
    proxy_state: Arc<Mutex<Vec<ProxySlot>>>,
    proxy_cursor: Arc<AtomicUsize>,
    allow_private: bool,
    allow_plain_http: bool,
    tls: rustls::ClientConfig,
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
        let user_agent = settings.user_agent();
        let tls = tls_client_config()?;
        let http = configure(timeout, connect_timeout, &user_agent, None, tls.clone())
            .build()
            .map_err(|error| Error::network("building the HTTP client", error))?;
        let mut proxy_clients: Vec<reqwest::Client> = Vec::new();
        for raw in settings.proxies() {
            let proxy = reqwest::Proxy::all(raw.as_str()).map_err(|error| {
                Error::Config(format!(
                    "an OWNSEARCH_PROXIES entry does not parse: {error}"
                ))
            })?;
            let client = configure(
                timeout,
                connect_timeout,
                &user_agent,
                Some(proxy),
                tls.clone(),
            )
            .build()
            .map_err(|error| Error::network("building a proxy client", error))?;
            proxy_clients.push(client);
        }
        let proxy_slots = vec![ProxySlot::default(); proxy_clients.len()];
        Ok(Self {
            http,
            pinned: Arc::new(Mutex::new(HashMap::new())),
            user_agent,
            user_agents: UserAgentRotation::default(),
            accept: settings.accept().to_owned(),
            max_bytes: settings.max_upstream_bytes(),
            timeout,
            connect_timeout,
            proxy_clients: Arc::new(proxy_clients),
            proxy_state: Arc::new(Mutex::new(proxy_slots)),
            proxy_cursor: Arc::new(AtomicUsize::new(0)),
            allow_private: settings.allow_private_instances(),
            allow_plain_http: settings.allow_plain_http_private(),
            tls,
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
            None,
            self.tls.clone(),
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

    fn pick_proxy_client(&self) -> (reqwest::Client, Option<usize>) {
        let total = self.proxy_clients.len();
        let start = self.proxy_cursor.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        let mut fallback: Option<(reqwest::Client, usize)> = None;
        for step in 0..total {
            let index = (start.saturating_add(step)) % total;
            let Some(client) = self.proxy_clients.get(index).cloned() else {
                continue;
            };
            let usable = self
                .proxy_state
                .lock()
                .ok()
                .and_then(|state| state.get(index).map(|slot| slot.usable_at(now)))
                .unwrap_or(true);
            if usable {
                return (client, Some(index));
            }
            if fallback.is_none() {
                fallback = Some((client, index));
            }
        }
        match fallback {
            Some((client, index)) => (client, Some(index)),
            None => (self.http.clone(), None),
        }
    }

    fn note_proxy_result(&self, slot: Option<usize>, ok: bool) {
        let Some(index) = slot else {
            return;
        };
        let Ok(mut state) = self.proxy_state.lock() else {
            return;
        };
        let Some(entry) = state.get_mut(index) else {
            return;
        };
        if ok {
            entry.failures = 0;
            entry.cooldown_until = None;
            return;
        }
        entry.failures = entry.failures.saturating_add(1);
        if entry.failures >= PROXY_FAILURE_LIMIT {
            entry.cooldown_until = Some(Instant::now() + PROXY_COOLDOWN);
            entry.failures = 0;
        }
    }

    pub async fn get(
        &self,
        url: &Url,
        cancellation: &CancellationToken,
        auth: Option<&crate::auth::InstanceAuth>,
    ) -> Result<Fetched> {
        let started = Instant::now();
        let mut current = url.clone();
        let mut hops = 0u8;
        let mut redirects: Vec<String> = Vec::new();
        loop {
            let resolved = crate::network_guard::resolved_public_address(&current).await;
            if resolved.is_none() && !self.allow_private {
                return Err(Error::PolicyRefused(
                    "the url resolves to a non-public address".into(),
                ));
            }
            let (client, proxy_slot) = if !self.proxy_clients.is_empty() && resolved.is_some() {
                self.pick_proxy_client()
            } else {
                let client = match (resolved, current.host()) {
                    (Some(address), Some(url::Host::Domain(host))) => {
                        self.pinned_client(host, address)?
                    }
                    _ => self.http.clone(),
                };
                (client, None)
            };
            if let Some(reason) = crate::transport::plain_http_refusal(
                &current,
                self.allow_private,
                self.allow_plain_http,
            ) {
                return Err(Error::PolicyRefused(reason));
            }
            let mut request = client
                .get(current.clone())
                .timeout(self.timeout)
                .header(USER_AGENT, self.user_agents.pick())
                .header(ACCEPT, self.accept.clone())
                .header(ACCEPT_LANGUAGE, "en-US,en;q=0.8");
            if let Some(auth) = auth
                && auth.applies_to(&current)
            {
                request = auth.apply(request);
            }
            let response = tokio::select! {
                result = request.send() => result,
                () = cancellation.cancelled() => return Err(Error::Cancelled),
            };
            let response = match response {
                Ok(response) => {
                    self.note_proxy_result(proxy_slot, true);
                    response
                }
                Err(error) if error.is_timeout() => {
                    self.note_proxy_result(proxy_slot, false);
                    return Err(Error::Timeout {
                        deadline_ms: u64::try_from(self.timeout.as_millis()).unwrap_or(0),
                    });
                }
                Err(error) if error.is_connect() => {
                    self.note_proxy_result(proxy_slot, false);
                    return Err(Error::network(
                        "opening a connection to the instance",
                        error,
                    ));
                }
                Err(error) => {
                    self.note_proxy_result(proxy_slot, false);
                    return Err(Error::network("reading an instance answer", error));
                }
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
            let retry_after_secs = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok());
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
                retry_after_secs: retry_after_secs.filter(|secs| *secs <= 604_800),
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
    fn every_pooled_user_agent_reads_as_a_desktop_browser() {
        let pool = crate::config::BROWSER_USER_AGENTS;
        assert!(pool.len() >= 4);
        for agent in pool {
            assert!(agent.starts_with("Mozilla/5.0 ("), "{agent}");
            assert!(agent.contains("AppleWebKit/537.36") || agent.contains("AppleWebKit/605.1.15"));
            assert!(!agent.contains("OwnSearch"), "{agent}");
        }
    }

    #[test]
    fn proxy_rotation_cycles_and_benches_a_failing_entry() {
        let settings = crate::config::Settings::load_with(
            crate::config::Overrides {
                state_dir: Some(std::path::PathBuf::from("/tmp/ownsearch-proxy-test")),
                ..crate::config::Overrides::default()
            },
            |name| match name {
                "HOME" => Some("/home/tester".to_owned()),
                "OWNSEARCH_PROXIES" => {
                    Some("http://127.0.0.1:8001, http://127.0.0.1:8002 ,,".to_owned())
                }
                _ => None,
            },
        )
        .expect("settings");
        assert_eq!(settings.proxies().len(), 2, "empty entries are dropped");
        let client = FleetClient::new(&settings).expect("client");
        let first = client.pick_proxy_client().1.expect("a proxy slot");
        let second = client.pick_proxy_client().1.expect("a proxy slot");
        assert_ne!(first, second, "the rotation must move on");
        for _ in 0..PROXY_FAILURE_LIMIT {
            client.note_proxy_result(Some(first), false);
        }
        for _ in 0..4 {
            assert_ne!(
                client.pick_proxy_client().1,
                Some(first),
                "a benched proxy must leave the rotation"
            );
        }
        client.note_proxy_result(Some(first), true);
        assert!(client.proxy_state.lock().expect("state")[first].usable_at(Instant::now()));
    }

    #[test]
    fn a_pick_always_returns_a_pooled_user_agent() {
        let rotation = UserAgentRotation::default();
        let pool = crate::config::BROWSER_USER_AGENTS;
        for _ in 0..64 {
            assert!(pool.contains(&rotation.pick()));
        }
    }

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
            retry_after_secs: None,
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
            retry_after_secs: None,
        };
        assert!(fetched.is_pdf());
    }

    #[test]
    fn the_shared_tls_config_builds_a_reqwest_client() {
        let tls = tls_client_config().expect("tls config");
        let built = configure(
            Duration::from_secs(5),
            Duration::from_secs(5),
            "ownsearch-test",
            None,
            tls,
        )
        .build();
        assert!(built.is_ok());
    }
}
