use std::collections::BTreeMap;

use url::Url;

const RRF_K: f64 = 60.0;
const GENERAL_ENGINE_WEIGHT: f64 = 1.0;
const SMALL_WEB_WEIGHTS: [(&str, f64); 5] = [
    ("wiby", 0.25),
    ("mwmbl", 0.4),
    ("stract", 0.4),
    ("right dao", 0.4),
    ("yep", 0.6),
];
const TRACKING_PARAMS: [&str; 9] = [
    "fbclid", "gclid", "gclsrc", "dclid", "msclkid", "igshid", "mc_cid", "mc_eid", "yclid",
];
const MAX_TITLE_CHARS: usize = 300;
const MAX_UNWRAP_HOPS: usize = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub instance_id: i64,
    pub instance_url: String,
    pub instance_score: f64,
    pub parser: String,
    pub engines: Vec<String>,
    pub positions: Vec<u32>,
    pub upstream_score: Option<f64>,
    pub category: Option<String>,
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
    pub category: Option<String>,
    pub score: f64,
    pub rank: u32,
    pub kind: String,
    pub truncated: bool,
    pub merged_from: usize,
    pub best_position: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergeOutput {
    pub results: Vec<MergedResult>,
    pub duplicate_groups: usize,
    pub results_before_merge: usize,
    pub truncated: bool,
}

pub struct MergeOptions<'a> {
    pub cap: usize,
    pub snippet_chars: usize,
    pub dedupe: bool,
    pub min_engines: usize,
    pub max_age_days: Option<u64>,
    pub now_unix: i64,
    pub engine_weights: &'a BTreeMap<String, f64>,
    pub exclude_engines: &'a [String],
    pub exclude_domains: &'a [String],
}

pub fn merge(observations: Vec<Observation>, options: &MergeOptions<'_>) -> MergeOutput {
    let mut groups: BTreeMap<String, Vec<Observation>> = BTreeMap::new();
    let mut results_before_merge = 0usize;
    for (index, observation) in observations.into_iter().enumerate() {
        let mut observation = observation;
        let had_engines = !observation.engines.is_empty();
        observation.engines = filter_engines(&observation.engines, options.exclude_engines);
        observation.engines.dedup();
        if observation.title.trim().is_empty()
            || excluded_domain(&observation.url, options.exclude_domains)
            || (had_engines && observation.engines.is_empty())
            || is_stale(&observation, options)
        {
            continue;
        }
        results_before_merge = results_before_merge.saturating_add(1);
        let key = if options.dedupe {
            dedupe_key(&observation.url)
        } else {
            format!("{index}:{}", observation.url)
        };
        groups.entry(key).or_default().push(observation);
    }
    let mut grouped: Vec<Vec<Observation>> = groups
        .into_values()
        .filter(|group| !below_minimum(group, options.min_engines))
        .collect();
    let duplicate_groups = grouped.iter().filter(|group| group.len() > 1).count();
    let scores = score_groups(&grouped, options.engine_weights);
    let mut results: Vec<MergedResult> = grouped
        .drain(..)
        .zip(scores)
        .map(|(group, score)| build_result(group, score, options.snippet_chars))
        .collect();
    results.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.url.cmp(&right.url))
    });
    let truncated = results.len() > options.cap;
    results.truncate(options.cap);
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

fn is_stale(observation: &Observation, options: &MergeOptions<'_>) -> bool {
    let Some(max_age) = options.max_age_days.filter(|days| *days > 0) else {
        return false;
    };
    let Some(published) = observation.published_date.as_deref() else {
        return false;
    };
    let Some(days) = date_days(published) else {
        return false;
    };
    let today = options.now_unix.div_euclid(86_400);
    let age = today.saturating_sub(days);
    age > i64::try_from(max_age).unwrap_or(i64::MAX)
}

pub(crate) fn date_days(raw: &str) -> Option<i64> {
    let head = raw.trim().get(..10)?;
    let mut parts = head.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if !(1900..=2200).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(crate::time::days_from_civil(year, month, day))
}

