use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Request, State};
use axum::http::{
    HeaderMap, HeaderValue, StatusCode,
    header::{AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER},
};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::serve::{Listener, ListenerExt};
use axum::{Router, middleware};
use rmcp::transport::StreamableHttpServerConfig;
use rmcp::transport::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use secrecy::ExposeSecret;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tower_http::timeout::RequestBodyDeadlineLayer;
use tracing::Instrument;

use super::{Metrics, OwnsearchHandler, shutdown_signal};
use crate::config::Settings;
use crate::error::{Error, Result};

const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_REQUEST_ID: usize = 128;
const MIN_TOKEN_CHARS: usize = 32;
const HEADER_READ_SECS: u64 = 10;
const MAX_CONNECTIONS: usize = 256;
const MAX_CONNECTIONS_PER_IP: usize = 16;
const MAX_CONCURRENT_REQUESTS: usize = 128;
const BODY_DEADLINE_SECS: u64 = 30;
const REST_TIMEOUT_SECS: u64 = 120;
const SHUTDOWN_GRACE_SECS: u64 = 10;
const FAILED_AUTH_WINDOW_SECS: u64 = 60;
const FAILED_AUTH_LIMIT: u32 = 5;
const LOCKOUT_SECS: u64 = 30;
const MAX_FAILED_AUTH_ENTRIES: usize = 1024;

type FailedAuth = Arc<std::sync::Mutex<HashMap<std::net::IpAddr, (u32, Instant, Instant)>>>;

#[derive(Clone)]
struct HttpState {
    tokens: Arc<Vec<secrecy::SecretString>>,
    metrics: Arc<Metrics>,
    gate: Arc<Semaphore>,
    request_seq: Arc<AtomicU64>,
    allowed_hosts: Arc<Vec<String>>,
    allowed_origins: Arc<Vec<String>>,
    failed: FailedAuth,
    context: Arc<crate::context::ToolContext>,
    ready: Arc<std::sync::atomic::AtomicBool>,
    cancel: tokio_util::sync::CancellationToken,
}

