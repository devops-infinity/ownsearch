use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::config::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SearchPreset {
    Web,
    Code,
    News,
    Images,
}

impl SearchPreset {
    pub const ALL: [Self; 4] = [Self::Web, Self::Code, Self::News, Self::Images];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Code => "code",
            Self::News => "news",
            Self::Images => "images",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedPreset {
    pub categories: Vec<String>,
    pub engines: Vec<String>,
    pub exclude_engines: Vec<String>,
    pub engine_weights: BTreeMap<String, f64>,
    pub min_engines: Option<usize>,
}

fn code_weights() -> BTreeMap<String, f64> {
    [
        ("github", 1.6),
        ("stackoverflow", 1.5),
        ("mdn", 1.4),
        ("docker hub", 1.3),
        ("pypi", 1.3),
        ("crates.io", 1.3),
    ]
    .into_iter()
    .map(|(engine, weight)| (engine.to_owned(), weight))
    .collect()
}

fn builtin(preset: SearchPreset) -> ResolvedPreset {
    match preset {
        SearchPreset::Web => ResolvedPreset::default(),
        SearchPreset::Code => ResolvedPreset {
            categories: vec!["it".to_owned(), "general".to_owned()],
            exclude_engines: vec!["wiby".to_owned(), "bing".to_owned(), "yep".to_owned()],
            engine_weights: code_weights(),
            ..ResolvedPreset::default()
        },
        SearchPreset::News => ResolvedPreset {
            categories: vec!["news".to_owned()],
            ..ResolvedPreset::default()
        },
        SearchPreset::Images => ResolvedPreset {
            categories: vec!["images".to_owned()],
            ..ResolvedPreset::default()
        },
    }
}

pub(crate) fn resolve(preset: SearchPreset, settings: &Settings) -> ResolvedPreset {
    let mut spec = builtin(preset);
    let Some(over) = settings.search_presets().get(preset.as_str()) else {
        return spec;
    };
    if let Some(categories) = &over.categories
        && !categories.is_empty()
    {
        spec.categories = categories.clone();
    }
    if let Some(allow) = &over.engines_allow
        && !allow.is_empty()
    {
        spec.engines = allow.clone();
    }
    if let Some(deny) = &over.engines_deny {
        spec.exclude_engines = deny.clone();
    }
    if let Some(weights) = &over.engine_weights {
        spec.engine_weights = weights.clone();
    }
    if let Some(min_engines) = over.min_engines {
        spec.min_engines = Some(min_engines);
    }
    spec
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::Overrides;

    fn settings(pairs: &[(&str, &str)]) -> Settings {
        let map: std::collections::BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        Settings::load_with(
            Overrides {
                state_dir: Some(PathBuf::from("/tmp/ownsearch-preset-test")),
                ..Overrides::default()
            },
            |name| {
                if name == "HOME" {
                    return Some("/home/tester".to_owned());
                }
                map.get(name).cloned()
            },
        )
        .expect("settings")
    }

    #[test]
    fn the_code_preset_uses_the_it_category_and_drops_noise_engines() {
        let spec = resolve(SearchPreset::Code, &settings(&[]));
        assert_eq!(spec.categories, vec!["it", "general"]);
        assert!(spec.exclude_engines.contains(&"wiby".to_owned()));
        assert_eq!(spec.engine_weights.get("github"), Some(&1.6));
    }

    #[test]
    fn the_news_and_images_presets_pick_their_category() {
        let settings = settings(&[]);
        assert_eq!(
            resolve(SearchPreset::News, &settings).categories,
            vec!["news"]
        );
        assert_eq!(
            resolve(SearchPreset::Images, &settings).categories,
            vec!["images"]
        );
        assert!(resolve(SearchPreset::Web, &settings).categories.is_empty());
    }

    #[test]
    fn a_profile_preset_replaces_the_built_in_one() {
        let dir = std::env::temp_dir().join(format!("ownsearch-preset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let profile = dir.join("profile.toml");
        std::fs::write(
            &profile,
            "[search_presets.code]\ncategories = [\"it\"]\nengines_deny = [\"wiby\"]\n[search_presets.code.engine_weights]\ngithub = 2.0\n",
        )
        .expect("write");
        let settings = Settings::load_with(
            Overrides {
                profile: Some(profile),
                state_dir: Some(dir.clone()),
                ..Overrides::default()
            },
            |name| {
                if name == "HOME" {
                    return Some("/home/tester".to_owned());
                }
                None
            },
        )
        .expect("settings");
        let spec = resolve(SearchPreset::Code, &settings);
        assert_eq!(spec.categories, vec!["it"]);
        assert_eq!(spec.exclude_engines, vec!["wiby"]);
        assert_eq!(spec.engine_weights.get("github"), Some(&2.0));
        assert!(!spec.engine_weights.contains_key("mdn"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
