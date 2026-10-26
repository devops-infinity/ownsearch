use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EventRequestPaused, FailRequestParams,
};
use chromiumoxide::cdp::browser_protocol::network::{ErrorReason, EventResponseReceived};
use chromiumoxide::{Browser, BrowserConfig};
use futures_util::StreamExt;
use tokio::sync::{Mutex, Semaphore};
use url::Url;

use crate::error::{Error, Result};

struct Session {
    browser: Browser,
    handler: tokio::task::JoinHandle<()>,
    created_at: Instant,
    last_used: Instant,
    misses: u32,
    profile: PathBuf,
}

struct RenderInner {
    gate: Semaphore,
    session: Mutex<Option<Session>>,
    executable: Option<String>,
    idle: Duration,
    no_sandbox: bool,
    allow_private: bool,
    watchdog: Duration,
    max_lifetime: Duration,
    stop: tokio_util::sync::CancellationToken,
    watchdog_started: std::sync::atomic::AtomicBool,
    profile: PathBuf,
}

#[derive(Clone)]
pub struct Renderer {
    inner: Option<Arc<RenderInner>>,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Renderer")
            .field("enabled", &self.enabled())
            .finish_non_exhaustive()
    }
}

impl Renderer {
    pub fn new(
        enabled: bool,
        executable: Option<String>,
        idle_secs: u64,
        no_sandbox: bool,
        allow_private: bool,
        watchdog_secs: u64,
        max_lifetime_secs: u64,
        profile_root: &Path,
    ) -> Self {
        if !enabled {
            return Self { inner: None };
        }
        Self {
            inner: Some(Arc::new(RenderInner {
                gate: Semaphore::new(1),
                session: Mutex::new(None),
                executable,
                idle: Duration::from_secs(idle_secs.max(30)),
                no_sandbox,
                allow_private,
                watchdog: Duration::from_secs(watchdog_secs.max(5)),
                max_lifetime: Duration::from_secs(max_lifetime_secs.max(60)),
                stop: tokio_util::sync::CancellationToken::new(),
                watchdog_started: std::sync::atomic::AtomicBool::new(false),
                profile: profile_root.join(format!("profile-{}", std::process::id())),
            })),
        }
    }

    pub fn enabled(&self) -> bool {
        self.inner.is_some()
    }

    pub fn unavailable() -> Self {
        Self { inner: None }
    }

    pub async fn page(&self, url: &Url, timeout: Duration) -> Result<(String, Option<String>)> {
        let inner = self.inner.as_ref().ok_or_else(|| {
            Error::PolicyRefused(
                "the browser is disabled; set OWNSEARCH_BROWSER=true to render JavaScript pages"
                    .into(),
            )
        })?;
        let _permit = inner
            .gate
            .acquire()
            .await
            .map_err(|_| Error::State("the browser gate closed".into()))?;
        let mut session = {
            let mut guard = inner.session.lock().await;
            if guard
                .as_ref()
                .is_some_and(|session| session.last_used.elapsed() > inner.idle)
            {
                shutdown(&mut guard).await;
            }
            guard.take()
        };
        if session.is_none() {
            session = Some(launch(inner).await?);
            start_watchdog(inner);
        }
        let outcome = tokio::time::timeout(timeout, async {
            let current = session
                .as_mut()
                .ok_or_else(|| Error::State("the browser session vanished".into()))?;
            let page = current
                .browser
                .new_page("about:blank")
                .await
                .map_err(browser_error)?;
            let guard = if inner.allow_private {
                None
            } else {
                Some(install_guard(&page).await?)
            };
            let rendered = async {
                page.goto(url.as_str()).await.map_err(browser_error)?;
                if !inner.allow_private {
                    ensure_public_final(&page).await?;
                }
                let html = page.content().await.map_err(browser_error)?;
                let markdown = chromiumoxide::content_markdown::content_markdown(&page)
                    .await
                    .ok()
                    .flatten();
                if let Some(guard) = guard.as_ref()
                    && guard.blocked_peer.load(std::sync::atomic::Ordering::SeqCst)
                {
                    return Err(Error::PolicyRefused(
                        "a page resource connected to a non-public address".into(),
                    ));
                }
                Ok::<_, Error>((html, markdown))
            }
            .await;
            drop(guard);
            let _ = page.close().await;
            rendered
        })
        .await;
        match outcome {
            Ok(Ok(value)) => {
                if inner.stop.is_cancelled() {
                    shutdown_owned(session).await;
                } else if let Some(mut current) = session {
                    current.last_used = Instant::now();
                    *inner.session.lock().await = Some(current);
                }
                Ok(value)
            }
            Ok(Err(error)) => {
                shutdown_owned(session).await;
                Err(error)
            }
            Err(_) => {
                shutdown_owned(session).await;
                Err(Error::Timeout {
                    deadline_ms: u64::try_from(timeout.as_millis()).unwrap_or(0),
                })
            }
        }
    }
    pub async fn shutdown(&self) {
        if let Some(inner) = &self.inner {
            inner.stop.cancel();
            let mut guard = inner.session.lock().await;
            shutdown(&mut guard).await;
        }
    }
}

