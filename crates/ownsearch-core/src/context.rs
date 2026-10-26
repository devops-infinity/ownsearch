use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use crate::cache::ResponseCache;
use crate::client::FleetClient;
use crate::config::Settings;
use crate::db::Store;
use crate::error::{Error, Result};
use crate::instances::BootstrapSnapshot;
use crate::ledger::{Ledger, args_hash};
use crate::tls_probe::TlsProber;

pub(crate) type HttpCalls = BTreeMap<String, u64>;
pub(crate) type HttpLatency = BTreeMap<String, (u64, u64, u64)>;

#[derive(Debug, Default)]
pub(crate) struct Metrics {
    calls: AtomicU64,
    errors: AtomicU64,
    cache_hits: AtomicU64,
    inflight: AtomicI64,
    per_tool: Mutex<BTreeMap<String, u64>>,
    http_calls: Mutex<HttpCalls>,
    http_latency: Mutex<HttpLatency>,
}

pub(crate) struct CallGuard {
    metrics: Arc<Metrics>,
}

impl std::fmt::Debug for CallGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CallGuard")
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        self.metrics.inflight.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Metrics {
    pub(crate) fn begin_call(self: &Arc<Self>) -> CallGuard {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inflight.fetch_add(1, Ordering::Relaxed);
        CallGuard {
            metrics: Arc::clone(self),
        }
    }

    pub(crate) fn count_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn count_tool_call(&self, tool: &str) {
        if let Ok(mut counts) = self.per_tool.lock() {
            *counts.entry(tool.to_owned()).or_insert(0) += 1;
        }
    }

    pub(crate) fn count_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> (u64, u64, i64) {
        (
            self.calls.load(Ordering::Relaxed),
            self.errors.load(Ordering::Relaxed),
            self.inflight.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn session_snapshot(&self) -> (u64, u64, u64) {
        (
            self.calls.load(Ordering::Relaxed),
            self.errors.load(Ordering::Relaxed),
            self.cache_hits.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn record_http(&self, method: &str, route: &str, status: u16, elapsed_ms: u64) {
        let key = format!("{method} {route}");
        if let Ok(mut counts) = self.http_calls.lock() {
            *counts.entry(format!("{key} {status}")).or_insert(0) += 1;
        }
        if let Ok(mut latencies) = self.http_latency.lock() {
            let entry = latencies.entry(key).or_insert((0, 0, 0));
            entry.0 = entry.0.saturating_add(1);
            entry.1 = entry.1.saturating_add(elapsed_ms);
            entry.2 = entry.2.max(elapsed_ms);
        }
    }

    pub(crate) fn http_snapshot(&self) -> (HttpCalls, HttpLatency) {
        let calls = self
            .http_calls
            .lock()
            .map(|counts| counts.clone())
            .unwrap_or_default();
        let latencies = self
            .http_latency
            .lock()
            .map(|values| values.clone())
            .unwrap_or_default();
        (calls, latencies)
    }

    pub(crate) fn per_tool(&self) -> BTreeMap<String, u64> {
        self.per_tool
            .lock()
            .map(|counts| counts.clone())
            .unwrap_or_default()
    }
}

pub struct ToolContext {
    pub settings: Settings,
    pub store: Arc<Store>,
    pub client: FleetClient,
    pub probe_client: FleetClient,
    pub tls: TlsProber,
    pub browser: crate::browser::Renderer,
    pub heartbeat: std::sync::Arc<crate::watchdog::Heartbeat>,
    pub ledger: std::sync::Arc<Ledger>,
    pub cache: ResponseCache,
    pub crud: crate::crud::CrudService,
    pub(crate) metrics: Arc<Metrics>,
    round_robin: std::sync::atomic::AtomicUsize,
    args_key: [u8; 32],
    fleet_generation: std::sync::atomic::AtomicU64,
    search_permits: std::sync::Arc<tokio::sync::Semaphore>,
    search_queue: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolContext")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl ToolContext {
    pub fn new(settings: Settings) -> Result<Self> {
        let search_max_concurrent = settings.search_max_concurrent();
        let store = Arc::new(Store::open(&settings.database_path())?);
        store.quick_check()?;
        let client = FleetClient::new(&settings)?;
        let probe_client = FleetClient::with_probe_timeout(&settings)?;
        let tls = TlsProber::new(
            settings.probe_timeout_secs(),
            settings.allow_private_instances(),
        )?;
        let browser = crate::browser::Renderer::new(
            settings.browser(),
            settings.browser_executable().map(str::to_owned),
            settings.browser_idle_secs(),
            settings.browser_no_sandbox(),
            settings.allow_private_instances(),
            settings.browser_watchdog_secs(),
            settings.browser_max_lifetime_secs(),
            &settings.state_dir().join("browser"),
        );
        let args_key = crate::ledger::load_or_create_key(&settings.ledger_dir().join("args.key"))?;
        let ledger = std::sync::Arc::new(Ledger::open(settings.ledger_path(), &args_key)?);
        let cache = ResponseCache::new(settings.cache_ttl_secs(), settings.cache_max_bytes());
        let crud = crate::crud::CrudService::new(&settings, Arc::clone(&store));
        let context = Self {
            settings,
            store,
            client,
            probe_client,
            tls,
            browser,
            heartbeat: std::sync::Arc::new(crate::watchdog::Heartbeat::new()),
            ledger,
            cache,
            crud,
            metrics: Arc::new(Metrics::default()),
            round_robin: std::sync::atomic::AtomicUsize::new(0),
            args_key,
            fleet_generation: std::sync::atomic::AtomicU64::new(0),
            search_permits: std::sync::Arc::new(tokio::sync::Semaphore::new(search_max_concurrent)),
            search_queue: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        context.maintain();
        Ok(context)
    }

    pub fn with_store(settings: Settings, store: Arc<Store>) -> Result<Self> {
        let search_max_concurrent = settings.search_max_concurrent();
        let client = FleetClient::new(&settings)?;
        let probe_client = FleetClient::with_probe_timeout(&settings)?;
        let tls = TlsProber::new(
            settings.probe_timeout_secs(),
            settings.allow_private_instances(),
        )?;
        let browser = crate::browser::Renderer::new(
            settings.browser(),
            settings.browser_executable().map(str::to_owned),
            settings.browser_idle_secs(),
            settings.browser_no_sandbox(),
            settings.allow_private_instances(),
            settings.browser_watchdog_secs(),
            settings.browser_max_lifetime_secs(),
            &settings.state_dir().join("browser"),
        );
        let args_key = crate::ledger::load_or_create_key(&settings.ledger_dir().join("args.key"))?;
        let ledger = std::sync::Arc::new(Ledger::open(settings.ledger_path(), &args_key)?);
        let cache = ResponseCache::new(settings.cache_ttl_secs(), settings.cache_max_bytes());
        let crud = crate::crud::CrudService::new(&settings, Arc::clone(&store));
        let context = Self {
            settings,
            store,
            client,
            probe_client,
            tls,
            browser,
            heartbeat: std::sync::Arc::new(crate::watchdog::Heartbeat::new()),
            ledger,
            cache,
            crud,
            metrics: Arc::new(Metrics::default()),
            round_robin: std::sync::atomic::AtomicUsize::new(0),
            args_key,
            fleet_generation: std::sync::atomic::AtomicU64::new(0),
            search_permits: std::sync::Arc::new(tokio::sync::Semaphore::new(search_max_concurrent)),
            search_queue: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        context.maintain();
        Ok(context)
    }

    pub(crate) async fn acquire_search(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        use std::sync::atomic::Ordering;
        struct Waiter<'a>(&'a std::sync::atomic::AtomicUsize);
        impl Drop for Waiter<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let waiting = self.search_queue.fetch_add(1, Ordering::SeqCst) + 1;
        if waiting > self.settings.search_queue_limit() {
            self.search_queue.fetch_sub(1, Ordering::SeqCst);
            return Err(Error::RateLimited {
                message: format!(
                    "{} searches are already waiting; the queue limit is {}",
                    waiting.saturating_sub(1),
                    self.settings.search_queue_limit()
                ),
            });
        }
        let waiter = Waiter(&self.search_queue);
        let permit = tokio::time::timeout(
            std::time::Duration::from_secs(self.settings.search_queue_wait_secs()),
            std::sync::Arc::clone(&self.search_permits).acquire_owned(),
        )
        .await;
        drop(waiter);
        match permit {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) => Err(Error::State("the search queue closed".into())),
            Err(_) => Err(Error::RateLimited {
                message: format!(
                    "no search slot freed within {} seconds",
                    self.settings.search_queue_wait_secs()
                ),
            }),
        }
    }

    pub fn bump_fleet_generation(&self) {
        self.fleet_generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn fleet_generation(&self) -> u64 {
        self.fleet_generation
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn next_rotation(&self, span: usize) -> usize {
        if span <= 1 {
            return 0;
        }
        self.round_robin
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % span
    }

    fn maintain(&self) {
        match crate::cleanup::clean(&self.settings, &self.ledger, &self.args_key) {
            Ok(report) => {
                for warning in &report.warnings {
                    tracing::warn!(%warning, "cleanup");
                }
                if report.ledger_rotated {
                    tracing::info!("rotated the call ledger at the keep boundary");
                }
            }
            Err(error) => tracing::warn!(
                %error,
                "the cleanup failed; run `ownsearch db maintenance`"
            ),
        }
    }
    pub fn hash_args(&self, value: &serde_json::Value) -> String {
        args_hash(&self.args_key, value)
    }

    pub fn ledger_key(&self) -> &[u8] {
        &self.args_key
    }

    pub fn ensure_bootstrap(&self) -> Result<(usize, usize)> {
        if self.store.bootstrap_imported()? || self.store.instance_count()? > 0 {
            return Ok((0, 0));
        }
        let snapshot = BootstrapSnapshot::embedded()?;
        let outcome = self
            .store
            .import_bootstrap(&snapshot, crate::time::now_unix())?;
        if let Err(error) = self.store.write_snapshot(&self.settings.snapshot_path()) {
            tracing::warn!(%error, "the state snapshot could not be written");
        }
        Ok(outcome)
    }
}
