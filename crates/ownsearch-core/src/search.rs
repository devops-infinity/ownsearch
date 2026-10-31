use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::context::ToolContext;
use crate::db::InstanceRow;
use crate::error::{Error, Result};
use crate::merge::{MergeOptions, Observation, merge};
use crate::response::{
    CacheInfo, Coverage, Notice, SearchOutput, SearchResultItem, SearchTiming, SourceFailure,
    SourceObservation, SourcesInfo, UnresponsiveEngine,
};
use crate::sanitize::{cap, sanitize};
use crate::searxng::{ProbeReport, SearchQuery, search_html, search_json};
use crate::types::{InstanceStatus, ProbeOutcome, SafeSearch, SearchMode, TimeRange};

const SNIPPET_CHARS: usize = 300;
const TITLE_CHARS: usize = 300;
const UNSCORED_FALLBACK_INSTANCES: usize = 32;
const INLINE_REFRESH_BUDGET_SECS: u64 = 2;

#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub query: String,
    pub categories: Vec<String>,
    pub engines: Vec<String>,
    pub language: Option<String>,
    pub page: u32,
    pub time_range: Option<TimeRange>,
    pub safe_search: SafeSearch,
    pub limit: usize,
    pub mode: SearchMode,
    pub deadline_override_secs: Option<u64>,
    pub dedupe: bool,
    pub min_engines: Option<usize>,
    pub instance_ids: Vec<i64>,
    pub engine_weights: BTreeMap<String, f64>,
    pub exclude_engines: Vec<String>,
    pub freshness_days: Option<u64>,
}

impl SearchRequest {
    pub fn to_query(&self) -> SearchQuery {
        SearchQuery {
            query: self.query.clone(),
            categories: self.categories.clone(),
            engines: self.engines.clone(),
            language: self.language.clone(),
            page: self.page,
            time_range: self.time_range,
            safe_search: self.safe_search,
        }
    }

    pub fn canonical(&self) -> serde_json::Value {
        serde_json::json!({
            "q": self.query,
            "categories": self.categories,
            "engines": self.engines,
            "language": self.language,
            "page": self.page,
            "time_range": self.time_range.map(TimeRange::as_str),
            "safeSearch": self.safe_search,
            "limit": self.limit,
            "mode": self.mode,
            "deadline": self.deadline_override_secs,
            "dedupe": self.dedupe,
            "minEngines": self.min_engines,
            "instanceIds": self.instance_ids,
            "engineWeights": self.engine_weights,
            "excludeEngines": self.exclude_engines,
            "freshnessDays": self.freshness_days,
        })
    }
}

