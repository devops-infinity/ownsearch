use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use ownsearch_core::{SafeSearch, SearchMode, SearchRequest, Settings, ToolContext};
use serde_json::json;

use crate::cli::BenchArgs;

const BUILT_IN_QUERIES: [&str; 10] = [
    "rust async runtime",
    "postgresql indexing",
    "next.js app router",
    "searxng engine list",
    "sqlite wal mode",
    "caddy reverse proxy",
    "docker compose healthcheck",
    "open source license MIT",
    "http 429 retry after",
    "tokio task spawn",
];

#[derive(Clone)]
struct Sample {
    latency_ms: u64,
    ok: bool,
    code: Option<String>,
    results: usize,
    cache_hit: bool,
    unresponsive: Vec<String>,
}

#[derive(Clone, Copy)]
struct Usage {
    rss_kb: u64,
    cpu_ticks: u64,
}

pub(crate) fn run(args: BenchArgs, common: &crate::cli::CommonArgs) -> anyhow::Result<()> {
    let queries = load_queries(&args)?;
    let mode = parse_mode(&args.mode)?;
    let count = usize::from(args.count.max(1));
    let concurrency = usize::from(args.concurrency.clamp(1, 64));
    let settings = Settings::load(common.overrides()).context("resolving settings")?;
    let duration = args
        .duration
        .map(Duration::from_secs)
        .filter(|value| !value.is_zero());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building the tokio runtime")?;
    let usage_before = process_usage();
    let started = Instant::now();
    let samples = runtime.block_on(run_bench(
        settings,
        BenchRun {
            queries,
            mode,
            count,
            concurrency,
            duration,
            instances: args.instances.clone(),
        },
    ))?;
    let elapsed = started.elapsed();
    let usage_after = process_usage();
    runtime.shutdown_timeout(Duration::from_millis(500));
    let report = build_report(
        &samples,
        elapsed,
        usage_before,
        usage_after,
        concurrency,
        &args.instances,
    );
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", text_report(&report));
    }
    Ok(())
}

struct BenchRun {
    queries: Vec<String>,
    mode: SearchMode,
    count: usize,
    concurrency: usize,
    duration: Option<Duration>,
    instances: Vec<i64>,
}

async fn run_bench(settings: Settings, run: BenchRun) -> anyhow::Result<Vec<Sample>> {
    let BenchRun {
        queries,
        mode,
        count,
        concurrency,
        duration,
        instances,
    } = run;
    let context = Arc::new(ToolContext::new(settings)?);
    let queries = Arc::new(queries);
    let counter = Arc::new(AtomicUsize::new(0));
    let total = if duration.is_some() {
        usize::MAX
    } else {
        count
    };
    let deadline = duration.map(|value| tokio::time::Instant::now() + value);
    let sink: Arc<Mutex<Vec<Sample>>> = Arc::new(Mutex::new(Vec::new()));
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..concurrency {
        let context = Arc::clone(&context);
        let queries = Arc::clone(&queries);
        let counter = Arc::clone(&counter);
        let sink = Arc::clone(&sink);
        let instances = instances.clone();
        set.spawn(async move {
            loop {
                if let Some(deadline) = deadline
                    && tokio::time::Instant::now() >= deadline
                {
                    break;
                }
                let index = counter.fetch_add(1, Ordering::Relaxed);
                if index >= total {
                    break;
                }
                let Some(query) = queries.get(index % queries.len()).cloned() else {
                    break;
                };
                let request = SearchRequest {
                    query,
                    categories: Vec::new(),
                    engines: Vec::new(),
                    language: None,
                    page: 1,
                    time_range: None,
                    safe_search: SafeSearch::Moderate,
                    limit: 10,
                    mode,
                    deadline_override_secs: None,
                    dedupe: true,
                    min_engines: None,
                    instance_ids: instances.clone(),
                    engine_weights: std::collections::BTreeMap::new(),
                    exclude_engines: Vec::new(),
                };
                let started = Instant::now();
                let outcome = ownsearch_core::search(
                    &context,
                    &request,
                    &tokio_util::sync::CancellationToken::new(),
                )
                .await;
                let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                let sample = match outcome {
                    Ok(outcome) => Sample {
                        latency_ms,
                        ok: true,
                        code: None,
                        results: outcome.output.results.len(),
                        cache_hit: outcome.cache_hit,
                        unresponsive: outcome
                            .output
                            .sources
                            .unresponsive_engines
                            .iter()
                            .map(|item| item.engine.clone())
                            .collect(),
                    },
                    Err(error) => Sample {
                        latency_ms,
                        ok: false,
                        code: Some(error.code().to_owned()),
                        results: 0,
                        cache_hit: false,
                        unresponsive: Vec::new(),
                    },
                };
                if let Ok(mut sink) = sink.lock() {
                    sink.push(sample);
                }
            }
        });
    }
    while set.join_next().await.is_some() {}
    let samples = sink.lock().map(|items| items.clone()).unwrap_or_default();
    Ok(samples)
}