pub async fn serve_http(settings: Settings) -> Result<()> {
    if let Some(error) = settings.bearer_token_error() {
        return Err(Error::Config(format!(
            "the stored bearer token could not be read: {error}; run `ownsearch config set-bearer-token` to store it again"
        )));
    }
    let tokens = settings.bearer_tokens().to_vec();
    let tokens_configured = !tokens.is_empty();
    let bind = settings.bind().to_owned();
    if !tokens_configured && !is_loopback(&bind) {
        return Err(Error::Config(
            "OWNSEARCH_BEARER_TOKENS is required when bind is not a loopback address".into(),
        ));
    }
    validate_tokens(&tokens)?;
    let allowed_hosts = allowed_hosts(&bind, settings.allowed_hosts());
    let allowed_origins = settings
        .allowed_origins()
        .map(<[String]>::to_vec)
        .unwrap_or_default();
    let host_policy = settings.allowed_hosts().is_none();
    let mcp_handler = OwnsearchHandler::new(settings)?;
    let ready = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let context = mcp_handler.context();
    {
        let bootstrap = Arc::clone(&context);
        tokio::task::spawn_blocking(move || bootstrap.ensure_bootstrap())
            .await
            .map_err(|error| Error::Io {
                context: "waiting for a storage task",
                source: std::io::Error::other(error),
            })??;
    }
    let shutdown_browser = Arc::clone(&context);
    let cancel = mcp_handler.cancellation_token();
    crate::browser::reap_orphans(&context.settings.state_dir().join("browser"));
    let _hang_watchdog = super::start_hang_watchdog(&context, cancel.clone());
    let metrics = mcp_handler.metrics();
    super::spawn_cleanup(mcp_handler.context(), cancel.clone());
    super::spawn_scanner(Arc::clone(&context), cancel.clone());
    let mut config = StreamableHttpServerConfig::default();
    config.json_response = true;
    config.legacy_session_mode = false;
    config = config.enforce_origin_validation();
    config.max_request_body_bytes = MAX_BODY_BYTES;
    config.allowed_hosts = allowed_hosts.clone();
    let service = StreamableHttpService::new(
        move || Ok(mcp_handler.clone()),
        Arc::new(NeverSessionManager::default()),
        config,
    );
    let state = HttpState {
        tokens: Arc::new(tokens),
        metrics,
        gate: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
        request_seq: Arc::new(AtomicU64::new(0)),
        allowed_hosts: Arc::new(allowed_hosts),
        allowed_origins: Arc::new(allowed_origins),
        failed: Arc::new(std::sync::Mutex::new(HashMap::new())),
        context,
        ready: Arc::clone(&ready),
        cancel: cancel.clone(),
    };
    let rest = Router::new()
        .route("/v1/search", post(rest_search))
        .route("/v1/fetch", post(rest_fetch))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REST_TIMEOUT_SECS),
        ));
    let preflight_routes = Router::new()
        .route("/v1/search", axum::routing::options(preflight))
        .route("/v1/fetch", axum::routing::options(preflight))
        .layer(middleware::from_fn_with_state(state.clone(), reject_origin));
    let protected = Router::new()
        .route("/startupz", get(startupz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics_handler))
        .route_service("/", service.clone())
        .route_service("/mcp", service)
        .merge(rest)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            record_http_metrics,
        ))
        .layer(
            tower_http::trace::TraceLayer::new_for_http()
                .make_span_with(|request: &Request<axum::body::Body>| {
                    tracing::info_span!(
                        "http_request",
                        method = %request.method(),
                        path = %request.uri().path()
                    )
                })
                .on_response(
                    |response: &Response, latency: Duration, _span: &tracing::Span| {
                        tracing::info!(
                            status = %response.status(),
                            latency_ms = u64::try_from(latency.as_millis()).unwrap_or(u64::MAX),
                            "request finished"
                        );
                    },
                ),
        )
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), gate_requests))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), require_host))
        .layer(middleware::from_fn_with_state(state.clone(), reject_origin))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            assign_request_id,
        ))
        .layer(RequestBodyDeadlineLayer::new(Duration::from_secs(
            BODY_DEADLINE_SECS,
        )))
        .with_state(state);
    let app = Router::new()
        .route("/healthz", get(healthz))
        .merge(preflight_routes)
        .merge(protected)
        .method_not_allowed_fallback(method_not_allowed)
        .fallback(not_found)
        .layer(middleware::from_fn(problem_fallback))
        .layer(middleware::from_fn(security_headers));
    let listener = TcpListener::bind(&bind).await.map_err(|error| Error::Io {
        context: "binding the HTTP listener",
        source: error,
    })?;
    if !tokens_configured
        && !listener
            .local_addr()
            .map(|address| address.ip().is_loopback())
            .unwrap_or(false)
    {
        return Err(Error::Config(format!(
            "{bind} resolved to a non-loopback address; set OWNSEARCH_BEARER_TOKENS before serving beyond this machine"
        )));
    }
    if !tokens_configured {
        tracing::warn!(
            bind = %bind,
            "serving HTTP without a bearer token; anything that can reach this port, including a tunnel or proxy, can read the fleet state"
        );
    }
    if !is_loopback(&bind) && host_policy {
        tracing::warn!(
            bind = %bind,
            "this bind is not a loopback address and OWNSEARCH_ALLOWED_HOSTS is unset, so a client that reaches this server by its LAN name or IP will be refused; set OWNSEARCH_ALLOWED_HOSTS to the names clients use"
        );
    }
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let listener = LimitedListener {
        inner: listener,
        connections: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        per_ip: Arc::new(std::sync::Mutex::new(HashMap::new())),
    }
    .tap_io(|_| {});
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = stop_rx.changed().await;
    });
    let mut handle = tokio::spawn(async move { server.await });
    tokio::select! {
        result = &mut handle => http_result(result),
        () = shutdown_signal() => {
            let _ = stop_tx.send(true);
            ready.store(false, Ordering::SeqCst);
            cancel.cancel();
            let drained =
                tokio::time::timeout(Duration::from_secs(SHUTDOWN_GRACE_SECS), &mut handle).await;
            shutdown_browser.heartbeat.stop();
            let _ = tokio::time::timeout(
                Duration::from_secs(SHUTDOWN_GRACE_SECS),
                shutdown_browser.browser.shutdown(),
            )
            .await;
            match drained {
                Ok(result) => http_result(result),
                Err(_) => {
                    tracing::warn!(
                        grace_secs = SHUTDOWN_GRACE_SECS,
                        "shutdown grace period elapsed with requests still open"
                    );
                    handle.abort();
                    let _ = handle.await;
                    Ok(())
                }
            }
        }
    }
}

fn http_result(
    result: std::result::Result<std::io::Result<()>, tokio::task::JoinError>,
) -> Result<()> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(Error::Io {
            context: "serving HTTP",
            source: error,
        }),
        Err(error) => Err(Error::Io {
            context: "waiting for the HTTP server task",
            source: std::io::Error::other(error),
        }),
    }
}