#[derive(Debug)]
pub struct SearchOutcome {
    pub output: SearchOutput,
    pub cache_hit: bool,
    pub cache_age: Option<u64>,
    pub instances: Vec<i64>,
    pub parser: Option<String>,
    pub http_class: Option<String>,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
struct SourceOutcome {
    instance_id: i64,
    parser: Option<String>,
    observations: Vec<Observation>,
    failure: Option<SourceFailure>,
    http_class: Option<String>,
    unresponsive: Vec<(String, String)>,
}

pub async fn search(
    ctx: &Arc<ToolContext>,
    request: &SearchRequest,
    cancellation: &CancellationToken,
) -> Result<SearchOutcome> {
    let started = Instant::now();
    if request.query.trim().is_empty() {
        return Err(Error::InvalidArgs("query is empty".into()));
    }
    if request.page == 0 {
        return Err(Error::InvalidArgs("page starts at 1".into()));
    }
    if !(1..=100).contains(&request.limit) {
        return Err(Error::InvalidArgs(format!(
            "limit {} is outside 1..100",
            request.limit
        )));
    }
    let deadline_secs = request
        .deadline_override_secs
        .unwrap_or_else(|| ctx.settings.search_deadline_secs(request.mode));
    let deadline_ms = deadline_secs.saturating_mul(1_000);
    let cache_key = format!(
        "ownsearch_search:{}:{}",
        ctx.fleet_generation(),
        ctx.hash_args(&request.canonical())
    );
    if let Some(outcome) = cached_search(ctx, &cache_key)? {
        return Ok(outcome);
    }
    let queue_started = Instant::now();
    let _search_slot = ctx.acquire_search().await?;
    let queue_ms = elapsed_ms(queue_started);
    let selection_started = Instant::now();
    let mut messages: Vec<Notice> = Vec::new();
    let rows = prepare_fleet(ctx, &mut messages).await?;
    let ordered = if request.instance_ids.is_empty() {
        let rotation = ctx.settings.round_robin().then(|| {
            ctx.next_rotation(rotation_span(
                rows.len(),
                target_instances(request.mode, ctx.settings.exhaustive_max()),
            ))
        });
        order_instances(&rows, request.mode, ctx.settings.exhaustive_max(), rotation)
    } else {
        let ids = request.instance_ids.clone();
        let store = Arc::clone(&ctx.store);
        let chosen = blocking_store(move || {
            store.list_instances(&crate::db::InstanceFilter {
                ids,
                limit: 64,
                ..crate::db::InstanceFilter::default()
            })
        })
        .await?;
        let chosen: Vec<InstanceRow> = chosen
            .into_iter()
            .filter(|row| {
                !row.denied && row.scan_enabled && row.kind != crate::types::InstanceType::Tor
            })
            .collect();
        order_instances(&chosen, request.mode, ctx.settings.exhaustive_max(), None)
    };
    if ordered.is_empty() {
        return Err(Error::NoFleet);
    }
    let target = target_instances(request.mode, ctx.settings.exhaustive_max());
    let selection_ms = elapsed_ms(selection_started);
    let freshness_days = request
        .freshness_days
        .unwrap_or_else(|| ctx.settings.freshness_days());
    let mut query = request.to_query();
    if query.time_range.is_none() {
        query.time_range = mapped_range(freshness_days);
    }
    let upstream_started = Instant::now();
    let hard_deadline = tokio::time::Instant::now() + Duration::from_millis(deadline_ms);
    let mut attempted_ids: Vec<i64> = Vec::new();
    let mut outcomes: Vec<SourceOutcome> = Vec::new();
    let initial: Vec<InstanceRow> = ordered.iter().take(target).cloned().collect();
    attempted_ids.extend(initial.iter().map(|row| row.instance_id));
    let attempts = if ctx.settings.search_failover() {
        ctx.settings.search_attempts().max(1)
    } else {
        1
    };
    outcomes.extend(
        run_batch(
            ctx,
            &initial,
            &query,
            cancellation,
            hard_deadline,
            attempts,
            &mut messages,
        )
        .await,
    );
    let mut successes = outcomes
        .iter()
        .filter(|outcome| outcome.failure.is_none())
        .count();
    for outcome in outcomes.iter().filter(|outcome| outcome.failure.is_some()) {
        if let Some(failure) = &outcome.failure
            && cools_down(&failure.code)
            && tokio::time::Instant::now() < hard_deadline
        {
            cool_down(
                ctx,
                outcome.instance_id,
                &failure.code,
                failure.retry_after_secs,
            )
            .await;
        }
    }
    if ctx.settings.search_failover() {
        let budget = ordered.len();
        let mut cursor = initial.len();
        while successes < target && cursor < budget && tokio::time::Instant::now() < hard_deadline {
            let Some(row) = ordered.get(cursor).cloned() else {
                break;
            };
            cursor += 1;
            attempted_ids.push(row.instance_id);
            let outcome = search_instance_with_retries(
                ctx,
                &row,
                &query,
                attempts,
                cancellation,
                hard_deadline,
            )
            .await;
            if outcome.failure.is_none() {
                successes = successes.saturating_add(1);
            } else if let Some(failure) = &outcome.failure
                && cools_down(&failure.code)
            {
                cool_down(
                    ctx,
                    row.instance_id,
                    &failure.code,
                    failure.retry_after_secs,
                )
                .await;
            }
            outcomes.push(outcome);
        }
    }
    let upstream_ms = elapsed_ms(upstream_started);
    let OutcomeSummary {
        observations,
        failed,
        succeeded,
        fallbacks,
        parser,
        http_class,
        engines,
        unresponsive,
    } = summarise_outcomes(outcomes);
    record_engine_stats(ctx, engines, &unresponsive).await;
    let merge_started = Instant::now();
    let mut engine_weights = ctx.settings.engine_weights().clone();
    engine_weights.extend(
        request
            .engine_weights
            .iter()
            .map(|(engine, weight)| (engine.clone(), *weight)),
    );
    let mut exclude_engines = ctx.settings.exclude_engines().to_vec();
    for engine in &request.exclude_engines {
        if !exclude_engines
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(engine))
        {
            exclude_engines.push(engine.clone());
        }
    }
    let merged = merge(
        observations,
        &MergeOptions {
            cap: request.limit,
            snippet_chars: SNIPPET_CHARS,
            dedupe: request.dedupe,
            min_engines: request
                .min_engines
                .unwrap_or_else(|| ctx.settings.min_engines()),
            max_age_days: (freshness_days > 0).then_some(freshness_days),
            now_unix: crate::time::now_unix(),
            engine_weights: &engine_weights,
            exclude_engines: &exclude_engines,
            exclude_domains: ctx.settings.exclude_domains(),
        },
    );
    let merge_ms = elapsed_ms(merge_started);
    let parse_started = Instant::now();
    let results = result_items(&merged);
    let parse_ms = elapsed_ms(parse_started);
    let attempted = attempted_ids.len();
    let succeeded_count = succeeded.len();
    let partial = !failed.is_empty();
    if !failed.is_empty() {
        messages.push(Notice::warning(
            "search.sources_failed",
            format!("{} of {} instances did not answer", failed.len(), attempted),
            "retry, or run a scan to refresh the fleet",
        ));
    }
    let mut fallbacks = fallbacks;
    fallbacks.sort_unstable();
    fallbacks.dedup();
    let outcome = search_outcome(&results, succeeded_count, &failed, &messages);
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let output = SearchOutput {
        results,
        sources: SourcesInfo {
            selected: attempted_ids.clone(),
            succeeded,
            failed: failed.clone(),
            fallbacks,
            unresponsive_engines: unresponsive,
        },
        coverage: Coverage {
            attempted,
            succeeded: succeeded_count,
            results_before_merge: merged.results_before_merge,
            duplicate_groups: merged.duplicate_groups,
            partial,
        },
        cache: CacheInfo {
            enabled: ctx.cache.enabled(),
            hit: false,
            age_secs: None,
        },
        truncated: merged.truncated,
        outcome,
        messages,
        errors: failed,
        timing: SearchTiming {
            elapsed_ms,
            deadline_ms,
            queue_ms,
            selection_ms,
            upstream_ms,
            parse_ms,
            merge_ms,
        },
    };
    if ctx.cache.enabled()
        && let Ok(value) = serde_json::to_value(&output)
        && let Err(error) = ctx.cache.put(cache_key, Arc::new(value))
    {
        tracing::warn!(%error, "the search answer was not cached");
    }
    let bytes = serde_json::to_vec(&output)
        .map(|body| body.len() as u64)
        .unwrap_or(0);
    Ok(SearchOutcome {
        output,
        cache_hit: false,
        cache_age: None,
        instances: attempted_ids,
        parser,
        http_class,
        bytes,
    })
}

