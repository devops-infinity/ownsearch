use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::context::ToolContext;
use crate::db::InstanceRow;
use crate::error::{Error, Result};
use crate::merge::{Observation, merge};
use crate::response::{
    CacheInfo, Coverage, SearchOutput, SearchResultItem, SearchTiming, SourceFailure,
    SourceObservation, SourcesInfo,
};
use crate::sanitize::{cap, sanitize};
use crate::searxng::{ProbeReport, SearchQuery, search_html, search_json};
use crate::types::{InstanceStatus, ProbeOutcome, SafeSearch, SearchMode, TimeRange};

const SNIPPET_CHARS: usize = 300;
const TITLE_CHARS: usize = 300;
const UNSCORED_FALLBACK_INSTANCES: usize = 32;
const SEARCH_DISCOVERY_BUDGET_SECS: u64 = 5;

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
        .unwrap_or_else(|| ctx.settings.search_deadline_secs());
    let deadline_ms = deadline_secs.saturating_mul(1_000);
    let cache_key = format!(
        "ownsearch_search:{}:{}",
        ctx.fleet_generation(),
        ctx.hash_args(&request.canonical())
    );
    if let Some(outcome) = cached_search(ctx, &cache_key)? {
        return Ok(outcome);
    }
    let _search_slot = ctx.acquire_search().await?;
    let mut warnings: Vec<String> = Vec::new();
    let rows = prepare_fleet(ctx, cancellation, &mut warnings).await?;
    let rotation = ctx.settings.round_robin().then(|| {
        ctx.next_rotation(rotation_span(
            rows.len(),
            target_instances(request.mode, ctx.settings.exhaustive_max()),
        ))
    });
    let ordered = order_instances(&rows, request.mode, ctx.settings.exhaustive_max(), rotation);
    if ordered.is_empty() {
        return Err(Error::NoFleet);
    }
    let target = target_instances(request.mode, ctx.settings.exhaustive_max());
    let query = request.to_query();
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
            &mut warnings,
        )
        .await,
    );
    let mut successes = outcomes
        .iter()
        .filter(|outcome| outcome.failure.is_none())
        .count();
    for outcome in outcomes.iter().filter(|outcome| outcome.failure.is_some()) {
        if let Some(failure) = &outcome.failure {
            cool_down(ctx, outcome.instance_id, &failure.code).await;
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
            } else if let Some(failure) = &outcome.failure {
                cool_down(ctx, row.instance_id, &failure.code).await;
            }
            outcomes.push(outcome);
        }
    }
    let OutcomeSummary {
        observations,
        failed,
        succeeded,
        parser,
        http_class,
    } = summarise_outcomes(outcomes);
    let merged = merge(observations, request.limit, SNIPPET_CHARS);
    let results = result_items(&merged);
    let attempted = attempted_ids.len();
    let succeeded_count = succeeded.len();
    let partial = !failed.is_empty();
    let mut fallbacks: Vec<i64> = results
        .iter()
        .filter_map(|result| result.sources.as_ref())
        .flatten()
        .filter(|source| source.parser == "html")
        .map(|source| source.instance_id)
        .collect();
    fallbacks.sort_unstable();
    fallbacks.dedup();
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let output = SearchOutput {
        results,
        sources: SourcesInfo {
            selected: attempted_ids.clone(),
            succeeded,
            failed: failed.clone(),
            fallbacks,
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
        warnings,
        errors: failed,
        timing: SearchTiming {
            elapsed_ms,
            deadline_ms,
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
    cancellation: &CancellationToken,
    warnings: &mut Vec<String>,
) -> Result<Vec<InstanceRow>> {
    {
        let ctx = Arc::clone(ctx);
        blocking_store(move || ctx.ensure_bootstrap()).await?;
    }
    if !ctx.settings.offline() {
        let budget = Duration::from_secs(SEARCH_DISCOVERY_BUDGET_SECS);
        match tokio::time::timeout(
            budget,
            crate::scanner::refresh_discovery(ctx, false, cancellation),
        )
        .await
        {
            Ok(Ok(outcome)) => {
                if let Some(error) = outcome.error {
                    warnings.push(format!("discovery refresh failed: {error}"));
                }
            }
            Ok(Err(error)) => warnings.push(format!("discovery refresh failed: {error}")),
            Err(_) => warnings.push(format!(
                "discovery refresh took longer than {SEARCH_DISCOVERY_BUDGET_SECS} seconds and was skipped; the stored fleet answered this search"
            )),
        }
    }
    let (rows, fleet_warning) = {
        let ctx = Arc::clone(ctx);
        blocking_store(move || resolve_fleet(&ctx)).await?
    };
    if let Some(warning) = fleet_warning {
        warnings.push(warning);
    }
    Ok(rows)
}

struct OutcomeSummary {
    observations: Vec<Observation>,
    failed: Vec<SourceFailure>,
    succeeded: Vec<i64>,
    parser: Option<String>,
    http_class: Option<String>,
}

fn summarise_outcomes(outcomes: Vec<SourceOutcome>) -> OutcomeSummary {
    let mut summary = OutcomeSummary {
        observations: Vec::new(),
        failed: Vec::new(),
        succeeded: Vec::new(),
        parser: None,
        http_class: None,
    };
    for outcome in outcomes {
        if let Some(failure) = outcome.failure {
            summary.failed.push(failure);
            continue;
        }
        summary.succeeded.push(outcome.instance_id);
        if summary.parser.is_none() {
            summary.parser.clone_from(&outcome.parser);
        }
        if summary.http_class.is_none() {
            summary.http_class.clone_from(&outcome.http_class);
        }
        summary.observations.extend(outcome.observations);
    }
    summary
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
                        engine: source.engine.clone(),
                        parser: source.parser.clone(),
                        rank: source.rank,
                        score: source.instance_score,
                    })
                    .collect(),
            ),
            published_date: result.published_date.clone(),
            score: result.score,
            rank: result.rank,
            kind: Some(result.kind.clone()),
            truncated: Some(result.truncated),
        })
        .collect()
}

