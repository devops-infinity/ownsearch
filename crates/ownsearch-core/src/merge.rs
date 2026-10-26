use std::collections::BTreeMap;

use crate::types::normalize_url;

#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub instance_id: i64,
    pub instance_url: String,
    pub instance_score: f64,
    pub parser: String,
    pub engine: Option<String>,
    pub rank: Option<u32>,
    pub title: String,
    pub url: String,
    pub snippet: Option<String>,
    pub published_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergedResult {
    pub title: String,
    pub url: String,
    pub snippet: Option<String>,
    pub engines: Vec<String>,
    pub sources: Vec<Observation>,
    pub published_date: Option<String>,
    pub score: f64,
    pub rank: u32,
    pub kind: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergeOutput {
    pub results: Vec<MergedResult>,
    pub duplicate_groups: usize,
    pub results_before_merge: usize,
    pub truncated: bool,
}

pub(crate) fn merge_key(url: &str) -> String {
    match normalize_url(url) {
        Ok(normalized) => {
            if normalized.starts_with("http://") {
                normalized.replacen("http://", "https://", 1)
            } else {
                normalized
            }
        }
        Err(_) => url.trim().to_owned(),
    }
}

pub fn merge(observations: Vec<Observation>, cap: usize, snippet_chars: usize) -> MergeOutput {
    let results_before_merge = observations.len();
    let mut groups: BTreeMap<String, Vec<Observation>> = BTreeMap::new();
    for observation in observations {
        if observation.title.trim().is_empty() {
            continue;
        }
        let key = merge_key(&observation.url);
        groups.entry(key).or_default().push(observation);
    }
    let duplicate_groups = groups.values().filter(|group| group.len() > 1).count();
    let mut results: Vec<MergedResult> = groups
        .into_values()
        .map(|group| build_result(group, snippet_chars))
        .collect();
    results.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.url.cmp(&right.url))
    });
    let truncated = results.len() > cap;
    results.truncate(cap);
    for (index, result) in results.iter_mut().enumerate() {
        result.rank = u32::try_from(index).unwrap_or(0).saturating_add(1);
    }
    MergeOutput {
        results,
        duplicate_groups,
        results_before_merge,
        truncated,
    }
}

const MAX_TITLE_CHARS: usize = 300;

