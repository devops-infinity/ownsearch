use std::sync::Arc;
use std::time::Instant;

use rmcp::model::{CallToolResult, ContentBlock};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::context::ToolContext;
use crate::db::InstanceFilter;
use crate::error::{Error, Result};
use crate::ledger::CallRecord;
use crate::projection::{MAX_RESULT_BYTES, fit_to_budget, shape_search, summary_text};
use crate::response::{
    DbQueryOutput, EngineStatInfo, HealthItem, InstanceHealthOutput, InstanceInfo,
    InstanceInfoOutput, ScanOutput, SessionOutput, ToolUsage,
};
use crate::search::{SearchRequest, search};
use crate::tools::args::{
    DbCrudArgs, DbQueryArgs, FetchArgs, InstanceHealthArgs, InstanceInfoArgs, ScanArgs, SearchArgs,
    SessionArgs,
};
use crate::types::TextMode;

pub async fn call(
    ctx: &Arc<ToolContext>,
    tool_name: &str,
    arguments: Option<&Map<String, Value>>,
    cancellation: &CancellationToken,
) -> CallToolResult {
    let started = Instant::now();
    let args_value = arguments
        .map(|map| Value::Object(map.clone()))
        .unwrap_or_else(|| json!({}));
    let args_hash = ctx.hash_args(&args_value);
    let outcome = dispatch(ctx, tool_name, arguments, cancellation).await;
    match outcome {
        Ok(result) => {
            let bytes = serde_json::to_vec(&result.structured)
                .map(|value| value.len() as u64)
                .unwrap_or(0);
            record(
                ctx,
                tool_name,
                started,
                CallRecord {
                    tool: tool_name.to_owned(),
                    operation: operation_of(tool_name).to_owned(),
                    status: "ok".to_owned(),
                    duration_ms: elapsed(started),
                    instances: instances_of(tool_name, &result.structured),
                    parser: result.parser.clone(),
                    http_class: result.http_class.clone(),
                    bytes,
                    result_count: result_count(&result.structured),
                    cache_state: if result.cache_hit { "hit" } else { "miss" }.to_owned(),
                    args_hash,
                },
            )
            .await;
            result.into_call_tool_result()
        }
        Err(error) => {
            if error.is_internal() {
                tracing::error!(
                    tool = %tool_name,
                    code = error.code(),
                    error = %error,
                    causes = ?error.causes(),
                    "tool call failed"
                );
            } else {
                tracing::warn!(
                    tool = %tool_name,
                    code = error.code(),
                    "tool call refused"
                );
            }
            record(
                ctx,
                tool_name,
                started,
                CallRecord {
                    tool: tool_name.to_owned(),
                    operation: operation_of(tool_name).to_owned(),
                    status: format!("error:{}", error.code()),
                    duration_ms: elapsed(started),
                    instances: Vec::new(),
                    parser: None,
                    http_class: None,
                    bytes: 0,
                    result_count: 0,
                    cache_state: "miss".to_owned(),
                    args_hash,
                },
            )
            .await;
            CallToolResult::structured_error(error.to_json())
        }
    }
}

struct ToolOutcome {
    structured: Value,
    text: String,
    parser: Option<String>,
    http_class: Option<String>,
    cache_hit: bool,
}

impl ToolOutcome {
    fn into_call_tool_result(self) -> CallToolResult {
        let mut result = CallToolResult::structured(self.structured);
        result.content = vec![ContentBlock::text(self.text)];
        result
    }
}

async fn dispatch(
    ctx: &Arc<ToolContext>,
    tool_name: &str,
    arguments: Option<&Map<String, Value>>,
    cancellation: &CancellationToken,
) -> Result<ToolOutcome> {
    match tool_name {
        "ownsearch_search" => run_search(ctx, parse(arguments)?, cancellation).await,
        "ownsearch_fetch" => run_fetch(ctx, parse(arguments)?, cancellation).await,
        "ownsearch_instance_info" => {
            let args = parse(arguments)?;
            let ctx = Arc::clone(ctx);
            blocking(move || instance_info(&ctx, args)).await
        }
        "ownsearch_instance_health" => {
            let args = parse(arguments)?;
            let ctx = Arc::clone(ctx);
            blocking(move || instance_health(&ctx, args)).await
        }
        "ownsearch_scan" => run_scan(ctx, parse(arguments)?, cancellation).await,
        "ownsearch_session" => {
            let args = parse(arguments)?;
            let ctx = Arc::clone(ctx);
            blocking(move || session(&ctx, args)).await
        }
        "ownsearch_db_query" => {
            let args = parse(arguments)?;
            let ctx = Arc::clone(ctx);
            blocking(move || db_query(&ctx, args)).await
        }
        "ownsearch_db_crud" => {
            let args = parse(arguments)?;
            let ctx = Arc::clone(ctx);
            let cancellation = cancellation.clone();
            blocking(move || db_crud(&ctx, args, cancellation.is_cancelled())).await
        }
        other => Err(Error::UnknownTool {
            name: other.chars().take(64).collect(),
        }),
    }
}