fn filter_engines(engines: &[String], excluded: &[String]) -> Vec<String> {
    engines
        .iter()
        .filter(|engine| {
            let name = engine.to_ascii_lowercase();
            !excluded.contains(&name)
        })
        .cloned()
        .collect()
}

fn excluded_domain(url: &str, excluded: &[String]) -> bool {
    if excluded.is_empty() {
        return false;
    }
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.trim_start_matches("www.").to_ascii_lowercase();
    excluded.iter().any(|domain| {
        let domain = domain.trim_start_matches("www.").to_ascii_lowercase();
        host == domain || host.ends_with(&format!(".{domain}"))
    })
}

fn below_minimum(group: &[Observation], min_engines: usize) -> bool {
    if min_engines <= 1 {
        return false;
    }
    let mut engines: Vec<String> = Vec::new();
    for observation in group {
        for engine in &observation.engines {
            let name = engine.to_ascii_lowercase();
            if !engines.contains(&name) {
                engines.push(name);
            }
        }
    }
    !engines.is_empty() && engines.len() < min_engines
}

pub(crate) fn engine_weight(engine: &str, weights: &BTreeMap<String, f64>) -> f64 {
    let name = engine.trim().to_ascii_lowercase();
    if let Some(weight) = weights.get(&name) {
        return *weight;
    }
    SMALL_WEB_WEIGHTS
        .iter()
        .find(|(small, _)| *small == name)
        .map(|(_, weight)| *weight)
        .unwrap_or(GENERAL_ENGINE_WEIGHT)
}

fn engine_pair(observation: &Observation, index: usize) -> (String, f64) {
    let engine = observation
        .engines
        .get(index)
        .cloned()
        .unwrap_or_else(|| observation.parser.clone());
    let position = observation
        .positions
        .get(index)
        .copied()
        .or_else(|| observation.positions.first().copied())
        .or(observation.rank)
        .unwrap_or(50);
    (engine, f64::from(position.max(1)))
}

pub(crate) fn reciprocal_rank(group: &[Observation], weights: &BTreeMap<String, f64>) -> f64 {
    let mut total = 0.0;
    for observation in group {
        let count = observation.engines.len().max(1);
        for index in 0..count {
            let (engine, position) = engine_pair(observation, index);
            total += engine_weight(&engine, weights) / (RRF_K + position);
        }
    }
    total
}

fn agreement(group: &[Observation]) -> (usize, usize) {
    let mut engines: Vec<String> = Vec::new();
    let mut instances: Vec<i64> = Vec::new();
    for observation in group {
        for engine in &observation.engines {
            let name = engine.to_ascii_lowercase();
            if !engines.contains(&name) {
                engines.push(name);
            }
        }
        if !instances.contains(&observation.instance_id) {
            instances.push(observation.instance_id);
        }
    }
    (engines.len(), instances.len())
}

fn upstream_score(group: &[Observation]) -> f64 {
    let scores: Vec<f64> = group
        .iter()
        .filter_map(|observation| observation.upstream_score)
        .filter(|score| score.is_finite())
        .map(|score| score.clamp(0.0, 1.0))
        .collect();
    if scores.is_empty() {
        return 0.0;
    }
    let count = f64::from(u32::try_from(scores.len()).unwrap_or(1));
    scores.iter().sum::<f64>() / count
}

pub(crate) fn score_groups(
    groups: &[Vec<Observation>],
    weights: &BTreeMap<String, f64>,
) -> Vec<f64> {
    let rankings: Vec<f64> = groups
        .iter()
        .map(|group| reciprocal_rank(group, weights))
        .collect();
    let max = rankings.iter().copied().fold(0.0_f64, f64::max);
    groups
        .iter()
        .zip(rankings)
        .map(|(group, ranking)| {
            let rrf_ratio = if max > 0.0 { ranking / max } else { 0.0 };
            let (engine_count, instance_count) = agreement(group);
            let engine_agreement = f64::from(
                u32::try_from(engine_count.saturating_sub(1))
                    .unwrap_or(0)
                    .min(4),
            ) / 4.0;
            let instance_agreement = f64::from(
                u32::try_from(instance_count.saturating_sub(1))
                    .unwrap_or(0)
                    .min(3),
            ) / 3.0;
            let searx = upstream_score(group);
            let score = 100.0
                * (0.55 * rrf_ratio
                    + 0.20 * engine_agreement
                    + 0.10 * instance_agreement
                    + 0.15 * searx);
            (score * 100.0).round() / 100.0
        })
        .collect()
}

