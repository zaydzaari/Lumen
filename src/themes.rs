//! Colour themes, compatible with Spicetify's theme ecosystem.
//!
//! Spicetify themes ship a `color.ini` with one or more named colour schemes
//! (`text`, `subtext`, `main`, `sidebar`, `player`, `card`, `button`, …) plus a
//! `user.css` that restyles Spotify's web UI. Lumen is a native app, so it can't
//! run CSS, but it maps every scheme in `color.ini` onto its own palette. That
//! covers what most people want from a theme: the colours.
//!
//! The marketplace lists the same themes the Spicetify Marketplace does: the
//! official `spicetify/spicetify-themes` repository (its `manifest.json`) plus
//! community repositories tagged with the GitHub topic `spicetify-themes`
//! (each with a root `manifest.json`), minus the Marketplace's blacklist.
//! Everything is fetched from GitHub on demand and cached for 12 hours.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// One named colour scheme (a `[section]` of `color.ini`). Keys are lowercase.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorScheme {
    pub name: String,
    pub colors: BTreeMap<String, [u8; 3]>,
}

impl ColorScheme {
    pub fn get(&self, key: &str) -> Option<[u8; 3]> {
        self.colors.get(key).copied()
    }

    /// Colours shown as swatches in the theme browser.
    pub fn swatches(&self) -> Vec<[u8; 3]> {
        ["main", "sidebar", "card", "text", "subtext", "button"]
            .iter()
            .filter_map(|k| self.get(k))
            .collect()
    }

    /// A scheme needs at least a background and a text colour to be usable.
    pub fn usable(&self) -> bool {
        self.get("main").is_some() && self.get("text").is_some()
    }
}

/// Parse a hex colour as written in `color.ini` (`1db954`, `#1DB954`, `fff`).
pub fn parse_ini_color(v: &str) -> Option<[u8; 3]> {
    let v = v.trim().trim_start_matches('#');
    let v = v.split(|c: char| c.is_whitespace() || c == ';').next()?;
    let hex = |s: &str| u8::from_str_radix(s, 16).ok();
    match v.len() {
        6 | 8 => Some([hex(&v[0..2])?, hex(&v[2..4])?, hex(&v[4..6])?]),
        3 => {
            let d = |i: usize| hex(&v[i..i + 1]).map(|x| x * 17);
            Some([d(0)?, d(1)?, d(2)?])
        }
        _ => None,
    }
}

/// Parse every scheme in a `color.ini`. Values that aren't plain hex colours
/// (e.g. `${xrdb:color1}`) are skipped; unusable schemes are dropped.
pub fn parse_color_ini(text: &str) -> Vec<ColorScheme> {
    let mut out: Vec<ColorScheme> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            out.push(ColorScheme {
                name: name.trim().to_string(),
                colors: BTreeMap::new(),
            });
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let Some(scheme) = out.last_mut() else {
            continue;
        };
        if let Some(c) = parse_ini_color(v) {
            scheme.colors.insert(k.trim().to_ascii_lowercase(), c);
        }
    }
    out.retain(ColorScheme::usable);
    out
}

/// A theme saved on this computer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledTheme {
    /// Stable id, e.g. `spicetify/spicetify-themes/Sleek`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    pub schemes: Vec<ColorScheme>,
}

/// A theme listed in the marketplace (not necessarily installed).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketTheme {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub preview: Option<String>,
    /// Raw URL of the theme's `color.ini`.
    pub color_ini: String,
    pub repo: String,
    pub stars: u32,
    pub official: bool,
}

// ------------------------------------------------------------------ storage

fn file_name(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        + ".json"
}

pub fn load_installed(dir: &Path) -> Vec<InstalledTheme> {
    let mut v: Vec<InstalledTheme> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<InstalledTheme>(&b).ok())
        .filter(|t| !t.schemes.is_empty())
        .collect();
    v.sort_by_key(|t| t.name.to_lowercase());
    v
}

pub fn save_installed(dir: &Path, t: &InstalledTheme) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name(&t.id));
    std::fs::write(&path, serde_json::to_vec_pretty(t)?)
        .with_context(|| format!("writing {}", path.display()))
}

pub fn remove_installed(dir: &Path, id: &str) -> Result<()> {
    let path = dir.join(file_name(id));
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Install a local `color.ini` (file picked or dropped by the user).
pub fn import_file(dir: &Path, path: &Path) -> Result<InstalledTheme> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let schemes = parse_color_ini(&text);
    if schemes.is_empty() {
        bail!("no colour schemes found in {}", path.display());
    }
    let name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Imported theme".into());
    let t = InstalledTheme {
        id: format!("local/{name}"),
        name,
        author: "You".into(),
        preview: None,
        repo: None,
        schemes,
    };
    save_installed(dir, &t)?;
    Ok(t)
}

// ------------------------------------------------------------------ marketplace

