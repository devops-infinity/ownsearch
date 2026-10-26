#![allow(dead_code)]
use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::types::{InstanceType, normalize_url};

pub(crate) const BOOTSTRAP_JSON: &str = include_str!("instances.bootstrap.json");

#[derive(Debug, Clone, Deserialize)]
pub struct BootstrapSnapshot {
    pub schema_version: u32,
    pub captured_at: String,
    pub(crate) source: BootstrapSource,
    pub instances: Vec<BootstrapInstance>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct BootstrapSource {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub page: Option<String>,
    #[serde(default)]
    pub snapshot_at: Option<String>,
    #[serde(default)]
    pub reported_online_https_count: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BootstrapInstance {
    pub url: String,
    #[serde(default)]
    pub rank: Option<u32>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub(crate) tls: Option<BootstrapTls>,
    #[serde(default)]
    pub http_grade: Option<String>,
    #[serde(default)]
    pub csp_grade: Option<String>,
    #[serde(default)]
    pub html_grade: Option<String>,
    #[serde(default)]
    pub analytics: Option<bool>,
    #[serde(default)]
    pub ipv6: Option<bool>,
    #[serde(default)]
    pub(crate) uptime: Option<Uptime>,
    #[serde(default)]
    pub(crate) timing: Option<Timing>,
    #[serde(default)]
    pub(crate) google: Option<GoogleEvidence>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct BootstrapTls {
    #[serde(default)]
    pub grade: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub certificate_not_after: Option<String>,
    #[serde(default)]
    pub certificate_days_remaining: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Uptime {
    #[serde(default)]
    pub day: Option<f64>,
    #[serde(default)]
    pub week: Option<f64>,
    #[serde(default)]
    pub month: Option<f64>,
    #[serde(default)]
    pub year: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Timing {
    #[serde(default)]
    pub initial_success_percent: Option<f64>,
    #[serde(default)]
    pub initial_seconds: Option<f64>,
    #[serde(default)]
    pub search_success_percent: Option<f64>,
    #[serde(default)]
    pub search_median_seconds: Option<f64>,
    #[serde(default)]
    pub google_success_percent: Option<f64>,
    #[serde(default)]
    pub google_median_seconds: Option<f64>,
    #[serde(default)]
    pub working_engine_count: Option<u32>,
    #[serde(default)]
    pub working_engines: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct GoogleEvidence {
    #[serde(default)]
    pub exact_in_default_engines: Option<bool>,
    #[serde(default)]
    pub dedicated_success_percent: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DiscoveryDocument {
    #[serde(default)]
    pub metadata: Option<DiscoveryMetadata>,
    #[serde(default)]
    pub instances: BTreeMap<String, DiscoveryInstance>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DiscoveryMetadata {
    #[serde(default)]
    pub timestamp: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct DiscoveryInstance {
    #[serde(default)]
    pub main: Option<bool>,
    #[serde(default)]
    pub network_type: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub analytics: Option<bool>,
    #[serde(default)]
    pub error: Option<Value>,

    #[serde(default)]
    pub http: Option<DiscoveryHttp>,
    #[serde(default)]
    pub tls: Option<DiscoveryTls>,
    #[serde(default)]
    pub timing: Option<DiscoveryTiming>,
    #[serde(default)]
    pub uptime: Option<DiscoveryUptime>,
    #[serde(default)]
    pub network: Option<DiscoveryNetwork>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct DiscoveryHttp {
    #[serde(default)]
    pub status_code: Option<u16>,
    #[serde(default)]
    pub grade: Option<String>,
    #[serde(default)]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct DiscoveryTls {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub grade: Option<String>,
    #[serde(default)]
    pub certificate: Option<DiscoveryCertificate>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct DiscoveryCertificate {
    #[serde(rename = "notAfter", default)]
    pub not_after: Option<String>,
    #[serde(rename = "notBefore", default)]
    pub not_before: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub issuer: Option<DiscoveryName>,
    #[serde(default)]
    pub subject: Option<DiscoveryName>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct DiscoveryName {
    #[serde(rename = "commonName", default)]
    pub common_name: Option<String>,
    #[serde(rename = "altName", default)]
    pub alt_name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct DiscoveryTiming {
    #[serde(default)]
    pub initial: Option<DiscoveryStage>,
    #[serde(default)]
    pub search: Option<DiscoverySearch>,
    #[serde(default)]
    pub search_go: Option<DiscoveryStage>,
    #[serde(default)]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct DiscoveryStage {
    #[serde(rename = "success_percentage", default)]
    pub success_percentage: Option<f64>,
    #[serde(default)]
    pub all: Option<DiscoveryValue>,
    #[serde(default)]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct DiscoveryValue {
    #[serde(default)]
    pub value: Option<f64>,
    #[serde(default)]
    pub median: Option<f64>,
    #[serde(default)]
    pub mean: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct DiscoverySearch {
    #[serde(rename = "success_percentage", default)]
    pub success_percentage: Option<f64>,
    #[serde(default)]
    pub all: Option<DiscoveryValue>,
    #[serde(default)]
    pub error: Option<Value>,
    #[serde(rename = "working_engines", default)]
    pub working_engines: Option<u32>,
    #[serde(rename = "working_engine_names", default)]
    pub working_engine_names: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct DiscoveryUptime {
    #[serde(rename = "uptimeDay", default)]
    pub day: Option<f64>,
    #[serde(rename = "uptimeWeek", default)]
    pub week: Option<f64>,
    #[serde(rename = "uptimeMonth", default)]
    pub month: Option<f64>,
    #[serde(rename = "uptimeYear", default)]
    pub year: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct DiscoveryNetwork {
    #[serde(default)]
    pub ipv6: Option<bool>,
    #[serde(default)]
    pub ips: BTreeMap<String, DiscoveryIp>,
    #[serde(rename = "asn_privacy", default)]
    pub asn_privacy: Option<u8>,
    #[serde(default)]
    pub dnssec: Option<u8>,
    #[serde(default)]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct DiscoveryIp {
    #[serde(default)]
    pub field_type: Option<String>,
    #[serde(rename = "asn_cidr", default)]
    pub asn_cidr: Option<String>,
    #[serde(default)]
    pub reverse: Option<String>,
    #[serde(rename = "https_port", default)]
    pub https_port: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredInstance {
    pub url: String,
    pub kind: InstanceType,
    pub rank: Option<u32>,
    pub version: Option<String>,
    pub tls_grade: Option<String>,
    pub tls_protocol: Option<String>,
    pub certificate_not_after: Option<i64>,
    pub certificate_days_remaining: Option<i64>,
    pub http_grade: Option<String>,
    pub csp_grade: Option<String>,
    pub html_grade: Option<String>,
    pub analytics: bool,
    pub ipv6: bool,
    pub uptime_month: Option<f64>,
    pub uptime_year: Option<f64>,
    pub search_success: Option<f64>,
    pub search_median_secs: Option<f64>,
    pub initial_success: Option<f64>,
    pub initial_secs: Option<f64>,
    pub google_success: Option<f64>,
    pub google_secs: Option<f64>,
    pub working_engines: Vec<String>,
    pub exact_google_capability: bool,
    pub google_cse_capability: bool,
    pub asn_cidr: Option<String>,
}

impl DiscoveredInstance {
    pub fn host(&self) -> String {
        url::Url::parse(&self.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default()
    }

    pub fn network_group(&self) -> Option<String> {
        self.asn_cidr.clone()
    }
}

impl BootstrapSnapshot {
    pub fn embedded() -> Result<Self> {
        let snapshot: Self = serde_json::from_str(BOOTSTRAP_JSON)
            .map_err(|error| Error::StateCorrupted(format!("embedded bootstrap: {error}")))?;
        if snapshot.schema_version != 1 {
            return Err(Error::StateCorrupted(format!(
                "embedded bootstrap is schema {}, which this binary does not read",
                snapshot.schema_version
            )));
        }
        Ok(snapshot)
    }

    pub fn to_discovered(&self, now: i64) -> Vec<DiscoveredInstance> {
        self.instances
            .iter()
            .filter_map(|instance| instance.to_discovered(now))
            .collect()
    }
}

impl BootstrapInstance {
    fn to_discovered(&self, now: i64) -> Option<DiscoveredInstance> {
        let url = normalize_url(&self.url).ok()?;
        if !url.starts_with("https://") {
            return None;
        }
        let tls = self.tls.clone().unwrap_or_default();
        let uptime = self.uptime.clone().unwrap_or_default();
        let timing = self.timing.clone().unwrap_or_default();
        let google = self.google.clone().unwrap_or_default();
        let certificate_not_after = tls
            .certificate_not_after
            .as_deref()
            .and_then(crate::time::parse_gmt_stamp);
        let days_remaining = certificate_not_after
            .map(|stamp| crate::time::days_until(stamp, now))
            .or(tls.certificate_days_remaining);
        Some(DiscoveredInstance {
            url,
            kind: InstanceType::parse(self.kind.as_deref().unwrap_or("normal")),
            rank: self.rank,
            version: self.version.clone(),
            tls_grade: tls.grade,
            tls_protocol: tls.protocol,
            certificate_not_after,
            certificate_days_remaining: days_remaining,
            http_grade: self.http_grade.clone(),
            csp_grade: self.csp_grade.clone(),
            html_grade: self.html_grade.clone(),
            analytics: self.analytics.unwrap_or(false),
            ipv6: self.ipv6.unwrap_or(false),
            uptime_month: uptime.month,
            uptime_year: uptime.year,
            search_success: timing.search_success_percent,
            search_median_secs: timing.search_median_seconds,
            initial_success: timing.initial_success_percent,
            initial_secs: timing.initial_seconds,
            google_success: timing
                .google_success_percent
                .or(google.dedicated_success_percent),
            google_secs: timing.google_median_seconds,
            working_engines: timing.working_engines,
            exact_google_capability: google.exact_in_default_engines.unwrap_or(false),
            google_cse_capability: false,
            asn_cidr: None,
        })
    }
}

impl DiscoveryDocument {
    pub(crate) fn parse(body: &[u8]) -> Result<Self> {
        serde_json::from_slice(body).map_err(|error| Error::parser("searx.space json", error))
    }

    pub(crate) fn snapshot_at(&self) -> Option<i64> {
        self.metadata
            .as_ref()
            .and_then(|metadata| metadata.timestamp)
            .and_then(|stamp| i64::try_from(stamp).ok())
    }

    pub(crate) fn to_discovered(&self, now: i64) -> Vec<DiscoveredInstance> {
        self.instances
            .iter()
            .filter(|(_, record)| record.main.unwrap_or(true))
            .filter_map(|(url, record)| record.to_discovered(url, now))
            .collect()
    }
}

impl DiscoveryInstance {
    fn to_discovered(&self, url: &str, now: i64) -> Option<DiscoveredInstance> {
        let url = normalize_url(url).ok()?;
        if !url.starts_with("https://") {
            return None;
        }
        let tls = self.tls.clone().unwrap_or_default();
        let certificate = tls.certificate.clone().unwrap_or_default();
        let timing = self.timing.clone().unwrap_or_default();
        let search = timing.search.clone().unwrap_or_default();
        let initial = timing.initial.clone().unwrap_or_default();
        let google_stage = timing.search_go.clone().unwrap_or_default();
        let uptime = self.uptime.clone().unwrap_or_default();
        let network = self.network.clone().unwrap_or_default();
        let certificate_not_after = certificate
            .not_after
            .as_deref()
            .and_then(crate::time::parse_gmt_stamp);
        let days_remaining = certificate_not_after
            .map(|stamp| crate::time::days_until(stamp, now))
            .unwrap_or(0);
        let asn_cidr = network
            .ips
            .values()
            .find_map(|record| record.asn_cidr.clone())
            .or_else(|| {
                network
                    .ips
                    .keys()
                    .next()
                    .map(|address| ip_prefix_group(address))
                    .filter(|group| !group.is_empty())
            });
        let working_engines = search.working_engine_names.clone();
        let exact_google_capability = working_engines
            .iter()
            .any(|engine| engine.eq_ignore_ascii_case("google"));
        let google_cse_capability = working_engines
            .iter()
            .any(|engine| engine.eq_ignore_ascii_case("google cse"));
        Some(DiscoveredInstance {
            url,
            kind: InstanceType::parse(self.network_type.as_deref().unwrap_or("unknown")),
            rank: None,
            version: self.version.clone(),
            tls_grade: tls.grade,
            tls_protocol: tls.version,
            certificate_not_after,
            certificate_days_remaining: Some(days_remaining),
            http_grade: self.http.as_ref().and_then(|http| http.grade.clone()),
            csp_grade: None,
            html_grade: None,
            analytics: self.analytics.unwrap_or(false),
            ipv6: network.ipv6.unwrap_or(false),
            uptime_month: uptime.month,
            uptime_year: uptime.year,
            search_success: search.success_percentage,
            search_median_secs: search.all.as_ref().and_then(|value| value.median),
            initial_success: initial.success_percentage,
            initial_secs: initial.all.as_ref().and_then(|value| value.value),
            google_success: google_stage.success_percentage,
            google_secs: google_stage.all.as_ref().and_then(|value| value.median),
            working_engines,
            exact_google_capability,
            google_cse_capability,
            asn_cidr: asn_cidr.filter(|group| !group.is_empty()),
        })
    }
}

fn ip_prefix_group(address: &str) -> String {
    if let Ok(parsed) = address.parse::<std::net::Ipv4Addr>() {
        let octets = parsed.octets();
        return format!(
            "{}.{}.{}.0/24",
            octets.first().copied().unwrap_or(0),
            octets.get(1).copied().unwrap_or(0),
            octets.get(2).copied().unwrap_or(0)
        );
    }
    if let Ok(parsed) = address.parse::<std::net::Ipv6Addr>() {
        let segments = parsed.segments();
        return format!(
            "{:x}:{:x}:{:x}::/48",
            segments.first().copied().unwrap_or(0),
            segments.get(1).copied().unwrap_or(0),
            segments.get(2).copied().unwrap_or(0)
        );
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_bootstrap_holds_every_candidate() {
        let snapshot = BootstrapSnapshot::embedded().expect("bootstrap");
        assert_eq!(snapshot.instances.len(), 67);
        assert_eq!(snapshot.schema_version, 1);
        let discovered = snapshot.to_discovered(1_792_935_594);
        assert_eq!(discovered.len(), 67);
        assert!(
            discovered
                .iter()
                .all(|item| item.url.starts_with("https://"))
        );
    }

    #[test]
    fn exact_google_and_cse_stay_separate() {
        let body = r#"{"instances": {
            "https://cse.example/": {"main": true, "timing": {"search": {"working_engine_names": ["google cse"]}}},
            "https://exact.example/": {"main": true, "timing": {"search": {"working_engine_names": ["google"]}}}
        }}"#;
        let document = DiscoveryDocument::parse(body.as_bytes()).expect("document");
        let found = document.to_discovered(0);
        let cse = found
            .iter()
            .find(|item| item.url.contains("cse.example"))
            .expect("cse record");
        let exact = found
            .iter()
            .find(|item| item.url.contains("exact.example"))
            .expect("exact record");
        assert!(cse.google_cse_capability);
        assert!(!cse.exact_google_capability);
        assert!(exact.exact_google_capability);
        assert!(!exact.google_cse_capability);
    }

    #[test]
    fn a_discovery_record_maps_to_the_internal_shape() {
        let body = r#"{
            "metadata": {"timestamp": 1793000584},
            "instances": {
                "https://searx.example/": {
                    "main": true,
                    "network_type": "normal",
                    "version": "2026.10.24+ef0ec03da",
                    "analytics": false,
                    "http": {"status_code": 200, "grade": "A+"},
                    "tls": {"version": "TLSv1.3", "grade": "A+", "certificate": {"notAfter": "Jan  5 19:32:34 2027 GMT"}},
                    "timing": {
                        "initial": {"success_percentage": 100.0, "all": {"value": 0.396}},
                        "search": {"success_percentage": 100.0, "all": {"median": 1.238}, "working_engine_names": ["duckduckgo", "google"]},
                        "search_go": {"success_percentage": 50.0, "all": {"median": 0.9}}
                    },
                    "uptime": {"uptimeMonth": 99.5, "uptimeYear": 99.0},
                    "network": {"ipv6": false, "ips": {"198.44.26.133": {"asn_cidr": "198.44.26.0/24"}}}
                }
            }
        }"#;
        let document = DiscoveryDocument::parse(body.as_bytes()).expect("document");
        assert_eq!(document.snapshot_at(), Some(1_793_000_584));
        let discovered = document.to_discovered(1_793_000_584);
        let record = discovered.first().expect("record");
        assert_eq!(record.url, "https://searx.example/");
        assert!(record.exact_google_capability);
        assert!(!record.google_cse_capability);
        assert_eq!(record.asn_cidr.as_deref(), Some("198.44.26.0/24"));
        assert_eq!(record.google_success, Some(50.0));
    }

    #[test]
    fn a_plain_http_record_is_not_a_normal_fleet_candidate() {
        let body = r#"{"instances": {"http://searx.example/": {"main": true}}}"#;
        let document = DiscoveryDocument::parse(body.as_bytes()).expect("document");
        assert!(document.to_discovered(0).is_empty());
    }

    #[test]
    fn a_tor_record_keeps_its_type() {
        let body =
            r#"{"instances": {"https://tor.example/": {"main": true, "network_type": "tor"}}}"#;
        let document = DiscoveryDocument::parse(body.as_bytes()).expect("document");
        let discovered = document.to_discovered(0);
        assert_eq!(
            discovered.first().map(|item| item.kind),
            Some(InstanceType::Tor)
        );
    }

    #[test]
    fn an_ip_without_a_prefix_still_yields_a_group() {
        assert_eq!(ip_prefix_group("198.44.26.133"), "198.44.26.0/24");
        assert_eq!(ip_prefix_group("2a01:4f8:1c1c::1"), "2a01:4f8:1c1c::/48");
        assert_eq!(ip_prefix_group("not-an-ip"), "");
    }
}