struct LimitedListener {
    inner: TcpListener,
    connections: Arc<Semaphore>,
    per_ip: Arc<std::sync::Mutex<HashMap<std::net::IpAddr, usize>>>,
}

struct LimitedStream {
    inner: TcpStream,
    _permit: OwnedSemaphorePermit,
    client_ip: std::net::IpAddr,
    tracked: bool,
    per_ip: Arc<std::sync::Mutex<HashMap<std::net::IpAddr, usize>>>,
    header_deadline: Instant,
    header_done: bool,
    header_tail: Vec<u8>,
}

impl Listener for LimitedListener {
    type Io = LimitedStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let permit = match Arc::clone(&self.connections).acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            match self.inner.accept().await {
                Ok((stream, address)) => {
                    let client_ip = address.ip();
                    let counted = !client_ip.is_loopback();
                    let (allowed, counted) = if !counted {
                        (true, false)
                    } else {
                        match self.per_ip.lock() {
                            Ok(mut counts) => {
                                let entry = counts.entry(client_ip).or_insert(0);
                                if *entry >= MAX_CONNECTIONS_PER_IP {
                                    (false, false)
                                } else {
                                    *entry += 1;
                                    (true, true)
                                }
                            }
                            Err(error) => {
                                tracing::error!(
                                    %error,
                                    "the connection counter is poisoned; accepting the connection untracked"
                                );
                                (true, false)
                            }
                        }
                    };
                    if !allowed {
                        drop(stream);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                    return (
                        LimitedStream {
                            inner: stream,
                            _permit: permit,
                            client_ip,
                            tracked: counted,
                            per_ip: Arc::clone(&self.per_ip),
                            header_deadline: Instant::now() + Duration::from_secs(HEADER_READ_SECS),
                            header_done: false,
                            header_tail: Vec::with_capacity(4),
                        },
                        address,
                    );
                }
                Err(error) => {
                    tracing::warn!(%error, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

impl Drop for LimitedStream {
    fn drop(&mut self) {
        if !self.tracked {
            return;
        }
        if let Ok(mut counts) = self.per_ip.lock()
            && let Some(count) = counts.get_mut(&self.client_ip)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counts.remove(&self.client_ip);
            }
        }
    }
}

impl AsyncRead for LimitedStream {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.header_done {
            return Pin::new(&mut this.inner).poll_read(context, buffer);
        }
        match Pin::new(&mut this.inner).poll_read(context, buffer) {
            Poll::Ready(Ok(())) => {
                if header_terminator_seen(&this.header_tail, buffer.filled()) {
                    this.header_done = true;
                    this.header_tail.clear();
                } else {
                    let filled = buffer.filled();
                    if let Some(tail) = filled
                        .len()
                        .checked_sub(3)
                        .and_then(|start| filled.get(start..))
                    {
                        this.header_tail.clear();
                        this.header_tail.extend_from_slice(tail);
                    } else {
                        this.header_tail.clear();
                        this.header_tail.extend_from_slice(filled);
                    }
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => {
                let mut deadline = Box::pin(tokio::time::sleep_until(
                    tokio::time::Instant::from_std(this.header_deadline),
                ));
                match deadline.as_mut().poll(context) {
                    Poll::Ready(()) => Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "the request header did not arrive in time",
                    ))),
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}

impl AsyncWrite for LimitedStream {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(context, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(context)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(context, buffers)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
        .headers_mut()
        .insert("x-frame-options", HeaderValue::from_static("DENY"));
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

async fn rest_search(State(state): State<HttpState>, headers: HeaderMap, body: Bytes) -> Response {
    rest_tool_call(state, "ownsearch_search", &headers, body).await
}

async fn rest_fetch(State(state): State<HttpState>, headers: HeaderMap, body: Bytes) -> Response {
    rest_tool_call(state, "ownsearch_fetch", &headers, body).await
}

fn request_id_of(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown")
        .to_owned()
}

fn problem(
    status: StatusCode,
    code: &str,
    message: &str,
    remedy: &str,
    instance: &str,
) -> Response {
    let kind = code.strip_prefix("ownsearch.").unwrap_or(code);
    let body = json!({
        "type": format!("urn:ownsearch:{kind}"),
        "title": status.canonical_reason().unwrap_or("Error"),
        "status": status.as_u16(),
        "detail": message,
        "instance": instance,
        "code": code,
        "remedy": remedy,
    });
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    if status == StatusCode::TOO_MANY_REQUESTS && !response.headers().contains_key(RETRY_AFTER) {
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from_static("1"));
    }
    response
}

async fn rest_tool_call(
    state: HttpState,
    tool_name: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    let instance = request_id_of(headers);
    let json_body = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
        });
    if !json_body {
        return problem(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "ownsearch.bad_request",
            "the body must be sent as application/json",
            "Send one JSON object with the tool arguments.",
            &instance,
        );
    }
    let arguments = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(map)) => map,
        Ok(Value::Null) => Map::new(),
        Ok(_) => {
            return problem(
                StatusCode::BAD_REQUEST,
                "ownsearch.bad_request",
                "the body must be a JSON object",
                "Send one JSON object with the tool arguments.",
                &instance,
            );
        }
        Err(error) => {
            return problem(
                StatusCode::BAD_REQUEST,
                "ownsearch.bad_request",
                &format!("the body is not valid JSON: {error}"),
                "Send one JSON object with the tool arguments.",
                &instance,
            );
        }
    };
    let guard = state.metrics.begin_call();
    let result =
        crate::tools::call(&state.context, tool_name, Some(&arguments), &state.cancel).await;
    let failed = result.is_error == Some(true);
    state.metrics.count_tool_call(tool_name);
    if failed {
        state.metrics.count_error();
    }
    drop(guard);
    let status = if result.is_error == Some(true) {
        let code = result
            .structured_content
            .as_ref()
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str);
        rest_error_status(code)
    } else {
        StatusCode::OK
    };
    match result.structured_content {
        Some(value) if failed => {
            let code = value
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("ownsearch.internal_error");
            let message = value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the tool refused the call");
            let remedy = value.get("remedy").and_then(Value::as_str).unwrap_or("");
            problem(status, code, message, remedy, &instance)
        }
        Some(value) => (status, Json(value)).into_response(),
        None => problem(
            status,
            "ownsearch.internal_error",
            "the tool returned no structured content",
            "",
            &instance,
        ),
    }
}

fn rest_error_status(code: Option<&str>) -> StatusCode {
    match code {
        Some("ownsearch.bad_request") => StatusCode::BAD_REQUEST,
        Some("ownsearch.policy_refused") => StatusCode::FORBIDDEN,
        Some("ownsearch.rate_limited") => StatusCode::TOO_MANY_REQUESTS,
        Some("ownsearch.timeout") => StatusCode::GATEWAY_TIMEOUT,
        Some("ownsearch.no_fleet") => StatusCode::SERVICE_UNAVAILABLE,
        Some("ownsearch.browser_failed") => StatusCode::SERVICE_UNAVAILABLE,
        Some("ownsearch.upstream_failed") => StatusCode::BAD_GATEWAY,
        Some("ownsearch.parser_failed") => StatusCode::BAD_GATEWAY,
        Some("ownsearch.not_found") => StatusCode::NOT_FOUND,
        Some("ownsearch.tool_not_found") => StatusCode::NOT_FOUND,
        Some("ownsearch.cancelled") => StatusCode::SERVICE_UNAVAILABLE,
        Some("ownsearch.auth_failed") => StatusCode::UNAUTHORIZED,
        Some("ownsearch.host_refused") | Some("ownsearch.origin_refused") => StatusCode::FORBIDDEN,
        Some("ownsearch.write_disabled") => StatusCode::FORBIDDEN,
        Some("ownsearch.confirmation_expired") => StatusCode::CONFLICT,
        Some("ownsearch.busy") => StatusCode::TOO_MANY_REQUESTS,
        Some("ownsearch.state_error")
        | Some("ownsearch.state_corrupted")
        | Some("ownsearch.state_newer")
        | Some("ownsearch.config_error")
        | Some("ownsearch.update_failed")
        | Some("ownsearch.internal_error") => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn not_found(headers: HeaderMap) -> Response {
    problem(
        StatusCode::NOT_FOUND,
        "ownsearch.not_found",
        "no route matches this path",
        "Check the path; the HTTP surface is documented in README.md.",
        &request_id_of(&headers),
    )
}

async fn method_not_allowed(headers: HeaderMap, uri: axum::extract::OriginalUri) -> Response {
    let allowed = match uri.path() {
        "/v1/search" | "/v1/fetch" => Some("POST, OPTIONS"),
        "/" | "/mcp" => Some("POST"),
        "/healthz" | "/startupz" | "/readyz" | "/metrics" => Some("GET"),
        _ => None,
    };
    let mut response = problem(
        StatusCode::METHOD_NOT_ALLOWED,
        "ownsearch.method_not_allowed",
        "this path does not accept that method",
        "Send the method the Allow header lists.",
        &request_id_of(&headers),
    );
    if let Some(allowed) = allowed
        && let Ok(value) = HeaderValue::from_str(allowed)
    {
        response
            .headers_mut()
            .insert(axum::http::header::ALLOW, value);
    }
    response
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "ok"})))
}

