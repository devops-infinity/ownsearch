use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use ownsearch_core::Overrides;

#[derive(Debug, Parser)]
#[command(
    name = "ownsearch",
    display_name = "OwnSearch",
    version,
    propagate_version = true,
    about = "OwnSearch, a quality-ranked SearXNG fleet server for MCP clients",
    long_about = "OwnSearch discovers public SearXNG instances, scores them from repeated probes, keeps the fleet in local SQLite state, and answers MCP searches from the best healthy instances."
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    #[command(about = "Serve the MCP tools over stdio, or over Streamable HTTP with --http")]
    Serve(ServeArgs),
    #[command(about = "Print every resolved setting and check local state")]
    Doctor(DoctorArgs),
    #[command(about = "Refresh discovery and probe the instance fleet")]
    Scan(ScanArgs),
    #[command(about = "Inspect and adjust instance lifecycle records")]
    Instances(InstancesArgs),
    #[command(about = "Manage the local SQLite state database")]
    Db(DbArgs),
    #[command(about = "Show or store the HTTP bearer token")]
    Config(ConfigArgs),
    #[command(about = "Verify or rotate the call ledger")]
    Audit(AuditArgs),
    #[command(about = "Write a CycloneDX 1.7 software bill of materials for the dependency graph")]
    Sbom(SbomArgs),
    #[command(about = "Print shell completions for a supported shell")]
    Completions {
        #[arg(value_enum, help = "Shell to generate completions for")]
        shell: clap_complete::Shell,
    },
    #[command(about = "Print the manual page")]
    Man,
}

#[derive(Debug, Args)]
pub(crate) struct CommonArgs {
    #[arg(long, global = true, help = "Path to a TOML profile file")]
    pub(crate) profile: Option<PathBuf>,
    #[arg(long, global = true, help = "Directory that holds the state database")]
    pub(crate) state_dir: Option<PathBuf>,
    #[arg(long, global = true, help = "Directory that holds the call ledger")]
    pub(crate) ledger_dir: Option<PathBuf>,
}

impl CommonArgs {
    pub(crate) fn overrides(&self) -> Overrides {
        Overrides {
            profile: self.profile.clone(),
            state_dir: self.state_dir.clone(),
            ledger_dir: self.ledger_dir.clone(),
            ..Overrides::default()
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct ServeArgs {
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    #[arg(
        long,
        help = "Serve Streamable HTTP instead of stdio",
        conflicts_with = "no_http"
    )]
    pub(crate) http: bool,
    #[arg(
        long = "no-http",
        help = "Serve over stdio even when the profile sets http = true"
    )]
    pub(crate) no_http: bool,
    #[arg(long, help = "Bind address for HTTP mode, for example 127.0.0.1:8774")]
    pub(crate) bind: Option<String>,
    #[arg(long, help = "Start from local state without fetching searx.space")]
    pub(crate) offline: bool,
    #[arg(
        long,
        help = "Allow the controlled HTML fallback for every instance this run",
        conflicts_with = "no_html_fallback"
    )]
    pub(crate) html_fallback: bool,
    #[arg(
        long = "no-html-fallback",
        help = "Disable the HTML fallback even when the profile enables it"
    )]
    pub(crate) no_html_fallback: bool,
    #[arg(
        long = "cache-ttl",
        help = "Enable the response cache for this many seconds"
    )]
    pub(crate) cache_ttl_secs: Option<u64>,
    #[arg(long, help = "Probe this many instances at once, from 1 to 8")]
    pub(crate) concurrency: Option<usize>,
}

