use std::path::Path;
use std::time::Duration;

use semver::Version;
use serde_json::Value;

use crate::error::{Error, Result};

const OWNER: &str = "devops-infinity";
const REPO: &str = "ownsearch";
const API: &str = "https://api.github.com";
const UPDATE_KEY: &str = "RWSPyQWAsXwP5vjgAFI/PAAtbFAd5tA4BlhyFrT+MCPNmE2J+jgT26kC";
const MAX_SUMS_BYTES: usize = 64 * 1024;
const MAX_SIGNATURE_BYTES: usize = 8 * 1024;

const TARGET: &str = env!("OWNSEARCH_TARGET");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallChannel {
    Installer,
    Homebrew,
    Cargo,
    Npm,
    Container,
}

impl InstallChannel {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Installer => "the install script",
            Self::Homebrew => "Homebrew",
            Self::Cargo => "cargo install",
            Self::Npm => "npm",
            Self::Container => "a container image",
        }
    }

    const fn upgrade_command(self) -> Option<&'static str> {
        match self {
            Self::Installer => None,
            Self::Homebrew => Some("brew upgrade ownsearch"),
            Self::Cargo => Some("cargo install --locked ownsearch"),
            Self::Npm => Some("npm install -g @devops-infinity/ownsearch"),
            Self::Container => Some("docker pull ghcr.io/devops-infinity/ownsearch:latest"),
        }
    }

    const fn after_upgrade(self) -> Option<&'static str> {
        match self {
            Self::Container => Some("restart the container after the pull"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpdateStatus {
    pub current: String,
    pub latest: String,
    pub available: bool,
    pub asset: String,
    pub archive_url: Option<String>,
    pub channel_command: Option<&'static str>,
    pub installed: bool,
}

fn detect_channel(executable: &Path, container: bool) -> InstallChannel {
    if container {
        return InstallChannel::Container;
    }
    let path = executable.to_string_lossy();
    if path.contains("/Cellar/") || path.contains("/homebrew/") || path.contains("/linuxbrew/") {
        return InstallChannel::Homebrew;
    }
    if path.contains("/.cargo/bin/") {
        return InstallChannel::Cargo;
    }
    if path.contains("/node_modules/") || path.contains("/_npx/") || path.contains("/.npm/") {
        return InstallChannel::Npm;
    }
    InstallChannel::Installer
}

fn in_container() -> bool {
    std::env::var("OWNSEARCH_IN_CONTAINER").as_deref() == Ok("1")
        || Path::new("/.dockerenv").exists()
        || Path::new("/run/.containerenv").exists()
}

fn asset_name() -> String {
    if cfg!(windows) {
        format!("ownsearch-{TARGET}.zip")
    } else {
        format!("ownsearch-{TARGET}.tar.gz")
    }
}

pub fn check() -> Result<UpdateStatus> {
    check_from(&api_base())
}

pub fn apply() -> Result<UpdateStatus> {
    apply_from(&api_base())
}

fn api_base() -> String {
    #[cfg(debug_assertions)]
    if let Ok(base) = std::env::var("OWNSEARCH_UPDATE_API_BASE")
        && !base.trim().is_empty()
    {
        return base;
    }
    API.to_owned()
}

fn check_from(base: &str) -> Result<UpdateStatus> {
    let channel = channel()?;
    let release = fetch_json(base, &format!("/repos/{OWNER}/{REPO}/releases/latest"))?;
    let tag = release
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Update("the latest release carries no tag".into()))?;
    let latest = tag.trim_start_matches('v');
    let latest_version = Version::parse(latest).map_err(|error| {
        Error::Update(format!(
            "the latest release tag {tag:?} is not a version: {error}"
        ))
    })?;
    let current_version = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| Error::Update(format!("the built version is not a version: {error}")))?;
    let asset = asset_name();
    let archive_url = asset_url(&release, &asset);
    Ok(UpdateStatus {
        current: env!("CARGO_PKG_VERSION").to_owned(),
        latest: latest.to_owned(),
        available: latest_version > current_version,
        asset,
        archive_url,
        channel_command: channel.upgrade_command(),
        installed: false,
    })
}

