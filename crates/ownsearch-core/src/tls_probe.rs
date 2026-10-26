use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;
use url::Url;
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::error::{Error, Result};
use crate::types::ProbeOutcome;

#[derive(Debug, Clone)]
pub(crate) struct TlsReport {
    pub outcome: ProbeOutcome,
    pub protocol: Option<String>,
    pub issuer: Option<String>,
    pub not_before: Option<i64>,
    pub not_after: Option<i64>,
    pub days_remaining: Option<i64>,
    pub hostname_verified: bool,
    pub chain_valid: bool,
    pub elapsed_ms: i64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

impl Default for TlsReport {
    fn default() -> Self {
        Self {
            outcome: ProbeOutcome::Refused,
            protocol: None,
            issuer: None,
            not_before: None,
            not_after: None,
            days_remaining: None,
            hostname_verified: false,
            chain_valid: false,
            elapsed_ms: 0,
            error_code: None,
            error_message: None,
        }
    }
}

impl TlsReport {
    pub(crate) fn failed(outcome: ProbeOutcome, message: impl Into<String>) -> Self {
        Self {
            outcome,
            error_code: Some(outcome.as_str().to_owned()),
            error_message: Some(message.into()),
            ..Self::default()
        }
    }

    pub(crate) fn is_ok(&self) -> bool {
        self.outcome.is_success() && self.hostname_verified && self.chain_valid
    }
}

pub struct TlsProber {
    config: Arc<rustls::ClientConfig>,
    timeout: Duration,
    allow_private: bool,
}

impl std::fmt::Debug for TlsProber {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TlsProber")
            .field("timeout", &self.timeout)
            .field("allow_private", &self.allow_private)
            .finish()
    }
}

impl TlsProber {
    pub fn new(timeout_secs: u64, allow_private: bool) -> Result<Self> {
        crate::client::install_crypto_provider();
        let mut roots = rustls::RootCertStore::empty();
        let loaded = rustls_native_certs::load_native_certs();
        let (added, _ignored) = roots.add_parsable_certificates(loaded.certs);
        if added == 0 {
            return Err(Error::Config(
                "no system root certificates could be loaded; install ca-certificates".into(),
            ));
        }
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Self {
            config: Arc::new(config),
            timeout: Duration::from_secs(timeout_secs.max(1)),
            allow_private,
        })
    }

    pub(crate) async fn probe(&self, url: &Url, cancellation: &CancellationToken) -> TlsReport {
        let Some(raw_host) = url.host_str() else {
            return TlsReport::failed(ProbeOutcome::Refused, "the url has no host");
        };
        let host = bare_host(raw_host);
        if url.scheme() != "https" {
            return TlsReport::failed(ProbeOutcome::Refused, "the url does not use https");
        }
        let port = url.port_or_known_default().unwrap_or(443);
        let started = Instant::now();
        let connect = async {
            let addresses: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port))
                .await
                .map_err(|error| Error::network("resolving the instance host", error))?
                .collect();
            if addresses.is_empty() {
                return Err(Error::network(
                    "resolving the instance host",
                    std::io::Error::other("no address"),
                ));
            }
            if !self.allow_private {
                for address in &addresses {
                    if !crate::network_guard::is_public(address.ip()) {
                        return Err(Error::PolicyRefused(format!(
                            "{host} resolves to {}, a non-public address",
                            address.ip()
                        )));
                    }
                }
            }
            let mut last: Option<Error> = None;
            let mut opened = None;
            for address in addresses {
                match TcpStream::connect(address).await {
                    Ok(stream) => {
                        opened = Some(stream);
                        break;
                    }
                    Err(error) => last = Some(Error::network("opening the TLS connection", error)),
                }
            }
            let Some(stream) = opened else {
                return Err(last.unwrap_or_else(|| {
                    Error::network(
                        "opening the TLS connection",
                        std::io::Error::other("every address was refused"),
                    )
                }));
            };
            let server_name = ServerName::try_from(host.to_owned()).map_err(|_| {
                Error::InvalidArgs(format!("{host} is not a valid TLS server name"))
            })?;
            let connector = TlsConnector::from(Arc::clone(&self.config));
            let connection = connector
                .connect(server_name, stream)
                .await
                .map_err(|error| Error::network("completing the TLS handshake", error))?;
            Ok::<_, Error>(connection)
        };
        let outcome = tokio::select! {
            result = tokio::time::timeout(self.timeout, connect) => result,
            () = cancellation.cancelled() => return TlsReport::failed(ProbeOutcome::Refused, "cancelled"),
        };
        let connection = match outcome {
            Err(_) => {
                return TlsReport::failed(
                    ProbeOutcome::Timeout,
                    format!(
                        "the TLS handshake did not finish in {}s",
                        self.timeout.as_secs()
                    ),
                );
            }
            Ok(Err(error)) => {
                let code = match &error {
                    Error::Timeout { .. } => ProbeOutcome::Timeout,
                    _ => ProbeOutcome::Refused,
                };
                let mut report = TlsReport::failed(code, error.to_string());
                report.elapsed_ms =
                    i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
                return report;
            }
            Ok(Ok(pair)) => pair,
        };
        let (_, session) = connection.get_ref();
        let protocol = session
            .protocol_version()
            .and_then(protocol_label)
            .map(str::to_owned);
        let mut report = TlsReport {
            outcome: ProbeOutcome::Success,
            protocol,
            hostname_verified: true,
            chain_valid: true,
            elapsed_ms: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
            ..TlsReport::default()
        };
        if let Some(certificate) = session
            .peer_certificates()
            .and_then(|certificates| certificates.first())
            && let Ok((_, parsed)) = X509Certificate::from_der(certificate.as_ref())
        {
            report.issuer = Some(parsed.issuer().to_string());
            report.not_before = Some(parsed.validity().not_before.timestamp());
            report.not_after = Some(parsed.validity().not_after.timestamp());
            report.days_remaining = Some(crate::time::days_until(
                parsed.validity().not_after.timestamp(),
                crate::time::now_unix(),
            ));
        }
        report
    }
}

