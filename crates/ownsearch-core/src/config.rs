use std::path::{Path, PathBuf};

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::types::{AiDbMode, Preset};

pub(crate) const KEYRING_SERVICE: &str = "ownsearch";
pub(crate) const KEYRING_USER: &str = "http-bearer-tokens";

pub(crate) const DEFAULT_DISCOVERY_URL: &str = "https://searx.space/data/instances.json";
pub(crate) const DEFAULT_REFRESH_SECS: u64 = 21_600;
pub(crate) const DEFAULT_BIND: &str = "127.0.0.1:8774";
pub(crate) const DEFAULT_RETENTION_SECS: u64 = 259_200;
pub(crate) const MIN_RETENTION_SECS: u64 = 3_600;
pub(crate) const MAX_RETENTION_SECS: u64 = 7_776_000;

pub(crate) const DEFAULT_USER_AGENT: &str =
    "OwnSearch (MCP fleet scanner; +https://github.com/devops-infinity/ownsearch)";

const MAX_PROFILE_BYTES: u64 = 64 * 1024;
const MAX_CACHE_BYTES: usize = 1024 * 1024 * 1024;
const MAX_TIMEOUT_SECS: u64 = 600;
const MAX_FLEET: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub http: Option<bool>,
    pub bind: Option<String>,
    pub profile: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    pub ledger_dir: Option<PathBuf>,
    pub offline: Option<bool>,
    pub html_fallback: Option<bool>,
    pub cache_ttl_secs: Option<u64>,
    pub scan_concurrency: Option<usize>,
}

#[derive(Clone)]
pub struct Settings {
    http: bool,
    bind: String,
    bearer_tokens: Vec<SecretString>,
    bearer_token_error: Option<String>,
    allowed_hosts: Option<Vec<String>>,
    allowed_origins: Option<Vec<String>>,
    profile_path: Option<PathBuf>,
    state_dir: PathBuf,
    ledger_dir: PathBuf,
    discovery_url: String,
    discovery_refresh_secs: u64,
    offline: bool,
    scan_concurrency: usize,
    scan_jitter_ms: u64,
    probe_timeout_secs: u64,
    scan_deadline_secs: u64,
    fetch_deadline_secs: u64,
    browser: bool,
    browser_executable: Option<String>,
    browser_idle_secs: u64,
    browser_no_sandbox: bool,
    browser_watchdog_secs: u64,
    browser_max_lifetime_secs: u64,
    watchdog_secs: u64,
    scan_min_interval_secs: u64,
    request_timeout_secs: u64,
    connect_timeout_secs: u64,
    max_upstream_bytes: usize,
    cooldown_secs: u64,
    probation_failures: u32,
    quarantine_failures: u32,
    promote_successes: u32,
    fleet_min: usize,
    fleet_max: usize,
    max_per_network: usize,
    html_fallback: bool,
    google_required: bool,
    cert_min_days: i64,
    search_deadline_secs: u64,
    exhaustive_max: usize,
    search_attempts: u32,
    search_failover: bool,
    round_robin: bool,
    result_cap: usize,
    search_max_concurrent: usize,
    search_queue_limit: usize,
    search_queue_wait_secs: u64,
    preset: Preset,
    cache_ttl_secs: u64,
    cache_max_bytes: usize,
    ai_db_mode: AiDbMode,
    ai_db_max_changes_per_call: usize,
    ai_db_confirm_ttl_secs: u64,
    ai_db_min_interval_secs: u64,
    keep_window_secs: u64,
    maintenance_interval_secs: u64,
    allow_private_instances: bool,
    capsolver_key: Option<SecretString>,
    capsolver_timeout_secs: u64,
    proxy: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    #[serde(default = "profile_version", rename = "version")]
    _version: u32,
    http: Option<bool>,
    bind: Option<String>,
    allowed_hosts: Option<Vec<String>>,
    allowed_origins: Option<Vec<String>>,
    state_dir: Option<PathBuf>,
    ledger_dir: Option<PathBuf>,
    discovery_url: Option<String>,
    discovery_refresh_secs: Option<u64>,
    offline: Option<bool>,
    scan_concurrency: Option<usize>,
    scan_jitter_ms: Option<u64>,
    probe_timeout_secs: Option<u64>,
    scan_deadline_secs: Option<u64>,
    fetch_deadline_secs: Option<u64>,
    browser: Option<bool>,
    browser_executable: Option<String>,
    browser_idle_secs: Option<u64>,
    browser_no_sandbox: Option<bool>,
    browser_watchdog_secs: Option<u64>,
    browser_max_lifetime_secs: Option<u64>,
    watchdog_secs: Option<u64>,
    scan_min_interval_secs: Option<u64>,
    request_timeout_secs: Option<u64>,
    connect_timeout_secs: Option<u64>,
    max_upstream_bytes: Option<usize>,
    cooldown_secs: Option<u64>,
    probation_failures: Option<u32>,
    quarantine_failures: Option<u32>,
    promote_successes: Option<u32>,
    fleet_min: Option<usize>,
    fleet_max: Option<usize>,
    max_per_network: Option<usize>,
    html_fallback: Option<bool>,
    google_required: Option<bool>,
    cert_min_days: Option<i64>,
    search_deadline_secs: Option<u64>,
    exhaustive_max: Option<usize>,
    search_attempts: Option<u32>,
    search_failover: Option<bool>,
    round_robin: Option<bool>,
    result_cap: Option<usize>,
    search_max_concurrent: Option<usize>,
    search_queue_limit: Option<usize>,
    search_queue_wait_secs: Option<u64>,
    preset: Option<Preset>,
    cache_ttl_secs: Option<u64>,
    cache_max_bytes: Option<usize>,
    ai_db_mode: Option<AiDbMode>,
    ai_db_max_changes_per_call: Option<usize>,
    ai_db_confirm_ttl_secs: Option<u64>,
    ai_db_min_interval_secs: Option<u64>,
    keep_window: Option<String>,
    maintenance_interval_secs: Option<u64>,
    allow_private_instances: Option<bool>,
    capsolver_key: Option<String>,
    capsolver_timeout_secs: Option<u64>,
    proxy: Option<String>,
}

impl Settings {
    pub fn load(overrides: Overrides) -> Result<Self> {
        Self::load_with(overrides, |name| std::env::var(name).ok())
    }

