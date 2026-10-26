#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod browser;
mod cache;
mod capsolver;
mod challenge;
mod cleanup;
mod client;
mod config;
mod context;
mod crud;
mod db;
mod error;
mod extract;
mod fetch;
mod fsutil;
mod instances;
mod ledger;
mod merge;
mod network_guard;
mod projection;
mod quality;
mod response;
mod sanitize;
mod scanner;
mod search;
mod searxng;
mod server;
mod time;
mod tls_probe;
mod tools;
mod types;
mod watchdog;

pub use browser::Renderer;
pub use cleanup::{CleanupReport, clean};
pub use client::{FleetClient, install_crypto_provider};
pub use config::{
    Overrides, Settings, default_config_dir, default_data_dir, format_keep_window, keychain_delete,
    keychain_get, keychain_set, parse_keep_window,
};
pub use context::ToolContext;
pub use crud::{CrudRequest, CrudService};
pub use db::{
    CapabilityRecord, EventRecord, InstanceFilter, InstanceRow, KeepWindow, ProbeRecord,
    ScanUpdate, StatusCounts, Store, TlsRecord, instance_to_json,
};
pub use error::{Error, Result};
pub use fetch::{FetchRequest, fetch as fetch_page};
pub use fsutil::write_atomic;
pub use instances::{BootstrapSnapshot, DiscoveredInstance};
pub use ledger::{CallRecord, Ledger, existing_key, load_or_create_key, rotate, verify};
pub use merge::{Observation, merge};
pub use response::ScanOutput;
pub use sanitize::{cap, sanitize};
pub use scanner::{ScanOptions, refresh_discovery, run_scan};
pub use search::{SearchRequest, search};
pub use searxng::{ProbeReport, SearchQuery, config_client, search_html, search_json};
pub use server::{OwnsearchHandler, serve_http, serve_stdio};
pub use time::{now_rfc3339, now_unix, parse_rfc3339, rfc3339};
pub use tools::call as call_tool;
pub use tools::definitions as tool_definitions;
pub use tools::specs::{SPECS, ToolSpec};
pub use types::{
    AiDbMode, CrudEntity, CrudOperation, FetchFormat, InstanceStatus, InstanceType, Preset,
    ProbeKind, ProbeOutcome, QueryEntity, SafeSearch, SearchMode, TextMode, TimeRange,
};
pub use watchdog::Heartbeat;

pub const APP_NAME: &str = "OwnSearch";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