async fn blocking<F>(work: F) -> Result<ToolOutcome>
where
    F: FnOnce() -> Result<ToolOutcome> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Error::Io {
            context: "waiting for a storage task",
            source: std::io::Error::other(error),
        })?
}

fn parse<T: DeserializeOwned>(arguments: Option<&Map<String, Value>>) -> Result<T> {
    let object = match arguments {
        Some(map) => Value::Object(map.clone()),
        None => Value::Object(Map::new()),
    };
    serde_json::from_value(object).map_err(|error| Error::InvalidArgs(error.to_string()))
}

async fn run_fetch(
    ctx: &Arc<ToolContext>,
    args: FetchArgs,
    cancellation: &CancellationToken,
) -> Result<ToolOutcome> {
    let request = args.validate()?;
    let outcome = crate::fetch::fetch(ctx, &request, cancellation).await?;
    let text = crate::fetch::summary(&outcome.output);
    Ok(ToolOutcome {
        structured: serde_json::to_value(outcome.output)?,
        text,
        parser: None,
        http_class: Some("2xx".to_owned()),
        cache_hit: outcome.cache_hit,
    })
}

async fn run_search(
    ctx: &Arc<ToolContext>,
    args: SearchArgs,
    cancellation: &CancellationToken,
) -> Result<ToolOutcome> {
    let validated = args.validate()?;
    let cap = ctx.settings.result_cap().max(1);
    let limit = validated.limit.unwrap_or(cap).min(cap);
    let preset = validated.preset.unwrap_or_else(|| ctx.settings.preset());
    let spec = validated
        .search_preset
        .map(|preset| crate::presets::resolve(preset, &ctx.settings));
    let categories = if validated.categories.is_empty() {
        spec.as_ref()
            .map(|spec| spec.categories.clone())
            .unwrap_or_default()
    } else {
        validated.categories.clone()
    };
    let engines = if validated.engines.is_empty() {
        spec.as_ref()
            .map(|spec| spec.engines.clone())
            .unwrap_or_default()
    } else {
        validated.engines.clone()
    };
    let engine_weights = spec
        .as_ref()
        .map(|spec| spec.engine_weights.clone())
        .unwrap_or_default();
    let exclude_engines = spec
        .as_ref()
        .map(|spec| spec.exclude_engines.clone())
        .unwrap_or_default();
    let min_engines = validated
        .min_engines
        .or_else(|| spec.as_ref().and_then(|spec| spec.min_engines));
    let request = SearchRequest {
        query: validated.query.clone(),
        categories,
        engines,
        language: validated.language.clone(),
        page: validated.page,
        time_range: validated.time_range,
        safe_search: validated.safe_search,
        limit,
        mode: validated.mode,
        deadline_override_secs: validated.timeout_secs,
        dedupe: validated.dedupe,
        min_engines,
        instance_ids: Vec::new(),
        engine_weights,
        exclude_engines,
        freshness_days: validated.freshness_days,
    };
    let deadline = validated
        .timeout_secs
        .unwrap_or_else(|| ctx.settings.search_deadline_secs(request.mode));
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(deadline.saturating_add(20)),
        search(ctx, &request, cancellation),
    )
    .await
    .map_err(|_| Error::Timeout {
        deadline_ms: deadline.saturating_mul(1_000),
    })??;
    let shaped = shape_search(&outcome.output, preset)?;
    let fitted = fit_to_budget(shaped, MAX_RESULT_BYTES);
    let text = match validated.text_mode {
        TextMode::Full => serde_json::to_string(&fitted)?,
        TextMode::Summary => summary_text(&outcome.output),
    };
    Ok(ToolOutcome {
        structured: fitted,
        text,
        parser: outcome.parser,
        http_class: outcome.http_class,
        cache_hit: outcome.cache_hit,
    })
}