fn bare_host(raw: &str) -> &str {
    raw.strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(raw)
}

fn protocol_label(version: rustls::ProtocolVersion) -> Option<&'static str> {
    match version {
        rustls::ProtocolVersion::TLSv1_2 => Some("TLSv1.2"),
        rustls::ProtocolVersion::TLSv1_3 => Some("TLSv1.3"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_https_url_is_refused_without_a_connection() {
        let prober = TlsProber::new(2, true).expect("prober");
        let url = Url::parse("http://searx.example/").expect("url");
        let cancellation = CancellationToken::new();
        let report = futures_block_on(prober.probe(&url, &cancellation));
        assert_eq!(report.outcome, ProbeOutcome::Refused);
        assert!(!report.is_ok());
    }

    #[test]
    fn a_loopback_host_is_refused_when_private_addresses_are_not_allowed() {
        let prober = TlsProber::new(1, false).expect("prober");
        let url = Url::parse("https://127.0.0.1:9/").expect("url");
        let cancellation = CancellationToken::new();
        let report = futures_block_on(prober.probe(&url, &cancellation));
        assert_eq!(report.outcome, ProbeOutcome::Refused);
        assert!(
            report
                .error_message
                .as_deref()
                .is_some_and(|message| message.contains("non-public"))
        );
    }

    #[test]
    fn an_ipv6_literal_host_loses_its_brackets() {
        assert_eq!(bare_host("[::1]"), "::1");
        assert_eq!(bare_host("[2606:4700::1111]"), "2606:4700::1111");
        assert_eq!(bare_host("searx.example"), "searx.example");
        let url = Url::parse("https://[::1]:8443/").expect("url");
        let host = url.host_str().expect("host");
        assert_eq!(
            ServerName::try_from(bare_host(host).to_owned())
                .expect("server name")
                .to_str(),
            "::1"
        );
    }

    fn futures_block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(future)
    }
}