    pub fn load_with<F>(overrides: Overrides, env: F) -> Result<Self>
    where
        F: Fn(&str) -> Option<String>,
    {
        let profile_path = match overrides
            .profile
            .clone()
            .or_else(|| non_empty_env(&env, "OWNSEARCH_PROFILE").map(PathBuf::from))
        {
            Some(path) => Some(path),
            None => config_dir_from(&env).map(|dir| dir.join("profile.toml")),
        };
        let profile = match profile_path.as_deref() {
            Some(path) => read_profile_if_present(path, overrides.profile.is_some())?,
            None => Profile::default(),
        };
        let (bearer_tokens, bearer_token_error) = bearer_tokens_from(&env);
        let allowed_hosts = allowed_list(
            &env,
            "OWNSEARCH_ALLOWED_HOSTS",
            profile.allowed_hosts.clone(),
        )?;
        let allowed_origins = allowed_list(
            &env,
            "OWNSEARCH_ALLOWED_ORIGINS",
            profile.allowed_origins.clone(),
        )?;
        let state_dir = match overrides
            .state_dir
            .or_else(|| non_empty_env(&env, "OWNSEARCH_STATE_DIR").map(PathBuf::from))
            .or(profile.state_dir)
        {
            Some(dir) => dir,
            None => state_dir_from(&env).ok_or_else(|| {
                Error::Config(
                    "no absolute base directory for state; set HOME or XDG_STATE_HOME, or pass OWNSEARCH_STATE_DIR"
                        .to_owned(),
                )
            })?,
        };
        if !state_dir.is_absolute() {
            return Err(Error::Config(format!(
                "state_dir {} must be an absolute path",
                state_dir.display()
            )));
        }
        let ledger_dir = match overrides
            .ledger_dir
            .or_else(|| non_empty_env(&env, "OWNSEARCH_LEDGER_DIR").map(PathBuf::from))
            .or(profile.ledger_dir)
        {
            Some(dir) => dir,
            None => data_dir_from(&env)
                .ok_or_else(|| {
                    Error::Config(
                        "no absolute base directory for the ledger; set HOME or XDG_DATA_HOME, or pass an explicit ledger directory"
                            .to_owned(),
                    )
                })?
                .join("ledger"),
        };
        if !ledger_dir.is_absolute() {
            return Err(Error::Config(format!(
                "ledger_dir {} must be an absolute path",
                ledger_dir.display()
            )));
        }
        let request_timeout_secs = bounded_u64(
            env_parse(&env, "OWNSEARCH_REQUEST_TIMEOUT_SECS")?
                .or(profile.request_timeout_secs)
                .unwrap_or(30),
            1,
            MAX_TIMEOUT_SECS,
            "request_timeout_secs",
        )?;
        let probe_timeout_secs = bounded_u64(
            env_parse(&env, "OWNSEARCH_PROBE_TIMEOUT_SECS")?
                .or(profile.probe_timeout_secs)
                .unwrap_or(15),
            1,
            MAX_TIMEOUT_SECS,
            "probe_timeout_secs",
        )?;
        let scan_deadline_secs = bounded_u64(
            env_parse(&env, "OWNSEARCH_SCAN_DEADLINE_SECS")?
                .or(profile.scan_deadline_secs)
                .unwrap_or(600),
            30,
            7_200,
            "scan_deadline_secs",
        )?;
        let fetch_deadline_secs = bounded_u64(
            env_parse(&env, "OWNSEARCH_FETCH_DEADLINE_SECS")?
                .or(profile.fetch_deadline_secs)
                .unwrap_or(60),
            5,
            600,
            "fetch_deadline_secs",
        )?;
        let browser = env_parse(&env, "OWNSEARCH_BROWSER")?
            .or(profile.browser)
            .unwrap_or(true);
        let browser_executable =
            non_empty_env(&env, "OWNSEARCH_BROWSER_PATH").or(profile.browser_executable);
        let browser_idle_secs = bounded_u64(
            env_parse(&env, "OWNSEARCH_BROWSER_IDLE_SECS")?
                .or(profile.browser_idle_secs)
                .unwrap_or(120),
            30,
            3_600,
            "browser_idle_secs",
        )?;
        let browser_no_sandbox = env_parse(&env, "OWNSEARCH_BROWSER_NO_SANDBOX")?
            .or(profile.browser_no_sandbox)
            .unwrap_or(false);
        let browser_watchdog_secs = bounded_u64(
            env_parse(&env, "OWNSEARCH_BROWSER_WATCHDOG_SECS")?
                .or(profile.browser_watchdog_secs)
                .unwrap_or(30),
            5,
            600,
            "browser_watchdog_secs",
        )?;
        let browser_max_lifetime_secs = bounded_u64(
            env_parse(&env, "OWNSEARCH_BROWSER_MAX_LIFETIME_SECS")?
                .or(profile.browser_max_lifetime_secs)
                .unwrap_or(1_800),
            60,
            86_400,
            "browser_max_lifetime_secs",
        )?;
        let watchdog_secs = bounded_u64(
            env_parse(&env, "OWNSEARCH_WATCHDOG_SECS")?
                .or(profile.watchdog_secs)
                .unwrap_or(120),
            0,
            3_600,
            "watchdog_secs",
        )?;
        let scan_min_interval_secs = bounded_u64(
            env_parse(&env, "OWNSEARCH_SCAN_MIN_INTERVAL_SECS")?
                .or(profile.scan_min_interval_secs)
                .unwrap_or(30),
            0,
            3_600,
            "scan_min_interval_secs",
        )?;
        let fleet_min = bounded_usize(
            env_parse(&env, "OWNSEARCH_FLEET_MIN")?
                .or(profile.fleet_min)
                .unwrap_or(6),
            1,
            MAX_FLEET,
            "fleet_min",
        )?;
        let fleet_max = bounded_usize(
            env_parse(&env, "OWNSEARCH_FLEET_MAX")?
                .or(profile.fleet_max)
                .unwrap_or(12),
            1,
            MAX_FLEET,
            "fleet_max",
        )?;
        if fleet_min > fleet_max {
            return Err(Error::Config(format!(
                "fleet_min {fleet_min} is above fleet_max {fleet_max}"
            )));
        }
        let cache_max_bytes = profile.cache_max_bytes.unwrap_or(16 * 1024 * 1024);
        if cache_max_bytes > MAX_CACHE_BYTES {
            return Err(Error::Config(format!(
                "cache_max_bytes is {cache_max_bytes}; the cap is {MAX_CACHE_BYTES} bytes (1 GiB)"
            )));
        }
        let ai_db_mode = match non_empty_env(&env, "OWNSEARCH_AI_DB_MODE") {
            Some(raw) => AiDbMode::parse(&raw).ok_or_else(|| {
                Error::Config(format!(
                    "OWNSEARCH_AI_DB_MODE is {raw:?}; use read-only or read-write"
                ))
            })?,
            None => profile.ai_db_mode.unwrap_or_default(),
        };
        let capsolver_key = non_empty_env(&env, "OWNSEARCH_CAPSOLVER_KEY")
            .or(profile.capsolver_key.clone())
            .map(SecretString::from);
        let capsolver_timeout_secs = bounded_u64(
            env_parse(&env, "OWNSEARCH_CAPSOLVER_TIMEOUT_SECS")?
                .or(profile.capsolver_timeout_secs)
                .unwrap_or(45),
            10,
            180,
            "capsolver_timeout_secs",
        )?;
        let proxy = non_empty_env(&env, "OWNSEARCH_PROXY").or(profile.proxy.clone());
        Ok(Self {
            http: overrides
                .http
                .or(env_parse(&env, "OWNSEARCH_HTTP")?)
                .or(profile.http)
                .unwrap_or(false),
            bind: overrides
                .bind
                .or_else(|| non_empty_env(&env, "OWNSEARCH_BIND"))
                .or(profile.bind)
                .unwrap_or_else(|| DEFAULT_BIND.to_owned()),
            bearer_tokens,
            bearer_token_error,
            allowed_hosts,
            allowed_origins,
            profile_path,
            state_dir,
            ledger_dir,
            discovery_url: discovery_url_from(&env, profile.discovery_url.clone())?,
            discovery_refresh_secs: bounded_u64(
                env_parse(&env, "OWNSEARCH_DISCOVERY_REFRESH_SECS")?
                    .or(profile.discovery_refresh_secs)
                    .unwrap_or(DEFAULT_REFRESH_SECS),
                60,
                30 * 24 * 3600,
                "discovery_refresh_secs",
            )?,
            offline: overrides
                .offline
                .or(env_parse(&env, "OWNSEARCH_OFFLINE")?)
                .or(profile.offline)
                .unwrap_or(false),
            scan_concurrency: bounded_usize(
                overrides
                    .scan_concurrency
                    .or(env_parse(&env, "OWNSEARCH_SCAN_CONCURRENCY")?)
                    .or(profile.scan_concurrency)
                    .unwrap_or(4),
                1,
                8,
                "scan_concurrency",
            )?,
            scan_jitter_ms: env_parse::<u64>(&env, "OWNSEARCH_SCAN_JITTER_MS")?
                .or(profile.scan_jitter_ms)
                .unwrap_or(250)
                .min(10_000),
            probe_timeout_secs,
            scan_deadline_secs,
            fetch_deadline_secs,
            browser,
            browser_executable,
            browser_idle_secs,
            browser_no_sandbox,
            browser_watchdog_secs,
            browser_max_lifetime_secs,
            watchdog_secs,
            scan_min_interval_secs,
            request_timeout_secs,
            connect_timeout_secs: bounded_u64(
                env_parse(&env, "OWNSEARCH_CONNECT_TIMEOUT_SECS")?
                    .or(profile.connect_timeout_secs)
                    .unwrap_or(5),
                1,
                60,
                "connect_timeout_secs",
            )?,
            max_upstream_bytes: bounded_usize(
                env_parse(&env, "OWNSEARCH_MAX_UPSTREAM_BYTES")?
                    .or(profile.max_upstream_bytes)
                    .unwrap_or(4 * 1024 * 1024),
                64 * 1024,
                32 * 1024 * 1024,
                "max_upstream_bytes",
            )?,
            cooldown_secs: env_parse::<u64>(&env, "OWNSEARCH_COOLDOWN_SECS")?
                .or(profile.cooldown_secs)
                .unwrap_or(3_600)
                .min(604_800),
            probation_failures: env_parse::<u32>(&env, "OWNSEARCH_PROBATION_FAILURES")?
                .or(profile.probation_failures)
                .unwrap_or(2)
                .max(1),
            quarantine_failures: env_parse::<u32>(&env, "OWNSEARCH_QUARANTINE_FAILURES")?
                .or(profile.quarantine_failures)
                .unwrap_or(4)
                .max(2),
            promote_successes: env_parse::<u32>(&env, "OWNSEARCH_PROMOTE_SUCCESSES")?
                .or(profile.promote_successes)
                .unwrap_or(2)
                .max(1),
            fleet_min,
            fleet_max,
            max_per_network: bounded_usize(
                env_parse(&env, "OWNSEARCH_MAX_PER_NETWORK")?
                    .or(profile.max_per_network)
                    .unwrap_or(2),
                1,
                MAX_FLEET,
                "max_per_network",
            )?,
            html_fallback: overrides
                .html_fallback
                .or(env_parse(&env, "OWNSEARCH_HTML_FALLBACK")?)
                .or(profile.html_fallback)
                .unwrap_or(true),
            google_required: env_parse(&env, "OWNSEARCH_GOOGLE_REQUIRED")?
                .or(profile.google_required)
                .unwrap_or(false),
            cert_min_days: i64::try_from(bounded_u64(
                cert_min_days(&env, profile.cert_min_days)?,
                0,
                365,
                "cert_min_days",
            )?)
            .unwrap_or(365),
            search_deadline_secs: bounded_u64(
                env_parse(&env, "OWNSEARCH_SEARCH_DEADLINE_SECS")?
                    .or(profile.search_deadline_secs)
                    .unwrap_or(30),
                1,
                MAX_TIMEOUT_SECS,
                "search_deadline_secs",
            )?,
            exhaustive_max: bounded_usize(
                env_parse(&env, "OWNSEARCH_EXHAUSTIVE_MAX")?
                    .or(profile.exhaustive_max)
                    .unwrap_or(16),
                1,
                16,
                "exhaustive_max",
            )?,
            search_attempts: bounded_usize(
                usize::try_from(
                    env_parse::<u32>(&env, "OWNSEARCH_SEARCH_ATTEMPTS")?
                        .or(profile.search_attempts)
                        .unwrap_or(3),
                )
                .unwrap_or(3),
                1,
                5,
                "search_attempts",
            )? as u32,
            search_failover: env_parse(&env, "OWNSEARCH_SEARCH_FAILOVER")?
                .or(profile.search_failover)
                .unwrap_or(true),
            round_robin: env_parse(&env, "OWNSEARCH_ROUND_ROBIN")?
                .or(profile.round_robin)
                .unwrap_or(true),
            result_cap: bounded_usize(
                env_parse(&env, "OWNSEARCH_RESULT_CAP")?
                    .or(profile.result_cap)
                    .unwrap_or(25),
                1,
                100,
                "result_cap",
            )?,
            search_max_concurrent: bounded_usize(
                env_parse(&env, "OWNSEARCH_SEARCH_MAX_CONCURRENT")?
                    .or(profile.search_max_concurrent)
                    .unwrap_or(8),
                1,
                64,
                "search_max_concurrent",
            )?,
            search_queue_limit: bounded_usize(
                env_parse(&env, "OWNSEARCH_SEARCH_QUEUE_LIMIT")?
                    .or(profile.search_queue_limit)
                    .unwrap_or(64),
                1,
                1_024,
                "search_queue_limit",
            )?,
            search_queue_wait_secs: bounded_u64(
                env_parse(&env, "OWNSEARCH_SEARCH_QUEUE_WAIT_SECS")?
                    .or(profile.search_queue_wait_secs)
                    .unwrap_or(30),
                1,
                300,
                "search_queue_wait_secs",
            )?,
            preset: profile.preset.unwrap_or_default(),
            cache_ttl_secs: overrides
                .cache_ttl_secs
                .or(env_parse(&env, "OWNSEARCH_CACHE_TTL_SECS")?)
                .or(profile.cache_ttl_secs)
                .unwrap_or(300),
            cache_max_bytes,
            ai_db_mode,
            ai_db_max_changes_per_call: bounded_usize(
                env_parse(&env, "OWNSEARCH_AI_DB_MAX_CHANGES_PER_CALL")?
                    .or(profile.ai_db_max_changes_per_call)
                    .unwrap_or(5),
                1,
                50,
                "ai_db_max_changes_per_call",
            )?,
            ai_db_confirm_ttl_secs: bounded_u64(
                env_parse(&env, "OWNSEARCH_AI_DB_CONFIRM_TTL_SECS")?
                    .or(profile.ai_db_confirm_ttl_secs)
                    .unwrap_or(120),
                10,
                3_600,
                "ai_db_confirm_ttl_secs",
            )?,
            ai_db_min_interval_secs: env_parse::<u64>(&env, "OWNSEARCH_AI_DB_MIN_INTERVAL_SECS")?
                .or(profile.ai_db_min_interval_secs)
                .unwrap_or(2)
                .min(60),
            keep_window_secs: match non_empty_env(&env, "OWNSEARCH_KEEP_WINDOW")
                .or(profile.keep_window)
            {
                Some(raw) => parse_keep_window(&raw)?,
                None => DEFAULT_RETENTION_SECS,
            },
            allow_private_instances: env_parse(&env, "OWNSEARCH_ALLOW_PRIVATE_INSTANCES")?
                .or(profile.allow_private_instances)
                .unwrap_or(false),
            capsolver_key,
            capsolver_timeout_secs,
            proxy,
            maintenance_interval_secs: bounded_u64(
                env_parse(&env, "OWNSEARCH_MAINTENANCE_INTERVAL_SECS")?
                    .or(profile.maintenance_interval_secs)
                    .unwrap_or(3_600),
                60,
                86_400,
                "maintenance_interval_secs",
            )?,
        })
    }