fn instance_info(ctx: &Arc<ToolContext>, args: InstanceInfoArgs) -> Result<ToolOutcome> {
    args.validate()?;
    let filter = InstanceFilter {
        instance_id: args.instance_id,
        url: args.url.clone(),
        status: args.status,
        limit: args.limit(),
        ..InstanceFilter::default()
    };
    let rows = ctx.store.list_instances(&filter)?;
    let ids: Vec<i64> = rows.iter().map(|row| row.instance_id).collect();
    let engines = ctx.store.engine_names_for(&ids).unwrap_or_else(|error| {
        tracing::warn!(%error, "engine names could not be read");
        std::collections::HashMap::new()
    });
    let categories = ctx.store.category_names_for(&ids).unwrap_or_else(|error| {
        tracing::warn!(%error, "category names could not be read");
        std::collections::HashMap::new()
    });
    let stats = ctx.store.engine_stats_for(&ids).unwrap_or_else(|error| {
        tracing::warn!(%error, "engine counters could not be read");
        std::collections::HashMap::new()
    });
    let auth = ctx.store.auth_for(&ids).unwrap_or_else(|error| {
        tracing::warn!(%error, "the instance credentials could not be read");
        std::collections::HashMap::new()
    });
    let now = crate::time::now_rfc3339();
    let instances: Vec<InstanceInfo> = rows
        .iter()
        .map(|row| {
            let (serving, serving_reason) = crate::db::serving_state(row, &now);
            InstanceInfo {
                instance_id: row.instance_id,
                slug: row.slug.clone(),
                url: row.url.clone(),
                kind: row.kind.as_str().to_owned(),
                status: row.status.as_str().to_owned(),
                version: row.version.clone(),
                score: row.score,
                rank: row.rank,
                hard_gate_passed: row.hard_gate_passed.unwrap_or(false),
                json_enabled: row.json_enabled,
                html_enabled: row.html_enabled,
                google_exact: row.google_exact,
                google_cse: row.google_cse,
                engines: engines.get(&row.instance_id).cloned().unwrap_or_default(),
                categories: categories
                    .get(&row.instance_id)
                    .cloned()
                    .unwrap_or_default(),
                engine_stats: stats
                    .get(&row.instance_id)
                    .map(|list| {
                        list.iter()
                            .map(|stat| EngineStatInfo {
                                engine: stat.engine.clone(),
                                successes: stat.successes,
                                failures: stat.failures,
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                priority: row.manual_priority,
                auth: auth.get(&row.instance_id).map(describe_auth),
                reasons: row.reasons.clone(),
                denied: row.denied,
                scan_enabled: row.scan_enabled,
                pinned: row.pinned,
                trusted: row.trusted,
                serving,
                serving_reason,
            }
        })
        .collect();
    let count = instances.len();
    let fleet = ctx.store.fleet(64, &crate::time::now_rfc3339())?.len();
    let text = format!("{count} instances listed; {fleet} in the active fleet");
    let output = InstanceInfoOutput { instances, count };
    Ok(ToolOutcome {
        structured: serde_json::to_value(output)?,
        text,
        parser: None,
        http_class: None,
        cache_hit: false,
    })
}

fn instance_health(ctx: &Arc<ToolContext>, args: InstanceHealthArgs) -> Result<ToolOutcome> {
    let filter = InstanceFilter {
        instance_id: args.instance_id,
        limit: args.limit(),
        ..InstanceFilter::default()
    };
    let rows = ctx.store.list_instances(&filter)?;
    let include_events = args.include_events.unwrap_or(true);
    let ids: Vec<i64> = rows.iter().map(|row| row.instance_id).collect();
    let events = if include_events {
        ctx.store.events_for_many(&ids, 3).unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };
    let metrics = ctx.store.discovery_metrics_for(&ids).unwrap_or_default();
    let yields = ctx.store.engine_yield_for(&ids).unwrap_or_default();
    let threshold = ctx.settings.engine_yield_threshold();
    let now = crate::time::now_rfc3339();
    let instances: Vec<HealthItem> = rows
        .iter()
        .map(|row| {
            let metrics = metrics.get(&row.instance_id);
            let engine_yield = yields.get(&row.instance_id).copied();
            let (serving, serving_reason) = crate::db::serving_state(row, &now);
            HealthItem {
                instance_id: row.instance_id,
                slug: row.slug.clone(),
                url: row.url.clone(),
                status: row.status.as_str().to_owned(),
                score: row.score,
                hard_gate_passed: row.hard_gate_passed.unwrap_or(false),
                certificate_days: metrics.and_then(|item| item.certificate_days_remaining),
                tls_protocol: metrics.and_then(|item| item.tls_protocol.clone()),
                tls_grade: metrics.and_then(|item| item.tls_grade.clone()),
                search_median_ms: metrics
                    .and_then(|item| item.search_median_secs)
                    .map(|value| (value * 1_000.0).round() as i64),
                json_enabled: row.json_enabled.unwrap_or(false),
                html_enabled: row.html_enabled.unwrap_or(false),
                cooldown_until: row.cooldown_until.clone(),
                consecutive_failures: row.consecutive_failures,
                consecutive_successes: row.consecutive_successes,
                engine_yield,
                engine_degraded: engine_yield.is_some_and(|value| value < threshold),
                json_throttled_until: row.json_throttled_until.clone(),
                trusted: row.trusted,
                reasons: row.reasons.clone(),
                last_events: events.get(&row.instance_id).cloned().unwrap_or_default(),
                denied: row.denied,
                scan_enabled: row.scan_enabled,
                pinned: row.pinned,
                serving,
                serving_reason,
            }
        })
        .collect();
    let count = instances.len();
    let flagged = instances
        .iter()
        .filter(|item| !item.hard_gate_passed)
        .count();
    let text = format!("{count} instances inspected; {flagged} failing a hard gate");
    Ok(ToolOutcome {
        structured: serde_json::to_value(InstanceHealthOutput { instances, count })?,
        text,
        parser: None,
        http_class: None,
        cache_hit: false,
    })
}

async fn run_scan(
    ctx: &Arc<ToolContext>,
    args: ScanArgs,
    cancellation: &CancellationToken,
) -> Result<ToolOutcome> {
    let options = args.options();
    if !options.dry_run {
        if ctx.settings.ai_db_mode() == crate::types::AiDbMode::ReadOnly {
            return Err(Error::WriteDisabled);
        }
        if args
            .reason
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            return Err(Error::InvalidArgs(
                "a writing scan needs a reason; pass dry_run false together with a short reason, or keep the default dry run"
                    .into(),
            ));
        }
    }
    let output: ScanOutput = crate::scanner::run_scan(ctx, &options, cancellation).await?;
    let text = format!(
        "{}; {} instances probed, {} added, {} missing, {} status changes",
        output.summary,
        output.probed,
        output.added,
        output.missing,
        output.status_changes.len()
    );
    Ok(ToolOutcome {
        structured: serde_json::to_value(output)?,
        text,
        parser: None,
        http_class: None,
        cache_hit: false,
    })
}

fn session(ctx: &Arc<ToolContext>, args: SessionArgs) -> Result<ToolOutcome> {
    let (calls, errors, cache_hits) = ctx.metrics.session_snapshot();
    let totals = ctx.ledger.totals();
    let (entries, head) = ctx.ledger.head();
    let counts = ctx.store.counts()?;
    let per_tool: Vec<ToolUsage> = if args.include_tools.unwrap_or(true) {
        totals
            .per_tool
            .iter()
            .map(|(tool, count)| ToolUsage {
                tool: tool.clone(),
                calls: *count,
                errors: totals.per_tool_errors.get(tool).copied().unwrap_or(0),
            })
            .collect()
    } else {
        Vec::new()
    };
    let output = SessionOutput {
        calls,
        errors,
        cache_enabled: ctx.cache.enabled(),
        cache_hits: cache_hits.max(totals.cache_hits),
        per_tool,
        ledger_entries: entries,
        ledger_head: Some(head),
        state_instances: counts.total,
        state_active: counts.active,
    };
    let text = format!(
        "{calls} calls, {errors} errors, {entries} ledger rows, {} active of {} instances",
        counts.active, counts.total
    );
    Ok(ToolOutcome {
        structured: serde_json::to_value(output)?,
        text,
        parser: None,
        http_class: None,
        cache_hit: false,
    })
}

fn db_query(ctx: &Arc<ToolContext>, args: DbQueryArgs) -> Result<ToolOutcome> {
    let filter = args.filter()?;
    let limit = filter.limit;
    let offset = filter.offset;
    let entity = args.entity;
    let page = ctx.store.query_entity(entity, &filter, limit, offset)?;
    let count = page.get("count").and_then(Value::as_u64).unwrap_or(0);
    let output = DbQueryOutput {
        entity: entity.as_str().to_owned(),
        count: usize::try_from(count).unwrap_or(0),
        rows: page
            .get("rows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };
    let text = format!("{count} {} rows", output.entity);
    Ok(ToolOutcome {
        structured: serde_json::to_value(output)?,
        text,
        parser: None,
        http_class: None,
        cache_hit: false,
    })
}

fn db_crud(ctx: &Arc<ToolContext>, args: DbCrudArgs, cancelled: bool) -> Result<ToolOutcome> {
    if cancelled {
        return Err(Error::Cancelled);
    }
    let outcome = db_crud_inner(ctx, args)?;
    ctx.bump_fleet_generation();
    Ok(outcome)
}

fn db_crud_inner(ctx: &Arc<ToolContext>, args: DbCrudArgs) -> Result<ToolOutcome> {
    let request = args.request()?;
    let output = ctx.crud.begin(request)?;
    let text = if output.applied {
        format!("{} {} applied", output.operation, output.entity)
    } else {
        format!(
            "{} {} preview ready; apply within {} seconds with the token",
            output.operation,
            output.entity,
            output.expires_in_secs.unwrap_or(0)
        )
    };
    Ok(ToolOutcome {
        structured: serde_json::to_value(output)?,
        text,
        parser: None,
        http_class: None,
        cache_hit: false,
    })
}

fn describe_auth(record: &crate::auth::AuthRecord) -> String {
    match record.kind {
        crate::auth::AuthKind::Header => format!(
            "header {} from {}",
            record.header_name.as_deref().unwrap_or("authorization"),
            record.secret_env
        ),
        crate::auth::AuthKind::Basic => format!(
            "basic user {} from {}",
            record.user_name.as_deref().unwrap_or("-"),
            record.secret_env
        ),
    }
}

fn operation_of(tool_name: &str) -> &'static str {
    match tool_name {
        "ownsearch_search" => "search",
        "ownsearch_fetch" => "fetch",
        "ownsearch_instance_info" => "instance_info",
        "ownsearch_instance_health" => "instance_health",
        "ownsearch_scan" => "scan",
        "ownsearch_session" => "session",
        "ownsearch_db_query" => "db_query",
        "ownsearch_db_crud" => "db_crud",
        _ => "unknown",
    }
}

fn instances_of(tool_name: &str, structured: &Value) -> Vec<i64> {
    match tool_name {
        "ownsearch_search" => structured
            .get("sources")
            .and_then(|sources| sources.get("selected"))
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn result_count(structured: &Value) -> u64 {
    structured
        .get("results")
        .and_then(Value::as_array)
        .map(|items| u64::try_from(items.len()).unwrap_or(0))
        .unwrap_or(0)
}

async fn record(ctx: &ToolContext, tool_name: &str, started: Instant, mut entry: CallRecord) {
    entry.duration_ms = elapsed(started);
    if entry.tool.is_empty() {
        entry.tool = tool_name.to_owned();
    }
    let ledger = ctx.ledger.clone();
    let key = ctx.ledger_key().to_vec();
    let written = tokio::task::spawn_blocking(move || ledger.append(entry, &key)).await;
    match written {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => tracing::warn!(%error, "the call could not be written to the ledger"),
        Err(error) => tracing::warn!(%error, "the ledger task stopped without a result"),
    }
}

fn elapsed(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_result_count_reads_the_results_array() {
        let value = json!({"results": [1, 2, 3]});
        assert_eq!(result_count(&value), 3);
        assert_eq!(result_count(&json!({})), 0);
    }

    #[test]
    fn selected_instances_are_read_from_the_sources_block() {
        let value = json!({"sources": {"selected": [4, 5]}});
        assert_eq!(instances_of("ownsearch_search", &value), vec![4, 5]);
        assert!(instances_of("ownsearch_session", &value).is_empty());
    }

    #[test]
    fn every_tool_has_an_operation_label() {
        for name in crate::tools::specs::tool_names() {
            assert_ne!(operation_of(name), "unknown", "{name}");
        }
    }
}