async fn startupz(State(state): State<HttpState>) -> impl IntoResponse {
    let store = std::sync::Arc::clone(&state.context.store);
    let checked = tokio::task::spawn_blocking(move || store.quick_check()).await;
    let healthy = match checked {
        Ok(Ok(())) => true,
        Ok(Err(error)) => {
            tracing::debug!(%error, "the startup quick check failed");
            false
        }
        Err(error) => {
            tracing::debug!(%error, "the startup quick check task failed");
            false
        }
    };
    if healthy {
        (StatusCode::OK, Json(json!({"status": "started"})))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "starting",
                "reason": "the state database did not answer its quick check",
            })),
        )
    }
}

async fn readyz(State(state): State<HttpState>) -> impl IntoResponse {
    if !state.ready.load(Ordering::SeqCst) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "not-ready", "reason": "shutting down"})),
        );
    }
    let store = std::sync::Arc::clone(&state.context.store);
    match tokio::task::spawn_blocking(move || store.ping()).await {
        Ok(Ok(())) => (StatusCode::OK, Json(json!({"status": "ready"}))),
        Ok(Err(error)) => {
            tracing::debug!(%error, "the readiness read failed");
            not_ready()
        }
        Err(error) => {
            tracing::debug!(%error, "the readiness task failed");
            not_ready()
        }
    }
}

