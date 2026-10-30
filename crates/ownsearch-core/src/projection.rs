use serde_json::Value;

use crate::error::Result;
use crate::response::SearchOutput;
use crate::types::Preset;

pub(crate) const MAX_RESULT_BYTES: usize = 262_144;

const MINIMAL_FIELDS: [&str; 5] = ["title", "url", "rank", "score", "sourceId"];
const STANDARD_FIELDS: [&str; 10] = [
    "title",
    "url",
    "snippet",
    "engines",
    "sources",
    "publishedDate",
    "score",
    "rank",
    "mergedFrom",
    "truncated",
];

pub(crate) fn shape_search(output: &SearchOutput, preset: Preset) -> Result<Value> {
    let mut value = serde_json::to_value(output)?;
    if preset != Preset::Full
        && let Some(sources) = value.get_mut("sources").and_then(Value::as_object_mut)
    {
        sources.remove("unresponsiveEngines");
    }
    let Some(object) = value.as_object_mut() else {
        return Ok(value);
    };
    let Some(results) = object.get_mut("results").and_then(Value::as_array_mut) else {
        return Ok(value);
    };
    let fields: &[&str] = match preset {
        Preset::Minimal => &MINIMAL_FIELDS,
        Preset::Standard => &STANDARD_FIELDS,
        Preset::Full => &[],
    };
    for result in results.iter_mut() {
        if let Some(result_object) = result.as_object_mut() {
            if preset == Preset::Minimal
                && let Some(first_source) = result_object
                    .get("sources")
                    .and_then(Value::as_array)
                    .and_then(|sources| sources.first())
                    .and_then(|source| source.get("instanceId"))
                    .cloned()
            {
                result_object.insert("sourceId".to_owned(), first_source);
            }
            if !fields.is_empty() {
                result_object.retain(|key, _| fields.contains(&key.as_str()));
            }
            if preset == Preset::Minimal
                && let Some(sources) = result_object.remove("sources")
            {
                let _ = sources;
            }
        }
    }
    Ok(value)
}

pub(crate) fn fit_to_budget(value: Value, max_bytes: usize) -> Value {
    if serialized_len(&value) <= max_bytes {
        return value;
    }
    let Some(total) = value.get("results").and_then(Value::as_array).map(Vec::len) else {
        return value;
    };
    let mut low = 0usize;
    let mut high = total;
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let candidate = truncated_to(&value, mid);
        if serialized_len(&candidate) <= max_bytes {
            low = mid;
        } else {
            high = mid.saturating_sub(1);
        }
    }
    truncated_to(&value, low)
}