fn load_queries(args: &BenchArgs) -> anyhow::Result<Vec<String>> {
    match &args.queries {
        Some(path) => {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let list: Vec<String> = raw
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .take(1_000)
                .map(str::to_owned)
                .collect();
            if list.is_empty() {
                anyhow::bail!("{} holds no query", path.display());
            }
            Ok(list)
        }
        None => {
            let take = usize::from(args.count.max(1)).min(BUILT_IN_QUERIES.len());
            Ok(BUILT_IN_QUERIES
                .iter()
                .take(take)
                .map(|query| (*query).to_owned())
                .collect())
        }
    }
}

fn parse_mode(raw: &str) -> anyhow::Result<SearchMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "fast" => Ok(SearchMode::Fast),
        "balanced" => Ok(SearchMode::Balanced),
        "exhaustive" => Ok(SearchMode::Exhaustive),
        other => anyhow::bail!("unknown mode {other}; use fast, balanced, or exhaustive"),
    }
}

fn percentile(sorted: &[u64], fraction: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let len = sorted.len();
    let scaled = fraction * f64::from(u32::try_from(len).unwrap_or(u32::MAX));
    let rank = (scaled.ceil() as usize).saturating_sub(1).min(len - 1);
    sorted.get(rank).copied().unwrap_or(0)
}

fn build_report(
    samples: &[Sample],
    elapsed: Duration,
    before: Option<Usage>,
    after: Option<Usage>,
    concurrency: usize,
    instances: &[i64],
) -> serde_json::Value {
    let mut latencies: Vec<u64> = samples.iter().map(|sample| sample.latency_ms).collect();
    latencies.sort_unstable();
    let count = samples.len();
    let errors: Vec<&Sample> = samples.iter().filter(|sample| !sample.ok).collect();
    let mut by_code: BTreeMap<String, usize> = BTreeMap::new();
    for sample in &errors {
        if let Some(code) = &sample.code {
            *by_code.entry(code.clone()).or_insert(0) += 1;
        }
    }
    let mut unresponsive: BTreeMap<String, usize> = BTreeMap::new();
    for sample in samples {
        for engine in &sample.unresponsive {
            *unresponsive.entry(engine.clone()).or_insert(0) += 1;
        }
    }
    let mut unresponsive_rows: Vec<(String, usize)> = unresponsive.into_iter().collect();
    unresponsive_rows
        .sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let mean = if latencies.is_empty() {
        0
    } else {
        latencies.iter().sum::<u64>() / u64::try_from(latencies.len()).unwrap_or(1)
    };
    let results_total: usize = samples.iter().map(|sample| sample.results).sum();
    let cache_hits = samples.iter().filter(|sample| sample.cache_hit).count();
    let ratio = |part: usize| {
        if count == 0 {
            0.0
        } else {
            (f64::from(u32::try_from(part).unwrap_or(0))
                / f64::from(u32::try_from(count).unwrap_or(1))
                * 10_000.0)
                .round()
                / 10_000.0
        }
    };
    let cpu_percent = match (before, after) {
        (Some(before), Some(after)) => {
            let seconds = elapsed.as_secs_f64();
            if seconds > 0.0 {
                let ticks = after.cpu_ticks.saturating_sub(before.cpu_ticks);
                (f64::from(u32::try_from(ticks).unwrap_or(u32::MAX)) / seconds * 10.0).round()
                    / 10.0
            } else {
                0.0
            }
        }
        _ => 0.0,
    };
    json!({
        "queries": count,
        "concurrency": concurrency,
        "instances": instances,
        "latencyMs": {
            "count": count,
            "p50": percentile(&latencies, 0.50),
            "p95": percentile(&latencies, 0.95),
            "p99": percentile(&latencies, 0.99),
            "mean": mean,
        },
        "resultsPerQuery": if count == 0 { 0.0 } else {
            (f64::from(u32::try_from(results_total).unwrap_or(0)) / f64::from(u32::try_from(count).unwrap_or(1)) * 10.0).round() / 10.0
        },
        "errorRate": ratio(errors.len()),
        "errorsByCode": by_code,
        "cacheHitRate": ratio(cache_hits),
        "unresponsiveEngines": unresponsive_rows
            .iter()
            .map(|(engine, hits)| json!({"engine": engine, "count": hits}))
            .collect::<Vec<_>>(),
        "process": {
            "rssKb": after.map(|usage| usage.rss_kb),
            "cpuPercent": cpu_percent,
        },
        "elapsedMs": u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
    })
}

