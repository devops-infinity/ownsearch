use std::sync::Arc;

use anyhow::Context;
use ownsearch_core::{ScanOptions, Settings, ToolContext, run_scan};

use crate::cli::ScanArgs;

pub(crate) fn run(args: ScanArgs, common: &crate::cli::CommonArgs) -> anyhow::Result<()> {
    let settings = Settings::load(args.overrides(common)).context("resolving settings")?;
    let options = ScanOptions {
        dry_run: args.dry_run,
        instance_id: args.instance,
        limit: usize::from(args.limit.clamp(1, 500)),
        force_discovery: args.force_discovery,
        probe_search: !args.no_search_probe,
        probe_google: !args.no_google_probe,
        force: args.force,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building the tokio runtime")?;
    let report = runtime.block_on(async move {
        let context = Arc::new(ToolContext::new(settings)?);
        run_scan(
            &context,
            &options,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
    })?;
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "{} {}",
        report.summary,
        report
            .run_id
            .as_deref()
            .map(|id| format!("({id})"))
            .unwrap_or_default()
    );
    println!(
        "discovered {}; added {}; changed {}; missing {}; probed {}",
        report.discovered, report.added, report.changed, report.missing, report.probed
    );
    if report.status_changes.is_empty() {
        println!("status changes: none");
    } else {
        println!("status changes:");
        for change in &report.status_changes {
            println!(
                "  {} {} -> {} ({})",
                change.instance_id,
                change.previous_status,
                change.next_status,
                change.reasons.join(", ")
            );
        }
    }
    println!("fleet: {} instances", report.fleet.len());
    for warning in &report.warnings {
        println!("warning: {warning}");
    }
    println!("elapsed: {} ms", report.elapsed_ms);
    Ok(())
}