fn not_ready() -> (StatusCode, Json<Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "status": "not-ready",
            "reason": "the state database did not answer a read",
        })),
    )
}

async fn metrics_handler(State(state): State<HttpState>) -> impl IntoResponse {
    let (calls, errors, inflight) = state.metrics.snapshot();
    let (http_calls, http_latency) = state.metrics.http_snapshot();
    let latency: serde_json::Map<String, Value> = http_latency
        .into_iter()
        .map(|(route, (count, sum, max))| {
            (
                route,
                json!({
                    "count": count,
                    "sumMs": sum,
                    "maxMs": max,
                }),
            )
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "calls": calls,
            "errors": errors,
            "inflight": inflight,
            "perTool": state.metrics.per_tool(),
            "httpCalls": http_calls,
            "httpLatency": latency,
        })),
    )
}

async fn reject_origin(State(state): State<HttpState>, request: Request, next: Next) -> Response {
    let instance = request_id_of(request.headers());
    let Some(origin) = request.headers().get(axum::http::header::ORIGIN) else {
        return next.run(request).await;
    };
    let Ok(origin) = origin.to_str() else {
        return rejection(
            StatusCode::FORBIDDEN,
            "ownsearch.origin_refused",
            "the Origin header is not text",
            &instance,
        );
    };
    let allowed = state
        .allowed_origins
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(origin));
    if !allowed {
        return rejection(
            StatusCode::FORBIDDEN,
            "ownsearch.origin_refused",
            "this Origin is not allowed; name it in OWNSEARCH_ALLOWED_ORIGINS",
            &instance,
        );
    }
    let origin = origin.to_owned();
    let mut response = next.run(request).await;
    allow_cross_origin(&mut response, &origin);
    response
}

fn allow_cross_origin(response: &mut Response, origin: &str) {
    let Ok(value) = HeaderValue::from_str(origin) else {
        return;
    };
    let headers = response.headers_mut();
    headers.insert(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    headers.insert(axum::http::header::VARY, HeaderValue::from_static("origin"));
}

async fn preflight(uri: axum::extract::OriginalUri) -> Response {
    let methods = match uri.path() {
        "/v1/search" | "/v1/fetch" => "POST, OPTIONS",
        _ => "GET, OPTIONS",
    };
    let mut response = StatusCode::NO_CONTENT.into_response();
    let headers = response.headers_mut();
    let methods = HeaderValue::from_str(methods).unwrap_or(HeaderValue::from_static("OPTIONS"));
    headers.insert(axum::http::header::ACCESS_CONTROL_ALLOW_METHODS, methods);
    headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("authorization, content-type, x-request-id"),
    );
    headers.insert(
        axum::http::header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    response
}

async fn gate_requests(State(state): State<HttpState>, request: Request, next: Next) -> Response {
    if matches!(
        request.uri().path(),
        "/healthz" | "/startupz" | "/readyz" | "/metrics"
    ) {
        return next.run(request).await;
    }
    let Ok(_permit) = state.gate.clone().try_acquire_owned() else {
        let mut response = rejection(
            StatusCode::SERVICE_UNAVAILABLE,
            "ownsearch.busy",
            "too many requests in flight on this server",
            &request_id_of(request.headers()),
        );
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from_static("1"));
        return response;
    };
    next.run(request).await
}