const OFFICIAL_REPO: &str = "spicetify/spicetify-themes";
const OFFICIAL_MANIFEST: &str =
    "https://raw.githubusercontent.com/spicetify/spicetify-themes/master/manifest.json";
const BLACKLIST: &str =
    "https://raw.githubusercontent.com/spicetify/marketplace/main/resources/blacklist.json";
const SEARCH: &str = "https://api.github.com/search/repositories?q=topic:spicetify-themes&sort=stars&order=desc&per_page=60";
const CACHE_TTL: Duration = Duration::from_secs(12 * 3600);

#[derive(Deserialize)]
struct ManifestEntry {
    name: Option<String>,
    description: Option<String>,
    preview: Option<String>,
    schemes: Option<String>,
    #[serde(default)]
    authors: Vec<ManifestAuthor>,
}

#[derive(Deserialize)]
struct ManifestAuthor {
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Manifest {
    Many(Vec<ManifestEntry>),
    One(ManifestEntry),
}

#[derive(Deserialize)]
struct SearchResp {
    items: Vec<SearchRepo>,
}

#[derive(Deserialize)]
struct SearchRepo {
    full_name: String,
    html_url: String,
    default_branch: String,
    stargazers_count: u32,
    #[serde(default)]
    archived: bool,
    owner: SearchOwner,
}

#[derive(Deserialize)]
struct SearchOwner {
    login: String,
}

/// Resolve a manifest path against the repo's raw base URL.
fn raw_url(base: &str, p: &str) -> String {
    if p.starts_with("http://") || p.starts_with("https://") {
        p.to_string()
    } else {
        format!(
            "{base}/{}",
            p.trim_start_matches("./").trim_start_matches('/')
        )
    }
}

fn entries(
    m: Manifest,
    repo: &str,
    repo_url: &str,
    base: &str,
    owner: &str,
    stars: u32,
    official: bool,
) -> Vec<MarketTheme> {
    let list = match m {
        Manifest::Many(v) => v,
        Manifest::One(e) => vec![e],
    };
    list.into_iter()
        .filter_map(|e| {
            let name = e.name?.trim().to_string();
            let schemes = e.schemes.filter(|s| !s.trim().is_empty())?;
            let author = e
                .authors
                .iter()
                .filter_map(|a| a.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            Some(MarketTheme {
                id: format!("{repo}/{name}"),
                description: e.description.unwrap_or_default().trim().to_string(),
                author: if author.is_empty() {
                    owner.to_string()
                } else {
                    author
                },
                preview: e.preview.map(|p| raw_url(base, &p)),
                color_ini: raw_url(base, &schemes),
                repo: repo_url.to_string(),
                stars,
                official,
                name,
            })
        })
        .collect()
}

async fn get_text(http: &reqwest::Client, url: &str) -> Result<String> {
    let r = http
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    if !r.status().is_success() {
        bail!("{url}: {}", r.status());
    }
    Ok(r.text().await?)
}

/// The marketplace list: cached copy if it's fresh, otherwise fetched.
pub async fn marketplace(
    http: &reqwest::Client,
    cache_dir: &Path,
    force: bool,
) -> Result<Vec<MarketTheme>> {
    let cache = cache_dir.join("themes-market.json");
    if !force {
        if let Some(v) = read_fresh(&cache) {
            return Ok(v);
        }
    }
    match fetch_marketplace(http).await {
        Ok(v) => {
            let _ = std::fs::create_dir_all(cache_dir);
            let _ = std::fs::write(&cache, serde_json::to_vec(&v).unwrap_or_default());
            Ok(v)
        }
        // Offline or rate-limited: an old list beats nothing.
        Err(e) => std::fs::read(&cache)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or(e),
    }
}

fn read_fresh(path: &Path) -> Option<Vec<MarketTheme>> {
    let age = std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())?;
    if age > CACHE_TTL {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

async fn fetch_marketplace(http: &reqwest::Client) -> Result<Vec<MarketTheme>> {
    let official = async {
        let text = get_text(http, OFFICIAL_MANIFEST).await?;
        let m: Manifest = serde_json::from_str(&text)?;
        anyhow::Ok(entries(
            m,
            OFFICIAL_REPO,
            "https://github.com/spicetify/spicetify-themes",
            "https://raw.githubusercontent.com/spicetify/spicetify-themes/master",
            "spicetify",
            u32::MAX,
            true,
        ))
    };
    let blacklist = async {
        get_text(http, BLACKLIST)
            .await
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("repos").cloned())
            .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
            .unwrap_or_default()
    };
    let search = async {
        let text = get_text(http, SEARCH).await?;
        anyhow::Ok(serde_json::from_str::<SearchResp>(&text)?.items)
    };
    let (official, blacklist, search) = tokio::join!(official, blacklist, search);
    let mut out = official.unwrap_or_else(|e| {
        tracing::warn!("official themes: {e:#}");
        Vec::new()
    });
    let repos: Vec<SearchRepo> = match search {
        Ok(v) => v,
        Err(e) if out.is_empty() => return Err(e.context("GitHub theme search")),
        Err(e) => {
            tracing::warn!("community themes: {e:#}");
            Vec::new()
        }
    };
    let repos: Vec<SearchRepo> = repos
        .into_iter()
        .filter(|r| !r.archived && r.full_name != OFFICIAL_REPO)
        .filter(|r| {
            !blacklist
                .iter()
                .any(|b| b.trim_end_matches('/').ends_with(&r.full_name))
        })
        .collect();
    // Fetch community manifests a few at a time.
    for chunk in repos.chunks(8) {
        let futs = chunk.iter().map(|r| async move {
            let base = format!(
                "https://raw.githubusercontent.com/{}/{}",
                r.full_name, r.default_branch
            );
            let text = get_text(http, &format!("{base}/manifest.json"))
                .await
                .ok()?;
            let m: Manifest = serde_json::from_str(&text).ok()?;
            Some(entries(
                m,
                &r.full_name,
                &r.html_url,
                &base,
                &r.owner.login,
                r.stargazers_count,
                false,
            ))
        });
        for v in futures_util::future::join_all(futs)
            .await
            .into_iter()
            .flatten()
        {
            out.extend(v);
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|t| seen.insert(t.id.clone()));
    Ok(out)
}

/// Download a marketplace theme's `color.ini` and turn it into an installed theme.
pub async fn install(
    http: &reqwest::Client,
    dir: PathBuf,
    t: MarketTheme,
) -> Result<InstalledTheme> {
    let text = get_text(http, &t.color_ini).await?;
    let schemes = parse_color_ini(&text);
    if schemes.is_empty() {
        bail!(
            "{} has no colour schemes Lumen can use (it styles Spotify with CSS only)",
            t.name
        );
    }
    let it = InstalledTheme {
        id: t.id,
        name: t.name,
        author: t.author,
        preview: t.preview,
        repo: Some(t.repo),
        schemes,
    };
    let save = it.clone();
    tokio::task::spawn_blocking(move || save_installed(&dir, &save)).await??;
    Ok(it)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INI: &str = "
[Wealthy]
; Green on dark grey background
text               = 8bc34a
subtext            = b4b4b4
main               = 202020
sidebar            = #202020
button             = 8BC34A ; comment
misc               = ${xrdb:color1}

[Broken]
text = fff

[Short]
main = 000
text = FFF
";

    #[test]
    fn parses_spicetify_color_ini() {
        let s = parse_color_ini(INI);
        assert_eq!(s.len(), 2, "scheme without `main` is dropped");
        assert_eq!(s[0].name, "Wealthy");
        assert_eq!(s[0].get("text"), Some([0x8b, 0xc3, 0x4a]));
        assert_eq!(s[0].get("sidebar"), Some([0x20, 0x20, 0x20]));
        assert_eq!(s[0].get("button"), Some([0x8b, 0xc3, 0x4a]));
        assert_eq!(s[0].get("misc"), None);
        assert_eq!(s[1].get("text"), Some([255, 255, 255]));
    }

    #[test]
    fn manifest_paths_resolve() {
        let m: Manifest = serde_json::from_str(
            r#"[{"name":"Sleek","description":"d","preview":"Sleek/coral.png","schemes":"Sleek/color.ini",
                 "authors":[{"name":"harbassan","url":"x"}]},
                {"name":"NoColors","usercss":"a.css"}]"#,
        )
        .unwrap();
        let v = entries(
            m,
            "o/r",
            "https://github.com/o/r",
            "https://raw/o/r/main",
            "o",
            5,
            false,
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].color_ini, "https://raw/o/r/main/Sleek/color.ini");
        assert_eq!(
            v[0].preview.as_deref(),
            Some("https://raw/o/r/main/Sleek/coral.png")
        );
        assert_eq!(v[0].author, "harbassan");
        let one: Manifest = serde_json::from_str(
            r#"{"name":"Comfy","preview":"https://x/y.png","schemes":"./Comfy/color.ini","authors":[]}"#,
        )
        .unwrap();
        let v = entries(one, "o/c", "u", "https://raw/o/c/main", "owner", 1, false);
        assert_eq!(v[0].color_ini, "https://raw/o/c/main/Comfy/color.ini");
        assert_eq!(v[0].preview.as_deref(), Some("https://x/y.png"));
        assert_eq!(v[0].author, "owner");
    }

    #[test]
    fn install_roundtrip() {
        let dir = std::env::temp_dir().join(format!("lumen-themes-{}", std::process::id()));
        let t = InstalledTheme {
            id: "a/b/C d".into(),
            name: "C d".into(),
            author: "x".into(),
            preview: None,
            repo: None,
            schemes: parse_color_ini(INI),
        };
        save_installed(&dir, &t).unwrap();
        assert_eq!(load_installed(&dir), vec![t.clone()]);
        remove_installed(&dir, &t.id).unwrap();
        assert!(load_installed(&dir).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