struct Guard {
    requests: tokio::task::JoinHandle<()>,
    peers: tokio::task::JoinHandle<()>,
    blocked_peer: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.requests.abort();
        self.peers.abort();
    }
}

async fn install_guard(page: &chromiumoxide::Page) -> Result<Guard> {
    let mut paused = page
        .event_listener::<EventRequestPaused>()
        .await
        .map_err(browser_error)?;
    let mut responses = page
        .event_listener::<EventResponseReceived>()
        .await
        .map_err(browser_error)?;
    let target = page.clone();
    let requests = tokio::spawn(async move {
        while let Some(event) = paused.next().await {
            let settled = if request_permitted(&event.request.url).await {
                target
                    .execute(ContinueRequestParams::new(event.request_id.clone()))
                    .await
                    .map(|_| ())
            } else {
                target
                    .execute(FailRequestParams::new(
                        event.request_id.clone(),
                        ErrorReason::BlockedByClient,
                    ))
                    .await
                    .map(|_| ())
            };
            if let Err(error) = settled {
                tracing::debug!(%error, "a paused browser request was not resolved");
            }
        }
    });
    let blocked_peer = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flagged = Arc::clone(&blocked_peer);
    let peers = tokio::spawn(async move {
        while let Some(event) = responses.next().await {
            let Some(raw) = event.response.remote_ip_address.as_deref() else {
                continue;
            };
            let Ok(address) = raw.parse::<std::net::IpAddr>() else {
                continue;
            };
            if !crate::network_guard::is_public(address) {
                tracing::warn!(
                    %address,
                    url = %event.response.url,
                    "a rendered page resource connected to a non-public address"
                );
                flagged.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    });
    Ok(Guard {
        requests,
        peers,
        blocked_peer,
    })
}

async fn request_permitted(raw: &str) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    match url.scheme() {
        "http" | "https" => crate::network_guard::url_is_public(&url).await,
        "about" | "data" | "blob" => true,
        _ => false,
    }
}

async fn ensure_public_final(page: &chromiumoxide::Page) -> Result<()> {
    let landed = page.url().await.map_err(browser_error)?;
    let Some(landed) = landed else {
        return Ok(());
    };
    if request_permitted(&landed).await {
        return Ok(());
    }
    Err(Error::PolicyRefused(format!(
        "the rendered page landed on {landed}, which is not a public address"
    )))
}

fn start_watchdog(inner: &Arc<RenderInner>) {
    if inner
        .watchdog_started
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }
    let watch = Arc::clone(inner);
    tokio::spawn(async move {
        let sweep = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = watch.stop.cancelled() => break,
                    () = tokio::time::sleep(watch.watchdog) => {}
                }
                let mut guard = watch.session.lock().await;
                let Some(session) = guard.as_mut() else {
                    continue;
                };
                let idle_too_long = session.last_used.elapsed() > watch.idle;
                let lived_too_long = session.created_at.elapsed() > watch.max_lifetime;
                if idle_too_long || lived_too_long {
                    shutdown(&mut guard).await;
                    continue;
                }
                let healthy = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    session.browser.version(),
                )
                .await
                .is_ok_and(|answer| answer.is_ok());
                if healthy {
                    session.misses = 0;
                } else {
                    session.misses = session.misses.saturating_add(1);
                    if session.misses >= 2 {
                        tracing::warn!(
                            "the browser this process launched stopped answering; closing it so the next page starts a fresh one"
                        );
                        shutdown(&mut guard).await;
                    }
                }
            }
        });
        if let Err(error) = sweep.await {
            tracing::error!(%error, "the browser watchdog stopped unexpectedly");
        }
    });
}

async fn launch(inner: &Arc<RenderInner>) -> Result<Session> {
    create_private_dir(&inner.profile)?;
    let mut builder = BrowserConfig::builder()
        .user_data_dir(&inner.profile)
        .arg("--no-proxy-server");
    if !inner.allow_private {
        builder = builder.enable_request_intercept();
    }
    if let Some(path) = &inner.executable {
        builder = builder.chrome_executable(path);
    }
    if inner.no_sandbox {
        builder = builder.no_sandbox();
    }
    let config = builder
        .build()
        .map_err(|message| Error::Config(format!("browser configuration: {message}")))?;
    let (browser, handler) = match Browser::launch(config).await {
        Ok(launched) => launched,
        Err(error) => {
            discard_profile(&inner.profile);
            return Err(browser_error(error));
        }
    };
    let handler = tokio::spawn(async move {
        if let Err(error) = handler.run().await {
            tracing::warn!(%error, "the browser handler stopped");
        }
    });
    Ok(Session {
        browser,
        handler,
        created_at: Instant::now(),
        last_used: Instant::now(),
        misses: 0,
        profile: inner.profile.clone(),
    })
}

async fn shutdown_owned(session: Option<Session>) {
    let mut owned = session;
    shutdown(&mut owned).await;
}

