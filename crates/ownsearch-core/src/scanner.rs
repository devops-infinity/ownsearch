use std::sync::Arc;

use rusqlite::Connection;
use serde_json::json;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::context::ToolContext;
use crate::db::{
    CapabilityRecord, EventRecord, ProbeRecord, SOURCE_ID_SEARX_SPACE, ScanTarget, ScanUpdate,
    Store, TlsRecord,
};
use crate::error::{Error, Result};
use crate::instances::{DiscoveredInstance, DiscoveryDocument};
use crate::quality::{
    Evidence, FleetCandidate, FleetSelection, HysteresisInput, assess, next_status, select_fleet,
    version_age_days,
};
use crate::response::{ScanOutput, StatusChange};
use crate::searxng::{ProbeReport, SearchQuery, config_client, search_html, search_json};
use crate::time::{jitter, now_rfc3339, now_unix, rfc3339};
use crate::tls_probe::TlsReport;
use crate::types::{InstanceStatus, ProbeKind, ProbeOutcome};

pub(crate) const PROBE_QUERY: &str = "ownsearch connectivity probe";
const SUCCESS_NEXT_SCAN_SECS: i64 = 3_600;
const FAILURE_NEXT_SCAN_SECS: i64 = 900;
const PROBE_STEP_COUNT: u64 = 5;
const QUARANTINE_NEXT_SCAN_SECS: i64 = 21_600;
const SCAN_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(120);
const RESOLVE_CONCURRENCY: usize = 8;
const RESOLVE_PASS_SECS: u64 = 60;

