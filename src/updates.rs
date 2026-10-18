//! Update checks and bounded, checksum-verified release downloads.
//! Installation is delegated to the platform's normal installer UI.
use anyhow::{Context as _, Result};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
const API: &str = "https://api.github.com/repos/zaydzaari/Lumen/releases/latest";
const LIMIT: u64 = 150 * 1024 * 1024;
const INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Windows,
    Flatpak,
    Deb,
    Archive,
    Unsupported,
}
impl Kind {
    pub fn detect() -> Self {
        if std::env::consts::ARCH != "x86_64" {
            return Self::Unsupported;
        }
        if cfg!(windows) {
            return Self::Windows;
        }
        if cfg!(target_os = "linux") {
            if std::env::var_os("FLATPAK_ID").is_some() {
                return Self::Flatpak;
            }
            if std::env::current_exe().is_ok_and(|p| p.starts_with("/usr"))
                && std::path::Path::new("/var/lib/dpkg/status").exists()
            {
                return Self::Deb;
            }
            return Self::Archive;
        }
        Self::Unsupported
    }
    fn asset(self, version: &str) -> String {
        match self {
            Self::Windows => format!("lumen-{version}-windows-x86_64-setup.exe"),
            Self::Flatpak => format!("lumen-{version}-linux-x86_64.flatpak"),
            Self::Deb => format!("lumen_{version}_amd64.deb"),
            Self::Archive => format!("lumen-{version}-linux-x86_64.tar.gz"),
            Self::Unsupported => String::new(),
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}
#[derive(Clone, Debug, Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Clone)]
pub struct Ready {
    pub path: PathBuf,
    hash: String,
    size: u64,
}
#[derive(Clone, Default)]
pub struct State {
    pub busy: bool,
    pub status: String,
    pub version: Option<String>,
    pub ready: Option<Ready>,
}
#[derive(Default)]
pub struct Updater {
    state: Arc<Mutex<State>>,
    checked: Option<Instant>,
    pub dismissed: Option<String>,
}
impl Updater {
    pub fn snapshot(&self) -> State {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn tick(&mut self, shared: &crate::backend::Shared, ctx: &egui::Context, enabled: bool) {
        if enabled && self.checked.is_none_or(|t| t.elapsed() >= INTERVAL) {
            self.check(shared, ctx, true);
        }
    }
    pub fn check(&mut self, shared: &crate::backend::Shared, ctx: &egui::Context, download: bool) {
        if self.snapshot().busy {
            return;
        }
        self.checked = Some(Instant::now());
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.busy = true;
            s.status = "Checking for updates…".into();
        }
        let state = self.state.clone();
        let shared = shared.clone();
        let ctx = ctx.clone();
        shared.rt.clone().spawn(async move {
            let result = fetch(&shared, Kind::detect(), download).await;
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            match result {
                Ok(Some((version, ready))) => {
                    s.status = if ready.is_some() {
                        format!("Lumen {version} is ready to install")
                    } else {
                        format!("Lumen {version} is available")
                    };
                    s.version = Some(version);
                    s.ready = ready;
                }
                Ok(None) => {
                    s.status = "You have the latest version".into();
                    s.version = None;
                    s.ready = None;
                }
                Err(e) => s.status = format!("Update check failed: {e:#}"),
            }
            s.busy = false;
            ctx.request_repaint();
        });
    }
}
fn stable_version(value: &str) -> Option<semver::Version> {
    let version = semver::Version::parse(value.strip_prefix('v').unwrap_or(value)).ok()?;
    (version.pre.is_empty() && version.build.is_empty()).then_some(version)
}
fn trusted_asset(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str() == Some("github.com")
            && u.username().is_empty()
            && u.password().is_none()
            && u.port_or_known_default() == Some(443)
            && u.path().starts_with("/zaydzaari/Lumen/releases/download/")
    })
}
fn checksum(text: &str, name: &str) -> Result<String> {
    let hashes: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (hash, file) = line.split_once(char::is_whitespace)?;
            (file.trim().trim_start_matches('*') == name
                && hash.len() == 64
                && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
        })
        .collect();
    anyhow::ensure!(
        hashes.len() == 1,
        "Release is missing an unambiguous checksum for {name}"
    );
    Ok(hashes[0].clone())
}
async fn bounded(http: &reqwest::Client, url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut response = http
        .get(url)
        .timeout(Duration::from_secs(180))
        .send()
        .await?
        .error_for_status()?;
    anyhow::ensure!(
        response.content_length().is_none_or(|n| n <= limit),
        "Update response is too large"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            (bytes.len() as u64) + chunk.len() as u64 <= limit,
            "Update response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn fetch(
    shared: &crate::backend::Shared,
    kind: Kind,
    download: bool,
) -> Result<Option<(String, Option<Ready>)>> {
    let release: Release = serde_json::from_slice(&bounded(&shared.http, API, 1 << 20).await?)?;
    let current = stable_version(env!("CARGO_PKG_VERSION")).context("Invalid current version")?;
    let next = stable_version(&release.tag_name).context("Invalid release version")?;
    if release.draft || release.prerelease || next <= current {
        return Ok(None);
    }
    let version = next.to_string();
    if !download || kind == Kind::Unsupported {
        return Ok(Some((version, None)));
    }
    let name = kind.asset(&version);
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == name)
        .context("No installer for this platform in the release")?;
    let sums = release
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS.txt")
        .context("The release has no checksum manifest")?;
    anyhow::ensure!(
        trusted_asset(&asset.browser_download_url) && trusted_asset(&sums.browser_download_url),
        "Unexpected update location"
    );
    anyhow::ensure!(asset.size > 0 && asset.size <= LIMIT, "Invalid update size");
    let manifest = bounded(&shared.http, &sums.browser_download_url, 64 * 1024).await?;
    let hash = checksum(std::str::from_utf8(&manifest)?, &name)?;
    let folder = shared.paths.cache_dir.join("updates").join(&version);
    std::fs::create_dir_all(&folder)?;
    let path = folder.join(&name);
    if matches_file(&path, &hash, asset.size) {
        return Ok(Some((
            version,
            Some(Ready {
                path,
                hash,
                size: asset.size,
            }),
        )));
    }
    let tmp = folder.join("download.part");
    let mut response = shared
        .http
        .get(&asset.browser_download_url)
        .timeout(Duration::from_secs(600))
        .send()
        .await?
        .error_for_status()?;
    let mut file = std::fs::File::create(&tmp)?;
    let mut count = 0u64;
    let mut digest = Sha256::new();
    while let Some(chunk) = response.chunk().await? {
        count += chunk.len() as u64;
        anyhow::ensure!(
            count <= asset.size && count <= LIMIT,
            "Update exceeded its advertised size"
        );
        file.write_all(&chunk)?;
        digest.update(&chunk);
    }
    file.sync_all()?;
    drop(file);
    anyhow::ensure!(
        count == asset.size && format!("{:x}", digest.finalize()) == hash,
        "Update checksum mismatch; installation cancelled"
    );
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(Some((
        version,
        Some(Ready {
            path,
            hash,
            size: asset.size,
        }),
    )))
}
fn matches_file(path: &std::path::Path, expected: &str, size: u64) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    if !file.metadata().is_ok_and(|m| m.len() == size) {
        return false;
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                total += count as u64;
                if total > LIMIT {
                    return false;
                }
                digest.update(&buffer[..count]);
            }
            Err(_) => return false,
        }
    }
    total == size && format!("{:x}", digest.finalize()) == expected
}
pub fn open_installer(ready: &Ready) -> Result<()> {
    anyhow::ensure!(
        matches_file(&ready.path, &ready.hash, ready.size),
        "The update file changed. Download it again before installing"
    );
    open::that_detached(&ready.path).context("Could not open the downloaded installer")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions_are_ordered_not_compared_as_text() {
        assert!(stable_version("v0.1.10").unwrap() > stable_version("0.1.9").unwrap());
        assert!(stable_version("v0.2.0-beta").is_none());
        assert!(stable_version("../bad").is_none());
    }
    #[test]
    fn checksums_require_exact_unique_asset() {
        let hash = "a".repeat(64);
        let text = format!("{hash}  lumen.exe\n");
        assert_eq!(checksum(&text, "lumen.exe").unwrap(), hash);
        assert!(checksum(&text, "lumen").is_err());
        assert!(checksum(&text.repeat(2), "lumen.exe").is_err());
        assert!(!trusted_asset(
            "https://github.com/evil/Lumen/releases/download/v1/app"
        ));
        assert!(trusted_asset(
            "https://github.com/zaydzaari/Lumen/releases/download/v0.1.1/app"
        ));
    }
    #[test]
    fn cached_installers_are_reverified_and_tampering_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update");
        std::fs::write(&path, b"release").unwrap();
        let hash = format!("{:x}", Sha256::digest(b"release"));
        assert!(matches_file(&path, &hash, 7));
        std::fs::write(&path, b"changed").unwrap();
        assert!(!matches_file(&path, &hash, 7));
        assert!(open_installer(&Ready {
            path,
            hash,
            size: 7
        })
        .is_err());
    }
}
