use anyhow::Context;
use ownsearch_core::Settings;

use crate::cli::{AuditAction, AuditArgs};

pub(crate) fn run(args: AuditArgs, common: &crate::cli::CommonArgs) -> anyhow::Result<()> {
    let settings = Settings::load(common.overrides()).context("resolving settings")?;
    match args.action {
        AuditAction::Verify { ledger_file } => {
            let path = ledger_file.unwrap_or_else(|| settings.ledger_path());
            if !path.exists() {
                println!("no ledger yet at {}", path.display());
                println!("the first tool call creates one; there is nothing to verify");
                return Ok(());
            }
            let key = ownsearch_core::load_or_create_key(&settings.ledger_dir().join("args.key"))?;
            let (entries, head) = ownsearch_core::verify(&path, &key)
                .with_context(|| format!("verifying {}", path.display()))?;
            println!("{} holds {entries} chained rows", path.display());
            println!("head: {head}");
            Ok(())
        }
        AuditAction::Rotate { ledger_file, force } => {
            let path = ledger_file.unwrap_or_else(|| settings.ledger_path());
            if !path.exists() {
                anyhow::bail!("{} does not exist yet", path.display());
            }
            if force {
                let archive = ownsearch_core::archive_unverified(&path)
                    .with_context(|| format!("archiving {}", path.display()))?;
                println!(
                    "archived the chain without verification to {}",
                    archive.display()
                );
                println!("the next call starts a fresh chain");
                return Ok(());
            }
            let key = ownsearch_core::load_or_create_key(&settings.ledger_dir().join("args.key"))?;
            let (entries, archive) = ownsearch_core::rotate(&path, &key)
                .with_context(|| format!("rotating {}", path.display()))?;
            println!("archived {entries} rows to {}", archive.display());
            println!("the next call starts a fresh chain");
            Ok(())
        }
    }
}
