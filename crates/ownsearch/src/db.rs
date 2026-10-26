use anyhow::Context;
use ownsearch_core::{Settings, Store};
use serde_json::json;

use crate::cli::{DbAction, DbArgs};

pub(crate) fn run(args: DbArgs) -> anyhow::Result<()> {
    let settings = Settings::load(args.common.overrides()).context("resolving settings")?;
    let path = settings.database_path();
    if matches!(args.action, DbAction::Path) {
        println!("{}", path.display());
        return Ok(());
    }
    if !path.exists() {
        if !matches!(args.action, DbAction::Maintenance) {
            anyhow::bail!(
                "{} does not exist yet; start the server or run a scan first",
                path.display()
            );
        }
        let key = ownsearch_core::load_or_create_key(&settings.ledger_dir().join("args.key"))?;
        let ledger = ownsearch_core::Ledger::open(settings.ledger_path(), &key)?;
        let cleaned = ownsearch_core::clean(&settings, &ledger, &key)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "stateDatabase": "not created yet",
                "cleanup": {
                    "ledgerRotated": cleaned.ledger_rotated,
                    "ledgerArchivesRemoved": cleaned.ledger_archives_removed,
                    "preMigrationRemoved": cleaned.pre_migration_removed,
                    "eventsTrimmed": cleaned.events_trimmed,
                    "warnings": cleaned.warnings,
                }
            }))?
        );
        return Ok(());
    }
    let store = Store::open(&path).context("opening the state database")?;
    match args.action {
        DbAction::Path => Ok(()),
        DbAction::Verify => {
            let version = store.schema_version()?;
            store.quick_check()?;
            store.integrity_check()?;
            store.foreign_key_check()?;
            let counts = store.counts()?;
            println!("{} is healthy", path.display());
            println!("schema version: {version}");
            println!("instances: {} ({} active)", counts.total, counts.active);
            println!("probes: {}; events: {}", counts.probes, counts.events);
            Ok(())
        }
        DbAction::Backup { out } => {
            let target = out.unwrap_or_else(|| {
                settings
                    .backups_dir()
                    .join(format!("instances-{}.db", crate::db::stamp()))
            });
            store.backup(&target)?;
            store.record_maintenance(
                "backup",
                "success",
                json!({"path": target.display().to_string()}),
            )?;
            println!("backup written to {}", target.display());
            Ok(())
        }
        DbAction::Export { out } => {
            let target = out.unwrap_or_else(|| settings.snapshot_path());
            let (count, bytes) = store.write_snapshot(&target)?;
            println!(
                "{count} instances exported to {} ({bytes} bytes)",
                target.display()
            );
            Ok(())
        }
        DbAction::Maintenance => {
            let checkpoint = store.maintenance()?;
            let expired = store.expire(ownsearch_core::KeepWindow::default())?;
            let key = ownsearch_core::load_or_create_key(&settings.ledger_dir().join("args.key"))?;
            let ledger = ownsearch_core::Ledger::open(settings.ledger_path(), &key)?;
            let cleaned = ownsearch_core::clean(&settings, &ledger, &key)?;
            let maintenance_report = json!({
                "checkpoint": checkpoint,
                "expired": expired,
                "cleanup": {
                    "ledgerRotated": cleaned.ledger_rotated,
                    "ledgerArchivesRemoved": cleaned.ledger_archives_removed,
                    "preMigrationRemoved": cleaned.pre_migration_removed,
                    "eventsTrimmed": cleaned.events_trimmed,
                    "warnings": cleaned.warnings,
                }
            });
            store.record_maintenance("maintenance", "success", maintenance_report.clone())?;
            println!("{}", serde_json::to_string_pretty(&maintenance_report)?);
            Ok(())
        }
        DbAction::Counts { json: as_json } => {
            let counts = store.counts()?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "total": counts.total,
                        "active": counts.active,
                        "probes": counts.probes,
                        "events": counts.events,
                        "byStatus": counts.by_status,
                        "byType": counts.by_type,
                    }))?
                );
            } else {
                println!("total: {}; active: {}", counts.total, counts.active);
                for (status, count) in &counts.by_status {
                    println!("  {status}: {count}");
                }
                for (kind, count) in &counts.by_type {
                    println!("  type {kind}: {count}");
                }
                println!("probes: {}; events: {}", counts.probes, counts.events);
            }
            Ok(())
        }
    }
}

pub(crate) fn stamp() -> String {
    ownsearch_core::now_rfc3339()
        .replace([':', '.'], "-")
        .replace('T', "_")
}