fn cached_search(ctx: &Arc<ToolContext>, cache_key: &str) -> Result<Option<SearchOutcome>> {
    let Some((cached, age)) = ctx.cache.get(cache_key)? else {
        return Ok(None);
    };
    let Ok(mut output) = serde_json::from_value::<SearchOutput>((*cached).clone()) else {
        return Ok(None);
    };
    output.cache = CacheInfo {
        enabled: true,
        hit: true,
        age_secs: Some(age),
    };
    ctx.metrics.count_cache_hit();
    Ok(Some(SearchOutcome {
        instances: output.sources.selected.clone(),
        output,
        cache_hit: true,
        cache_age: Some(age),
        parser: None,
        http_class: Some("cache".to_owned()),
        bytes: 0,
    }))
}

async fn prepare_fleet(
    ctx: &Arc<ToolContext>,
    messages: &mut Vec<Notice>,
) -> Result<Vec<InstanceRow>> {
    {
        let ctx = Arc::clone(ctx);
        blocking_store(move || ctx.ensure_bootstrap()).await?;
    }
    let refresh = (!ctx.settings.offline() && ctx.settings.public_fleet())
        .then(|| ctx.spawn_discovery_refresh())
        .flatten();
    let (rows, fleet_warning) = {
        let ctx = Arc::clone(ctx);
        blocking_store(move || resolve_fleet(&ctx)).await?
    };
    if let Some(warning) = fleet_warning {
        messages.push(Notice::warning(
            "search.fleet_fallback",
            warning,
            "run a scan to score the fleet",
        ));
    }
    if rows.is_empty()
        && let Some(handle) = refresh
    {
        let budget = std::time::Duration::from_secs(INLINE_REFRESH_BUDGET_SECS);
        let _ = tokio::time::timeout(budget, handle).await;
        let (rows, fleet_warning) = {
            let ctx = Arc::clone(ctx);
            blocking_store(move || resolve_fleet(&ctx)).await?
        };
        if let Some(warning) = fleet_warning {
            messages.push(Notice::warning(
                "search.fleet_fallback",
                warning,
                "run a scan to score the fleet",
            ));
        }
        return Ok(rows);
    }
    Ok(rows)
}

struct OutcomeSummary {
    observations: Vec<Observation>,
    failed: Vec<SourceFailure>,
    succeeded: Vec<i64>,
    fallbacks: Vec<i64>,
    parser: Option<String>,
    http_class: Option<String>,
    engines: std::collections::BTreeMap<i64, Vec<String>>,
    unresponsive: Vec<UnresponsiveEngine>,
}

fn summarise_outcomes(outcomes: Vec<SourceOutcome>) -> OutcomeSummary {
    let mut summary = OutcomeSummary {
        observations: Vec::new(),
        failed: Vec::new(),
        succeeded: Vec::new(),
        fallbacks: Vec::new(),
        parser: None,
        http_class: None,
        engines: std::collections::BTreeMap::new(),
        unresponsive: Vec::new(),
    };
    for outcome in outcomes {
        let SourceOutcome {
            instance_id,
            parser,
            observations,
            failure,
            http_class,
            unresponsive,
        } = outcome;
        for (engine, reason) in unresponsive {
            summary.unresponsive.push(UnresponsiveEngine {
                instance_id,
                engine,
                reason,
            });
        }
        if let Some(failure) = failure {
            summary.failed.push(failure);
            continue;
        }
        summary.succeeded.push(instance_id);
        if let Some(name) = &parser
            && name != "json"
        {
            summary.fallbacks.push(instance_id);
        }
        if summary.parser.is_none() {
            summary.parser.clone_from(&parser);
        }
        if summary.http_class.is_none() {
            summary.http_class.clone_from(&http_class);
        }
        let entry = summary.engines.entry(instance_id).or_default();
        for observation in &observations {
            for engine in &observation.engines {
                if !entry
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(engine))
                {
                    entry.push(engine.clone());
                }
            }
        }
        summary.observations.extend(observations);
    }
    summary
}

async fn record_engine_stats(
    ctx: &Arc<ToolContext>,
    engines: std::collections::BTreeMap<i64, Vec<String>>,
    unresponsive: &[UnresponsiveEngine],
) {
    let mut failures: std::collections::BTreeMap<i64, Vec<String>> =
        std::collections::BTreeMap::new();
    for item in unresponsive {
        let entry = failures.entry(item.instance_id).or_default();
        if !entry.iter().any(|engine| engine == &item.engine) {
            entry.push(item.engine.clone());
        }
    }
    let mut ids: Vec<i64> = engines.keys().copied().collect();
    for id in failures.keys() {
        if !ids.contains(id) {
            ids.push(*id);
        }
    }
    for id in ids {
        let ok = engines.get(&id).cloned().unwrap_or_default();
        let bad = failures.get(&id).cloned().unwrap_or_default();
        if ok.is_empty() && bad.is_empty() {
            continue;
        }
        let store = Arc::clone(&ctx.store);
        match tokio::task::spawn_blocking(move || store.record_engine_stats(id, &ok, &bad)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(%error, instance_id = id, "the engine counters could not be written");
            }
            Err(error) => {
                tracing::warn!(%error, instance_id = id, "the engine counter task failed");
            }
        }
    }
}

fn result_items(merged: &crate::merge::MergeOutput) -> Vec<SearchResultItem> {
    merged
        .results
        .iter()
        .map(|result| SearchResultItem {
            title: result.title.clone(),
            url: result.url.clone(),
            snippet: result.snippet.clone(),
            engines: Some(result.engines.clone()),
            sources: Some(
                result
                    .sources
                    .iter()
                    .map(|source| SourceObservation {
                        instance_id: source.instance_id,
                        instance_slug: crate::types::instance_slug(&source.instance_url),
                        instance_url: source.instance_url.clone(),
                        engines: source.engines.clone(),
                        parser: source.parser.clone(),
                        rank: source.rank,
                        score: source.instance_score,
                    })
                    .collect(),
            ),
            published_date: result.published_date.clone(),
            category: result.category.clone(),
            score: result.score,
            rank: result.rank,
            merged_from: result.merged_from,
            best_position: result.best_position,
            kind: Some(result.kind.clone()),
            truncated: Some(result.truncated),
        })
        .collect()
}