    pub fn http(&self) -> bool {
        self.http
    }

    pub fn bind(&self) -> &str {
        &self.bind
    }

    pub fn bearer_tokens(&self) -> &[SecretString] {
        &self.bearer_tokens
    }

    pub fn bearer_token_error(&self) -> Option<&str> {
        self.bearer_token_error.as_deref()
    }

    pub fn allowed_hosts(&self) -> Option<&[String]> {
        self.allowed_hosts.as_deref()
    }

    pub fn allowed_origins(&self) -> Option<&[String]> {
        self.allowed_origins.as_deref()
    }

    pub fn profile_path(&self) -> Option<&Path> {
        self.profile_path.as_deref()
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn database_path(&self) -> PathBuf {
        self.state_dir.join("instances.db")
    }

    pub fn snapshot_path(&self) -> PathBuf {
        self.state_dir.join("instances.snapshot.json")
    }

    pub fn events_path(&self) -> PathBuf {
        self.state_dir.join("instances.events.jsonl")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.state_dir.join("backups")
    }

    pub fn ledger_dir(&self) -> &Path {
        &self.ledger_dir
    }

    pub fn ledger_path(&self) -> PathBuf {
        self.ledger_dir.join("calls.jsonl")
    }

    pub fn discovery_url(&self) -> &str {
        &self.discovery_url
    }

    pub fn discovery_refresh_secs(&self) -> u64 {
        self.discovery_refresh_secs
    }

    pub fn offline(&self) -> bool {
        self.offline
    }

    pub fn scan_concurrency(&self) -> usize {
        self.scan_concurrency
    }

    pub fn scan_jitter_ms(&self) -> u64 {
        self.scan_jitter_ms
    }

    pub fn probe_timeout_secs(&self) -> u64 {
        self.probe_timeout_secs
    }

    pub fn scan_deadline_secs(&self) -> u64 {
        self.scan_deadline_secs
    }

    pub fn fetch_deadline_secs(&self) -> u64 {
        self.fetch_deadline_secs
    }

    pub fn browser(&self) -> bool {
        self.browser
    }

    pub fn browser_executable(&self) -> Option<&str> {
        self.browser_executable.as_deref()
    }

    pub fn browser_idle_secs(&self) -> u64 {
        self.browser_idle_secs
    }

    pub fn browser_no_sandbox(&self) -> bool {
        self.browser_no_sandbox
    }

    pub fn browser_watchdog_secs(&self) -> u64 {
        self.browser_watchdog_secs
    }

    pub fn browser_max_lifetime_secs(&self) -> u64 {
        self.browser_max_lifetime_secs
    }

    pub fn watchdog_secs(&self) -> u64 {
        self.watchdog_secs
    }

    pub fn scan_min_interval_secs(&self) -> u64 {
        self.scan_min_interval_secs
    }

    pub fn request_timeout_secs(&self) -> u64 {
        self.request_timeout_secs
    }

    pub fn connect_timeout_secs(&self) -> u64 {
        self.connect_timeout_secs
    }

    pub fn max_upstream_bytes(&self) -> usize {
        self.max_upstream_bytes
    }

    pub fn cooldown_secs(&self) -> u64 {
        self.cooldown_secs
    }

    pub fn probation_failures(&self) -> u32 {
        self.probation_failures
    }

    pub fn quarantine_failures(&self) -> u32 {
        self.quarantine_failures
    }

    pub fn promote_successes(&self) -> u32 {
        self.promote_successes
    }

    pub fn fleet_min(&self) -> usize {
        self.fleet_min
    }

    pub fn fleet_max(&self) -> usize {
        self.fleet_max
    }

    pub fn max_per_network(&self) -> usize {
        self.max_per_network
    }

    pub fn html_fallback(&self) -> bool {
        self.html_fallback
    }

    pub fn google_required(&self) -> bool {
        self.google_required
    }

    pub fn cert_min_days(&self) -> i64 {
        self.cert_min_days
    }

    pub fn search_deadline_secs(&self) -> u64 {
        self.search_deadline_secs
    }

    pub fn exhaustive_max(&self) -> usize {
        self.exhaustive_max
    }

    pub fn search_attempts(&self) -> u32 {
        self.search_attempts
    }

    pub fn search_failover(&self) -> bool {
        self.search_failover
    }

    pub fn round_robin(&self) -> bool {
        self.round_robin
    }

    pub fn result_cap(&self) -> usize {
        self.result_cap
    }

    pub fn search_max_concurrent(&self) -> usize {
        self.search_max_concurrent
    }

    pub fn search_queue_limit(&self) -> usize {
        self.search_queue_limit
    }

    pub fn search_queue_wait_secs(&self) -> u64 {
        self.search_queue_wait_secs
    }

    pub fn preset(&self) -> Preset {
        self.preset
    }

    pub fn cache_ttl_secs(&self) -> u64 {
        self.cache_ttl_secs
    }

    pub fn cache_max_bytes(&self) -> usize {
        self.cache_max_bytes
    }

    pub fn ai_db_mode(&self) -> AiDbMode {
        self.ai_db_mode
    }

    pub fn ai_db_max_changes_per_call(&self) -> usize {
        self.ai_db_max_changes_per_call
    }

    pub fn ai_db_confirm_ttl_secs(&self) -> u64 {
        self.ai_db_confirm_ttl_secs
    }

    pub fn ai_db_min_interval_secs(&self) -> u64 {
        self.ai_db_min_interval_secs
    }

    pub fn keep_window_secs(&self) -> u64 {
        self.keep_window_secs
    }

    pub fn allow_private_instances(&self) -> bool {
        self.allow_private_instances
    }

    pub fn capsolver_key(&self) -> Option<&SecretString> {
        self.capsolver_key.as_ref()
    }

    pub fn capsolver_timeout_secs(&self) -> u64 {
        self.capsolver_timeout_secs
    }

    pub fn proxy(&self) -> Option<&str> {
        self.proxy.as_deref()
    }

    pub fn maintenance_interval_secs(&self) -> u64 {
        self.maintenance_interval_secs
    }

    pub fn user_agent(&self) -> String {
        format!("{DEFAULT_USER_AGENT} v{}", crate::VERSION)
    }

    pub fn describe(&self) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = vec![
            (
                "profile".to_owned(),
                path_text(self.profile_path.as_deref()),
            ),
            ("state_dir".to_owned(), self.state_dir.display().to_string()),
            (
                "database".to_owned(),
                self.database_path().display().to_string(),
            ),
            (
                "ledger_dir".to_owned(),
                self.ledger_dir.display().to_string(),
            ),
            ("discovery_url".to_owned(), self.discovery_url.clone()),
            (
                "discovery_refresh_secs".to_owned(),
                self.discovery_refresh_secs.to_string(),
            ),
            ("http".to_owned(), self.http.to_string()),
            ("bind".to_owned(), self.bind.clone()),
            (
                "bearer_tokens".to_owned(),
                format!("{} configured", self.bearer_tokens.len()),
            ),
            (
                "allowed_hosts".to_owned(),
                list_text(self.allowed_hosts.as_deref()),
            ),
            (
                "allowed_origins".to_owned(),
                list_text(self.allowed_origins.as_deref()),
            ),
            ("offline".to_owned(), self.offline.to_string()),
            (
                "scan_concurrency".to_owned(),
                self.scan_concurrency.to_string(),
            ),
            ("scan_jitter_ms".to_owned(), self.scan_jitter_ms.to_string()),
            (
                "probe_timeout_secs".to_owned(),
                self.probe_timeout_secs.to_string(),
            ),
            (
                "scan_deadline_secs".to_owned(),
                self.scan_deadline_secs.to_string(),
            ),
            (
                "fetch_deadline_secs".to_owned(),
                self.fetch_deadline_secs.to_string(),
            ),
            ("browser".to_owned(), self.browser.to_string()),
            (
                "browser_executable".to_owned(),
                self.browser_executable.clone().unwrap_or_default(),
            ),
            (
                "browser_idle_secs".to_owned(),
                self.browser_idle_secs.to_string(),
            ),
            (
                "browser_no_sandbox".to_owned(),
                self.browser_no_sandbox.to_string(),
            ),
            (
                "browser_watchdog_secs".to_owned(),
                self.browser_watchdog_secs.to_string(),
            ),
            (
                "browser_max_lifetime_secs".to_owned(),
                self.browser_max_lifetime_secs.to_string(),
            ),
            ("watchdog_secs".to_owned(), self.watchdog_secs.to_string()),
            (
                "scan_min_interval_secs".to_owned(),
                self.scan_min_interval_secs.to_string(),
            ),
            (
                "request_timeout_secs".to_owned(),
                self.request_timeout_secs.to_string(),
            ),
            (
                "max_upstream_bytes".to_owned(),
                self.max_upstream_bytes.to_string(),
            ),
            ("cooldown_secs".to_owned(), self.cooldown_secs.to_string()),
            (
                "probation_failures".to_owned(),
                self.probation_failures.to_string(),
            ),
            (
                "quarantine_failures".to_owned(),
                self.quarantine_failures.to_string(),
            ),
            (
                "promote_successes".to_owned(),
                self.promote_successes.to_string(),
            ),
            ("fleet_min".to_owned(), self.fleet_min.to_string()),
            ("fleet_max".to_owned(), self.fleet_max.to_string()),
            (
                "max_per_network".to_owned(),
                self.max_per_network.to_string(),
            ),
            ("html_fallback".to_owned(), self.html_fallback.to_string()),
            (
                "google_required".to_owned(),
                self.google_required.to_string(),
            ),
            ("cert_min_days".to_owned(), self.cert_min_days.to_string()),
            (
                "search_deadline_secs".to_owned(),
                self.search_deadline_secs.to_string(),
            ),
            ("exhaustive_max".to_owned(), self.exhaustive_max.to_string()),
            (
                "search_attempts".to_owned(),
                self.search_attempts.to_string(),
            ),
            (
                "search_failover".to_owned(),
                self.search_failover.to_string(),
            ),
            ("round_robin".to_owned(), self.round_robin.to_string()),
            ("result_cap".to_owned(), self.result_cap.to_string()),
            (
                "search_max_concurrent".to_owned(),
                self.search_max_concurrent.to_string(),
            ),
            (
                "search_queue_limit".to_owned(),
                self.search_queue_limit.to_string(),
            ),
            (
                "search_queue_wait_secs".to_owned(),
                self.search_queue_wait_secs.to_string(),
            ),
            ("preset".to_owned(), self.preset.as_str().to_owned()),
            ("cache_ttl_secs".to_owned(), self.cache_ttl_secs.to_string()),
            ("ai_db_mode".to_owned(), self.ai_db_mode.as_str().to_owned()),
            (
                "ai_db_max_changes_per_call".to_owned(),
                self.ai_db_max_changes_per_call.to_string(),
            ),
            (
                "ai_db_confirm_ttl_secs".to_owned(),
                self.ai_db_confirm_ttl_secs.to_string(),
            ),
            (
                "allow_private_instances".to_owned(),
                self.allow_private_instances.to_string(),
            ),
            (
                "capsolver".to_owned(),
                if self.capsolver_key.is_some() {
                    "configured".to_owned()
                } else {
                    "disabled".to_owned()
                },
            ),
            (
                "capsolver_timeout_secs".to_owned(),
                self.capsolver_timeout_secs.to_string(),
            ),
            (
                "proxy".to_owned(),
                if self.proxy.is_some() {
                    "configured".to_owned()
                } else {
                    "disabled".to_owned()
                },
            ),
            (
                "keep_window".to_owned(),
                format_keep_window(self.keep_window_secs),
            ),
            (
                "maintenance_interval_secs".to_owned(),
                self.maintenance_interval_secs.to_string(),
            ),
        ];
        if let Some(error) = &self.bearer_token_error {
            rows.push(("bearer_token_error".to_owned(), mask_text(error)));
        }
        rows
    }
}

