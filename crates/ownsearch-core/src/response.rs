use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SourceObservation {
    pub instance_id: i64,
    pub instance_slug: String,
    pub instance_url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub engines: Vec<String>,
    pub parser: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct UnresponsiveEngine {
    pub instance_id: i64,
    pub engine: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SearchResultItem {
    pub title: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engines: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<SourceObservation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub score: f64,
    pub rank: u32,
    #[serde(default)]
    pub merged_from: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_position: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SourceFailure {
    pub instance_id: i64,
    pub url: String,
    pub code: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SourcesInfo {
    #[serde(default)]
    pub selected: Vec<i64>,
    #[serde(default)]
    pub succeeded: Vec<i64>,
    #[serde(default)]
    pub failed: Vec<SourceFailure>,
    #[serde(default)]
    pub fallbacks: Vec<i64>,
    #[serde(default)]
    pub unresponsive_engines: Vec<UnresponsiveEngine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Coverage {
    pub attempted: usize,
    pub succeeded: usize,
    pub results_before_merge: usize,
    pub duplicate_groups: usize,
    pub partial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CacheInfo {
    pub enabled: bool,
    pub hit: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SearchTiming {
    pub elapsed_ms: u64,
    pub deadline_ms: u64,
    #[serde(default)]
    pub queue_ms: u64,
    #[serde(default)]
    pub selection_ms: u64,
    #[serde(default)]
    pub upstream_ms: u64,
    #[serde(default)]
    pub parse_ms: u64,
    #[serde(default)]
    pub merge_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SearchOutput {
    pub results: Vec<SearchResultItem>,
    pub sources: SourcesInfo,
    pub coverage: Coverage,
    pub cache: CacheInfo,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub errors: Vec<SourceFailure>,
    pub timing: SearchTiming,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub(crate) struct EngineStatInfo {
    pub engine: String,
    pub successes: i64,
    pub failures: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub(crate) struct InstanceInfo {
    pub instance_id: i64,
    pub slug: String,
    pub url: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<i64>,
    pub hard_gate_passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_enabled: Option<bool>,
    pub google_exact: bool,
    pub google_cse: bool,
    #[serde(default)]
    pub engines: Vec<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub engine_stats: Vec<EngineStatInfo>,
    #[serde(default)]
    pub priority: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    #[serde(default)]
    pub reasons: Vec<String>,
    pub denied: bool,
    pub scan_enabled: bool,
    pub pinned: bool,
    pub trusted: bool,
    pub serving: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serving_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub(crate) struct InstanceInfoOutput {
    pub instances: Vec<InstanceInfo>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub(crate) struct HealthItem {
    pub instance_id: i64,
    pub slug: String,
    pub url: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub hard_gate_passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_days: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_grade: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_median_ms: Option<i64>,
    #[serde(default)]
    pub json_enabled: bool,
    #[serde(default)]
    pub html_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown_until: Option<String>,
    #[serde(default)]
    pub consecutive_failures: i64,
    #[serde(default)]
    pub consecutive_successes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_yield: Option<f64>,
    #[serde(default)]
    pub engine_degraded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_throttled_until: Option<String>,
    pub trusted: bool,
    pub denied: bool,
    pub scan_enabled: bool,
    pub pinned: bool,
    pub serving: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serving_reason: Option<String>,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub last_events: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub(crate) struct InstanceHealthOutput {
    pub instances: Vec<HealthItem>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct StatusChange {
    pub instance_id: i64,
    pub slug: String,
    pub url: String,
    pub previous_status: String,
    pub next_status: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ScanOutput {
    pub dry_run: bool,
    #[serde(default)]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub discovered: usize,
    pub added: usize,
    pub changed: usize,
    pub missing: usize,
    pub probed: usize,
    #[serde(default)]
    pub status_changes: Vec<StatusChange>,
    #[serde(default)]
    pub fleet: Vec<i64>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct FetchPage {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct FetchOutput {
    pub url: String,
    pub final_url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redirects: Vec<String>,
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub format: String,
    pub fetch_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub content: String,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<FetchPage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub cache_hit: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_age_secs: Option<u64>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub(crate) struct ToolUsage {
    pub tool: String,
    pub calls: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub(crate) struct SessionOutput {
    pub calls: u64,
    pub errors: u64,
    pub cache_enabled: bool,
    pub cache_hits: u64,
    #[serde(default)]
    pub per_tool: Vec<ToolUsage>,
    pub ledger_entries: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_head: Option<String>,
    pub state_instances: i64,
    pub state_active: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub(crate) struct DbQueryOutput {
    pub entity: String,
    pub count: usize,
    pub rows: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct DbCrudOutput {
    pub operation: String,
    pub entity: String,
    pub applied: bool,
    pub dry_run: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<serde_json::Value>,
    #[serde(default)]
    pub refused_reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<serde_json::Value>,
}