fn build_result(group: Vec<Observation>, score: f64, snippet_chars: usize) -> MergedResult {
    let merged_from = group.len();
    let best_url = group
        .iter()
        .map(|observation| observation.url.as_str())
        .min_by_key(|url| if url.starts_with("https://") { 0 } else { 1 })
        .unwrap_or_default()
        .to_owned();
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
        .filter_map(|observation| observation.snippet.as_deref())
        .max_by_key(|snippet| snippet.chars().count())
        .map(|snippet| {
            let (capped, cut) =
                crate::sanitize::cap(&crate::sanitize::sanitize(snippet), snippet_chars);
            (capped, cut)
        });
    let mut engines: Vec<String> = Vec::new();
    for observation in &group {
        for engine in &observation.engines {
            if !engine.is_empty()
                && !engines
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(engine))
            {
                engines.push(engine.clone());
            }
        }
    }
    let published_date = group
        .iter()
        .find_map(|observation| observation.published_date.clone());
    let category = group
        .iter()
        .find_map(|observation| observation.category.clone());
    let best_position = group
        .iter()
        .filter_map(|observation| observation.rank)
        .min();
    MergedResult {
        title,
        url: best_url,
        snippet: snippet.as_ref().map(|(text, _)| text.clone()),
        engines,
        sources: group,
        published_date,
        category,
        score,
        rank: 0,
        kind: "web".to_owned(),
        truncated: snippet.map(|(_, cut)| cut).unwrap_or(false),
        merged_from,
        best_position,
    }
}

pub fn dedupe_key(raw: &str) -> String {
    let mut candidate = raw.trim().to_owned();
    for _ in 0..MAX_UNWRAP_HOPS {
        match unwrap_redirect(&candidate) {
            Some(next) => candidate = next,
            None => break,
        }
    }
    let Ok(parsed) = Url::parse(&candidate) else {
        return candidate;
    };
    let Some(host) = parsed.host_str() else {
        return candidate;
    };
    let mut key = host.trim_start_matches("www.").to_ascii_lowercase();
    if let Some(port) = parsed.port()
        && !(parsed.scheme() == "https" && port == 443)
        && !(parsed.scheme() == "http" && port == 80)
    {
        key.push(':');
        key.push_str(&port.to_string());
    }
    let path = strip_amp(parsed.path());
    key.push_str(path.trim_end_matches('/'));
    let mut params: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(name, value)| !is_tracking(name) && !is_amp_param(name, value))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.into_owned()))
        .collect();
    params.sort();
    if !params.is_empty() {
        key.push('?');
        for (index, (name, value)) in params.iter().enumerate() {
            if index > 0 {
                key.push('&');
            }
            key.push_str(name);
            key.push('=');
            key.push_str(value);
        }
    }
    key
}

fn is_tracking(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("utm_") || TRACKING_PARAMS.contains(&name.as_str())
}

fn is_amp_param(name: &str, value: &str) -> bool {
    match name.to_ascii_lowercase().as_str() {
        "amp" => value != "0",
        "output" => value.eq_ignore_ascii_case("amp"),
        _ => false,
    }
}

fn strip_amp(path: &str) -> &str {
    let lowered = path.to_ascii_lowercase();
    for suffix in ["/amp", "/amp.html", "/amp/"] {
        if lowered.ends_with(suffix) {
            return path
                .get(..path.len().saturating_sub(suffix.len()))
                .unwrap_or(path);
        }
    }
    path
}