async fn shutdown(session: &mut Option<Session>) {
    let Some(mut current) = session.take() else {
        return;
    };
    let close = Duration::from_secs(5);
    let _ = tokio::time::timeout(close, current.browser.close()).await;
    let _ = tokio::time::timeout(close, current.browser.wait()).await;
    let _ = tokio::time::timeout(close, current.browser.kill()).await;
    current.handler.abort();
    discard_profile(&current.profile);
}

fn create_private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .map_err(|error| Error::State(format!("cannot create {}: {error}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn discard_profile(path: &Path) {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, profile = %path.display(), "could not remove the browser profile")
        }
    }
}

pub(crate) fn reap_orphans(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            reap_profile(&path);
        }
    }
}

fn reap_profile(path: &Path) {
    let owner = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("profile-"))
        .and_then(|pid| pid.parse::<u32>().ok());
    if owner.is_none_or(|pid| pid == std::process::id() || owner_alive(pid)) {
        return;
    }
    close_browsers_using(path);
    if let Err(error) = std::fs::remove_dir_all(path) {
        tracing::warn!(%error, profile = %path.display(), "could not remove an abandoned browser profile");
    }
}

#[cfg(target_os = "linux")]
fn owner_alive(pid: u32) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .is_ok_and(|command| contains(&command, b"ownsearch"))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn owner_alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .is_ok_and(|output| output.status.success() && contains(&output.stdout, b"ownsearch"))
}

#[cfg(not(unix))]
fn owner_alive(_pid: u32) -> bool {
    true
}

#[cfg(target_os = "linux")]
fn close_browsers_using(profile: &Path) {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    let needle = profile.as_os_str().as_encoded_bytes();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let Ok(command) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        if contains(&command, needle) && contains(&command, b"chrom") {
            let _ = std::process::Command::new("kill")
                .arg("-9")
                .arg(pid.to_string())
                .status();
        }
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn close_browsers_using(profile: &Path) {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-axo", "pid=,command="])
        .output()
    else {
        return;
    };
    let needle = profile.as_os_str().as_encoded_bytes();
    for line in output.stdout.split(|byte| *byte == b'\n') {
        let Some(split) = line.iter().position(|byte| byte.is_ascii_whitespace()) else {
            continue;
        };
        let Ok(pid) = std::str::from_utf8(&line[..split])
            .unwrap_or_default()
            .trim()
            .parse::<u32>()
        else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let command = &line[split..];
        if contains(command, needle) && contains(command, b"chrom") {
            let _ = std::process::Command::new("kill")
                .arg("-9")
                .arg(pid.to_string())
                .status();
        }
    }
}

#[cfg(not(unix))]
fn close_browsers_using(_profile: &Path) {}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn browser_error(error: chromiumoxide::error::CdpError) -> Error {
    Error::Network {
        context: "rendering the page in the browser",
        source: Box::new(std::io::Error::other(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_abandoned_profile_is_removed_and_the_live_one_stays() {
        let root = std::env::temp_dir().join(format!("ownsearch-browser-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let live = root.join(format!("profile-{}", std::process::id()));
        let abandoned = root.join("profile-4294967294");
        std::fs::create_dir_all(&live).expect("live");
        std::fs::create_dir_all(&abandoned).expect("abandoned");
        reap_orphans(&root);
        assert!(live.is_dir());
        assert!(!abandoned.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_page_request_is_permitted_only_for_a_public_address() {
        assert!(request_permitted("https://1.1.1.1/").await);
        assert!(!request_permitted("https://169.254.169.254/latest/meta-data/").await);
        assert!(!request_permitted("http://127.0.0.1:8774/").await);
        assert!(request_permitted("about:blank").await);
        assert!(
            !request_permitted("file:///etc/passwd").await,
            "a file url must never be permitted"
        );
    }

    fn chrome_present() -> bool {
        [
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/bin/google-chrome-stable",
        ]
        .iter()
        .any(|path| std::path::Path::new(path).exists())
    }

    #[tokio::test]
    async fn a_private_page_is_blocked_when_private_addresses_are_refused() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        if !chrome_present() {
            return;
        }
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/page"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("<html><body>reachable</body></html>", "text/html"),
            )
            .mount(&server)
            .await;
        let root = std::env::temp_dir().join(format!("ownsearch-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let url = Url::parse(&format!("{}/page", server.uri())).expect("url");
        let refused = Renderer::new(true, None, 60, false, false, 30, 120, &root.join("refused"));
        let blocked = refused.page(&url, Duration::from_secs(30)).await;
        assert!(blocked.is_err(), "{blocked:?}");
        refused.shutdown().await;
        let allowed = Renderer::new(true, None, 60, false, true, 30, 120, &root.join("allowed"));
        let rendered = allowed.page(&url, Duration::from_secs(30)).await;
        assert!(
            rendered
                .as_ref()
                .is_ok_and(|(html, _)| html.contains("reachable")),
            "{rendered:?}"
        );
        allowed.shutdown().await;
        let _ = std::fs::remove_dir_all(&root);
    }
}