fn apply_from(base: &str) -> Result<UpdateStatus> {
    guard_channel(channel()?)?;
    let status = check_from(base)?;
    if !status.available {
        return Ok(status);
    }
    if status.archive_url.is_none() {
        return Err(Error::Update(format!(
            "release {} has no {} asset, so this platform cannot update itself; download the release from https://github.com/{OWNER}/{REPO}/releases/latest and install it by hand",
            status.latest, status.asset
        )));
    }
    let sums = fetch_asset(base, "sha256.sum", MAX_SUMS_BYTES)?;
    let signature = fetch_asset(base, "sha256.sum.minisig", MAX_SIGNATURE_BYTES)?;
    verify_signature(&sums, &signature)?;
    let sums_text = String::from_utf8(sums)
        .map_err(|_| Error::Update("the signed sums file is not text".into()))?;
    let digest = digest_for(&sums_text, &status.asset).ok_or_else(|| {
        Error::Update(format!(
            "{} is not listed in the signed sha256.sum, so the update cannot be verified; install the release from https://github.com/{OWNER}/{REPO}/releases/latest by hand",
            status.asset
        ))
    })?;
    let installed = self_update::backends::github::Update::configure()
        .repo_owner(OWNER)
        .repo_name(REPO)
        .bin_name("ownsearch")
        .current_version(env!("CARGO_PKG_VERSION"))
        .verify_checksum(self_update::Checksum::Sha256(digest))
        .check_install_path_writable(true)
        .build()
        .map_err(|error| Error::Update(format!("preparing the update: {error}")))?
        .update()
        .map_err(|error| {
            let text = error.to_string().to_ascii_lowercase();
            let hint = if text.contains("permission") || text.contains("denied") {
                "; the directory that holds the binary must be writable"
            } else {
                "; the old binary is unchanged"
            };
            Error::Update(format!("installing the update: {error}{hint}"))
        })?;
    Ok(UpdateStatus {
        latest: installed.version().to_owned(),
        installed: true,
        ..status
    })
}

fn channel() -> Result<InstallChannel> {
    let executable = std::env::current_exe()
        .map_err(|error| Error::Update(format!("locating the running binary: {error}")))?;
    Ok(detect_channel(&executable, in_container()))
}