fn build_result(group: Vec<Observation>, snippet_chars: usize) -> MergedResult {
    let best_url = group
        .iter()
        .map(|observation| observation.url.clone())
        .min_by_key(|url| if url.starts_with("https://") { 0 } else { 1 })
        .unwrap_or_default();
    let title = group
        .iter()
        .map(|observation| {
            crate::sanitize::cap(
                &crate::sanitize::sanitize(observation.title.trim()),
                MAX_TITLE_CHARS,
            )
            .0
        })
        .max_by_key(|title| title.chars().count())
        .unwrap_or_default();
    let snippet = group
        .iter()
        .filter_map(|observation| observation.snippet.clone())
        .max_by_key(|snippet| snippet.chars().count())
        .map(|snippet| {
            let (capped, cut) =
                crate::sanitize::cap(&crate::sanitize::sanitize(&snippet), snippet_chars);
            (capped, cut)
        });
    let mut engines: Vec<String> = Vec::new();
    for observation in &group {
        if let Some(engine) = &observation.engine
            && !engine.is_empty()
            && !engines
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(engine))
        {
            engines.push(engine.clone());
        }
    }
    let published_date = group
        .iter()
        .find_map(|observation| observation.published_date.clone());
    let best_instance_score = group
        .iter()
        .map(|observation| observation.instance_score)
        .fold(0.0_f64, f64::max);
    let best_rank = group
        .iter()
        .filter_map(|observation| observation.rank)
        .min()
        .unwrap_or(50);
    let instances: Vec<i64> = {
        let mut ids: Vec<i64> = group
            .iter()
            .map(|observation| observation.instance_id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    let agreement = f64::from(u32::try_from(instances.len()).unwrap_or(0).min(3)) / 3.0;
    let quality = (best_instance_score / 100.0).clamp(0.0, 1.0);
    let rank_score = (1.0 - (f64::from(best_rank.saturating_sub(1)) / 20.0)).clamp(0.0, 1.0);
    let freshness = if published_date.is_some() { 0.6 } else { 0.2 };
    let score = 100.0 * (0.35 * quality + 0.30 * rank_score + 0.25 * agreement + 0.10 * freshness);
    MergedResult {
        title,
        url: best_url,
        snippet: snippet.as_ref().map(|(text, _)| text.clone()),
        engines,
        sources: group,
        published_date,
        score: (score * 100.0).round() / 100.0,
        rank: 0,
        kind: "web".to_owned(),
        truncated: snippet.map(|(_, cut)| cut).unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(instance_id: i64, url: &str, title: &str, snippet: &str) -> Observation {
        Observation {
            instance_id,
            instance_url: format!("https://searx{instance_id}.example/"),
            instance_score: 90.0,
            parser: "json".to_owned(),
            engine: Some("google".to_owned()),
            rank: Some(1),
            title: title.to_owned(),
            url: url.to_owned(),
            snippet: Some(snippet.to_owned()),
            published_date: None,
        }
    }

    #[test]
    fn one_url_from_three_instances_becomes_one_result_with_three_sources() {
        let observations = vec![
            observation(1, "https://a.example/page", "Page", "short"),
            observation(
                2,
                "https://a.example/page",
                "A longer page title",
                "the longest snippet here",
            ),
            observation(3, "https://a.example/page#frag", "Page", "middle snippet"),
        ];
        let output = merge(observations, 10, 300);
        assert_eq!(output.results.len(), 1);
        let result = output.results.first().expect("result");
        assert_eq!(result.sources.len(), 3);
        assert_eq!(result.title, "A longer page title");
        assert_eq!(result.snippet.as_deref(), Some("the longest snippet here"));
        assert_eq!(output.duplicate_groups, 1);
    }

    #[test]
    fn http_and_https_forms_merge_and_https_wins() {
        let observations = vec![
            observation(1, "http://a.example/page", "Page", "one"),
            observation(2, "https://a.example/page", "Page", "two"),
        ];
        let output = merge(observations, 10, 300);
        assert_eq!(output.results.len(), 1);
        assert_eq!(
            output.results.first().map(|result| result.url.as_str()),
            Some("https://a.example/page")
        );
    }

    #[test]
    fn agreement_raises_the_final_score() {
        let single = merge(
            vec![observation(1, "https://a.example/", "A", "x")],
            10,
            300,
        );
        let agreed = merge(
            vec![
                observation(1, "https://a.example/", "A", "x"),
                observation(2, "https://a.example/", "A", "x"),
                observation(3, "https://a.example/", "A", "x"),
            ],
            10,
            300,
        );
        assert!(
            agreed.results.first().map(|result| result.score)
                > single.results.first().map(|result| result.score)
        );
    }

    #[test]
    fn the_cap_sets_the_truncation_flag_without_inventing_urls() {
        let observations: Vec<Observation> = (0..5)
            .map(|index| observation(1, &format!("https://a.example/{index}"), "T", "s"))
            .collect();
        let output = merge(observations, 2, 300);
        assert_eq!(output.results.len(), 2);
        assert!(output.truncated);
        assert!(
            output
                .results
                .iter()
                .all(|result| result.url.starts_with("https://"))
        );
    }

    #[test]
    fn merging_group_by_group_equals_merging_at_once() {
        let first = vec![
            observation(1, "https://a.example/", "A", "one"),
            observation(2, "https://b.example/", "B", "two"),
        ];
        let second = vec![
            observation(3, "https://a.example/", "A", "three"),
            observation(4, "https://c.example/", "C", "four"),
        ];
        let mut all = first.clone();
        all.extend(second.clone());
        let at_once = merge(all, 10, 300);
        let left = merge(first, 10, 300);
        let right = merge(second, 10, 300);
        let mut two_stage: Vec<Observation> = left
            .results
            .into_iter()
            .flat_map(|result| result.sources)
            .collect();
        two_stage.extend(right.results.into_iter().flat_map(|result| result.sources));
        let staged = merge(two_stage, 10, 300);
        assert_eq!(
            at_once
                .results
                .iter()
                .map(|result| &result.url)
                .collect::<Vec<_>>(),
            staged
                .results
                .iter()
                .map(|result| &result.url)
                .collect::<Vec<_>>()
        );
    }
}