fn serialized_len(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

fn truncated_to(value: &Value, keep: usize) -> Value {
    let mut candidate = value.clone();
    if let Some(results) = candidate.get_mut("results").and_then(Value::as_array_mut) {
        results.truncate(keep);
    }
    if let Some(object) = candidate.as_object_mut() {
        object.insert("truncated".to_owned(), Value::Bool(true));
    }
    candidate
}

pub(crate) fn summary_text(output: &SearchOutput) -> String {
    let count = output.results.len();
    let top = output
        .results
        .first()
        .map(|result| result.title.clone())
        .filter(|title| !title.is_empty());
    let sources = output.coverage.succeeded;
    let attempted = output.coverage.attempted;
    let mut line = format!("{count} results from {sources} of {attempted} instances");
    if let Some(top) = top {
        let (capped, _) = crate::sanitize::cap(&top, 80);
        line.push_str(&format!("; top: {capped}"));
    }
    if output.coverage.partial {
        line.push_str("; partial coverage");
    }
    if output.truncated {
        line.push_str("; capped");
    }
    if output.cache.hit {
        line.push_str("; served from cache");
    }
    line.push_str(&format!("; {} ms", output.timing.elapsed_ms));
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response::{
        CacheInfo, Coverage, SearchResultItem, SearchTiming, SourceObservation, SourcesInfo,
    };

    fn output() -> SearchOutput {
        SearchOutput {
            results: vec![SearchResultItem {
                title: "A page".to_owned(),
                url: "https://a.example/".to_owned(),
                snippet: Some("a snippet".to_owned()),
                engines: Some(vec!["google".to_owned()]),
                sources: Some(vec![SourceObservation {
                    instance_id: 7,
                    instance_slug: "searx-example".to_owned(),
                    instance_url: "https://searx.example/".to_owned(),
                    engines: vec!["google".to_owned()],
                    parser: "json".to_owned(),
                    rank: Some(1),
                    score: 90.0,
                }]),
                published_date: Some("2026-10-20".to_owned()),
                category: None,
                merged_from: 2,
                best_position: Some(1),
                score: 88.5,
                rank: 1,
                kind: Some("web".to_owned()),
                truncated: Some(false),
            }],
            sources: SourcesInfo {
                selected: vec![7],
                succeeded: vec![7],
                failed: Vec::new(),
                fallbacks: Vec::new(),
                unresponsive_engines: Vec::new(),
            },
            coverage: Coverage {
                attempted: 1,
                succeeded: 1,
                results_before_merge: 1,
                duplicate_groups: 0,
                partial: false,
            },
            cache: CacheInfo {
                enabled: false,
                hit: false,
                age_secs: None,
            },
            truncated: false,
            warnings: Vec::new(),
            errors: Vec::new(),
            timing: SearchTiming {
                elapsed_ms: 120,
                deadline_ms: 12_000,
                queue_ms: 1,
                selection_ms: 1,
                upstream_ms: 100,
                parse_ms: 2,
                merge_ms: 3,
            },
        }
    }

    #[test]
    fn the_minimal_preset_keeps_only_the_declared_fields() {
        let shaped = shape_search(&output(), Preset::Minimal).expect("shape");
        let result = shaped
            .get("results")
            .and_then(Value::as_array)
            .and_then(|results| results.first())
            .expect("result");
        let keys: Vec<&String> = result.as_object().expect("object").keys().collect();
        for key in &keys {
            assert!(MINIMAL_FIELDS.contains(&key.as_str()), "unexpected {key}");
        }
        assert_eq!(result.get("sourceId").and_then(Value::as_i64), Some(7));
        assert!(shaped.get("sources").is_some());
        assert!(shaped.get("coverage").is_some());
        assert!(shaped.get("timing").is_some());
    }

    #[test]
    fn the_standard_preset_keeps_snippet_and_sources() {
        let shaped = shape_search(&output(), Preset::Standard).expect("shape");
        let result = shaped
            .get("results")
            .and_then(Value::as_array)
            .and_then(|results| results.first())
            .expect("result");
        assert_eq!(
            result.get("snippet").and_then(Value::as_str),
            Some("a snippet")
        );
        assert!(result.get("sources").is_some());
        assert!(result.get("publishedDate").is_some());
    }

    #[test]
    fn the_full_preset_keeps_every_field() {
        let shaped = shape_search(&output(), Preset::Full).expect("shape");
        let result = shaped
            .get("results")
            .and_then(Value::as_array)
            .and_then(|results| results.first())
            .expect("result");
        assert_eq!(result.get("kind").and_then(Value::as_str), Some("web"));
    }

    #[test]
    fn the_budget_drops_trailing_results_and_marks_the_cut() {
        let mut output = output();
        output.results = (0..20)
            .map(|index| SearchResultItem {
                title: format!("title {index}"),
                url: format!("https://a.example/{index}"),
                snippet: Some("x".repeat(200)),
                engines: Some(vec!["google".to_owned()]),
                sources: Some(vec![SourceObservation {
                    instance_id: 1,
                    instance_slug: "searx-example".to_owned(),
                    instance_url: "https://searx.example/".to_owned(),
                    engines: Vec::new(),
                    parser: "json".to_owned(),
                    rank: Some(1),
                    score: 50.0,
                }]),
                published_date: None,
                category: None,
                merged_from: 1,
                best_position: Some(1),
                score: 50.0,
                rank: u32::try_from(index).unwrap_or(0) + 1,
                kind: Some("web".to_owned()),
                truncated: Some(false),
            })
            .collect();
        let shaped = shape_search(&output, Preset::Standard).expect("shape");
        let fitted = fit_to_budget(shaped, 4_000);
        assert_eq!(fitted.get("truncated").and_then(Value::as_bool), Some(true));
        assert!(
            serde_json::to_vec(&fitted)
                .map(|bytes| bytes.len())
                .unwrap_or(0)
                <= 4_000
        );
    }

    #[test]
    fn the_summary_names_the_top_result_and_the_partial_state() {
        let mut output = output();
        output.coverage.partial = true;
        let text = summary_text(&output);
        assert!(text.contains("1 results from 1 of 1 instances"));
        assert!(text.contains("top: A page"));
        assert!(text.contains("partial coverage"));
    }
}
