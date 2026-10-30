use std::sync::Arc;

use anyhow::Context;
use ownsearch_core::{Settings, ToolContext};

use crate::cli::HealthArgs;

pub(crate) fn run(args: HealthArgs, common: &crate::cli::CommonArgs) -> anyhow::Result<()> {
    let settings = Settings::load(common.overrides()).context("resolving settings")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building the tokio runtime")?;
    let report = runtime.block_on(async move {
        let context = Arc::new(ToolContext::new(settings)?);
        ownsearch_core::probe_instance_health(
            &context,
            args.instance,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
    });
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));
    let report = report?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "instance {} {}: {} ({}); {}{}",
            report.instance_id,
            if report.ok { "healthy" } else { "unhealthy" },
            report.outcome,
            report.status,
            report.reasons.join(", "),
            report
                .detail
                .as_deref()
                .map(|detail| format!("; {detail}"))
                .unwrap_or_default()
        );
    }
    if report.ok {
        Ok(())
    } else {
        anyhow::bail!(
            "instance {} did not answer the probe: {}{}",
            report.instance_id,
            report.reasons.join(", "),
            report
                .detail
                .as_deref()
                .map(|detail| format!(" ({detail})"))
                .unwrap_or_default()
        )
    }
}