fn path_text(path: Option<&Path>) -> String {
    match path {
        Some(path) => path.display().to_string(),
        None => "<none>".to_owned(),
    }
}

fn list_text(values: Option<&[String]>) -> String {
    match values {
        Some(values) if !values.is_empty() => values.join(","),
        _ => "<none>".to_owned(),
    }
}

fn mask_text(text: &str) -> String {
    let shown: String = text.chars().take(120).collect();
    crate::sanitize::sanitize(&shown)
}

pub fn parse_keep_window(raw: &str) -> Result<u64> {
    let trimmed = raw.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Err(Error::Config("the keep window is empty".into()));
    }
    let (number, multiplier) = match trimmed.chars().last() {
        Some('s') => (&trimmed[..trimmed.len() - 1], 1),
        Some('m') => (&trimmed[..trimmed.len() - 1], 60),
        Some('h') => (&trimmed[..trimmed.len() - 1], 3_600),
        Some('d') => (&trimmed[..trimmed.len() - 1], 86_400),
        _ => (trimmed.as_str(), 1),
    };
    if number.is_empty() || !number.chars().all(|character| character.is_ascii_digit()) {
        return Err(Error::Config(format!(
            "the keep window {raw:?} is not a number with an optional s, m, h, or d suffix"
        )));
    }
    let quantity = number
        .parse::<u64>()
        .map_err(|_| Error::Config(format!("the keep window {raw:?} is too large")))?;
    let secs = quantity.saturating_mul(multiplier);
    if !(MIN_RETENTION_SECS..=MAX_RETENTION_SECS).contains(&secs) {
        return Err(Error::Config(format!(
            "the keep window {raw:?} is outside 1 hour to 90 days"
        )));
    }
    Ok(secs)
}