fn unwrap_redirect(raw: &str) -> Option<String> {
    let parsed = Url::parse(raw).ok()?;
    let host = parsed
        .host_str()?
        .trim_start_matches("www.")
        .to_ascii_lowercase();
    let path = parsed.path();
    let mut target: Option<String> = None;
    if host.starts_with("google.") && path == "/url" {
        target = query_param(&parsed, "q").or_else(|| query_param(&parsed, "url"));
    } else if host == "duckduckgo.com" && path.starts_with("/l/") {
        target = query_param(&parsed, "uddg");
    } else if host == "l.facebook.com" && path == "/l.php" {
        target = query_param(&parsed, "u");
    } else if host == "youtube.com" && path == "/redirect" {
        target = query_param(&parsed, "q");
    }
    let target = target?;
    let parsed_target = Url::parse(&target).ok()?;
    matches!(parsed_target.scheme(), "http" | "https").then(|| target)
}

fn query_param(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(
        instance_id: i64,
        url: &str,
        title: &str,
        snippet: &str,
        engines: &[&str],
        positions: &[u32],
    ) -> Observation {
        Observation {
            instance_id,
            instance_url: format!("https://searx{instance_id}.example/"),
            instance_score: 90.0,
            parser: "json".to_owned(),
            engines: engines.iter().map(|engine| (*engine).to_owned()).collect(),
            positions: positions.to_vec(),
            upstream_score: Some(1.0),
            category: Some("general".to_owned()),
            rank: positions.first().copied(),
            title: title.to_owned(),
            url: url.to_owned(),
            snippet: Some(snippet.to_owned()),
            published_date: None,
        }
    }

    fn options<'a>(
        weights: &'a BTreeMap<String, f64>,
        excludes: &'a [String],
        domains: &'a [String],
    ) -> MergeOptions<'a> {
        MergeOptions {
            cap: 25,
            snippet_chars: 300,
            dedupe: true,
            min_engines: 1,
            max_age_days: None,
            now_unix: 1_790_000_000,
            engine_weights: weights,
            exclude_engines: excludes,
            exclude_domains: domains,
        }
    }

    #[test]
    fn one_url_from_three_instances_becomes_one_result_with_three_sources() {
        let weights = BTreeMap::new();
        let output = merge(
            vec![
                observation(
                    1,
                    "https://a.example/page",
                    "Page",
                    "short",
                    &["google"],
                    &[1],
                ),
                observation(
                    2,
                    "https://a.example/page",
                    "A longer page title",
                    "the longest snippet here",
                    &["bing"],
                    &[3],
                ),
                observation(
                    3,
                    "https://a.example/page#frag",
                    "Page",
                    "middle snippet",
                    &["duckduckgo"],
                    &[5],
                ),
            ],
            &options(&weights, &[], &[]),
        );
        assert_eq!(output.results.len(), 1);
        let result = output.results.first().expect("result");
        assert_eq!(result.sources.len(), 3);
        assert_eq!(result.merged_from, 3);
        assert_eq!(result.title, "A longer page title");
        assert_eq!(result.snippet.as_deref(), Some("the longest snippet here"));
        assert_eq!(result.engines, vec!["google", "bing", "duckduckgo"]);
        assert_eq!(output.duplicate_groups, 1);
    }

    #[test]
    fn url_normalization_merges_the_common_forms() {
        let key = dedupe_key("HTTPS://WWW.Example.COM:443/Path/?utm_source=x&b=2&a=1#frag");
        assert_eq!(key, "example.com/Path?a=1&b=2");
        assert_eq!(dedupe_key("http://example.com/Path"), "example.com/Path");
        assert_eq!(
            dedupe_key("https://example.com/story/amp"),
            "example.com/story"
        );
        assert_eq!(
            dedupe_key("https://example.com/story?amp=1"),
            "example.com/story"
        );
        assert_eq!(dedupe_key("https://example.com/"), "example.com");
        assert_eq!(dedupe_key("https://example.com"), "example.com");
    }

    #[test]
    fn redirect_links_are_unwrapped() {
        assert_eq!(
            dedupe_key("https://www.google.com/url?q=https%3A%2F%2Fexample.com%2Fa&sa=U"),
            "example.com/a"
        );
        assert_eq!(
            dedupe_key("https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fb"),
            "example.com/b"
        );
        assert_eq!(
            dedupe_key("https://l.facebook.com/l.php?u=https%3A%2F%2Fexample.com%2Fc"),
            "example.com/c"
        );
    }

    #[test]
    fn tracking_parameters_do_not_create_a_second_result() {
        let weights = BTreeMap::new();
        let output = merge(
            vec![
                observation(
                    1,
                    "https://a.example/page?utm_source=x",
                    "Page",
                    "one",
                    &["google"],
                    &[1],
                ),
                observation(
                    2,
                    "https://a.example/page?fbclid=abc",
                    "Page",
                    "two",
                    &["bing"],
                    &[2],
                ),
            ],
            &options(&weights, &[], &[]),
        );
        assert_eq!(output.results.len(), 1);
    }

    #[test]
    fn a_three_engine_result_outranks_a_single_wiby_result() {
        let weights = BTreeMap::new();
        let output = merge(
            vec![
                observation(
                    1,
                    "https://wiby.example/odd",
                    "An odd old page",
                    "small web",
                    &["wiby"],
                    &[1],
                ),
                observation(
                    2,
                    "https://nextjs.org/blog",
                    "Next.js releases",
                    "official",
                    &["google"],
                    &[1],
                ),
                observation(
                    3,
                    "https://nextjs.org/blog",
                    "Next.js releases",
                    "official",
                    &["bing"],
                    &[3],
                ),
                observation(
                    4,
                    "https://nextjs.org/blog",
                    "Next.js releases",
                    "official copy",
                    &["duckduckgo"],
                    &[5],
                ),
            ],
            &options(&weights, &[], &[]),
        );
        let first = output.results.first().expect("result");
        assert_eq!(first.url, "https://nextjs.org/blog");
        assert_eq!(first.engines.len(), 3);
        let wiby = output
            .results
            .iter()
            .find(|result| result.url.contains("wiby"))
            .expect("wiby result");
        assert!(first.score > wiby.score);
    }

    #[test]
    fn a_custom_weight_flips_the_order() {
        let mut weights = BTreeMap::new();
        weights.insert("wiby".to_owned(), 8.0);
        let output = merge(
            vec![
                observation(
                    1,
                    "https://wiby.example/odd",
                    "Odd",
                    "small",
                    &["wiby"],
                    &[1],
                ),
                observation(2, "https://a.example/", "A", "x", &["google"], &[2]),
            ],
            &options(&weights, &[], &[]),
        );
        assert_eq!(
            output.results.first().map(|result| result.url.as_str()),
            Some("https://wiby.example/odd")
        );
    }

    #[test]
    fn min_engines_drops_a_single_engine_result() {
        let weights = BTreeMap::new();
        let mut settings = options(&weights, &[], &[]);
        settings.min_engines = 2;
        let output = merge(
            vec![
                observation(1, "https://a.example/", "A", "x", &["google"], &[1]),
                observation(1, "https://b.example/", "B", "y", &["google"], &[2]),
                observation(2, "https://b.example/", "B", "y", &["bing"], &[2]),
            ],
            &settings,
        );
        assert_eq!(output.results.len(), 1);
        assert_eq!(
            output.results.first().map(|result| result.url.as_str()),
            Some("https://b.example/")
        );
    }

    #[test]
    fn excluded_engines_and_domains_are_removed() {
        let weights = BTreeMap::new();
        let engines = vec!["wiby".to_owned()];
        let domains = vec!["b.example".to_owned()];
        let output = merge(
            vec![
                observation(1, "https://a.example/", "A", "x", &["wiby"], &[1]),
                observation(1, "https://b.example/x", "B", "y", &["google"], &[1]),
                observation(
                    1,
                    "https://c.example/x",
                    "C",
                    "z",
                    &["google", "wiby"],
                    &[1, 2],
                ),
            ],
            &options(&weights, &engines, &domains),
        );
        let urls: Vec<&str> = output
            .results
            .iter()
            .map(|result| result.url.as_str())
            .collect();
        assert_eq!(urls, vec!["https://c.example/x"]);
        assert_eq!(
            output.results.first().map(|result| result.engines.clone()),
            Some(vec!["google".to_owned()])
        );
    }

    #[test]
    fn dedupe_off_keeps_each_observation() {
        let weights = BTreeMap::new();
        let mut settings = options(&weights, &[], &[]);
        settings.dedupe = false;
        let output = merge(
            vec![
                observation(1, "https://a.example/", "A", "x", &["google"], &[1]),
                observation(2, "https://a.example/", "A", "x", &["bing"], &[1]),
            ],
            &settings,
        );
        assert_eq!(output.results.len(), 2);
        assert!(output.results.iter().all(|result| result.merged_from == 1));
    }

    #[test]
    fn a_result_known_to_be_older_than_the_window_is_dropped() {
        let weights = BTreeMap::new();
        let mut settings = options(&weights, &[], &[]);
        settings.max_age_days = Some(365);
        let mut old = observation(1, "https://old.example/", "Old", "x", &["google"], &[1]);
        old.published_date = Some("2020-01-01".to_owned());
        let mut fresh = observation(2, "https://new.example/", "New", "y", &["bing"], &[1]);
        fresh.published_date = Some("2026-10-01T12:00:00".to_owned());
        let undated = observation(
            3,
            "https://undated.example/",
            "Undated",
            "z",
            &["brave"],
            &[1],
        );
        let output = merge(vec![old, fresh, undated], &settings);
        let urls: Vec<&str> = output
            .results
            .iter()
            .map(|result| result.url.as_str())
            .collect();
        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"https://new.example/"));
        assert!(urls.contains(&"https://undated.example/"));
        assert!(!urls.contains(&"https://old.example/"));
    }

    #[test]
    fn a_zero_window_keeps_an_old_result() {
        let weights = BTreeMap::new();
        let mut settings = options(&weights, &[], &[]);
        settings.max_age_days = Some(0);
        let mut old = observation(1, "https://old.example/", "Old", "x", &["google"], &[1]);
        old.published_date = Some("2020-01-01".to_owned());
        let output = merge(vec![old], &settings);
        assert_eq!(output.results.len(), 1);
    }

    #[test]
    fn a_date_prefix_parses_in_the_shapes_searxng_returns() {
        assert!(date_days("2026-10-14T12:30:13").is_some());
        assert!(date_days("2026-10-14 12:30:13").is_some());
        assert!(date_days("2026-10-14").is_some());
        assert_eq!(date_days("2026-13-01"), None);
        assert_eq!(date_days("not a date"), None);
        assert_eq!(date_days(""), None);
    }

    #[test]
    fn the_cap_sets_the_truncation_flag_without_inventing_urls() {
        let weights = BTreeMap::new();
        let mut settings = options(&weights, &[], &[]);
        settings.cap = 2;
        let observations: Vec<Observation> = (0..5)
            .map(|index| {
                observation(
                    1,
                    &format!("https://a.example/{index}"),
                    "T",
                    "s",
                    &["google"],
                    &[index],
                )
            })
            .collect();
        let output = merge(observations, &settings);
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
    fn http_and_https_forms_merge_and_https_wins() {
        let weights = BTreeMap::new();
        let output = merge(
            vec![
                observation(1, "http://a.example/page", "Page", "one", &["google"], &[1]),
                observation(2, "https://a.example/page", "Page", "two", &["bing"], &[2]),
            ],
            &options(&weights, &[], &[]),
        );
        assert_eq!(output.results.len(), 1);
        assert_eq!(
            output.results.first().map(|result| result.url.as_str()),
            Some("https://a.example/page")
        );
    }

    #[test]
    fn merging_group_by_group_equals_merging_at_once() {
        let weights = BTreeMap::new();
        let first = vec![
            observation(1, "https://a.example/", "A", "one", &["google"], &[1]),
            observation(2, "https://b.example/", "B", "two", &["bing"], &[1]),
        ];
        let second = vec![
            observation(3, "https://a.example/", "A", "three", &["ddg"], &[2]),
            observation(4, "https://c.example/", "C", "four", &["google"], &[1]),
        ];
        let mut all = first.clone();
        all.extend(second.clone());
        let at_once = merge(all, &options(&weights, &[], &[]));
        assert_eq!(at_once.results.len(), 3);
        assert_eq!(
            at_once
                .results
                .iter()
                .filter(|result| result.merged_from > 1)
                .count(),
            1
        );
    }
}