fn resolve_fleet(ctx: &ToolContext) -> Result<(Vec<InstanceRow>, Option<String>)> {
    let now = crate::time::now_rfc3339();
    let public_fleet = ctx.settings.public_fleet();
    let not_cooling = |row: &InstanceRow| {
        row.cooldown_until
            .as_deref()
            .is_none_or(|until| until <= now.as_str())
    };
    let allowed = |row: &InstanceRow| public_fleet || row.source == "manual";
    let fleet = ctx.store.fleet(64, &now)?;
    if !fleet.is_empty() {
        let ids: Vec<i64> = fleet.iter().map(|row| row.instance_id).collect();
        let rows = ctx.store.list_instances(&crate::db::InstanceFilter {
            ids,
            limit: 64,
            ..crate::db::InstanceFilter::default()
        })?;
        let rows: Vec<InstanceRow> = rows
            .into_iter()
            .filter(not_cooling)
            .filter(allowed)
            .collect();
        if !rows.is_empty() {
            return Ok((rows, None));
        }
    }
    let probation: Vec<InstanceRow> = ctx
        .store
        .list_instances(&crate::db::InstanceFilter {
            status: Some(InstanceStatus::Probation),
            min_score: Some(1.0),
            limit: 32,
            ..crate::db::InstanceFilter::default()
        })?
        .into_iter()
        .filter(not_cooling)
        .filter(allowed)
        .filter(|row| row.scan_enabled && !row.denied)
        .collect();
    if !probation.is_empty() {
        return Ok((
            probation,
            Some("no active fleet yet; probation instances were used for this search".to_owned()),
        ));
    }
    let candidates: Vec<InstanceRow> = ctx
        .store
        .list_instances(&crate::db::InstanceFilter {
            status: Some(InstanceStatus::Candidate),
            min_score: Some(1.0),
            limit: 32,
            ..crate::db::InstanceFilter::default()
        })?
        .into_iter()
        .filter(not_cooling)
        .filter(allowed)
        .filter(|row| row.scan_enabled && !row.denied)
        .filter(|row| row.hard_gate_passed == Some(true))
        .collect();
    if !candidates.is_empty() {
        return Ok((
            candidates,
            Some(
                "no active fleet yet; scanned candidates that passed every hard gate were used"
                    .to_owned(),
            ),
        ));
    }
    let unscored: Vec<InstanceRow> = ctx
        .store
        .list_instances(&crate::db::InstanceFilter {
            limit: 500,
            ..crate::db::InstanceFilter::default()
        })?
        .into_iter()
        .filter(|row| {
            !row.denied
                && row.scan_enabled
                && allowed(row)
                && not_cooling(row)
                && row.hard_gate_passed == Some(true)
                && row.kind != crate::types::InstanceType::Tor
                && matches!(
                    row.status,
                    InstanceStatus::Candidate | InstanceStatus::Probation
                )
        })
        .take(UNSCORED_FALLBACK_INSTANCES)
        .collect();
    if unscored.is_empty() {
        let hint = if public_fleet {
            "run `ownsearch scan` to score the fleet"
        } else {
            "public_fleet is off and no instance was added; run `ownsearch instances add URL` for your own SearXNG"
        };
        return Ok((Vec::new(), Some(format!("no instance can answer: {hint}"))));
    }
    Ok((
        unscored,
        Some(
            "no scored instance yet; the highest ranked discovered instances that passed every hard gate were tried for this search"
                .to_owned(),
        ),
    ))
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

fn mapped_range(days: u64) -> Option<TimeRange> {
    match days {
        0 => None,
        1 => Some(TimeRange::Day),
        2..=7 => Some(TimeRange::Week),
        8..=31 => Some(TimeRange::Month),
        32..=365 => Some(TimeRange::Year),
        _ => None,
    }
}

fn target_instances(mode: SearchMode, exhaustive_max: usize) -> usize {
    match mode {
        SearchMode::Fast => 1,
        SearchMode::Balanced => 3,
        SearchMode::Exhaustive => exhaustive_max.max(1),
    }
}

fn rotation_span(rows: usize, target: usize) -> usize {
    rows.min(target.saturating_mul(2).max(2))
}

fn capability_rank(row: &InstanceRow) -> u8 {
    if row.json_enabled == Some(true) {
        0
    } else if row.html_enabled == Some(true) {
        1
    } else {
        2
    }
}

fn order_instances(
    rows: &[InstanceRow],
    mode: SearchMode,
    exhaustive_max: usize,
    rotation: Option<usize>,
) -> Vec<InstanceRow> {
    let mut ordered: Vec<InstanceRow> = rows
        .iter()
        .filter(|row| !row.denied && row.kind != crate::types::InstanceType::Tor)
        .cloned()
        .collect();
    ordered.sort_by(|left, right| {
        right
            .trusted
            .cmp(&left.trusted)
            .then_with(|| right.manual_priority.cmp(&left.manual_priority))
            .then_with(|| capability_rank(left).cmp(&capability_rank(right)))
            .then_with(|| {
                right
                    .score
                    .unwrap_or(0.0)
                    .partial_cmp(&left.score.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.instance_id.cmp(&right.instance_id))
    });
    if let Some(offset) = rotation {
        let span = rotation_span(ordered.len(), target_instances(mode, exhaustive_max));
        if span > 1
            && let Some(slice) = ordered.get_mut(..span)
        {
            slice.rotate_left(offset % span);
        }
    }
    ordered
}

#[cfg(test)]
fn select_instances(
    rows: &[InstanceRow],
    mode: SearchMode,
    exhaustive_max: usize,
) -> Vec<InstanceRow> {
    order_instances(rows, mode, exhaustive_max, None)
        .into_iter()
        .take(target_instances(mode, exhaustive_max))
        .collect()
}

fn cools_down(code: &str) -> bool {
    matches!(
        code,
        "ownsearch.upstream_failed" | "ownsearch.timeout" | "ownsearch.rate_limited"
    )
}

async fn cool_down(
    ctx: &Arc<ToolContext>,
    instance_id: i64,
    code: &str,
    retry_after_secs: Option<u64>,
) {
    let wait = retry_after_secs
        .unwrap_or_else(|| ctx.settings.cooldown_secs())
        .min(604_800);
    let until = crate::time::rfc3339(
        crate::time::now_unix().saturating_add(i64::try_from(wait).unwrap_or(3_600)),
    );
    let store = Arc::clone(&ctx.store);
    let code = code.to_owned();
    let written =
        tokio::task::spawn_blocking(move || store.mark_search_failure(instance_id, &until, &code))
            .await;
    match written {
        Ok(Ok(())) => ctx.bump_fleet_generation(),
        Ok(Err(error)) => {
            tracing::warn!(%error, instance_id, "the cooldown was not recorded");
        }
        Err(error) => {
            tracing::warn!(%error, instance_id, "the cooldown task failed");
        }
    }
}

async fn search_instance_with_retries(
    ctx: &ToolContext,
    row: &InstanceRow,
    query: &SearchQuery,
    attempts: u32,
    cancellation: &CancellationToken,
    deadline: tokio::time::Instant,
) -> SourceOutcome {
    let mut attempt = 0u32;
    loop {
        attempt = attempt.saturating_add(1);
        let outcome =
            match tokio::time::timeout_at(deadline, search_instance(ctx, row, query, cancellation))
                .await
            {
                Ok(outcome) => outcome,
                Err(_) => {
                    return failure(
                        row,
                        "ownsearch.timeout",
                        "the deadline passed while an instance was answering".to_owned(),
                    );
                }
            };
        if outcome.failure.is_none() || !retryable(&outcome) || attempt >= attempts {
            return outcome;
        }
        if tokio::time::Instant::now() >= deadline {
            return outcome;
        }
        let base = 100u64.saturating_mul(1u64 << attempt.saturating_sub(1).min(4));
        let mut jitter = [0u8; 1];
        let _ = getrandom::fill(&mut jitter);
        let delay = base.saturating_add(u64::from(jitter[0]) % 50);
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
}

fn retryable(outcome: &SourceOutcome) -> bool {
    outcome.failure.as_ref().is_some_and(|failure| {
        matches!(
            failure.code.as_str(),
            "ownsearch.upstream_failed" | "ownsearch.timeout"
        )
    })
}

async fn run_batch(
    ctx: &Arc<ToolContext>,
    rows: &[InstanceRow],
    query: &SearchQuery,
    cancellation: &CancellationToken,
    hard_deadline: tokio::time::Instant,
    attempts: u32,
    messages: &mut Vec<Notice>,
) -> Vec<SourceOutcome> {
    let mut set: JoinSet<SourceOutcome> = JoinSet::new();
    for row in rows {
        let ctx = Arc::clone(ctx);
        let query = query.clone();
        let cancellation = cancellation.clone();
        let row = row.clone();
        set.spawn(async move {
            search_instance_with_retries(&ctx, &row, &query, attempts, &cancellation, hard_deadline)
                .await
        });
    }
    let mut outcomes = Vec::new();
    loop {
        let next = tokio::time::timeout_at(hard_deadline, set.join_next()).await;
        match next {
            Ok(Some(Ok(outcome))) => outcomes.push(outcome),
            Ok(Some(Err(error))) => messages.push(Notice::warning(
                "search.task_failed",
                format!("one search task failed: {error}"),
                "read the other sources, or retry if that one matters",
            )),
            Ok(None) => break,
            Err(_) => {
                set.abort_all();
                while set.join_next().await.is_some() {}
                let answered: Vec<i64> =
                    outcomes.iter().map(|outcome| outcome.instance_id).collect();
                for row in rows {
                    if !answered.contains(&row.instance_id) {
                        outcomes.push(failure(
                            row,
                            "ownsearch.timeout",
                            "this instance did not answer before the deadline".to_owned(),
                        ));
                    }
                }
                messages.push(Notice::warning(
                    "search.deadline",
                    "the deadline passed before every instance answered; the missing ones are listed in sources.failed",
                    "raise the deadline, or use fast mode for fewer instances",
                ));
                break;
            }
        }
    }
    outcomes
}

async fn search_instance(
    ctx: &ToolContext,
    row: &InstanceRow,
    query: &SearchQuery,
    cancellation: &CancellationToken,
) -> SourceOutcome {
    let _permit = ctx.acquire_instance(row.instance_id, row.trusted).await;
    let base = match Url::parse(&row.url) {
        Ok(base) => base,
        Err(error) => {
            return failure(
                row,
                "ownsearch.bad_request",
                format!("the instance url does not parse: {error}"),
            );
        }
    };
    let auth = match instance_auth(ctx, row) {
        Ok(auth) => auth,
        Err(error) => return failure(row, error.code(), error.to_string()),
    };
    let auth = auth.as_ref();
    let now = crate::time::now_rfc3339();
    let throttled = row
        .json_throttled_until
        .as_deref()
        .is_some_and(|until| until > now.as_str());
    let mut report = if throttled {
        ProbeReport::failed(
            ProbeOutcome::RateLimited,
            "the instance is limiting JSON requests until its window ends".to_owned(),
        )
    } else {
        search_json::fetch(&ctx.client, &base, query, cancellation, auth).await
    };
    if report.outcome == ProbeOutcome::ParserError
        && let Some(sample) = &report.sample
    {
        crate::parser_sample::save(
            &ctx.settings,
            row.instance_id,
            row.version.as_deref(),
            sample,
        );
    }
    let unresponsive = report
        .value
        .as_ref()
        .map(unresponsive_entries)
        .unwrap_or_default();
    if report.outcome.is_success() {
        let observations = observations_from_json(row, &report);
        let unusable = observations.is_empty()
            && report
                .value
                .as_ref()
                .is_some_and(|answer| !answer.results.is_empty());
        if !unusable {
            if row.json_throttled_until.is_some() {
                clear_json_throttled(ctx, row.instance_id).await;
            }
            return SourceOutcome {
                instance_id: row.instance_id,
                parser: Some("json".to_owned()),
                observations,
                failure: None,
                http_class: report.http_status.map(http_class),
                unresponsive,
            };
        }
        report.outcome = ProbeOutcome::ParserError;
        report.error_message = Some("the results had no usable title or url".to_owned());
    }
    if report.outcome == ProbeOutcome::ParserError {
        for format in ["rss", "csv"] {
            if let Ok(entries) =
                crate::formats::fetch(&ctx.client, &base, query, format, cancellation, auth).await
                && !entries.is_empty()
            {
                let observations = observations_from_formats(row, format, &entries);
                if !observations.is_empty() {
                    return SourceOutcome {
                        instance_id: row.instance_id,
                        parser: Some(format.to_owned()),
                        observations,
                        failure: None,
                        http_class: None,
                        unresponsive: Vec::new(),
                    };
                }
            }
        }
    }
    if report.outcome == ProbeOutcome::ParserError
        && ctx.browser.enabled()
        && !row.trusted
        && auth.is_none()
        && let Ok(url) = crate::client::search_url(&base, &[("q", query.query.clone())])
        && let Ok((html, _)) = ctx
            .browser
            .page(&url, std::time::Duration::from_secs(30))
            .await
    {
        let page = crate::searxng::search_html::HtmlSearch::parse(&html, &url);
        if !page.blocked && !page.results.is_empty() {
            let observations = observations_from_page(row, &page);
            if !observations.is_empty() {
                return SourceOutcome {
                    instance_id: row.instance_id,
                    parser: Some("browser".to_owned()),
                    observations,
                    failure: None,
                    http_class: Some(http_class(200)),
                    unresponsive: Vec::new(),
                };
            }
        }
    }
    let html_allowed = row.html_fallback_allowed || ctx.settings.html_fallback();
    let limiter_throttled = report.outcome == ProbeOutcome::RateLimited;
    let denied = matches!(
        report.outcome,
        ProbeOutcome::Refused | ProbeOutcome::ParserError | ProbeOutcome::HttpError
    ) || limiter_throttled;
    if denied && html_allowed {
        let html = search_html::fetch(&ctx.client, &base, query, cancellation, auth).await;
        if html.outcome.is_success() {
            if limiter_throttled && !throttled {
                mark_json_throttled(ctx, row.instance_id, report.retry_after_secs).await;
            }
            let observations = observations_from_html(row, &html);
            return SourceOutcome {
                instance_id: row.instance_id,
                parser: Some("html".to_owned()),
                observations,
                failure: None,
                http_class: html.http_status.map(http_class),
                unresponsive: Vec::new(),
            };
        }
        let retry_after = html.retry_after_secs.or(report.retry_after_secs);
        if report.outcome == ProbeOutcome::RateLimited {
            return failure_after(
                row,
                Some("json"),
                "ownsearch.rate_limited",
                "the instance is limiting requests".to_owned(),
                retry_after,
            );
        }
        return failure_after(
            row,
            Some("html"),
            code_for(html.outcome, html.http_status),
            error_text(&html.error_message, html.outcome),
            retry_after,
        );
    }
    failure_after(
        row,
        Some("json"),
        code_for(report.outcome, report.http_status),
        error_text(&report.error_message, report.outcome),
        report.retry_after_secs,
    )
}

const DEFAULT_JSON_THROTTLE_SECS: u64 = 3_600;

async fn mark_json_throttled(ctx: &ToolContext, instance_id: i64, retry_after: Option<u64>) {
    let secs = retry_after
        .unwrap_or(DEFAULT_JSON_THROTTLE_SECS)
        .min(604_800);
    let until = crate::time::rfc3339(
        crate::time::now_unix().saturating_add(i64::try_from(secs).unwrap_or(3600)),
    );
    let store = Arc::clone(&ctx.store);
    if let Err(error) =
        tokio::task::spawn_blocking(move || store.mark_json_throttled(instance_id, &until)).await
    {
        tracing::warn!(%error, instance_id, "the json throttle was not recorded");
    }
}

async fn clear_json_throttled(ctx: &ToolContext, instance_id: i64) {
    let store = Arc::clone(&ctx.store);
    if let Err(error) =
        tokio::task::spawn_blocking(move || store.clear_json_throttled(instance_id)).await
    {
        tracing::warn!(%error, instance_id, "the json throttle was not cleared");
    }
}

fn clean_url(raw: &str) -> Option<String> {
    let normalized = crate::types::normalize_url(raw).ok()?;
    if normalized.starts_with("https://") || normalized.starts_with("http://") {
        Some(normalized)
    } else {
        None
    }
}

fn clean_short_text(raw: &str) -> String {
    let (capped, _) = cap(&sanitize(raw), 64);
    capped
}

fn observations_from_json(
    row: &InstanceRow,
    report: &ProbeReport<search_json::JsonSearch>,
) -> Vec<Observation> {
    let Some(search) = &report.value else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, item) in search.results.iter().enumerate() {
        let Some(url) = item.url.as_deref().and_then(clean_url) else {
            continue;
        };
        let Some(title) = item.title.as_deref() else {
            continue;
        };
        let (title, _) = cap(&sanitize(title), TITLE_CHARS);
        if title.is_empty() {
            continue;
        }
        let snippet = item
            .content
            .as_deref()
            .map(sanitize)
            .filter(|text| !text.is_empty());
        let engines: Vec<String> = item
            .engine_names()
            .iter()
            .map(|name| clean_short_text(name))
            .filter(|name| !name.is_empty())
            .collect();
        let fallback = u32::try_from(index).unwrap_or(0).saturating_add(1);
        let positions: Vec<u32> = (0..engines.len())
            .map(|position| {
                item.positions
                    .get(position)
                    .copied()
                    .or_else(|| item.positions.first().copied())
                    .unwrap_or(fallback)
            })
            .collect();
        let rank = positions.iter().copied().min();
        out.push(Observation {
            instance_id: row.instance_id,
            instance_url: row.url.clone(),
            instance_score: row.score.unwrap_or(0.0),
            parser: "json".to_owned(),
            engines,
            positions,
            upstream_score: item.score,
            category: item
                .category
                .as_deref()
                .map(clean_short_text)
                .filter(|category| !category.is_empty()),
            rank,
            title,
            url,
            snippet,
            published_date: item.published_date.as_deref().map(clean_short_text),
        });
    }
    out
}

fn observations_from_page(
    row: &InstanceRow,
    page: &crate::searxng::search_html::HtmlSearch,
) -> Vec<Observation> {
    page.results
        .iter()
        .filter_map(|item| {
            let url = clean_url(&item.url)?;
            let (title, _) = cap(&sanitize(&item.title), TITLE_CHARS);
            if title.is_empty() {
                return None;
            }
            let snippet = item.snippet.as_deref().map(|raw| {
                let (text, _) = cap(&sanitize(raw), 300);
                text
            });
            let engines: Vec<String> = item
                .engine
                .as_deref()
                .map(|names| {
                    names
                        .split(',')
                        .map(|name| clean_short_text(name.trim()))
                        .filter(|name| !name.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            Some(Observation {
                instance_id: row.instance_id,
                instance_url: row.url.clone(),
                instance_score: row.score.unwrap_or(0.0),
                parser: "browser".to_owned(),
                engines,
                positions: vec![item.position],
                upstream_score: None,
                category: None,
                rank: Some(item.position),
                title,
                url,
                snippet,
                published_date: None,
            })
        })
        .collect()
}

fn observations_from_formats(
    row: &InstanceRow,
    parser: &str,
    entries: &[crate::formats::Entry],
) -> Vec<Observation> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let url = clean_url(&entry.url)?;
            let (title, _) = cap(&sanitize(&entry.title), TITLE_CHARS);
            if title.is_empty() {
                return None;
            }
            let snippet = (!entry.snippet.is_empty()).then(|| {
                let (text, _) = cap(&sanitize(&entry.snippet), 300);
                text
            });
            let position = u32::try_from(index + 1).unwrap_or(u32::MAX);
            Some(Observation {
                instance_id: row.instance_id,
                instance_url: row.url.clone(),
                instance_score: row.score.unwrap_or(0.0),
                parser: parser.to_owned(),
                engines: entry
                    .engine
                    .as_deref()
                    .map(clean_short_text)
                    .filter(|name| !name.is_empty())
                    .into_iter()
                    .collect(),
                positions: vec![position],
                upstream_score: None,
                category: None,
                rank: Some(position),
                title,
                url,
                snippet,
                published_date: None,
            })
        })
        .collect()
}

fn observations_from_html(
    row: &InstanceRow,
    report: &ProbeReport<search_html::HtmlSearch>,
) -> Vec<Observation> {
    let Some(search) = &report.value else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in &search.results {
        let Some(url) = clean_url(&item.url) else {
            continue;
        };
        let (title, _) = cap(&sanitize(&item.title), TITLE_CHARS);
        if title.is_empty() {
            continue;
        }
        let snippet = item
            .snippet
            .as_deref()
            .map(|text| cap(&sanitize(text), SNIPPET_CHARS).0)
            .filter(|text| !text.is_empty());
        let engines: Vec<String> = item
            .engine
            .as_deref()
            .map(|names| {
                names
                    .split(',')
                    .map(|name| clean_short_text(name.trim()))
                    .filter(|name| !name.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        out.push(Observation {
            instance_id: row.instance_id,
            instance_url: row.url.clone(),
            instance_score: row.score.unwrap_or(0.0),
            parser: "html".to_owned(),
            engines,
            positions: vec![item.position],
            upstream_score: None,
            category: None,
            rank: Some(item.position),
            title,
            url,
            snippet,
            published_date: None,
        });
    }
    out
}

fn unresponsive_entries(search: &search_json::JsonSearch) -> Vec<(String, String)> {
    search
        .unresponsive_engines
        .iter()
        .filter_map(|entry| {
            let list = entry.as_array()?;
            let engine = list.first()?.as_str()?;
            let reason = list
                .get(1)
                .and_then(Value::as_str)
                .unwrap_or("no reason given");
            Some((clean_short_text(engine), clean_short_text(reason)))
        })
        .collect()
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn instance_auth(
    ctx: &ToolContext,
    row: &InstanceRow,
) -> Result<Option<crate::auth::InstanceAuth>> {
    let mut records = ctx.store.auth_for(&[row.instance_id])?;
    let Some(record) = records.remove(&row.instance_id) else {
        return Ok(None);
    };
    crate::auth::InstanceAuth::resolve(&row.url, record).map(Some)
}

fn failure(row: &InstanceRow, code: &str, reason: String) -> SourceOutcome {
    failure_after(row, None, code, reason, None)
}

fn failure_after(
    row: &InstanceRow,
    parser: Option<&str>,
    code: &str,
    reason: String,
    retry_after_secs: Option<u64>,
) -> SourceOutcome {
    SourceOutcome {
        instance_id: row.instance_id,
        parser: None,
        observations: Vec::new(),
        failure: Some(SourceFailure {
            instance_id: row.instance_id,
            url: row.url.clone(),
            parser: parser.map(str::to_owned),
            code: code.to_owned(),
            reason: cap(&sanitize(&reason), 200).0,
            retry_after_secs,
        }),
        http_class: None,
        unresponsive: Vec::new(),
    }
}

fn code_for(outcome: ProbeOutcome, status: Option<u16>) -> &'static str {
    match outcome {
        ProbeOutcome::RateLimited => "ownsearch.rate_limited",
        ProbeOutcome::ParserError => "ownsearch.parser_failed",
        ProbeOutcome::Timeout => "ownsearch.timeout",
        ProbeOutcome::Blocked | ProbeOutcome::Refused => "ownsearch.policy_refused",
        ProbeOutcome::HttpError => status
            .and_then(client_error_code)
            .unwrap_or("ownsearch.upstream_failed"),
        ProbeOutcome::Success => "ownsearch.upstream_failed",
    }
}

fn client_error_code(status: u16) -> Option<&'static str> {
    match status {
        408 | 429 => None,
        400 | 422 => Some("ownsearch.bad_request"),
        400..=499 => Some("ownsearch.policy_refused"),
        _ => None,
    }
}

fn error_text(message: &Option<String>, outcome: ProbeOutcome) -> String {
    message
        .clone()
        .unwrap_or_else(|| format!("the instance failed with {}", outcome.as_str()))
}

fn search_outcome(
    results: &[SearchResultItem],
    succeeded: usize,
    failed: &[SourceFailure],
    messages: &[Notice],
) -> String {
    if results.is_empty() {
        return if succeeded == 0 && !failed.is_empty() {
            "failed".to_owned()
        } else {
            "empty".to_owned()
        };
    }
    if !failed.is_empty() || !messages.is_empty() {
        "partial".to_owned()
    } else {
        "success".to_owned()
    }
}

fn http_class(status: u16) -> String {
    let class = status / 100;
    format!("{class}xx")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::InstanceType;

    fn row(instance_id: i64, score: f64, status: InstanceStatus) -> InstanceRow {
        InstanceRow {
            instance_id,
            slug: format!("searx{instance_id}-example"),
            url: format!("https://searx{instance_id}.example/"),
            host: format!("searx{instance_id}.example"),
            kind: InstanceType::Normal,
            status,
            version: None,
            score: Some(score),
            rank: Some(instance_id),
            hard_gate_passed: Some(true),
            reasons: Vec::new(),
            json_enabled: Some(true),
            html_enabled: Some(false),
            google_exact: false,
            google_cse: false,
            consecutive_successes: 2,
            consecutive_failures: 0,
            last_success_at: None,
            last_failure_at: None,
            next_scan_at: None,
            cooldown_until: None,
            scan_enabled: true,
            html_fallback_allowed: false,
            discovery_rank: None,
            network_group: None,
            pinned: false,
            denied: false,
            trusted: false,
            source: "test".to_owned(),
            json_throttled_until: None,
            manual_priority: 0,
            added_at: String::new(),
        }
    }

    #[test]
    fn the_mode_decides_who_receives_the_query() {
        let rows: Vec<InstanceRow> = (1..=6)
            .map(|id| row(id, 100.0 - id as f64, InstanceStatus::Active))
            .collect();
        assert_eq!(select_instances(&rows, SearchMode::Fast, 5).len(), 1);
        assert_eq!(select_instances(&rows, SearchMode::Balanced, 5).len(), 3);
        assert_eq!(select_instances(&rows, SearchMode::Exhaustive, 5).len(), 5);
    }

    #[test]
    fn a_tor_instance_never_receives_a_query() {
        let mut rows = vec![row(1, 90.0, InstanceStatus::Active)];
        if let Some(first) = rows.first_mut() {
            first.kind = InstanceType::Tor;
        }
        assert!(select_instances(&rows, SearchMode::Balanced, 5).is_empty());
    }

    #[test]
    fn http_classes_are_reported_as_a_family() {
        assert_eq!(http_class(200), "2xx");
        assert_eq!(http_class(429), "4xx");
        assert_eq!(http_class(503), "5xx");
    }

    #[test]
    fn every_probe_outcome_maps_to_its_own_failure_code() {
        assert_eq!(
            code_for(ProbeOutcome::RateLimited, Some(429)),
            "ownsearch.rate_limited"
        );
        assert_eq!(
            code_for(ProbeOutcome::ParserError, Some(200)),
            "ownsearch.parser_failed"
        );
        assert_eq!(code_for(ProbeOutcome::Timeout, None), "ownsearch.timeout");
        assert_eq!(
            code_for(ProbeOutcome::Blocked, Some(401)),
            "ownsearch.policy_refused"
        );
        assert_eq!(
            code_for(ProbeOutcome::Refused, Some(403)),
            "ownsearch.policy_refused"
        );
        assert_eq!(
            code_for(ProbeOutcome::HttpError, Some(503)),
            "ownsearch.upstream_failed"
        );
        assert_eq!(
            code_for(ProbeOutcome::HttpError, Some(400)),
            "ownsearch.bad_request"
        );
        assert_eq!(
            code_for(ProbeOutcome::HttpError, Some(404)),
            "ownsearch.policy_refused"
        );
        assert_eq!(
            code_for(ProbeOutcome::HttpError, Some(408)),
            "ownsearch.upstream_failed"
        );
    }

    #[test]
    fn an_instance_that_answers_outranks_a_higher_score_that_refuses() {
        let mut refusing = row(1, 91.0, InstanceStatus::Candidate);
        refusing.json_enabled = Some(false);
        refusing.html_enabled = Some(false);
        let answering = row(2, 25.0, InstanceStatus::Candidate);
        let mut html_only = row(3, 50.0, InstanceStatus::Candidate);
        html_only.json_enabled = Some(false);
        html_only.html_enabled = Some(true);
        let selected =
            select_instances(&[refusing, answering, html_only], SearchMode::Exhaustive, 3);
        let ids: Vec<i64> = selected.iter().map(|row| row.instance_id).collect();
        assert_eq!(
            ids,
            vec![2, 3, 1],
            "an instance that answers comes before a higher score that refuses"
        );
    }
}
