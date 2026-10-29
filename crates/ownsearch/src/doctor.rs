use anyhow::Context;
use ownsearch_core::{Settings, Store};
use serde_json::json;

use crate::cli::DoctorArgs;

pub(crate) fn run(args: DoctorArgs) -> anyhow::Result<()> {
    let settings = Settings::load(args.common.overrides()).context("resolving settings")?;
    let rows = settings.describe();
    let database = settings.database_path();
    let (state_details, state_text) = inspect_state(&database);
    let (ledger_details, ledger_text) = inspect_ledger(&settings);
    if args.json {
        let mut settings_map = serde_json::Map::new();
        for (key, value) in &rows {
            settings_map.insert(key.clone(), json!(value));
        }
        let report = json!({
            "settings": settings_map,
            "state": state_details,
            "ledger": ledger_details,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("ownsearch {}", ownsearch_core::VERSION);
    for (key, value) in &rows {
        println!("{key}: {value}");
    }
    println!("state: {state_text}");
    println!("ledger: {ledger_text}");
    Ok(())
}

fn inspect_state(path: &std::path::Path) -> (serde_json::Value, String) {
    if !path.exists() {
        return (
            json!({"path": path, "status": "absent"}),
            format!(
                "{} does not exist yet; the first run creates it",
                path.display()
            ),
        );
    }
    match Store::open(path) {
        Ok(store) => {
            let version = store.schema_version().unwrap_or(-1);
            let check = store.quick_check();
            let counts = store.counts();
            match (check, counts) {
                (Ok(()), Ok(counts)) => (
                    json!({
                        "path": path,
                        "status": "ok",
                        "schemaVersion": version,
                        "quickCheck": "ok",
                        "instances": counts.total,
                        "active": counts.active,
                    }),
                    format!(
                        "{} schema {version}, quick check ok, {} instances, {} active",
                        path.display(),
                        counts.total,
                        counts.active
                    ),
                ),
                (Err(error), _) => (
                    json!({
                        "path": path,
                        "status": "quick_check_failed",
                        "schemaVersion": version,
                        "error": error.to_string(),
                    }),
                    format!(
                        "{} schema {version}, quick check failed: {error}",
                        path.display()
                    ),
                ),
                (_, Err(error)) => (
                    json!({
                        "path": path,
                        "status": "counts_failed",
                        "schemaVersion": version,
                        "error": error.to_string(),
                    }),
                    format!(
                        "{} schema {version}, counts failed: {error}",
                        path.display()
                    ),
                ),
            }
        }
        Err(error) => (
            json!({"path": path, "status": "unopenable", "error": error.to_string()}),
            format!("{} could not be opened: {error}", path.display()),
        ),
    }
}

fn inspect_ledger(settings: &Settings) -> (serde_json::Value, String) {
    let path = settings.ledger_path();
    if !path.exists() {
        return (
            json!({"path": path, "status": "absent"}),
            format!("{} does not exist yet", path.display()),
        );
    }
    let key_path = settings.ledger_dir().join("args.key");
    let Some(key) = ownsearch_core::existing_key(&key_path) else {
        return (
            json!({
                "path": path,
                "status": "key_missing",
                "keyPath": key_path,
            }),
            format!(
                "{} exists but {} is missing",
                path.display(),
                key_path.display()
            ),
        );
    };
    match ownsearch_core::verify(&path, &key) {
        Ok((entries, head)) => (
            json!({
                "path": path,
                "status": "ok",
                "entries": entries,
                "head": head,
            }),
            format!(
                "{} entries, head {}",
                entries,
                head.chars().take(12).collect::<String>()
            ),
        ),
        Err(error) => (
            json!({"path": path, "status": "chain_failed", "error": error.to_string()}),
            format!("chain check failed: {error}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_state_file_is_reported_without_creating_it() {
        let path = std::env::temp_dir().join(format!("ownsearch-doctor-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (_, text) = inspect_state(&path);
        assert!(text.contains("does not exist yet"));
        assert!(!path.exists());
    }
}