async fn assign_request_id(
    State(state): State<HttpState>,
    mut request: Request,
    next: Next,
) -> Response {
    let present = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| valid_request_id(value))
        .map(str::to_owned);
    let request_id = present.unwrap_or_else(|| {
        let seq = state.request_seq.fetch_add(1, Ordering::Relaxed);
        format!("req-{}-{seq}", unix_nanos())
    });
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        request
            .headers_mut()
            .insert(axum::http::HeaderName::from_static("x-request-id"), value);
    }
    let echo = request_id.clone();
    let span = tracing::info_span!("http_request", request_id = %request_id);
    let mut response = next.run(request).instrument(span).await;
    if let Ok(value) = HeaderValue::from_str(&echo) {
        response
            .headers_mut()
            .insert(axum::http::HeaderName::from_static("x-request-id"), value);
    }
    response
}

async fn require_host(State(state): State<HttpState>, request: Request, next: Next) -> Response {
    let instance = request_id_of(request.headers());
    let Some(host) = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_host)
    else {
        return rejection(
            StatusCode::BAD_REQUEST,
            "ownsearch.bad_request",
            "the Host header is missing or malformed",
            &instance,
        );
    };
    let allowed = state
        .allowed_hosts
        .iter()
        .filter_map(|entry| parse_host(entry))
        .any(|candidate| host_matches(&host, &candidate));
    if !allowed {
        return rejection(
            StatusCode::FORBIDDEN,
            "ownsearch.host_refused",
            "this Host is not allowed; name it in OWNSEARCH_ALLOWED_HOSTS or use localhost",
            &instance,
        );
    }
    next.run(request).await
}

async fn require_bearer(State(state): State<HttpState>, request: Request, next: Next) -> Response {
    if state.tokens.is_empty() {
        return next.run(request).await;
    }
    let instance = request_id_of(request.headers());
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    let address = client_address(request.headers(), peer);
    if let Some(address) = address
        && let Some(remaining) = lockout_remaining(&state, address)
    {
        let mut response = rejection(
            StatusCode::TOO_MANY_REQUESTS,
            "ownsearch.rate_limited",
            "too many failed authentication attempts",
            &instance,
        );
        response.headers_mut().insert(
            RETRY_AFTER,
            HeaderValue::from_str(&remaining.to_string()).unwrap_or(HeaderValue::from_static("1")),
        );
        return response;
    }
    let Some(presented) = bearer_token(&request) else {
        note_failure(&state, address);
        return unauthorized("a bearer token is required", &instance);
    };
    let matched = state
        .tokens
        .iter()
        .any(|token| token_matches(presented, token.expose_secret()));
    if !matched {
        note_failure(&state, address);
        return unauthorized("the bearer token is not valid", &instance);
    }
    next.run(request).await
}

fn unauthorized(message: &'static str, instance: &str) -> Response {
    let mut response = rejection(
        StatusCode::UNAUTHORIZED,
        "ownsearch.auth_failed",
        message,
        instance,
    );
    response.headers_mut().insert(
        axum::http::header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"ownsearch\""),
    );
    response
}

fn client_address(headers: &HeaderMap, peer: Option<std::net::IpAddr>) -> Option<std::net::IpAddr> {
    let peer = peer?;
    if !peer.is_loopback() {
        return Some(peer);
    }
    forwarded_client(headers).or(Some(peer))
}

fn forwarded_client(headers: &HeaderMap) -> Option<std::net::IpAddr> {
    let raw = headers.get("x-forwarded-for")?.to_str().ok()?;
    raw.rsplit(',').next()?.trim().parse().ok()
}

fn bearer_token(request: &Request) -> Option<&str> {
    let header = request.headers().get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token)
}

