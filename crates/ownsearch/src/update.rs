use anyhow::Context;
use serde_json::json;

use crate::cli::UpdateArgs;

pub(crate) fn run(args: UpdateArgs) -> anyhow::Result<()> {
    let status = if args.check {
        ownsearch_core::update_check().context("checking for a newer release")?
    } else {
        ownsearch_core::update_apply().context("updating ownsearch")?
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "current": status.current,
                "latest": status.latest,
                "available": status.available,
                "asset": status.asset,
                "archiveUrl": status.archive_url,
                "channelCommand": status.channel_command,
                "applied": status.installed,
            }))?
        );
        return Ok(());
    }
    if !status.available {
        println!("ownsearch {} is the latest release", status.current);
    } else if args.check {
        match status.channel_command {
            Some(command) => println!(
                "ownsearch {} is available; {} is installed; update it with `{command}`",
                status.latest, status.current
            ),
            None => println!(
                "ownsearch {} is available; {} is installed; run `ownsearch update` to install it",
                status.latest, status.current
            ),
        }
    } else {
        println!(
            "updated ownsearch {} to {}; the new version starts the next time ownsearch runs",
            status.current, status.latest
        );
    }
    Ok(())
}
