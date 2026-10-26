use anyhow::Context;
use ownsearch_core::Settings;

use crate::cli::{AuditAction, AuditArgs};

pub(crate) fn run(args: AuditArgs) -> anyhow::Result<()> {
    let settings = Settings::load(args.common.overrides()).context("resolving settings")?;
    match args.action {
        AuditAction::Verify { ledger_file } => {
            let path = ledger_file.unwrap_or_else(|| settings.ledger_path());
            let key = ownsearch_core::load_or_create_key(&settings.ledger_dir().join("args.key"))?;
            let (entries, head) = ownsearch_core::verify(&path, &key)
                .with_context(|| format!("verifying {}", path.display()))?;
            println!("{} holds {entries} chained rows", path.display());
            println!("head: {head}");
            Ok(())
        }
        AuditAction::Rotate { ledger_file } => {
            let path = ledger_file.unwrap_or_else(|| settings.ledger_path());
            if !path.exists() {
                anyhow::bail!("{} does not exist yet", path.display());
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