fn lockout_remaining(state: &HttpState, address: std::net::IpAddr) -> Option<u64> {
    let Ok(mut failed) = state.failed.lock() else {
        return None;
    };
    failed.retain(|_, (_, _, last)| last.elapsed() < Duration::from_secs(LOCKOUT_SECS));
    failed.get(&address).and_then(|(count, _, last)| {
        let elapsed = last.elapsed();
        (*count >= FAILED_AUTH_LIMIT && elapsed < Duration::from_secs(LOCKOUT_SECS))
            .then(|| LOCKOUT_SECS.saturating_sub(elapsed.as_secs()).max(1))
    })
}

fn note_failure(state: &HttpState, address: Option<std::net::IpAddr>) {
    let Some(address) = address else {
        return;
    };
    let Ok(mut failed) = state.failed.lock() else {
        return;
    };
    if failed.len() >= MAX_FAILED_AUTH_ENTRIES {
        failed.retain(|_, (_, _, last)| last.elapsed() < Duration::from_secs(LOCKOUT_SECS));
        while failed.len() >= MAX_FAILED_AUTH_ENTRIES {
            let Some(oldest) = failed
                .iter()
                .min_by_key(|(_, (_, _, last))| *last)
                .map(|(address, _)| *address)
            else {
                break;
            };
            failed.remove(&oldest);
        }
    }
    let now = Instant::now();
    let entry = failed.entry(address).or_insert((0, now, now));
    if entry.1.elapsed() > Duration::from_secs(FAILED_AUTH_WINDOW_SECS) {
        *entry = (1, now, now);
    } else {
        entry.0 = entry.0.saturating_add(1);
        entry.2 = now;
    }
}

fn validate_tokens(tokens: &[secrecy::SecretString]) -> Result<()> {
    for token in tokens {
        if token.expose_secret().chars().count() < MIN_TOKEN_CHARS {
            return Err(Error::Config(format!(
                "a bearer token is shorter than {MIN_TOKEN_CHARS} characters; use a random token"
            )));
        }
    }
    Ok(())
}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REQUEST_ID
        && value.chars().all(|character| character.is_ascii_graphic())
}

fn unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0)
}

fn token_matches(presented: &str, expected: &str) -> bool {
    use sha2::{Digest, Sha256};
    let left = Sha256::digest(presented.as_bytes());
    let right = Sha256::digest(expected.as_bytes());
    let mut difference = 0u8;
    for (left, right) in left.iter().zip(right.iter()) {
        difference |= left ^ right;
    }
    difference == 0
}

fn header_terminator_seen(tail: &[u8], bytes: &[u8]) -> bool {
    let ends_headers = |slice: &[u8]| {
        slice.windows(4).any(|window| window == b"\r\n\r\n")
            || slice.windows(2).any(|window| window == b"\n\n")
    };
    if ends_headers(bytes) {
        return true;
    }
    if tail.is_empty() {
        return false;
    }
    let mut joined = Vec::with_capacity(tail.len() + bytes.len().min(4));
    joined.extend_from_slice(tail);
    if let Some(head) = bytes.get(..4) {
        joined.extend_from_slice(head);
    }
    ends_headers(&joined)
}

fn is_loopback(bind: &str) -> bool {
    match bind.parse::<SocketAddr>() {
        Ok(address) => address.ip().is_loopback(),
        Err(_) => match bind.rsplit_once(':') {
            Some((host, _)) => host.eq_ignore_ascii_case("localhost"),
            None => false,
        },
    }
}

fn allowed_hosts(bind: &str, configured: Option<&[String]>) -> Vec<String> {
    if let Some(hosts) = configured
        && !hosts.is_empty()
    {
        return hosts.to_vec();
    }
    let mut hosts = vec![
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
        "[::1]".to_owned(),
    ];
    if let Some(host) = host_from_bind(bind)
        && !hosts
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&host))
    {
        hosts.push(host);
    }
    hosts
}

fn host_from_bind(bind: &str) -> Option<String> {
    let address = bind.parse::<SocketAddr>().ok()?;
    Some(address.ip().to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HostIdentity {
    name: String,
    port: Option<u16>,
}

fn parse_host(value: &str) -> Option<HostIdentity> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 255 {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':').and_then(|port| port.parse().ok());
        return Some(HostIdentity {
            name: host.to_ascii_lowercase(),
            port,
        });
    }
    match trimmed.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|character| character.is_ascii_digit()) => {
            Some(HostIdentity {
                name: host.to_ascii_lowercase(),
                port: port.parse().ok(),
            })
        }
        _ => Some(HostIdentity {
            name: trimmed.to_ascii_lowercase(),
            port: None,
        }),
    }
}