static DISCOVERY_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub dry_run: bool,
    pub instance_id: Option<i64>,
    pub limit: usize,
    pub force_discovery: bool,
    pub probe_search: bool,
    pub probe_google: bool,
    pub force: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            dry_run: true,
            instance_id: None,
            limit: 200,
            force_discovery: false,
            probe_search: true,
            probe_google: true,
            force: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DiscoveryOutcome {
    pub run_id: Option<String>,
    pub discovered: usize,
    pub added: usize,
    pub changed: usize,
    pub missing: usize,
    pub skipped: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct InstanceProbe {
    instance_id: i64,
    url: String,
    previous: InstanceStatus,
    tls: TlsReport,
    config_outcome: ProbeOutcome,
    config: Option<config_client::ConfigDocument>,
    config_status: Option<u16>,
    json_outcome: ProbeOutcome,
    json_report: Option<ProbeReport<search_json::JsonSearch>>,
    html_outcome: Option<ProbeOutcome>,
    html_report: Option<ProbeReport<search_html::HtmlSearch>>,
    google_outcome: Option<ProbeOutcome>,
    google_report: Option<ProbeReport<search_json::JsonSearch>>,
    rate_limited: bool,
    blocked: bool,
    parser_drift: bool,
    successes: i64,
    failures: i64,
    score: f64,
    reasons: Vec<String>,
    hard_gate_passed: bool,
    next_status: InstanceStatus,
    next_scan_at: Option<String>,
    cooldown_until: Option<String>,
    version: Option<String>,
}

impl InstanceProbe {
    fn succeeded(&self) -> bool {
        self.hard_gate_passed
            && (self.json_outcome.is_success() || self.html_outcome == Some(ProbeOutcome::Success))
    }
}

pub async fn refresh_discovery(
    ctx: &ToolContext,
    force: bool,
    cancellation: &CancellationToken,
) -> Result<DiscoveryOutcome> {
    if ctx.settings.offline() {
        return Ok(DiscoveryOutcome {
            skipped: true,
            ..DiscoveryOutcome::default()
        });
    }
    let _gate = DISCOVERY_GATE.lock().await;
    let last_run = {
        let store = Arc::clone(&ctx.store);
        blocking_store(move || store.last_run_at(SOURCE_ID_SEARX_SPACE)).await?
    };
    if !force
        && let Some(last) = last_run
        && now_unix().saturating_sub(last)
            < i64::try_from(ctx.settings.discovery_refresh_secs()).unwrap_or(i64::MAX)
    {
        return Ok(DiscoveryOutcome {
            skipped: true,
            ..DiscoveryOutcome::default()
        });
    }
    {
        let store = Arc::clone(&ctx.store);
        let endpoint = ctx.settings.discovery_url().to_owned();
        blocking_store(move || {
            store.ensure_source(SOURCE_ID_SEARX_SPACE, "searx.space", &endpoint)
        })
        .await?;
    }
    let run_id = {
        let store = Arc::clone(&ctx.store);
        blocking_store(move || store.begin_run(SOURCE_ID_SEARX_SPACE, crate::VERSION)).await?
    };
    let url = Url::parse(ctx.settings.discovery_url())
        .map_err(|error| Error::Config(format!("discovery_url: {error}")))?;
    let fetched = match ctx.client.get(&url, "application/json", cancellation).await {
        Ok(fetched) => fetched,
        Err(error) => {
            let message = error.to_string();
            let code = error.code().to_owned();
            let store = Arc::clone(&ctx.store);
            let run = run_id.clone();
            let recorded = message.clone();
            blocking_store(move || {
                store.record_run_failure(&run, &code, &recorded)?;
                store.finish_run(&run, "error", (0, 0, 0))
            })
            .await?;
            return Ok(DiscoveryOutcome {
                run_id: Some(run_id),
                error: Some(message),
                ..DiscoveryOutcome::default()
            });
        }
    };
    if !fetched.is_success() || !fetched.is_json() {
        let message = format!(
            "searx.space answered HTTP {} with {}",
            fetched.status,
            fetched.content_type.as_deref().unwrap_or("no content type")
        );
        let store = Arc::clone(&ctx.store);
        let run = run_id.clone();
        let recorded = message.clone();
        blocking_store(move || {
            store.record_run_failure(&run, "ownsearch.upstream_failed", &recorded)?;
            store.finish_run(&run, "error", (0, 0, 0))
        })
        .await?;
        return Ok(DiscoveryOutcome {
            run_id: Some(run_id),
            error: Some(message),
            ..DiscoveryOutcome::default()
        });
    }
    let document = match DiscoveryDocument::parse(&fetched.body) {
        Ok(document) => document,
        Err(error) => {
            let message = error.to_string();
            let code = error.code().to_owned();
            let store = Arc::clone(&ctx.store);
            let run = run_id.clone();
            let recorded = message.clone();
            blocking_store(move || {
                store.record_run_failure(&run, &code, &recorded)?;
                store.finish_run(&run, "error", (0, 0, 0))
            })
            .await?;
            return Ok(DiscoveryOutcome {
                run_id: Some(run_id),
                error: Some(message),
                ..DiscoveryOutcome::default()
            });
        }
    };
    let digest = {
        use sha2::{Digest, Sha256};
        Sha256::digest(&fetched.body).to_vec()
    };
    {
        let store = Arc::clone(&ctx.store);
        let run = run_id.clone();
        let published_at = document.snapshot_at();
        let source_digest = digest.clone();
        blocking_store(move || store.record_run_source(&run, published_at, &source_digest)).await?;
    }
    let now = now_unix();
    let stamp = rfc3339(now);
    let loaded = public_records(ctx, document.to_discovered(now)).await;
    let source_len = loaded.source_len;
    let resolved = loaded.complete;
    let discovered = loaded.kept;
    let discovered_len = discovered.len();
    let (added, changed, missing) = {
        let store = Arc::clone(&ctx.store);
        let run = run_id.clone();
        let run_stamp = stamp.clone();
        blocking_store(move || {
            let (added, changed, present) = store.import_discovered(
                &discovered,
                SOURCE_ID_SEARX_SPACE,
                Some(&run),
                &run_stamp,
            )?;
            let known = usize::try_from(store.instance_count()?).unwrap_or(0);
            let plausible = resolved
                && source_len >= 20
                && (known == 0 || source_len.saturating_mul(2) >= known)
                && source_len <= MAX_DISCOVERY_RECORDS;
            let missing = if plausible {
                store.mark_missing(&present, &run, &run_stamp)?
            } else {
                tracing::warn!(
                    discovered = discovered_len,
                    known,
                    "the discovery document is too small to trust; no instance was marked missing"
                );
                0
            };
            store.finish_run(
                &run,
                "success",
                (u32_len(discovered_len), u32_len(changed), missing),
            )?;
            Ok((added, changed, missing))
        })
        .await?
    };
    {
        let store = Arc::clone(&ctx.store);
        let snapshot_path = ctx.settings.snapshot_path();
        let events_path = ctx.settings.events_path();
        let line = json!({
            "event": "discovery_run",
            "runId": run_id,
            "at": stamp,
            "discovered": discovered_len,
            "added": added,
            "changed": changed,
            "missing": missing,
        })
        .to_string();
        blocking_store(move || {
            if let Err(error) = store.write_snapshot(&snapshot_path) {
                tracing::warn!(%error, "the discovery snapshot was not written");
            }
            if let Err(error) = crate::fsutil::append_line(&events_path, &line) {
                tracing::warn!(%error, "the discovery event was not appended");
            }
            Ok(())
        })
        .await?;
    }
    Ok(DiscoveryOutcome {
        run_id: Some(run_id),
        discovered: discovered_len,
        added,
        changed,
        missing: usize::try_from(missing).unwrap_or(0),
        skipped: false,
        error: None,
    })
}

fn u32_len(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

async fn blocking_store<T, F>(work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Error::Io {
            context: "waiting for a storage task",
            source: std::io::Error::other(error),
        })?
}

#[derive(Default)]
struct ScanState {
    running: bool,
    finished_at: Option<std::time::Instant>,
}

static SCAN_STATES: std::sync::Mutex<std::collections::BTreeMap<std::path::PathBuf, ScanState>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

struct ScanGuard {
    database: std::path::PathBuf,
    _lock: Option<std::fs::File>,
    completed: bool,
}

impl ScanGuard {
    async fn claim(ctx: &ToolContext, force: bool) -> Result<Self> {
        let database = ctx.settings.database_path();
        let gap = std::time::Duration::from_secs(ctx.settings.scan_min_interval_secs());
        {
            let mut states = SCAN_STATES
                .lock()
                .map_err(|_| Error::State("the scan guard lock was poisoned".into()))?;
            let state = states.entry(database.clone()).or_default();
            if state.running {
                return Err(Error::PolicyRefused(
                    "another scan is already running on this state database; wait for it to finish"
                        .into(),
                ));
            }
            if !force
                && !gap.is_zero()
                && let Some(finished) = state.finished_at
                && finished.elapsed() < gap
            {
                return Err(Error::PolicyRefused(format!(
                    "a scan finished less than {} seconds ago; wait before scanning again",
                    gap.as_secs()
                )));
            }
        }
        let lock = Self::wait_for_lock(ctx, force).await?;
        {
            let mut states = SCAN_STATES
                .lock()
                .map_err(|_| Error::State("the scan guard lock was poisoned".into()))?;
            states.entry(database.clone()).or_default().running = true;
        }
        Ok(Self {
            database,
            _lock: Some(lock),
            completed: false,
        })
    }

    async fn wait_for_lock(ctx: &ToolContext, force: bool) -> Result<std::fs::File> {
        let path = ctx.settings.state_dir().join("scan");
        let wait = if force {
            SCAN_LOCK_WAIT
        } else {
            std::time::Duration::from_secs(5)
        };
        let deadline = std::time::Instant::now() + wait;
        loop {
            match crate::fsutil::try_file_lock(&path) {
                Ok(Some(file)) => return Ok(file),
                Ok(None) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                Ok(None) => {
                    return Err(Error::PolicyRefused(
                        "another session is scanning this state database; wait for it to finish"
                            .into(),
                    ));
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl ScanGuard {
    fn mark_completed(&mut self) {
        self.completed = true;
    }
}

impl Drop for ScanGuard {
    fn drop(&mut self) {
        if let Ok(mut states) = SCAN_STATES.lock()
            && let Some(state) = states.get_mut(&self.database)
        {
            state.running = false;
            if self.completed {
                state.finished_at = Some(std::time::Instant::now());
            }
        }
    }
}

pub async fn run_scan(
    ctx: &Arc<ToolContext>,
    options: &ScanOptions,
    cancellation: &CancellationToken,
) -> Result<ScanOutput> {
    let started = std::time::Instant::now();
    let mut guard = ScanGuard::claim(ctx, options.force).await?;
    if options.dry_run {
        let output = dry_run_scan(ctx, options, cancellation, started).await?;
        guard.mark_completed();
        return Ok(output);
    }
    if !ctx.settings.offline() {
        let context = Arc::clone(ctx);
        blocking_store(move || context.ensure_bootstrap()).await?;
    }
    let discovery = refresh_discovery(ctx, options.force_discovery, cancellation).await?;
    let mut warnings: Vec<String> = Vec::new();
    if let Some(error) = &discovery.error {
        warnings.push(format!("discovery refresh failed: {error}"));
    }
    if discovery.skipped && !options.force_discovery {
        warnings.push("discovery state is fresh; the existing fleet was used".to_owned());
    }
    let targets = match options.instance_id {
        Some(instance_id) => {
            let context = Arc::clone(ctx);
            let force = options.force;
            vec![blocking_store(move || explicit_target(&context, instance_id, force)).await?]
        }
        None => {
            let store = Arc::clone(&ctx.store);
            let now = now_rfc3339();
            let limit = options.limit;
            let force = options.force;
            blocking_store(move || store.scan_targets(limit, &now, force)).await?
        }
    };
    let mut probes: Vec<InstanceProbe> = Vec::new();
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(ctx.settings.scan_deadline_secs().max(30));
    let concurrency = ctx.settings.scan_concurrency().max(1);
    if concurrency <= 1 {
        for target in &targets {
            if tokio::time::Instant::now() >= deadline {
                warnings.push(format!(
                    "the scan deadline of {} seconds passed; the remaining instances were not probed",
                    ctx.settings.scan_deadline_secs()
                ));
                break;
            }
            jitter(ctx.settings.scan_jitter_ms()).await;
            let probe =
                tokio::time::timeout_at(deadline, probe_one(ctx, target, options, cancellation))
                    .await
                    .unwrap_or_else(|_| {
                        failed_probe(
                            ctx,
                            target,
                            ProbeOutcome::Timeout,
                            "the scan deadline passed".to_owned(),
                        )
                    });
            persist_probe(ctx, &probe, options.dry_run).await?;
            probes.push(probe);
        }
    } else {
        let permits = Arc::new(tokio::sync::Semaphore::new(concurrency));
        let mut set: JoinSet<InstanceProbe> = JoinSet::new();
        for target in targets {
            let permit = match permits.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => break,
            };
            let ctx = Arc::clone(ctx);
            let options = options.clone();
            let cancellation = cancellation.clone();
            set.spawn(async move {
                let _permit = permit;
                jitter(ctx.settings.scan_jitter_ms()).await;
                probe_one(&ctx, &target, &options, &cancellation).await
            });
        }
        loop {
            let joined = tokio::time::timeout_at(deadline, set.join_next()).await;
            match joined {
                Ok(Some(Ok(probe))) => {
                    persist_probe(ctx, &probe, options.dry_run).await?;
                    probes.push(probe);
                }
                Ok(Some(Err(error))) => {
                    tracing::error!(%error, "a probe task stopped without a report");
                    warnings.push(format!("a probe task failed: {error}"));
                }
                Ok(None) => break,
                Err(_) => {
                    set.abort_all();
                    while set.join_next().await.is_some() {}
                    warnings.push(format!(
                        "the scan deadline of {} seconds passed; the remaining instances were not probed",
                        ctx.settings.scan_deadline_secs()
                    ));
                    break;
                }
            }
        }
    }
    let status_changes: Vec<StatusChange> = probes
        .iter()
        .filter(|probe| probe.previous != probe.next_status)
        .map(|probe| StatusChange {
            instance_id: probe.instance_id,
            slug: crate::types::instance_slug(&probe.url),
            url: probe.url.clone(),
            previous_status: probe.previous.as_str().to_owned(),
            next_status: probe.next_status.as_str().to_owned(),
            reasons: probe.reasons.clone(),
        })
        .collect();
    let fleet = if options.dry_run {
        dry_run_fleet(&probes, ctx)
    } else {
        let selection = {
            let context = Arc::clone(ctx);
            blocking_store(move || select_and_rank(&context)).await?
        };
        if !probes.is_empty() {
            let store = Arc::clone(&ctx.store);
            let recorded = blocking_store(move || {
                let snapshot = store.export_snapshot()?;
                store.record_fleet_snapshot("scan", &snapshot)
            })
            .await;
            if let Err(error) = recorded {
                tracing::warn!(%error, "the fleet snapshot was not recorded");
            }
        }
        {
            let store = Arc::clone(&ctx.store);
            let snapshot_path = ctx.settings.snapshot_path();
            if let Err(error) = blocking_store(move || store.write_snapshot(&snapshot_path)).await {
                warnings.push(format!("the state snapshot was not rewritten: {error}"));
            }
        }
        let cleaner = Arc::clone(ctx);
        let cleaned = tokio::task::spawn_blocking(move || {
            crate::cleanup::clean(&cleaner.settings, &cleaner.ledger, cleaner.ledger_key())
        })
        .await;
        match cleaned {
            Ok(Ok(report)) => warnings.extend(report.warnings),
            Ok(Err(error)) => warnings.push(format!(
                "the cleanup failed: {error}; run `ownsearch db maintenance`"
            )),
            Err(error) => warnings.push(format!("the cleanup task failed: {error}")),
        }
        if selection.short_of_minimum {
            warnings.push(format!(
                "the active fleet holds fewer than the {} instance minimum",
                ctx.settings.fleet_min()
            ));
        }
        selection.ranks.into_iter().map(|(id, _)| id).collect()
    };
    let probed = probes.len();
    ctx.bump_fleet_generation();
    guard.mark_completed();
    Ok(ScanOutput {
        dry_run: options.dry_run,
        run_id: discovery.run_id,
        discovered: discovery.discovered,
        added: discovery.added,
        changed: discovery.changed,
        missing: discovery.missing,
        probed,
        status_changes,
        fleet,
        warnings,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

async fn dry_run_scan(
    ctx: &Arc<ToolContext>,
    options: &ScanOptions,
    cancellation: &CancellationToken,
    started: std::time::Instant,
) -> Result<ScanOutput> {
    let discovery = discovery_preview(ctx, options.force_discovery, cancellation).await?;
    let mut warnings: Vec<String> = Vec::new();
    if let Some(error) = &discovery.error {
        warnings.push(format!("discovery refresh failed: {error}"));
    }
    if discovery.skipped && !options.force_discovery {
        warnings.push("discovery state is fresh; the existing fleet was used".to_owned());
    }
    let targets = match options.instance_id {
        Some(instance_id) => {
            let context = Arc::clone(ctx);
            let force = options.force;
            vec![blocking_store(move || explicit_target(&context, instance_id, force)).await?]
        }
        None => {
            let store = Arc::clone(&ctx.store);
            let now = now_rfc3339();
            let limit = options.limit;
            let force = options.force;
            let stored = blocking_store(move || store.scan_targets(limit, &now, force)).await?;
            if stored.is_empty() {
                if ctx.settings.offline() {
                    warnings.push(
                        "offline mode: the state database holds no instances, so nothing was previewed"
                            .to_owned(),
                    );
                    Vec::new()
                } else {
                    warnings.push(
                        "the state database holds no instances yet; the embedded bootstrap was previewed"
                            .to_owned(),
                    );
                    bootstrap_targets(options.limit)?
                }
            } else {
                stored
            }
        }
    };
    let mut probes: Vec<InstanceProbe> = Vec::new();
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(ctx.settings.scan_deadline_secs().max(30));
    for target in &targets {
        if tokio::time::Instant::now() >= deadline {
            warnings.push(format!(
                "the scan deadline of {} seconds passed; the remaining instances were not previewed",
                ctx.settings.scan_deadline_secs()
            ));
            break;
        }
        jitter(ctx.settings.scan_jitter_ms()).await;
        probes.push(
            tokio::time::timeout_at(deadline, probe_one(ctx, target, options, cancellation))
                .await
                .unwrap_or_else(|_| {
                    failed_probe(
                        ctx,
                        target,
                        ProbeOutcome::Timeout,
                        "the scan deadline passed".to_owned(),
                    )
                }),
        );
    }
    let status_changes: Vec<StatusChange> = probes
        .iter()
        .filter(|probe| probe.previous != probe.next_status)
        .map(|probe| StatusChange {
            instance_id: probe.instance_id,
            slug: crate::types::instance_slug(&probe.url),
            url: probe.url.clone(),
            previous_status: probe.previous.as_str().to_owned(),
            next_status: probe.next_status.as_str().to_owned(),
            reasons: probe.reasons.clone(),
        })
        .collect();
    let fleet = dry_run_fleet(&probes, ctx);
    Ok(ScanOutput {
        dry_run: true,
        run_id: None,
        discovered: discovery.discovered,
        added: discovery.added,
        changed: discovery.changed,
        missing: discovery.missing,
        probed: probes.len(),
        status_changes,
        fleet,
        warnings,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

fn bootstrap_targets(limit: usize) -> Result<Vec<ScanTarget>> {
    let snapshot = crate::instances::BootstrapSnapshot::embedded()?;
    let now = now_unix();
    Ok(snapshot
        .to_discovered(now)
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(index, record)| ScanTarget {
            instance_id: i64::try_from(index).unwrap_or(0).saturating_add(1),
            url: record.url,
            status: crate::types::InstanceStatus::Candidate,
            kind: record.kind,
            html_fallback_allowed: false,
            consecutive_successes: 0,
            consecutive_failures: 0,
        })
        .collect())
}

async fn discovery_preview(
    ctx: &ToolContext,
    force: bool,
    cancellation: &CancellationToken,
) -> Result<DiscoveryOutcome> {
    if ctx.settings.offline() {
        return Ok(DiscoveryOutcome {
            skipped: true,
            ..DiscoveryOutcome::default()
        });
    }
    if !force
        && let Some(last) = {
            let store = Arc::clone(&ctx.store);
            blocking_store(move || store.last_run_at(SOURCE_ID_SEARX_SPACE)).await?
        }
        && now_unix().saturating_sub(last)
            < i64::try_from(ctx.settings.discovery_refresh_secs()).unwrap_or(i64::MAX)
    {
        return Ok(DiscoveryOutcome {
            skipped: true,
            ..DiscoveryOutcome::default()
        });
    }
    let url = Url::parse(ctx.settings.discovery_url())
        .map_err(|error| Error::Config(format!("discovery_url: {error}")))?;
    let fetched = match ctx.client.get(&url, "application/json", cancellation).await {
        Ok(fetched) => fetched,
        Err(error) => {
            return Ok(DiscoveryOutcome {
                error: Some(error.to_string()),
                ..DiscoveryOutcome::default()
            });
        }
    };
    if !fetched.is_success() || !fetched.is_json() {
        return Ok(DiscoveryOutcome {
            error: Some(format!("searx.space answered HTTP {}", fetched.status)),
            ..DiscoveryOutcome::default()
        });
    }
    let document = DiscoveryDocument::parse(&fetched.body)?;
    let discovered = public_records(ctx, document.to_discovered(now_unix()))
        .await
        .kept;
    let known = {
        let store = Arc::clone(&ctx.store);
        blocking_store(move || store.known_urls()).await?
    };
    let urls: std::collections::HashSet<String> =
        discovered.iter().map(|item| item.url.clone()).collect();
    let known_set: std::collections::HashSet<&String> = known.iter().collect();
    let added = urls.iter().filter(|url| !known_set.contains(url)).count();
    let changed = urls.iter().filter(|url| known_set.contains(url)).count();
    let missing = known.iter().filter(|url| !urls.contains(*url)).count();
    Ok(DiscoveryOutcome {
        run_id: None,
        discovered: discovered.len(),
        added,
        changed,
        missing,
        skipped: false,
        error: None,
    })
}

struct PublicRecords {
    kept: Vec<DiscoveredInstance>,
    source_len: usize,
    complete: bool,
}

async fn public_records(ctx: &ToolContext, mut records: Vec<DiscoveredInstance>) -> PublicRecords {
    let source_len = records.len();
    cap_records(&mut records);
    if ctx.settings.allow_private_instances() {
        return PublicRecords {
            kept: records,
            source_len,
            complete: true,
        };
    }
    let count = records.len();
    let permits = Arc::new(tokio::sync::Semaphore::new(RESOLVE_CONCURRENCY));
    let mut set: JoinSet<(usize, DiscoveredInstance, bool)> = JoinSet::new();
    for (index, record) in records.into_iter().enumerate() {
        let permits = Arc::clone(&permits);
        set.spawn(async move {
            let Ok(_permit) = permits.acquire_owned().await else {
                return (index, record, false);
            };
            let allowed = record_is_public(&record.url, false).await;
            (index, record, allowed)
        });
    }
    let mut checked: Vec<Option<(DiscoveredInstance, bool)>> = (0..count).map(|_| None).collect();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(RESOLVE_PASS_SECS);
    let mut complete = true;
    loop {
        match tokio::time::timeout_at(deadline, set.join_next()).await {
            Ok(Some(Ok((index, record, allowed)))) => {
                if let Some(slot) = checked.get_mut(index) {
                    *slot = Some((record, allowed));
                }
            }
            Ok(Some(Err(_))) => continue,
            Ok(None) => break,
            Err(_) => {
                set.abort_all();
                while set.join_next().await.is_some() {}
                complete = false;
                tracing::warn!(
                    records = count,
                    seconds = RESOLVE_PASS_SECS,
                    "the discovery address check ran out of time; no instance is marked missing"
                );
                break;
            }
        }
    }
    let mut kept = Vec::with_capacity(count);
    for slot in checked.into_iter().flatten() {
        let (record, allowed) = slot;
        if allowed {
            kept.push(record);
        } else {
            tracing::warn!(
                instance = %record.url,
                "the discovery record resolves to a non-public address; it was skipped"
            );
        }
    }
    PublicRecords {
        kept,
        source_len,
        complete,
    }
}

fn unique_reasons(reasons: Vec<String>) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(reasons.len());
    let mut out: Vec<String> = Vec::with_capacity(reasons.len());
    for reason in reasons {
        if seen.insert(reason.clone()) {
            out.push(reason);
        }
    }
    out
}

const MAX_DISCOVERY_RECORDS: usize = 500;

fn cap_records(records: &mut Vec<crate::instances::DiscoveredInstance>) {
    if records.len() > MAX_DISCOVERY_RECORDS {
        tracing::warn!(
            records = records.len(),
            cap = MAX_DISCOVERY_RECORDS,
            "the discovery document holds more records than the cap; the rest were ignored"
        );
        records.truncate(MAX_DISCOVERY_RECORDS);
    }
}

fn explicit_target(ctx: &ToolContext, instance_id: i64, force: bool) -> Result<ScanTarget> {
    let row = ctx
        .store
        .get_instance(instance_id)?
        .ok_or_else(|| Error::InvalidArgs(format!("instance {instance_id} does not exist")))?;
    if row.denied {
        return Err(Error::PolicyRefused(format!(
            "instance {instance_id} is denied by an operator override"
        )));
    }
    if !row.scan_enabled {
        return Err(Error::PolicyRefused(format!(
            "instance {instance_id} has scanning disabled"
        )));
    }
    if row.status == crate::types::InstanceStatus::Retired {
        return Err(Error::PolicyRefused(format!(
            "instance {instance_id} is retired"
        )));
    }
    let now = now_rfc3339();
    if let Some(until) = row.cooldown_until.as_deref()
        && until > now.as_str()
    {
        return Err(Error::PolicyRefused(format!(
            "instance {instance_id} is cooling down until {until}"
        )));
    }
    if !force
        && let Some(next) = row.next_scan_at.as_deref()
        && next > now.as_str()
    {
        return Err(Error::PolicyRefused(format!(
            "instance {instance_id} is scheduled for its next scan at {next}; pass force to probe it now"
        )));
    }
    Ok(ScanTarget {
        instance_id: row.instance_id,
        url: row.url,
        status: row.status,
        kind: row.kind,
        html_fallback_allowed: row.html_fallback_allowed,
        consecutive_successes: row.consecutive_successes,
        consecutive_failures: row.consecutive_failures,
    })
}

async fn record_is_public(url: &str, allow_private: bool) -> bool {
    if allow_private {
        return true;
    }
    match url::Url::parse(url) {
        Ok(parsed) => crate::network_guard::url_is_public(&parsed).await,
        Err(_) => false,
    }
}

async fn probe_one(
    ctx: &ToolContext,
    target: &ScanTarget,
    options: &ScanOptions,
    cancellation: &CancellationToken,
) -> InstanceProbe {
    let limit = std::time::Duration::from_secs(
        ctx.settings
            .probe_timeout_secs()
            .saturating_mul(PROBE_STEP_COUNT)
            .max(15),
    );
    match tokio::time::timeout(limit, probe_one_inner(ctx, target, options, cancellation)).await {
        Ok(probe) => probe,
        Err(_) => failed_probe(
            ctx,
            target,
            ProbeOutcome::Timeout,
            "the probe deadline passed before the instance answered".to_owned(),
        ),
    }
}

async fn probe_one_inner(
    ctx: &ToolContext,
    target: &ScanTarget,
    options: &ScanOptions,
    cancellation: &CancellationToken,
) -> InstanceProbe {
    let base = match Url::parse(&target.url) {
        Ok(base) => base,
        Err(error) => {
            return failed_probe(ctx, target, ProbeOutcome::Refused, format!("url: {error}"));
        }
    };
    let tls = ctx.tls.probe(&base, cancellation).await;
    let config_report = config_client::fetch(&ctx.probe_client, &base, cancellation).await;
    let mut rate_limited = config_report.rate_limited;
    let mut blocked = config_report.blocked;
    let config = config_report.value.clone();
    let config_outcome = config_report.outcome;
    let mut json_report = None;
    let mut json_outcome = ProbeOutcome::Refused;
    if options.probe_search && !rate_limited {
        let query = SearchQuery::new(PROBE_QUERY);
        let report = search_json::fetch(&ctx.probe_client, &base, &query, cancellation).await;
        rate_limited |= report.rate_limited;
        blocked |= report.blocked;
        json_outcome = report.outcome;
        json_report = Some(report);
    }
    let mut html_report = None;
    let mut html_outcome = None;
    let json_denied = matches!(
        json_outcome,
        ProbeOutcome::Refused | ProbeOutcome::ParserError | ProbeOutcome::HttpError
    );
    let html_allowed = target.html_fallback_allowed || ctx.settings.html_fallback();
    if options.probe_search && json_denied && html_allowed && !rate_limited {
        let query = SearchQuery::new(PROBE_QUERY);
        let report = search_html::fetch(&ctx.probe_client, &base, &query, cancellation).await;
        rate_limited |= report.rate_limited;
        blocked |= report.blocked;
        html_outcome = Some(report.outcome);
        html_report = Some(report);
    }
    let mut formats_ok = false;
    if options.probe_search && json_denied && !rate_limited && !blocked {
        let query = SearchQuery::new(PROBE_QUERY);
        for format in ["rss", "csv"] {
            if let Ok(entries) =
                crate::formats::fetch(&ctx.probe_client, &base, &query, format, cancellation).await
                && !entries.is_empty()
            {
                formats_ok = true;
                break;
            }
        }
    }
    let mut browser_ok = false;
    if options.probe_search
        && json_denied
        && !rate_limited
        && !blocked
        && ctx.browser.enabled()
        && let Ok(url) = crate::client::search_url(&base, &[("q", PROBE_QUERY.to_owned())])
        && let Ok((html, _)) = ctx
            .browser
            .page(&url, std::time::Duration::from_secs(30))
            .await
    {
        let page = search_html::HtmlSearch::parse(&html, &base);
        browser_ok = !page.blocked && !page.results.is_empty();
    }
    let mut google_report = None;
    let mut google_outcome = None;
    let exact_google_capability = config
        .as_ref()
        .map(config_client::ConfigDocument::exact_google)
        .unwrap_or(false);
    if options.probe_google && exact_google_capability && json_outcome.is_success() && !rate_limited
    {
        let mut query = SearchQuery::new(PROBE_QUERY);
        query.engines = vec!["google".to_owned()];
        let report = search_json::fetch(&ctx.probe_client, &base, &query, cancellation).await;
        rate_limited |= report.rate_limited;
        blocked |= report.blocked;
        google_outcome = Some(report.outcome);
        google_report = Some(report);
    }
    let parser_drift = html_report
        .as_ref()
        .map(|report| {
            report
                .value
                .as_ref()
                .map(|parsed| parsed.drifted)
                .unwrap_or(false)
        })
        .unwrap_or(false);
    let metrics = {
        let store = Arc::clone(&ctx.store);
        let instance_id = target.instance_id;
        blocking_store(move || Ok(store.discovery_metrics(instance_id).ok().flatten()))
            .await
            .ok()
            .flatten()
    };
    let evidence = build_evidence(ProbeEvidence {
        metrics: metrics.as_ref(),
        tls: &tls,
        config: config.as_ref(),
        config_outcome,
        json_outcome,
        html_outcome,
        google_outcome,
        parser_drift,
        successes: target.consecutive_successes,
        failures: target.consecutive_failures,
        trusted_local: ctx.settings.allow_private_instances()
            && crate::network_guard::host_is_local(&base),
    });
    let assessment = assess(
        &evidence,
        crate::quality::Policy {
            cert_min_days: ctx.settings.cert_min_days(),
            google_required: ctx.settings.google_required(),
        },
    );
    let succeeded = assessment.hard_gate_passed
        && (json_outcome.is_success()
            || html_outcome == Some(ProbeOutcome::Success)
            || formats_ok
            || browser_ok);
    let inconclusive = evidence.json_ok.is_none() && evidence.html_ok.is_none();
    let successes = if succeeded {
        target.consecutive_successes.saturating_add(1)
    } else {
        0
    };
    let failures = if succeeded {
        0
    } else if inconclusive {
        target.consecutive_failures
    } else {
        target.consecutive_failures.saturating_add(1)
    };
    let transition = next_status(HysteresisInput {
        current: target.status,
        successes: u32::try_from(successes).unwrap_or(u32::MAX),
        failures: u32::try_from(failures).unwrap_or(u32::MAX),
        promotion_ready: succeeded,
        gate_failed: !assessment.hard_gate_passed,
        rate_limited,
        blocked,
        denied: false,
        promote_successes: ctx.settings.promote_successes(),
        probation_failures: ctx.settings.probation_failures(),
        quarantine_failures: ctx.settings.quarantine_failures(),
    });
    let now = now_unix();
    let next_scan_at = if rate_limited || blocked {
        Some(rfc3339(now.saturating_add(
            i64::try_from(ctx.settings.cooldown_secs()).unwrap_or(604_800),
        )))
    } else if transition.status == InstanceStatus::Quarantined {
        Some(rfc3339(now.saturating_add(QUARANTINE_NEXT_SCAN_SECS)))
    } else if succeeded {
        Some(rfc3339(now.saturating_add(SUCCESS_NEXT_SCAN_SECS)))
    } else {
        Some(rfc3339(now.saturating_add(FAILURE_NEXT_SCAN_SECS)))
    };
    let cooldown_until = (rate_limited || blocked).then(|| {
        rfc3339(now.saturating_add(i64::try_from(ctx.settings.cooldown_secs()).unwrap_or(604_800)))
    });
    let version = config
        .as_ref()
        .and_then(|document| document.version.clone())
        .or_else(|| metrics.as_ref().and_then(|item| item.version.clone()));
    let mut reasons = assessment.reasons.clone();
    reasons.extend(transition.reasons.clone());
    if json_outcome.is_success() && json_report.is_some() {
        reasons.push("probe.json_ok".to_owned());
    }
    if html_outcome == Some(ProbeOutcome::Success) {
        reasons.push("probe.html_ok".to_owned());
    }
    if blocked {
        reasons.push("probe.blocked".to_owned());
    }
    let reasons = unique_reasons(reasons);
    InstanceProbe {
        instance_id: target.instance_id,
        url: target.url.clone(),
        previous: target.status,
        tls,
        config_outcome,
        config,
        config_status: config_report.http_status,
        json_outcome,
        json_report,
        html_outcome,
        html_report,
        google_outcome,
        google_report,
        rate_limited,
        blocked,
        parser_drift,
        successes,
        failures,
        score: assessment.score,
        reasons,
        hard_gate_passed: assessment.hard_gate_passed,
        next_status: transition.status,
        next_scan_at,
        cooldown_until,
        version,
    }
}

fn failed_probe(
    ctx: &ToolContext,
    target: &ScanTarget,
    outcome: ProbeOutcome,
    message: String,
) -> InstanceProbe {
    let failures = target.consecutive_failures.saturating_add(1);
    let transition = next_status(HysteresisInput {
        current: target.status,
        successes: 0,
        failures: u32::try_from(failures).unwrap_or(u32::MAX),
        promotion_ready: false,
        gate_failed: false,
        rate_limited: false,
        blocked: false,
        denied: false,
        promote_successes: ctx.settings.promote_successes(),
        probation_failures: ctx.settings.probation_failures(),
        quarantine_failures: ctx.settings.quarantine_failures(),
    });
    let now = now_unix();
    let next_scan_at = Some(rfc3339(now.saturating_add(
        if transition.status == InstanceStatus::Quarantined {
            QUARANTINE_NEXT_SCAN_SECS
        } else {
            FAILURE_NEXT_SCAN_SECS
        },
    )));
    let mut reasons = vec![format!("probe.{}", outcome.as_str())];
    reasons.extend(transition.reasons);
    InstanceProbe {
        instance_id: target.instance_id,
        url: target.url.clone(),
        previous: target.status,
        tls: TlsReport::failed(outcome, message.clone()),
        config_outcome: outcome,
        config: None,
        config_status: None,
        json_outcome: outcome,
        json_report: None,
        html_outcome: None,
        html_report: None,
        google_outcome: None,
        google_report: None,
        rate_limited: false,
        blocked: false,
        parser_drift: false,
        successes: 0,
        failures,
        score: 0.0,
        reasons: unique_reasons(reasons),
        hard_gate_passed: false,
        next_status: transition.status,
        next_scan_at,
        cooldown_until: None,
        version: None,
    }
}

struct ProbeEvidence<'a> {
    metrics: Option<&'a DiscoveredInstance>,
    tls: &'a TlsReport,
    config: Option<&'a config_client::ConfigDocument>,
    config_outcome: ProbeOutcome,
    json_outcome: ProbeOutcome,
    html_outcome: Option<ProbeOutcome>,
    google_outcome: Option<ProbeOutcome>,
    parser_drift: bool,
    successes: i64,
    failures: i64,
    trusted_local: bool,
}

fn evidence_value(outcome: ProbeOutcome) -> Option<bool> {
    match outcome {
        ProbeOutcome::Success => Some(true),
        ProbeOutcome::HttpError | ProbeOutcome::ParserError => Some(false),
        _ => None,
    }
}

fn build_evidence(input: ProbeEvidence<'_>) -> Evidence {
    let ProbeEvidence {
        metrics,
        tls,
        config,
        config_outcome,
        json_outcome,
        html_outcome,
        google_outcome,
        parser_drift,
        successes,
        failures,
        trusted_local,
    } = input;
    let version = config
        .and_then(|document| document.version.clone())
        .or_else(|| metrics.and_then(|item| item.version.clone()));
    Evidence {
        trusted_local,
        tls_ok: tls.is_ok(),
        certificate_days: tls
            .days_remaining
            .or(metrics.and_then(|item| item.certificate_days_remaining)),
        analytics: metrics.map(|item| item.analytics).unwrap_or(false),
        config_ok: match config_outcome {
            ProbeOutcome::Success => Some(true),
            ProbeOutcome::ParserError => Some(false),
            _ => None,
        },
        json_ok: evidence_value(json_outcome),
        html_ok: html_outcome.and_then(evidence_value),
        google_ok: google_outcome.map(ProbeOutcome::is_success),
        uptime_month: metrics.and_then(|item| item.uptime_month),
        uptime_year: metrics.and_then(|item| item.uptime_year),
        search_success: metrics.and_then(|item| item.search_success),
        search_median_secs: metrics.and_then(|item| item.search_median_secs),
        initial_success: metrics.and_then(|item| item.initial_success),
        initial_secs: metrics.and_then(|item| item.initial_secs),
        google_secs: metrics.and_then(|item| item.google_secs),
        tls_grade: metrics.and_then(|item| item.tls_grade.clone()).or_else(|| {
            tls.protocol.as_deref().map(|protocol| {
                if protocol.contains("1.3") {
                    "A+".to_owned()
                } else {
                    "A".to_owned()
                }
            })
        }),
        tls_protocol: tls
            .protocol
            .clone()
            .or_else(|| metrics.and_then(|item| item.tls_protocol.clone())),
        version_age_days: version
            .as_deref()
            .and_then(|value| version_age_days(value, now_unix())),
        engine_count: config.map(|document| document.engines.len()).unwrap_or(0),
        category_count: config
            .map(|document| document.categories.len())
            .unwrap_or(0),
        consecutive_successes: u32::try_from(successes).unwrap_or(0),
        consecutive_failures: u32::try_from(failures).unwrap_or(0),
        parser_drift,
    }
}

async fn persist_probe(ctx: &Arc<ToolContext>, probe: &InstanceProbe, dry_run: bool) -> Result<()> {
    if dry_run {
        return Ok(());
    }
    let store = Arc::clone(&ctx.store);
    let probe = probe.clone();
    tokio::task::spawn_blocking(move || {
        store.write_atomic(|connection| {
            let tls_probe = Store::insert_probe_on(
                connection,
                &ProbeRecord {
                    instance_id: probe.instance_id,
                    run_id: None,
                    kind: ProbeKind::Tls,
                    outcome: probe.tls.outcome,
                    started_at: now_rfc3339(),
                    completed_at: now_rfc3339(),
                    duration_ms: probe.tls.elapsed_ms,
                    http_status: None,
                    content_type: None,
                    result_count: None,
                    response_bytes: None,
                    response_digest_sha256: None,
                    error_code: probe.tls.error_code.clone(),
                    error_message: probe.tls.error_message.clone(),
                },
            )?;
            let tls_id = Store::insert_tls_on(
                connection,
                &TlsRecord {
                    instance_id: probe.instance_id,
                    probe_id: tls_probe,
                    protocol: probe.tls.protocol.clone(),
                    issuer: probe.tls.issuer.clone(),
                    not_before: probe.tls.not_before,
                    not_after: probe.tls.not_after,
                    hostname_verified: probe.tls.hostname_verified,
                    chain_valid: probe.tls.chain_valid,
                    error_code: probe.tls.error_code.clone(),
                    error_message: probe.tls.error_message.clone(),
                },
            )?;
            insert_simple_probe(
                connection,
                probe.instance_id,
                ProbeKind::Config,
                probe.config_outcome,
                probe.config_status,
                probe
                    .config
                    .as_ref()
                    .map(|document| document.engines.len())
                    .map(|len| i64::try_from(len).unwrap_or(0)),
            )?;
            if let Some(report) = &probe.json_report {
                insert_report_probe(connection, probe.instance_id, ProbeKind::JsonSearch, report)?;
            }
            if let Some(report) = &probe.html_report {
                insert_report_probe_html(connection, probe.instance_id, report)?;
            }
            if let Some(report) = &probe.google_report {
                insert_report_probe(connection, probe.instance_id, ProbeKind::Google, report)?;
            }
            let capability_id = Store::insert_capability_on(
                connection,
                &CapabilityRecord {
                    instance_id: probe.instance_id,
                    run_id: None,
                    outcome: probe.config_outcome,
                    http_status: probe.config_status,
                    duration_ms: None,
                    response_digest_sha256: None,
                    instance_name: probe
                        .config
                        .as_ref()
                        .and_then(|document| document.instance_name.clone()),
                    categories: probe
                        .config
                        .as_ref()
                        .map(|document| document.categories.clone())
                        .unwrap_or_default(),
                    engines: probe
                        .config
                        .as_ref()
                        .map(|document| document.engines.clone())
                        .unwrap_or_default(),
                    locales: probe
                        .config
                        .as_ref()
                        .map(|document| document.locales.clone())
                        .unwrap_or_default(),
                    plugins: probe
                        .config
                        .as_ref()
                        .map(|document| document.plugins.clone())
                        .unwrap_or_default(),
                    safe_search_default: probe
                        .config
                        .as_ref()
                        .and_then(|document| document.safe_search),
                    json_enabled: probe.json_outcome.is_success(),
                    csv_enabled: false,
                    rss_enabled: false,
                    html_enabled: probe.html_outcome == Some(ProbeOutcome::Success),
                    google_exact_seen: probe
                        .config
                        .as_ref()
                        .map(config_client::ConfigDocument::exact_google)
                        .unwrap_or(false),
                    google_cse_seen: probe
                        .config
                        .as_ref()
                        .map(config_client::ConfigDocument::google_cse)
                        .unwrap_or(false),
                },
            )?;
            let now = rfc3339(now_unix());
            Store::apply_scan_update_on(
                connection,
                &ScanUpdate {
                    instance_id: probe.instance_id,
                    status: probe.next_status,
                    previous_status: probe.previous,
                    successes: probe.successes,
                    failures: probe.failures,
                    score: probe.score,
                    rank: None,
                    hard_gate_passed: probe.hard_gate_passed,
                    reasons: probe.reasons.clone(),
                    next_scan_at: probe.next_scan_at.clone(),
                    cooldown_until: probe.cooldown_until.clone(),
                    last_success_at: probe.succeeded().then(|| now.clone()),
                    last_failure_at: (!probe.succeeded()).then(|| now.clone()),
                    version: probe.version.clone(),
                    capability_id: Some(capability_id),
                    tls_id: Some(tls_id),
                },
            )?;
            Store::record_metrics_on(
                connection,
                probe.instance_id,
                ProbeKind::Tls,
                probe.tls.elapsed_ms,
                probe.tls.outcome.is_success(),
                0,
                0,
            )?;
            Store::record_metrics_on(
                connection,
                probe.instance_id,
                ProbeKind::Config,
                0,
                probe.config_outcome.is_success(),
                0,
                0,
            )?;
            if let Some(report) = &probe.json_report {
                Store::record_metrics_on(
                    connection,
                    probe.instance_id,
                    ProbeKind::JsonSearch,
                    report.elapsed_ms,
                    report.outcome.is_success(),
                    report
                        .value
                        .as_ref()
                        .map(|search| i64::try_from(search.result_count()).unwrap_or(0))
                        .unwrap_or(0),
                    i64::try_from(report.bytes).unwrap_or(0),
                )?;
            }
            if let Some(report) = &probe.html_report {
                Store::record_metrics_on(
                    connection,
                    probe.instance_id,
                    ProbeKind::HtmlSearch,
                    report.elapsed_ms,
                    report.outcome.is_success(),
                    report
                        .value
                        .as_ref()
                        .map(|search| i64::try_from(search.results.len()).unwrap_or(0))
                        .unwrap_or(0),
                    i64::try_from(report.bytes).unwrap_or(0),
                )?;
            }
            if let Some(report) = &probe.google_report {
                Store::record_metrics_on(
                    connection,
                    probe.instance_id,
                    ProbeKind::Google,
                    report.elapsed_ms,
                    report.outcome.is_success(),
                    report
                        .value
                        .as_ref()
                        .map(|search| i64::try_from(search.result_count()).unwrap_or(0))
                        .unwrap_or(0),
                    i64::try_from(report.bytes).unwrap_or(0),
                )?;
            }
            if let Err(error) = Store::append_event_on(
                connection,
                &EventRecord {
                    kind: "probe_completed".to_owned(),
                    instance_id: Some(probe.instance_id),
                    run_id: None,
                    probe_id: Some(tls_probe),
                    old_value: Some(probe.previous.as_str().to_owned()),
                    new_value: Some(probe.next_status.as_str().to_owned()),
                    reasons: probe.reasons.clone(),
                    payload: Some(json!({
                        "json": probe.json_outcome.as_str(),
                        "html": probe.html_outcome.map(ProbeOutcome::as_str),
                        "google": probe.google_outcome.map(ProbeOutcome::as_str),
                        "score": probe.score,
                        "rateLimited": probe.rate_limited,
                        "blocked": probe.blocked,
                        "parserDrift": probe.parser_drift,
                    })),
                },
            ) {
                tracing::warn!(%error, "the probe event could not be written");
            }
            Ok(())
        })
    })
    .await
    .map_err(|error| Error::Io {
        context: "waiting for the probe write",
        source: std::io::Error::other(error),
    })?
}

struct ProbeFields {
    kind: ProbeKind,
    outcome: ProbeOutcome,
    duration_ms: i64,
    http_status: Option<u16>,
    result_count: Option<i64>,
    response_bytes: Option<i64>,
    error_code: Option<String>,
    error_message: Option<String>,
}

fn insert_probe_row(connection: &Connection, instance_id: i64, fields: ProbeFields) -> Result<()> {
    Store::insert_probe_on(
        connection,
        &ProbeRecord {
            instance_id,
            run_id: None,
            kind: fields.kind,
            outcome: fields.outcome,
            started_at: now_rfc3339(),
            completed_at: now_rfc3339(),
            duration_ms: fields.duration_ms,
            http_status: fields.http_status,
            content_type: None,
            result_count: fields.result_count,
            response_bytes: fields.response_bytes,
            response_digest_sha256: None,
            error_code: fields.error_code,
            error_message: fields.error_message,
        },
    )?;
    Ok(())
}

fn insert_simple_probe(
    connection: &Connection,
    instance_id: i64,
    kind: ProbeKind,
    outcome: ProbeOutcome,
    http_status: Option<u16>,
    result_count: Option<i64>,
) -> Result<()> {
    insert_probe_row(
        connection,
        instance_id,
        ProbeFields {
            kind,
            outcome,
            duration_ms: 0,
            http_status,
            result_count,
            response_bytes: None,
            error_code: None,
            error_message: None,
        },
    )
}

fn insert_report_probe(
    connection: &Connection,
    instance_id: i64,
    kind: ProbeKind,
    report: &ProbeReport<search_json::JsonSearch>,
) -> Result<()> {
    insert_probe_row(
        connection,
        instance_id,
        ProbeFields {
            kind,
            outcome: report.outcome,
            duration_ms: report.elapsed_ms,
            http_status: report.http_status,
            result_count: report
                .value
                .as_ref()
                .map(|search| i64::try_from(search.result_count()).unwrap_or(0)),
            response_bytes: Some(i64::try_from(report.bytes).unwrap_or(0)),
            error_code: report.error_code.clone(),
            error_message: report.error_message.clone(),
        },
    )
}

fn insert_report_probe_html(
    connection: &Connection,
    instance_id: i64,
    report: &ProbeReport<search_html::HtmlSearch>,
) -> Result<()> {
    insert_probe_row(
        connection,
        instance_id,
        ProbeFields {
            kind: ProbeKind::HtmlSearch,
            outcome: report.outcome,
            duration_ms: report.elapsed_ms,
            http_status: report.http_status,
            result_count: report
                .value
                .as_ref()
                .map(|search| i64::try_from(search.results.len()).unwrap_or(0)),
            response_bytes: Some(i64::try_from(report.bytes).unwrap_or(0)),
            error_code: report.error_code.clone(),
            error_message: report.error_message.clone(),
        },
    )
}

fn select_and_rank(ctx: &ToolContext) -> Result<FleetSelection> {
    let rows = ctx.store.list_instances(&crate::db::InstanceFilter {
        limit: 500,
        ..crate::db::InstanceFilter::default()
    })?;
    let candidates: Vec<FleetCandidate> = rows
        .iter()
        .filter(|row| {
            row.hard_gate_passed == Some(true)
                && row.kind != crate::types::InstanceType::Tor
                && !row.denied
                && (matches!(
                    row.status,
                    InstanceStatus::Active | InstanceStatus::Probation
                ) || (row.pinned
                    && matches!(
                        row.status,
                        InstanceStatus::Candidate | InstanceStatus::Missing
                    )))
        })
        .map(|row| FleetCandidate {
            instance_id: row.instance_id,
            score: row.score.unwrap_or(0.0),
            network_group: row.network_group.clone(),
            google_exact: row.google_exact,
            pinned: row.pinned,
        })
        .collect();
    let selection = select_fleet(
        &candidates,
        ctx.settings.fleet_min(),
        ctx.settings.fleet_max(),
        ctx.settings.max_per_network(),
        ctx.settings.google_required(),
    );
    ctx.store.update_fleet_ranks(&selection.ranks)?;
    Ok(selection)
}

fn dry_run_fleet(probes: &[InstanceProbe], ctx: &ToolContext) -> Vec<i64> {
    let candidates: Vec<FleetCandidate> = probes
        .iter()
        .filter(|probe| probe.hard_gate_passed && probe.succeeded())
        .map(|probe| FleetCandidate {
            instance_id: probe.instance_id,
            score: probe.score,
            network_group: None,
            google_exact: probe
                .config
                .as_ref()
                .map(config_client::ConfigDocument::exact_google)
                .unwrap_or(false),
            pinned: false,
        })
        .collect();
    select_fleet(
        &candidates,
        ctx.settings.fleet_min(),
        ctx.settings.fleet_max(),
        ctx.settings.max_per_network(),
        ctx.settings.google_required(),
    )
    .ranks
    .into_iter()
    .map(|(id, _)| id)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_scan_is_a_dry_run() {
        let options = ScanOptions::default();
        assert!(options.dry_run);
        assert!(options.probe_search);
        assert!(options.probe_google);
        assert_eq!(options.limit, 200);
    }

    #[test]
    fn the_probe_query_is_a_fixed_low_signal_string() {
        assert_eq!(PROBE_QUERY, "ownsearch connectivity probe");
    }
}