pub fn format_keep_window(secs: u64) -> String {
    if secs.is_multiple_of(86_400) {
        format!("{}d", secs / 86_400)
    } else if secs.is_multiple_of(3_600) {
        format!("{}h", secs / 3_600)
    } else {
        format!("{secs}s")
    }
}

fn cert_min_days(env: &impl Fn(&str) -> Option<String>, profile: Option<i64>) -> Result<u64> {
    if let Some(value) = env_parse::<u64>(env, "OWNSEARCH_CERT_MIN_DAYS")? {
        return Ok(value);
    }
    match profile {
        Some(value) => u64::try_from(value)
            .map_err(|_| Error::Config(format!("cert_min_days is {value}; use zero or more"))),
        None => Ok(14),
    }
}

fn bounded_u64(value: u64, low: u64, high: u64, name: &str) -> Result<u64> {
    if (low..=high).contains(&value) {
        Ok(value)
    } else {
        Err(Error::Config(format!(
            "{name} is {value}; use {low} to {high}"
        )))
    }
}

fn bounded_usize(value: usize, low: usize, high: usize, name: &str) -> Result<usize> {
    if (low..=high).contains(&value) {
        Ok(value)
    } else {
        Err(Error::Config(format!(
            "{name} is {value}; use {low} to {high}"
        )))
    }
}