fn resolve_fleet(ctx: &ToolContext) -> Result<(Vec<InstanceRow>, Option<String>)> {
    let now = crate::time::now_rfc3339();
    let not_cooling = |row: &InstanceRow| {
        row.cooldown_until
            .as_deref()
            .is_none_or(|until| until <= now.as_str())
    };
    let fleet = ctx.store.fleet(64, &now)?;
    if !fleet.is_empty() {
        let ids: Vec<i64> = fleet.iter().map(|row| row.instance_id).collect();
        let rows = ctx.store.list_instances(&crate::db::InstanceFilter {
            ids,
            limit: 64,
            ..crate::db::InstanceFilter::default()
        })?;
        let rows: Vec<InstanceRow> = rows.into_iter().filter(not_cooling).collect();
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
                && not_cooling(row)
                && row.hard_gate_passed != Some(false)
                && row.kind != crate::types::InstanceType::Tor
                && matches!(
                    row.status,
                    InstanceStatus::Candidate | InstanceStatus::Probation
                )
        })
        .take(UNSCORED_FALLBACK_INSTANCES)
        .collect();
    if unscored.is_empty() {
        return Ok((Vec::new(), None));
    }
    Ok((
        unscored,
        Some(
            "no scored instance yet; the highest ranked discovered instances were tried for this search"
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
        capability_rank(left)
            .cmp(&capability_rank(right))
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

async fn cool_down(ctx: &Arc<ToolContext>, instance_id: i64, code: &str) {
    let until = crate::time::rfc3339(
        crate::time::now_unix()
            .saturating_add(i64::try_from(ctx.settings.cooldown_secs()).unwrap_or(3_600)),
    );
    let store = Arc::clone(&ctx.store);
    let code = code.to_owned();
    let written =
        tokio::task::spawn_blocking(move || store.mark_search_failure(instance_id, &until, &code))
            .await;
    match written {
        Ok(Ok(())) => {}
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
                        "the search deadline passed while one instance was answering".to_owned(),
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
    warnings: &mut Vec<String>,
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
            Ok(Some(Err(error))) => warnings.push(format!("a search task failed: {error}")),
            Ok(None) => break,
            Err(_) => {
                set.abort_all();
                while set.join_next().await.is_some() {}
                warnings.push(
                    "the search deadline passed; unfinished instances were dropped".to_owned(),
                );
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
    let base = match Url::parse(&row.url) {
        Ok(base) => base,
        Err(error) => {
            return failure(row, "ownsearch.bad_request", format!("url: {error}"));
        }
    };
    let report = search_json::fetch(&ctx.client, &base, query, cancellation).await;
    if report.outcome.is_success() {
        let observations = observations_from_json(row, &report);
        if observations.is_empty()
            && report
                .value
                .as_ref()
                .is_some_and(|answer| !answer.results.is_empty())
        {
            return failure(
                row,
                "ownsearch.parser_failed",
                "the instance answered results that carried no usable title and url".to_owned(),
            );
        }
        return SourceOutcome {
            instance_id: row.instance_id,
            parser: Some("json".to_owned()),
            observations,
            failure: None,
            http_class: report.http_status.map(http_class),
        };
    }
    if report.outcome == ProbeOutcome::ParserError {
        for format in ["rss", "csv"] {
            if let Ok(entries) =
                crate::formats::fetch(&ctx.client, &base, query, format, cancellation).await
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
                    };
                }
            }
        }
    }
    let html_allowed = row.html_fallback_allowed || ctx.settings.html_fallback();
    let denied = matches!(
        report.outcome,
        ProbeOutcome::Refused | ProbeOutcome::ParserError | ProbeOutcome::HttpError
    );
    if denied && html_allowed {
        let html = search_html::fetch(&ctx.client, &base, query, cancellation).await;
        if html.outcome.is_success() {
            let observations = observations_from_html(row, &html);
            return SourceOutcome {
                instance_id: row.instance_id,
                parser: Some("html".to_owned()),
                observations,
                failure: None,
                http_class: html.http_status.map(http_class),
            };
        }
        return failure(
            row,
            code_for(html.outcome, html.http_status),
            error_text(&html.error_message, html.outcome),
        );
    }
    failure(
        row,
        code_for(report.outcome, report.http_status),
        error_text(&report.error_message, report.outcome),
    )
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
        let engine = item
            .engine_names()
            .first()
            .map(|name| clean_short_text(name));
        out.push(Observation {
            instance_id: row.instance_id,
            instance_url: row.url.clone(),
            instance_score: row.score.unwrap_or(0.0),
            parser: "json".to_owned(),
            engine,
            rank: item
                .observed_rank()
                .or_else(|| Some(u32::try_from(index).unwrap_or(0).saturating_add(1))),
            title,
            url,
            snippet,
            published_date: item.published_date.as_deref().map(clean_short_text),
        });
    }
    out
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
            Some(Observation {
                instance_id: row.instance_id,
                instance_url: row.url.clone(),
                instance_score: row.score.unwrap_or(0.0),
                parser: parser.to_owned(),
                engine: entry.engine.clone(),
                rank: Some(u32::try_from(index + 1).unwrap_or(u32::MAX)),
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
        out.push(Observation {
            instance_id: row.instance_id,
            instance_url: row.url.clone(),
            instance_score: row.score.unwrap_or(0.0),
            parser: "html".to_owned(),
            engine: item.engine.as_deref().map(clean_short_text),
            rank: Some(item.position),
            title,
            url,
            snippet,
            published_date: None,
        });
    }
    out
}

fn failure(row: &InstanceRow, code: &str, reason: String) -> SourceOutcome {
    SourceOutcome {
        instance_id: row.instance_id,
        parser: None,
        observations: Vec::new(),
        failure: Some(SourceFailure {
            instance_id: row.instance_id,
            url: row.url.clone(),
            code: code.to_owned(),
            reason: cap(&sanitize(&reason), 200).0,
        }),
        http_class: None,
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
        .unwrap_or_else(|| format!("the instance answered {}", outcome.as_str()))
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
