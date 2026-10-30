#![allow(dead_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ownsearch_core::Overrides;
use ownsearch_core::Settings;

pub(crate) fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ownsearch-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

pub(crate) fn settings(dir: &Path) -> Settings {
    settings_with(dir, &[])
}

pub(crate) fn settings_with(dir: &Path, extra: &[(&str, &str)]) -> Settings {
    let mut map: BTreeMap<String, String> = [
        ("HOME".to_owned(), "/home/tester".to_owned()),
        ("OWNSEARCH_STATE_DIR".to_owned(), dir.display().to_string()),
        (
            "OWNSEARCH_LEDGER_DIR".to_owned(),
            dir.join("ledger").display().to_string(),
        ),
        ("OWNSEARCH_OFFLINE".to_owned(), "true".to_owned()),
        (
            "OWNSEARCH_AI_DB_MIN_INTERVAL_SECS".to_owned(),
            "0".to_owned(),
        ),
        (
            "OWNSEARCH_ALLOW_PRIVATE_INSTANCES".to_owned(),
            "true".to_owned(),
        ),
        (
            "OWNSEARCH_ALLOW_PLAIN_HTTP_PRIVATE".to_owned(),
            "true".to_owned(),
        ),
    ]
    .into_iter()
    .collect();
    for (key, value) in extra {
        map.insert((*key).to_owned(), (*value).to_owned());
    }
    Settings::load_with(
        Overrides {
            state_dir: Some(dir.to_path_buf()),
            ledger_dir: Some(dir.join("ledger")),
            ..Overrides::default()
        },
        |name| map.get(name).cloned(),
    )
    .expect("settings")
}

pub(crate) fn settings_live(dir: &Path) -> Settings {
    let map: BTreeMap<String, String> = [
        ("HOME".to_owned(), "/home/tester".to_owned()),
        ("OWNSEARCH_STATE_DIR".to_owned(), dir.display().to_string()),
        (
            "OWNSEARCH_LEDGER_DIR".to_owned(),
            dir.join("ledger").display().to_string(),
        ),
        ("OWNSEARCH_SCAN_JITTER_MS".to_owned(), "0".to_owned()),
        ("OWNSEARCH_HTML_FALLBACK".to_owned(), "true".to_owned()),
        (
            "OWNSEARCH_ALLOW_PRIVATE_INSTANCES".to_owned(),
            "true".to_owned(),
        ),
        (
            "OWNSEARCH_ALLOW_PLAIN_HTTP_PRIVATE".to_owned(),
            "true".to_owned(),
        ),
    ]
    .into_iter()
    .collect();
    Settings::load_with(
        Overrides {
            state_dir: Some(dir.to_path_buf()),
            ledger_dir: Some(dir.join("ledger")),
            offline: Some(false),
            html_fallback: Some(true),
            ..Overrides::default()
        },
        |name| map.get(name).cloned(),
    )
    .expect("settings")
}