fn env_parse<T: std::str::FromStr>(
    env: &impl Fn(&str) -> Option<String>,
    name: &str,
) -> Result<Option<T>> {
    let Some(raw) = env(name) else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed.parse::<T>().map(Some).map_err(|_| {
        Error::Config(format!(
            "{name} is set to {trimmed:?}, which is not a valid {}",
            std::any::type_name::<T>()
        ))
    })
}

fn bearer_tokens_from(
    env: &impl Fn(&str) -> Option<String>,
) -> (Vec<SecretString>, Option<String>) {
    if let Some(raw) = non_empty_env(env, "OWNSEARCH_BEARER_TOKENS") {
        return (split_tokens(&raw), None);
    }
    match keychain_get() {
        Ok(Some(raw)) => (split_tokens(raw.expose_secret()), None),
        Ok(None) => (Vec::new(), None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    }
}

fn split_tokens(raw: &str) -> Vec<SecretString> {
    raw.split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(SecretString::from)
        .collect()
}

fn allowed_list(
    env: &impl Fn(&str) -> Option<String>,
    name: &str,
    profile: Option<Vec<String>>,
) -> Result<Option<Vec<String>>> {
    let from_env = non_empty_env(env, name)
        .map(|raw| parse_list(&raw))
        .filter(|values| !values.is_empty());
    match from_env {
        Some(values) => Some(clean_list(values)),
        None => profile.map(clean_list),
    }
    .transpose()
}

fn discovery_url_from(
    env: &impl Fn(&str) -> Option<String>,
    profile: Option<String>,
) -> Result<String> {
    let raw = non_empty_env(env, "OWNSEARCH_DISCOVERY_URL")
        .or(profile)
        .unwrap_or_else(|| DEFAULT_DISCOVERY_URL.to_owned());
    let parsed = url::Url::parse(&raw)
        .map_err(|error| Error::Config(format!("discovery_url {raw:?} does not parse: {error}")))?;
    let loopback = parsed.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if parsed.scheme() != "https" && !loopback {
        return Err(Error::Config(format!(
            "discovery_url {raw:?} must use https"
        )));
    }
    Ok(raw)
}

fn non_empty_env(env: &impl Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    env(name)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn parse_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn clean_list(values: Vec<String>) -> Result<Vec<String>> {
    let cleaned: Vec<String> = values
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect();
    if cleaned.is_empty() {
        return Err(Error::Config(
            "a list setting holds no value after trimming; name at least one value or remove the key"
                .to_owned(),
        ));
    }
    Ok(cleaned)
}

const fn profile_version() -> u32 {
    1
}

fn read_profile_if_present(path: &Path, explicit: bool) -> Result<Profile> {
    let present = match path.try_exists() {
        Ok(found) => found,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(Error::Config(format!(
                "cannot check profile {}: {error}",
                path.display()
            )));
        }
    };
    let present = present || std::fs::symlink_metadata(path).is_ok();
    if !present {
        if explicit {
            return Err(Error::Config(format!(
                "profile {} does not exist; fix --profile or OWNSEARCH_PROFILE",
                path.display()
            )));
        }
        return Ok(Profile::default());
    }
    read_profile(path)
}

fn read_profile(path: &Path) -> Result<Profile> {
    use std::io::Read as _;

    let metadata = std::fs::metadata(path).map_err(|error| {
        Error::Config(format!(
            "cannot inspect profile {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(Error::Config(format!(
            "profile {} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() > MAX_PROFILE_BYTES {
        return Err(Error::Config(format!(
            "profile {} is {} bytes, above the {MAX_PROFILE_BYTES} byte cap",
            path.display(),
            metadata.len()
        )));
    }
    let file = std::fs::File::open(path).map_err(|error| {
        Error::Config(format!("cannot read profile {}: {error}", path.display()))
    })?;
    let mut raw = String::new();
    file.take(MAX_PROFILE_BYTES.saturating_add(1))
        .read_to_string(&mut raw)
        .map_err(|error| {
            Error::Config(format!("cannot read profile {}: {error}", path.display()))
        })?;
    if raw.len() as u64 > MAX_PROFILE_BYTES {
        return Err(Error::Config(format!(
            "profile {} grew past the {MAX_PROFILE_BYTES} byte cap while it was read",
            path.display()
        )));
    }
    let table: toml::Table = toml::from_str(&raw)
        .map_err(|error| Error::Config(format!("profile {}: {error}", path.display())))?;
    if let Some(toml::Value::Integer(version)) = table.get("version")
        && *version > 1
    {
        return Err(Error::Config(format!(
            "{} is version {version}, newer than this binary supports (1)",
            path.display()
        )));
    }
    toml::from_str::<Profile>(&raw)
        .map_err(|error| Error::Config(format!("profile {}: {error}", path.display())))
}

pub fn default_config_dir() -> PathBuf {
    config_dir_from(&read_env_value).unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_data_dir() -> PathBuf {
    data_dir_from(&read_env_value).unwrap_or_else(|| PathBuf::from("."))
}

fn config_dir_from(env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    base_dir(env, "XDG_CONFIG_HOME", ".config", "APPDATA").map(|base| base.join("ownsearch"))
}

fn data_dir_from(env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    base_dir(env, "XDG_DATA_HOME", ".local/share", "LOCALAPPDATA")
        .map(|base| base.join("ownsearch"))
}

fn state_dir_from(env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    base_dir(env, "XDG_STATE_HOME", ".local/state", "LOCALAPPDATA")
        .map(|base| base.join("ownsearch"))
}

fn read_env_value(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn base_dir(
    env: &impl Fn(&str) -> Option<String>,
    variable: &str,
    fallback: &str,
    windows_variable: &str,
) -> Option<PathBuf> {
    if let Some(path) = env(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Some(path);
    }
    if let Some(home) = env("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Some(home.join(fallback));
    }
    #[cfg(windows)]
    if let Some(path) = env(windows_variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Some(path);
    }
    #[cfg(not(windows))]
    let _ = windows_variable;
    None
}

pub fn keychain_get() -> Result<Option<SecretString>> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|error| Error::Config(format!("keychain entry: {error}")))?;
    match entry.get_password() {
        Ok(secret) => Ok(Some(SecretString::from(secret))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error @ (keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_))) => {
            tracing::debug!(%error, "the keychain is not available; no stored token was read");
            Ok(None)
        }
        Err(error) => Err(Error::Config(format!("keychain read: {error}"))),
    }
}

pub fn keychain_set(secret: &SecretString) -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|error| Error::Config(format!("keychain entry: {error}")))?;
    entry
        .set_password(secret.expose_secret())
        .map_err(|error| Error::Config(format!("keychain write: {error}")))
}

pub fn keychain_delete() -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|error| Error::Config(format!("keychain entry: {error}")))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(Error::Config(format!("keychain delete: {error}"))),
    }
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Settings")
            .field("http", &self.http)
            .field("bind", &self.bind)
            .field("bearer_tokens", &self.bearer_tokens.len())
            .field("state_dir", &self.state_dir)
            .field("ledger_dir", &self.ledger_dir)
            .field("discovery_url", &self.discovery_url)
            .field("offline", &self.offline)
            .field("fleet_min", &self.fleet_min)
            .field("fleet_max", &self.fleet_max)
            .field("html_fallback", &self.html_fallback)
            .field("ai_db_mode", &self.ai_db_mode)
            .field("profile_path", &self.profile_path)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn environment(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn load_with(pairs: &[(&str, &str)]) -> Result<Settings> {
        let map = environment(pairs);
        Settings::load_with(Overrides::default(), |name| map.get(name).cloned())
    }

    #[test]
    fn the_defaults_match_the_documented_policy() {
        let settings = load_with(&[
            ("HOME", "/home/tester"),
            ("XDG_STATE_HOME", "/home/tester/.local/state"),
        ])
        .expect("settings");
        assert_eq!(settings.fleet_min(), 6);
        assert_eq!(settings.fleet_max(), 12);
        assert_eq!(settings.max_per_network(), 2);
        assert_eq!(settings.discovery_refresh_secs(), 21_600);
        assert!(settings.html_fallback());
        assert!(!settings.google_required());
        assert_eq!(settings.ai_db_mode(), AiDbMode::ReadWrite);
        assert_eq!(settings.search_deadline_secs(), 30);
        assert_eq!(settings.exhaustive_max(), 16);
        assert_eq!(settings.result_cap(), 25);
        assert_eq!(settings.cache_ttl_secs(), 300);
        assert_eq!(settings.search_max_concurrent(), 8);
        assert_eq!(settings.search_queue_limit(), 64);
        assert_eq!(
            settings.database_path(),
            PathBuf::from("/home/tester/.local/state/ownsearch/instances.db")
        );
        assert!(settings.capsolver_key().is_none());
        assert_eq!(settings.capsolver_timeout_secs(), 45);
        assert!(settings.proxy().is_none());
    }

    #[test]
    fn the_proxy_comes_from_the_environment_and_never_reaches_the_display() {
        let settings = load_with(&[
            ("HOME", "/home/tester"),
            ("OWNSEARCH_PROXY", "socks5h://user:secret@127.0.0.1:1080"),
        ])
        .expect("settings");
        assert_eq!(
            settings.proxy(),
            Some("socks5h://user:secret@127.0.0.1:1080")
        );
        let rows = settings.describe();
        let shown = rows
            .iter()
            .find(|(name, _)| name == "proxy")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        assert_eq!(shown, "configured");
        assert!(
            !rows.iter().any(|(_, value)| value.contains("secret@")),
            "the proxy credentials must never reach the settings display"
        );
    }

    #[test]
    fn the_capsolver_key_comes_from_the_environment_and_never_reaches_the_display() {
        let settings = load_with(&[
            ("HOME", "/home/tester"),
            ("OWNSEARCH_CAPSOLVER_KEY", "cap-key-value"),
            ("OWNSEARCH_CAPSOLVER_TIMEOUT_SECS", "90"),
        ])
        .expect("settings");
        assert!(settings.capsolver_key().is_some());
        assert_eq!(settings.capsolver_timeout_secs(), 90);
        let rows = settings.describe();
        let shown = rows
            .iter()
            .find(|(name, _)| name == "capsolver")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        assert_eq!(shown, "configured");
        assert!(
            !rows
                .iter()
                .any(|(_, value)| value.contains("cap-key-value")),
            "the key must never reach the settings display"
        );
        let error = load_with(&[
            ("HOME", "/home/tester"),
            ("OWNSEARCH_CAPSOLVER_TIMEOUT_SECS", "5"),
        ])
        .expect_err("out of range");
        assert!(error.to_string().contains("capsolver_timeout_secs"));
    }

    #[test]
    fn environment_values_win_over_the_profile() {
        let map = environment(&[
            ("HOME", "/home/tester"),
            ("OWNSEARCH_FLEET_MAX", "8"),
            ("OWNSEARCH_HTML_FALLBACK", "true"),
        ]);
        let settings = Settings::load_with(Overrides::default(), |name| map.get(name).cloned())
            .expect("settings");
        assert_eq!(settings.fleet_max(), 8);
        assert!(settings.html_fallback());
    }

    #[test]
    fn an_unknown_profile_key_is_refused() {
        let dir = std::env::temp_dir().join(format!("ownsearch-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let profile = dir.join("profile.toml");
        std::fs::write(&profile, "fleet_max = 8\nmystery = 1\n").expect("write");
        let error = Settings::load_with(
            Overrides {
                profile: Some(profile),
                state_dir: Some(dir.clone()),
                ..Overrides::default()
            },
            |name| match name {
                "HOME" => Some("/home/tester".to_owned()),
                _ => None,
            },
        )
        .expect_err("unknown key");
        assert!(error.to_string().contains("mystery"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_future_profile_version_is_refused() {
        let dir = std::env::temp_dir().join(format!("ownsearch-profile-v-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let profile = dir.join("profile.toml");
        std::fs::write(&profile, "version = 9\n").expect("write");
        let error = Settings::load_with(
            Overrides {
                profile: Some(profile),
                state_dir: Some(dir.clone()),
                ..Overrides::default()
            },
            |name| match name {
                "HOME" => Some("/home/tester".to_owned()),
                _ => None,
            },
        )
        .expect_err("future version");
        assert!(error.to_string().contains("newer than this binary"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_out_of_range_value_names_its_bounds() {
        let error = load_with(&[("HOME", "/home/tester"), ("OWNSEARCH_FLEET_MAX", "999")])
            .expect_err("range");
        assert!(error.to_string().contains("fleet_max"));
    }

    #[test]
    fn a_minimum_above_the_maximum_is_refused() {
        let error = load_with(&[
            ("HOME", "/home/tester"),
            ("OWNSEARCH_FLEET_MIN", "9"),
            ("OWNSEARCH_FLEET_MAX", "8"),
        ])
        .expect_err("order");
        assert!(error.to_string().contains("above fleet_max"));
    }

    #[test]
    fn the_watchdog_switches_off_at_zero_and_reports_its_bounds() {
        let disabled = load_with(&[("HOME", "/home/tester"), ("OWNSEARCH_WATCHDOG_SECS", "0")])
            .expect("settings");
        assert_eq!(disabled.watchdog_secs(), 0);
        let enabled = load_with(&[("HOME", "/home/tester"), ("OWNSEARCH_WATCHDOG_SECS", "45")])
            .expect("settings");
        assert_eq!(enabled.watchdog_secs(), 45);
        let error = load_with(&[
            ("HOME", "/home/tester"),
            ("OWNSEARCH_BROWSER_WATCHDOG_SECS", "1"),
        ])
        .expect_err("range");
        assert!(error.to_string().contains("browser_watchdog_secs"));
    }

    #[test]
    fn the_write_mode_parses_from_the_environment() {
        let settings = load_with(&[
            ("HOME", "/home/tester"),
            ("OWNSEARCH_AI_DB_MODE", "read-write"),
        ])
        .expect("settings");
        assert_eq!(settings.ai_db_mode(), AiDbMode::ReadWrite);
        let error = load_with(&[("HOME", "/home/tester"), ("OWNSEARCH_AI_DB_MODE", "write")])
            .expect_err("mode");
        assert!(error.to_string().contains("read-write"));
    }

    #[test]
    fn a_masked_value_is_cut_to_120_characters() {
        let long = "a".repeat(200);
        assert_eq!(mask_text(&long).chars().count(), 120);
        assert_eq!(mask_text("short"), "short");
        assert_eq!(mask_text("a\u{200B}b"), "ab");
    }

    #[test]
    fn the_keep_window_accepts_hours_and_days_and_rejects_bad_values() {
        assert_eq!(parse_keep_window("3d").expect("days"), 259_200);
        assert_eq!(parse_keep_window("2h").expect("hours"), 7_200);
        assert_eq!(parse_keep_window("3600").expect("seconds"), 3_600);
        assert_eq!(parse_keep_window("1D").expect("case"), 86_400);
        assert!(parse_keep_window("0").is_err());
        assert!(parse_keep_window("59m").is_err());
        assert!(parse_keep_window("91d").is_err());
        assert!(parse_keep_window("-2h").is_err());
        assert!(parse_keep_window("abc").is_err());
        assert_eq!(format_keep_window(259_200), "3d");
        assert_eq!(format_keep_window(7_200), "2h");
    }
}