fn host_matches(request: &HostIdentity, allowed: &HostIdentity) -> bool {
    if request.name != allowed.name {
        return false;
    }
    match (request.port, allowed.port) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

fn rejection(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    instance: &str,
) -> Response {
    let mut response = problem(status, code, message, remedy_for(code), instance);
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

async fn problem_fallback(request: Request, next: Next) -> Response {
    let instance = request_id_of(request.headers());
    let response = next.run(request).await;
    let status = response.status();
    if !matches!(status.as_u16(), 408 | 413 | 415 | 431) {
        return response;
    }
    let described = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/problem+json"));
    if described {
        return response;
    }
    let (code, message) = match status.as_u16() {
        408 => (
            "ownsearch.timeout",
            "the request body did not arrive in time",
        ),
        413 => (
            "ownsearch.bad_request",
            "the request body is larger than the 1 MiB limit",
        ),
        415 => (
            "ownsearch.bad_request",
            "the request body type is not supported",
        ),
        _ => (
            "ownsearch.bad_request",
            "the request headers are not acceptable",
        ),
    };
    let mut rewritten = problem(status, code, message, remedy_for(code), &instance);
    rewritten
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    rewritten
}

async fn record_http_metrics(
    State(state): State<HttpState>,
    request: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let method = request.method().to_string();
    let route = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_else(|| request.uri().path().to_owned());
    let response = next.run(request).await;
    state.metrics.record_http(
        &method,
        &route,
        response.status().as_u16(),
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    );
    response
}

fn remedy_for(code: &'static str) -> &'static str {
    match code {
        "ownsearch.auth_failed" => {
            "Send Authorization: Bearer <token> with a configured OWNSEARCH_BEARER_TOKENS value."
        }
        "ownsearch.origin_refused" => {
            "Name this Origin in OWNSEARCH_ALLOWED_ORIGINS, or send no Origin header."
        }
        "ownsearch.host_refused" => {
            "Name this Host in OWNSEARCH_ALLOWED_HOSTS, or call the server through localhost."
        }
        "ownsearch.rate_limited" => "Wait for the window in Retry-After, then retry.",
        "ownsearch.busy" => "Retry shortly; the server is at its in-flight request limit.",
        "ownsearch.bad_request" => {
            "Fix the request body: it must be a JSON object with the fields the tool documents."
        }
        _ => "Read the detail field for the cause, and the code for the stable name.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_public_bind_without_tokens_is_refused() {
        assert!(!is_loopback("0.0.0.0:8774"));
        assert!(!is_loopback("192.168.1.10:8774"));
        assert!(is_loopback("127.0.0.1:8774"));
        assert!(is_loopback("localhost:8774"));
        assert!(is_loopback("[::1]:8774"));
    }

    #[test]
    fn a_short_token_is_refused() {
        let token = secrecy::SecretString::from("short");
        assert!(validate_tokens(&[token]).is_err());
        let token = secrecy::SecretString::from("a".repeat(32));
        assert!(validate_tokens(&[token]).is_ok());
    }

    #[test]
    fn token_comparison_is_length_aware() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abcd"));
        assert!(!token_matches("abc", "abd"));
    }

    #[test]
    fn host_matching_accepts_a_default_and_a_named_form() {
        let request = parse_host("127.0.0.1:8774").expect("request");
        let allowed = parse_host("127.0.0.1").expect("allowed");
        assert!(host_matches(&request, &allowed));
        let other = parse_host("evil.example").expect("other");
        assert!(!host_matches(&request, &other));
    }

    #[test]
    fn the_default_host_list_covers_loopback() {
        let hosts = allowed_hosts("127.0.0.1:8774", None);
        assert!(hosts.iter().any(|host| host == "localhost"));
        assert!(hosts.iter().any(|host| host == "127.0.0.1"));
    }

    #[test]
    fn a_configured_host_list_replaces_the_default() {
        let hosts = allowed_hosts("127.0.0.1:8774", Some(&["ownsearch.example".to_owned()]));
        assert_eq!(hosts, vec!["ownsearch.example".to_owned()]);
    }

    #[test]
    fn a_request_id_must_be_a_short_graphic_string() {
        assert!(valid_request_id("abc-123"));
        assert!(!valid_request_id(""));
        assert!(!valid_request_id("with space"));
        assert!(!valid_request_id(&"a".repeat(200)));
    }
}
