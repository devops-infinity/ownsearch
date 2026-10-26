use anyhow::Context;
use ownsearch_core::{Settings, Store};
use serde_json::json;

use crate::cli::DoctorArgs;

pub(crate) fn run(args: DoctorArgs) -> anyhow::Result<()> {
    let settings = Settings::load(args.common.overrides()).context("resolving settings")?;
    let rows = settings.describe();
    let database = settings.database_path();
    let state = inspect_state(&database);
    let ledger = inspect_ledger(&settings);
    if args.json {
        let mut settings_map = serde_json::Map::new();
        for (key, value) in &rows {
            settings_map.insert(key.clone(), json!(value));
        }
        let report = json!({
            "settings": settings_map,
            "state": state,
            "ledger": ledger,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("ownsearch {}", ownsearch_core::VERSION);
    for (key, value) in &rows {
        println!("{key}: {value}");
    }
    println!("state: {}", state);
    println!("ledger: {}", ledger);
    Ok(())
}

fn inspect_state(path: &std::path::Path) -> String {
    if !path.exists() {
        return format!(
            "{} does not exist yet; the first run creates it",
            path.display()
        );
    }
    match Store::open(path) {
        Ok(store) => {
            let version = store.schema_version().unwrap_or(-1);
            let check = store.quick_check();
            let counts = store.counts();
            match (check, counts) {
                (Ok(()), Ok(counts)) => format!(
                    "{} schema {version}, quick check ok, {} instances, {} active",
                    path.display(),
                    counts.total,
                    counts.active
                ),
                (Err(error), _) => format!(
                    "{} schema {version}, quick check failed: {error}",
                    path.display()
                ),
                (_, Err(error)) => format!(
                    "{} schema {version}, counts failed: {error}",
                    path.display()
                ),
            }
        }
        Err(error) => format!("{} could not be opened: {error}", path.display()),
    }
}

fn inspect_ledger(settings: &Settings) -> String {
    let path = settings.ledger_path();
    if !path.exists() {
        return format!("{} does not exist yet", path.display());
    }
    let key_path = settings.ledger_dir().join("args.key");
    let Some(key) = ownsearch_core::existing_key(&key_path) else {
        return format!(
            "{} exists but {} is missing",
            path.display(),
            key_path.display()
        );
    };
    match ownsearch_core::verify(&path, &key) {
        Ok((entries, head)) => format!(
            "{} entries, head {}",
            entries,
            head.chars().take(12).collect::<String>()
        ),
        Err(error) => format!("chain check failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_state_file_is_reported_without_creating_it() {
        let path = std::env::temp_dir().join(format!("ownsearch-doctor-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let text = inspect_state(&path);
        assert!(text.contains("does not exist yet"));
        assert!(!path.exists());
    }
}