impl ServeArgs {
    pub(crate) fn overrides(&self) -> Overrides {
        let http = if self.http {
            Some(true)
        } else if self.no_http {
            Some(false)
        } else {
            None
        };
        let html_fallback = if self.html_fallback {
            Some(true)
        } else if self.no_html_fallback {
            Some(false)
        } else {
            None
        };
        Overrides {
            http,
            bind: self.bind.clone(),
            offline: self.offline.then_some(true),
            html_fallback,
            cache_ttl_secs: self.cache_ttl_secs,
            scan_concurrency: self.concurrency,
            ..self.common.overrides()
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct DoctorArgs {
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    #[arg(long, help = "Print the report as JSON")]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ScanArgs {
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    #[arg(long, help = "Report what would change without writing state")]
    pub(crate) dry_run: bool,
    #[arg(long, help = "Probe only this instance identifier")]
    pub(crate) instance: Option<i64>,
    #[arg(
        long,
        default_value_t = 200,
        help = "Probe at most this many instances"
    )]
    pub(crate) limit: u16,
    #[arg(long, help = "Force a searx.space refresh even when state is fresh")]
    pub(crate) force_discovery: bool,
    #[arg(long, help = "Skip the one low-rate JSON search probe")]
    pub(crate) no_search_probe: bool,
    #[arg(long, help = "Skip the exact-Google probe")]
    pub(crate) no_google_probe: bool,
    #[arg(
        long,
        help = "Probe every instance now, before its scheduled scan time"
    )]
    pub(crate) force: bool,
    #[arg(long, help = "Print the report as JSON")]
    pub(crate) json: bool,
    #[arg(
        long,
        help = "Allow HTML probe fallback for this run",
        conflicts_with = "no_html_fallback"
    )]
    pub(crate) html_fallback: bool,
    #[arg(long = "no-html-fallback", help = "Disable the HTML probe fallback")]
    pub(crate) no_html_fallback: bool,
}