fn text_report(report: &serde_json::Value) -> String {
    let latency = report.get("latencyMs").cloned().unwrap_or_default();
    let field = |value: &serde_json::Value, name: &str| {
        value.get(name).cloned().unwrap_or_default().to_string()
    };
    let percent = |value: &serde_json::Value, name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_f64)
            .map(|ratio| format!("{:.2}%", ratio * 100.0))
            .unwrap_or_else(|| "0.00%".to_owned())
    };
    let engines = report
        .get("unresponsiveEngines")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| format!("{} {}", field(row, "engine"), field(row, "count")))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| "none".to_owned());
    let process = report.get("process").cloned().unwrap_or_default();
    format!(
        "bench: {} queries, {} in flight, {} ms wall\nlatency ms: p50 {}, p95 {}, p99 {}, mean {}\nresults per query: {}\nerrors: {} ({})\ncache hit rate: {}\nunresponsive engines: {}\nprocess: rss {} KiB, cpu {}\n",
        field(report, "queries"),
        field(report, "concurrency"),
        field(report, "elapsedMs"),
        field(&latency, "p50"),
        field(&latency, "p95"),
        field(&latency, "p99"),
        field(&latency, "mean"),
        field(report, "resultsPerQuery"),
        percent(report, "errorRate"),
        report.get("errorsByCode").cloned().unwrap_or_default(),
        percent(report, "cacheHitRate"),
        engines,
        field(&process, "rssKb"),
        field(&process, "cpuPercent"),
    )
}

fn process_usage() -> Option<Usage> {
    #[cfg(target_os = "linux")]
    {
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let rss_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        let (_, rest) = stat.rsplit_once(')')?;
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let utime: u64 = fields.get(11)?.parse().ok()?;
        let stime: u64 = fields.get(12)?.parse().ok()?;
        Some(Usage {
            rss_kb: rss_pages.saturating_mul(4),
            cpu_ticks: utime.saturating_add(stime),
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(latency_ms: u64, ok: bool) -> Sample {
        Sample {
            latency_ms,
            ok,
            code: (!ok).then(|| "ownsearch.timeout".to_owned()),
            results: 3,
            cache_hit: false,
            unresponsive: vec!["exaapi".to_owned()],
        }
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let values = vec![10, 20, 30, 40, 50];
        assert_eq!(percentile(&values, 0.50), 30);
        assert_eq!(percentile(&values, 0.95), 50);
        assert_eq!(percentile(&values, 0.99), 50);
        assert_eq!(percentile(&[], 0.5), 0);
    }

    #[test]
    fn the_report_counts_errors_and_engines() {
        let samples = vec![sample(10, true), sample(100, false), sample(30, true)];
        let report = build_report(&samples, Duration::from_secs(2), None, None, 2, &[7]);
        assert_eq!(report.get("queries").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(
            report
                .get("errorsByCode")
                .and_then(|v| v.get("ownsearch.timeout"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );
        let unresponsive = report
            .get("unresponsiveEngines")
            .and_then(|v| v.as_array())
            .map(Vec::len);
        assert_eq!(unresponsive, Some(1));
        assert!(text_report(&report).contains("bench: 3 queries"));
    }
}