fn guard_channel(channel: InstallChannel) -> Result<()> {
    match channel.upgrade_command() {
        None => Ok(()),
        Some(command) => {
            let advice = channel
                .after_upgrade()
                .map(|advice| format!("; {advice}"))
                .unwrap_or_default();
            Err(Error::Update(format!(
                "ownsearch here is installed by {}; update it with `{command}`{advice}",
                channel.as_str()
            )))
        }
    }
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(format!(
            "OwnSearch (self-update; +https://github.com/{OWNER}/{REPO}) v{}",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .map_err(|error| Error::network("building the update client", error))
}

fn token() -> Option<String> {
    std::env::var("GH_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn authorized(request: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
    match token() {
        Some(token) => request.header("authorization", format!("Bearer {token}")),
        None => request,
    }
}

fn fetch_json(base: &str, path: &str) -> Result<Value> {
    let response = authorized(client()?.get(format!("{base}{path}")))
        .header("accept", "application/vnd.github+json")
        .send()
        .map_err(|error| Error::network("fetching the latest release", error))?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| Error::network("reading the latest release", error))?;
    if !status.is_success() {
        let hint = match status.as_u16() {
            403 | 429 => {
                "GitHub rate limited this request; set GH_TOKEN or GITHUB_TOKEN and try again"
            }
            404 => {
                "GitHub has no published release here; check https://github.com/{OWNER}/{REPO}/releases"
            }
            _ => "GitHub refused the request; try again later",
        };
        return Err(Error::Update(format!(
            "fetching the latest release failed with {status}: {hint}"
        )));
    }
    serde_json::from_str(&body)
        .map_err(|error| Error::Update(format!("the latest release did not parse: {error}")))
}

fn asset_url(release: &Value, asset: &str) -> Option<String> {
    release
        .get("assets")
        .and_then(Value::as_array)?
        .iter()
        .find(|item| item.get("name").and_then(Value::as_str) == Some(asset))
        .and_then(|item| item.get("browser_download_url"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn fetch_asset(base: &str, name: &str, cap: usize) -> Result<Vec<u8>> {
    let release = fetch_json(base, &format!("/repos/{OWNER}/{REPO}/releases/latest"))?;
    let url = asset_url(&release, name).ok_or_else(|| {
        Error::Update(format!(
            "the latest release publishes no {name}, so the update cannot be verified; install the release from https://github.com/{OWNER}/{REPO}/releases/latest by hand"
        ))
    })?;
    let response = authorized(client()?.get(url))
        .send()
        .map_err(|error| Error::network("downloading a release signature", error))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .map_err(|error| Error::network("reading a release signature", error))?;
    if !status.is_success() {
        return Err(Error::Update(format!(
            "GitHub answered {status} for {name}"
        )));
    }
    if bytes.len() > cap {
        return Err(Error::Update(format!(
            "{name} is larger than the {cap} byte cap"
        )));
    }
    Ok(bytes.to_vec())
}

fn verify_signature(sums: &[u8], signature: &[u8]) -> Result<()> {
    let key = minisign_verify::PublicKey::from_base64(UPDATE_KEY)
        .map_err(|error| Error::Update(format!("the embedded update key is malformed: {error}")))?;
    let text = std::str::from_utf8(signature)
        .map_err(|_| Error::Update("the release signature is not text".into()))?;
    let signature = minisign_verify::Signature::decode(text).map_err(|error| {
        Error::Update(format!(
            "the release signature is malformed: {error}; do not install this download, and report it at https://github.com/{OWNER}/{REPO}/security/advisories/new"
        ))
    })?;
    key.verify(sums, &signature, false).map_err(|error| {
        Error::Update(format!(
            "the release signature did not verify: {error}; do not install this download, and report it at https://github.com/{OWNER}/{REPO}/security/advisories/new"
        ))
    })
}

fn digest_for(sums: &str, asset: &str) -> Option<String> {
    for line in sums.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(digest) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        let name = name.trim_start_matches('*').trim_start_matches("./");
        if name == asset
            && digest.len() == 64
            && digest
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Some(digest.to_ascii_lowercase());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer as WireMockServer, ResponseTemplate};

    struct Server {
        runtime: tokio::runtime::Runtime,
        server: WireMockServer,
    }

    impl Server {
        fn start() -> Self {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("runtime");
            let server = runtime.block_on(WireMockServer::start());
            Self { runtime, server }
        }

        fn uri(&self) -> String {
            self.server.uri()
        }

        fn route(&self, path: &str, status: u16, body: &str) {
            let mock = Mock::given(wiremock::matchers::method("GET"))
                .and(wiremock::matchers::path(path.to_owned()))
                .respond_with(ResponseTemplate::new(status).set_body_string(body.to_owned()));
            self.runtime.block_on(self.server.register(mock));
        }

        fn release(&self, tag: &str) {
            let asset = asset_name();
            let base = self.uri();
            let body = format!(
                r#"{{"tag_name":"{tag}","assets":[
                    {{"name":"sha256.sum","browser_download_url":"{base}/sums"}},
                    {{"name":"sha256.sum.minisig","browser_download_url":"{base}/sums.minisig"}},
                    {{"name":"{asset}","browser_download_url":"{base}/archive"}}
                ]}}"#
            );
            self.route(&release_path(), 200, &body);
        }
    }

    const FOREIGN_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\n\
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\n\
trusted comment: timestamp:1633700835\tfile:test\tprehashed\n\
wLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==\n";

    fn release_path() -> String {
        format!("/repos/{OWNER}/{REPO}/releases/latest")
    }

    #[test]
    fn the_channel_follows_the_binary_location() {
        let cases = [
            (
                "/home/linuxbrew/.linuxbrew/bin/ownsearch",
                false,
                InstallChannel::Homebrew,
            ),
            (
                "/opt/homebrew/Cellar/ownsearch/0.3.0/bin/ownsearch",
                false,
                InstallChannel::Homebrew,
            ),
            (
                "/home/you/.cargo/bin/ownsearch",
                false,
                InstallChannel::Cargo,
            ),
            (
                "/usr/lib/node_modules/@devops-infinity/ownsearch/run.js",
                false,
                InstallChannel::Npm,
            ),
            (
                "/home/you/.local/bin/ownsearch",
                false,
                InstallChannel::Installer,
            ),
            ("/usr/local/bin/ownsearch", true, InstallChannel::Container),
        ];
        for (path, container, expected) in cases {
            assert_eq!(
                detect_channel(Path::new(path), container),
                expected,
                "{path}"
            );
        }
    }

    #[test]
    fn only_the_install_script_channel_updates_itself() {
        assert!(InstallChannel::Installer.upgrade_command().is_none());
        for channel in [
            InstallChannel::Homebrew,
            InstallChannel::Cargo,
            InstallChannel::Npm,
            InstallChannel::Container,
        ] {
            assert!(channel.upgrade_command().is_some(), "{channel:?}");
            assert!(!channel.as_str().is_empty());
        }
    }

    #[test]
    fn the_digest_is_read_from_the_signed_sums() {
        let sums = "# comment\n\
                    aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  ownsearch-aarch64-apple-darwin.tar.gz\n\
                    bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  ownsearch-x86_64-unknown-linux-gnu.tar.gz\n\
                    cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  *./ownsearch-x86_64-pc-windows-msvc.zip\n";
        assert_eq!(
            digest_for(sums, "ownsearch-x86_64-unknown-linux-gnu.tar.gz"),
            Some("b".repeat(64))
        );
        assert_eq!(
            digest_for(sums, "ownsearch-x86_64-pc-windows-msvc.zip"),
            Some("c".repeat(64))
        );
        assert_eq!(
            digest_for(sums, "ownsearch-x86_64-unknown-linux-musl.tar.gz"),
            None
        );
        assert_eq!(
            digest_for("", "ownsearch-x86_64-unknown-linux-gnu.tar.gz"),
            None
        );
        assert_eq!(
            digest_for(
                "abc  ownsearch-x86_64-unknown-linux-gnu.tar.gz",
                "ownsearch-x86_64-unknown-linux-gnu.tar.gz"
            ),
            None
        );
    }

    #[test]
    fn the_embedded_key_parses_and_a_garbage_signature_is_refused() {
        assert!(minisign_verify::PublicKey::from_base64(UPDATE_KEY).is_ok());
        assert!(verify_signature(b"sums", b"not a signature").is_err());
        assert!(verify_signature(b"sums", b"").is_err());
    }

    #[test]
    fn the_asset_name_carries_the_target() {
        let name = asset_name();
        assert!(name.starts_with("ownsearch-"));
        assert!(name.contains(TARGET), "{name}");
    }

    #[test]
    fn only_the_installer_channel_passes_the_guard() {
        assert!(guard_channel(InstallChannel::Installer).is_ok());
        for channel in [
            InstallChannel::Homebrew,
            InstallChannel::Cargo,
            InstallChannel::Npm,
            InstallChannel::Container,
        ] {
            let error = guard_channel(channel).expect_err("refused");
            assert!(error.to_string().contains("update it with"), "{error}");
        }
    }

    #[test]
    fn the_api_base_names_a_url() {
        assert!(api_base().starts_with("http"));
    }

    #[test]
    fn a_newer_release_is_reported_with_its_archive() {
        let server = Server::start();
        server.release("v99.0.0");
        let status = check_from(&server.uri()).expect("check");
        assert!(status.available);
        assert_eq!(status.latest, "99.0.0");
        assert_eq!(
            status.archive_url,
            Some(format!("{}/archive", server.uri()))
        );
    }

    #[test]
    fn an_older_or_equal_release_is_not_offered() {
        let server = Server::start();
        server.release(&format!("v{}", env!("CARGO_PKG_VERSION")));
        let status = check_from(&server.uri()).expect("check");
        assert!(!status.available);
        assert!(!apply_from(&server.uri()).expect("apply").available);
    }

    #[test]
    fn a_release_without_this_platform_asset_names_the_release_page() {
        let server = Server::start();
        server.route(
            &release_path(),
            200,
            r#"{"tag_name":"v99.0.0","assets":[]}"#,
        );
        let status = check_from(&server.uri()).expect("check");
        assert!(status.available);
        assert_eq!(status.archive_url, None);
        let error = apply_from(&server.uri()).expect_err("apply");
        assert!(error.to_string().contains("releases/latest"), "{error}");
    }

    #[test]
    fn an_api_error_names_the_token() {
        let server = Server::start();
        server.route(&release_path(), 403, "");
        let error = check_from(&server.uri()).expect_err("check");
        assert!(error.to_string().contains("GH_TOKEN"), "{error}");
    }

    #[test]
    fn a_release_that_is_not_json_is_refused() {
        let server = Server::start();
        server.route(&release_path(), 200, "<html>");
        assert!(check_from(&server.uri()).is_err());
    }

    #[test]
    fn a_release_without_a_tag_is_refused() {
        let server = Server::start();
        server.route(&release_path(), 200, r#"{"assets":[]}"#);
        assert!(check_from(&server.uri()).is_err());
    }

    #[test]
    fn a_tag_that_is_not_a_version_is_refused() {
        let server = Server::start();
        server.release("not-a-version");
        assert!(check_from(&server.uri()).is_err());
    }

    #[test]
    fn an_asset_download_is_capped_and_reported() {
        let server = Server::start();
        server.release("v99.0.0");
        server.route("/sums", 200, "x");
        let base = server.uri();
        assert_eq!(fetch_asset(&base, "sha256.sum", 8).expect("asset"), b"x");
        assert!(fetch_asset(&base, "sha256.sum", 0).is_err());
        assert!(fetch_asset(&base, "missing-sum", 8).is_err());
    }

    #[test]
    fn a_signature_that_does_not_verify_stops_the_update() {
        let server = Server::start();
        server.release("v99.0.0");
        server.route("/sums", 200, "sums");
        server.route("/sums.minisig", 200, FOREIGN_SIGNATURE);
        let error = apply_from(&server.uri()).expect_err("apply");
        assert!(error.to_string().contains("did not verify"), "{error}");
        assert!(error.to_string().contains("security/advisories"), "{error}");
    }

    #[test]
    fn a_tag_without_the_v_prefix_is_accepted() {
        let server = Server::start();
        server.release("99.0.0");
        let status = check_from(&server.uri()).expect("check");
        assert_eq!(status.latest, "99.0.0");
        assert!(status.available);
    }

    #[test]
    fn an_asset_without_a_download_url_is_ignored() {
        let server = Server::start();
        let body = format!(
            r#"{{"tag_name":"v99.0.0","assets":[{{"name":"{}"}}]}}"#,
            asset_name()
        );
        server.route(&release_path(), 200, &body);
        let status = check_from(&server.uri()).expect("check");
        assert_eq!(status.archive_url, None);
    }

    #[test]
    fn a_server_error_is_reported_with_its_status() {
        let server = Server::start();
        server.route(&release_path(), 500, "");
        let error = check_from(&server.uri()).expect_err("check");
        assert!(error.to_string().contains("500"), "{error}");
    }

    #[test]
    fn an_empty_release_body_is_refused() {
        let server = Server::start();
        server.route(&release_path(), 200, "");
        assert!(check_from(&server.uri()).is_err());
    }

    #[test]
    fn a_sums_asset_that_answers_an_error_is_reported() {
        let server = Server::start();
        server.release("v99.0.0");
        server.route("/sums", 404, "");
        let error = apply_from(&server.uri()).expect_err("apply");
        assert!(error.to_string().contains("sha256.sum"), "{error}");
        assert!(error.to_string().contains("404"), "{error}");
    }

    #[test]
    fn an_uppercase_digest_is_lowercased() {
        let digest = "A".repeat(64);
        let sums = format!("{digest}  ownsearch-{TARGET}.tar.gz\n");
        assert_eq!(
            digest_for(&sums, &format!("ownsearch-{TARGET}.tar.gz")),
            Some("a".repeat(64))
        );
    }

    #[test]
    fn a_malformed_signature_stops_the_update() {
        let server = Server::start();
        server.release("v99.0.0");
        server.route("/sums", 200, "sums");
        server.route("/sums.minisig", 200, "garbage");
        let error = apply_from(&server.uri()).expect_err("apply");
        assert!(error.to_string().contains("malformed"), "{error}");
        assert!(error.to_string().contains("security/advisories"), "{error}");
    }
}