impl ScanArgs {
    pub(crate) fn overrides(&self) -> Overrides {
        let html_fallback = if self.html_fallback {
            Some(true)
        } else if self.no_html_fallback {
            Some(false)
        } else {
            None
        };
        Overrides {
            html_fallback,
            ..self.common.overrides()
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct InstancesArgs {
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    #[command(subcommand)]
    pub(crate) action: InstancesAction,
}

#[derive(Debug, Subcommand)]
pub(crate) enum InstancesAction {
    #[command(about = "List instances best first")]
    List {
        #[arg(long, help = "Filter by lifecycle status")]
        status: Option<String>,
        #[arg(long, default_value_t = 25, help = "Rows to print")]
        limit: u16,
        #[arg(long, help = "Print the rows as JSON")]
        json: bool,
    },
    #[command(about = "Show one instance by id or url")]
    Show {
        #[arg(help = "Instance identifier")]
        instance_id: Option<i64>,
        #[arg(long, help = "Instance URL")]
        url: Option<String>,
        #[arg(long, help = "Print the row as JSON")]
        json: bool,
    },
    #[command(about = "Pin an instance so it stays eligible")]
    Pin {
        #[arg(help = "Instance identifier")]
        instance_id: i64,
    },
    #[command(about = "Remove a pin from an instance")]
    Unpin {
        #[arg(help = "Instance identifier")]
        instance_id: i64,
    },
    #[command(about = "Deny an instance so the scanner stops probing it; undo with `allow`")]
    Deny {
        #[arg(help = "Instance identifier")]
        instance_id: i64,
        #[arg(long, help = "Reason recorded with the change")]
        reason: Option<String>,
    },
    #[command(about = "Clear a denial and let the scanner probe the instance again")]
    Allow {
        #[arg(help = "Instance identifier")]
        instance_id: i64,
        #[arg(long, help = "Reason recorded with the change")]
        reason: Option<String>,
    },
    #[command(about = "Clear a policy override and reset the status to candidate")]
    ResetStatus {
        #[arg(help = "Instance identifier")]
        instance_id: i64,
    },
    #[command(about = "List the recorded lifecycle and policy events")]
    Events {
        #[arg(help = "Instance identifier; omit for the newest events across all instances")]
        instance_id: Option<i64>,
        #[arg(long, default_value_t = 25, help = "Rows to print")]
        limit: u16,
        #[arg(long, help = "Print the rows as JSON")]
        json: bool,
    },
    #[command(about = "Add an instance by URL, including a localhost or Docker instance")]
    Add {
        #[arg(help = "Instance URL, for example http://localhost:8888/")]
        url: String,
        #[arg(long, help = "Pin the instance so it stays eligible")]
        pin: bool,
        #[arg(long, help = "Print the new row as JSON")]
        json: bool,
    },
    #[command(about = "Retire an instance so it leaves the fleet and the scanner")]
    Retire {
        #[arg(help = "Instance identifier")]
        instance_id: i64,
        #[arg(long, help = "Reason recorded with the change")]
        reason: Option<String>,
    },
    #[command(
        about = "Return a retired, missing, quarantined, or denied instance to candidate and make it scannable again"
    )]
    Restore {
        #[arg(help = "Instance identifier")]
        instance_id: i64,
    },
}

#[derive(Debug, Args)]
pub(crate) struct DbArgs {
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    #[command(subcommand)]
    pub(crate) action: DbAction,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DbAction {
    #[command(about = "Print the resolved database path")]
    Path,
    #[command(about = "Check the schema version, integrity, and foreign keys")]
    Verify,
    #[command(about = "Write a consistent backup next to the database")]
    Backup {
        #[arg(
            long,
            help = "Backup file path; defaults to backups/instances-<stamp>.db"
        )]
        out: Option<PathBuf>,
    },
    #[command(about = "Write a portable JSON snapshot")]
    Export {
        #[arg(long, help = "Snapshot file path; defaults to instances.snapshot.json")]
        out: Option<PathBuf>,
    },
    #[command(about = "Checkpoint the WAL, vacuum, and record the run")]
    Maintenance,
    #[command(about = "Report row counts by status and type")]
    Counts {
        #[arg(long, help = "Print the counts as JSON")]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct ConfigArgs {
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    #[command(subcommand)]
    pub(crate) action: ConfigAction,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigAction {
    #[command(about = "Print the resolved settings with every secret masked")]
    Show {
        #[arg(long, help = "Print the settings as JSON")]
        json: bool,
    },
    #[command(about = "Read the bearer token list from stdin and store it in the OS keychain")]
    SetBearerToken,
    #[command(about = "Remove the stored bearer token list from the OS keychain")]
    DeleteBearerToken,
}

#[derive(Debug, Args)]
pub(crate) struct AuditArgs {
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    #[command(subcommand)]
    pub(crate) action: AuditAction,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AuditAction {
    #[command(about = "Recompute the ledger hash chain and report any break")]
    Verify {
        #[arg(
            long = "file",
            help = "Ledger file, defaulting to calls.jsonl in the data directory"
        )]
        ledger_file: Option<PathBuf>,
    },
    #[command(
        about = "Verify the chain, archive the ledger, and start a fresh chain on the next write"
    )]
    Rotate {
        #[arg(
            long = "file",
            help = "Ledger file, defaulting to calls.jsonl in the data directory"
        )]
        ledger_file: Option<PathBuf>,
    },
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_tree_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_negative_flags_turn_a_profile_setting_off() {
        let args = ServeArgs {
            common: CommonArgs {
                profile: None,
                state_dir: None,
                ledger_dir: None,
            },
            http: false,
            no_http: true,
            bind: None,
            offline: true,
            html_fallback: false,
            no_html_fallback: true,
            cache_ttl_secs: None,
            concurrency: Some(2),
        };
        let overrides = args.overrides();
        assert_eq!(overrides.http, Some(false));
        assert_eq!(overrides.html_fallback, Some(false));
        assert_eq!(overrides.offline, Some(true));
        assert_eq!(overrides.scan_concurrency, Some(2));
    }

    #[test]
    fn the_scan_defaults_are_safe() {
        let args = ScanArgs {
            common: CommonArgs {
                profile: None,
                state_dir: None,
                ledger_dir: None,
            },
            dry_run: false,
            instance: None,
            limit: 200,
            force_discovery: false,
            no_search_probe: false,
            no_google_probe: false,
            force: false,
            json: false,
            html_fallback: false,
            no_html_fallback: false,
        };
        assert_eq!(args.overrides().html_fallback, None);
    }
}

#[derive(Debug, Args)]
pub(crate) struct SbomArgs {
    #[arg(
        long,
        help = "Path to Cargo.lock; defaults to the nearest one above the working directory"
    )]
    pub(crate) lock: Option<PathBuf>,
    #[arg(
        long,
        help = "Write the document to this file instead of standard output"
    )]
    pub(crate) out: Option<PathBuf>,
}
