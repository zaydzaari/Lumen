//! Configuration (TOML in the OS-standard config dir) and application paths.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// All on-disk locations used by Lumen.
#[derive(Clone, Debug)]
pub struct Paths {
    /// `config.toml` and `plugins/` live here.
    pub config_dir: PathBuf,
    /// Logs, secrets (Linux), plugin data.
    pub data_dir: PathBuf,
    /// Audio cache (librespot format), album art, lyrics.
    pub cache_dir: PathBuf,
}

impl Paths {
    /// Resolve the OS-standard directories (or an override root, used by tests and
    /// `LUMEN_HOME` for portable installs).
    pub fn resolve() -> Result<Self> {
        if let Some(root) = std::env::var_os("LUMEN_HOME") {
            return Ok(Self::under(Path::new(&root)));
        }
        let dirs = directories::ProjectDirs::from("dev", "lumen", "Lumen")
            .context("could not determine the home directory")?;
        Ok(Self {
            config_dir: dirs.config_dir().to_path_buf(),
            data_dir: dirs.data_local_dir().to_path_buf(),
            cache_dir: dirs.cache_dir().to_path_buf(),
        })
    }

    /// All directories under one root (portable mode / tests).
    pub fn under(root: &Path) -> Self {
        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
        }
    }

    pub fn create_all(&self) -> Result<()> {
        for d in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.plugins_dir(),
            &self.logs_dir(),
        ] {
            fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
        }
        Ok(())
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }
    pub fn plugins_dir(&self) -> PathBuf {
        self.config_dir.join("plugins")
    }
    /// Each plugin gets `<this>/<plugin name>/` for its files.
    pub fn plugin_data_root(&self) -> PathBuf {
        self.data_dir.join("plugin-data")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
    pub fn secrets_dir(&self) -> PathBuf {
        self.data_dir.join("secrets")
    }
    pub fn audio_cache_dir(&self) -> PathBuf {
        self.cache_dir.join("audio")
    }
    pub fn art_cache_dir(&self) -> PathBuf {
        self.cache_dir.join("art")
    }
    pub fn lyrics_cache_dir(&self) -> PathBuf {
        self.cache_dir.join("lyrics")
    }
    /// Installed colour themes (one JSON file each).
    pub fn themes_dir(&self) -> PathBuf {
        self.config_dir.join("themes")
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioConfig {
    /// 96, 160 or 320 kbps.
    pub bitrate: u16,
    /// Keep playing similar songs when the queue runs out.
    pub autoplay: bool,
    pub normalisation: bool,
    /// With normalisation: "loud", "normal" or "quiet" (Spotify's Volume level).
    pub volume_level: String,
    pub gapless: bool,
    /// 0..=100
    pub volume: u8,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            bitrate: 320,
            autoplay: true,
            normalisation: false,
            volume_level: "normal".into(),
            gapless: true,
            volume: 70,
        }
    }
}

impl AudioConfig {
    /// Clamp the bitrate to one of the values Spotify serves.
    pub fn normalized_bitrate(&self) -> u16 {
        match self.bitrate {
            0..=128 => 96,
            129..=240 => 160,
            _ => 320,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CacheConfig {
    /// Use librespot's encrypted audio cache so replays don't re-stream.
    pub audio_cache: bool,
    /// Audio cache size limit in MB (librespot evicts automatically).
    pub audio_cache_mb: u64,
    /// Album art disk cache limit in MB.
    pub art_cache_mb: u64,
    /// Max decoded album-art textures kept in memory.
    pub art_memory_items: usize,
    /// Lyrics disk cache limit in MB.
    pub lyrics_cache_mb: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            audio_cache: true,
            audio_cache_mb: 2048,
            art_cache_mb: 64,
            art_memory_items: 150,
            lyrics_cache_mb: 16,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiConfig {
    pub theme: ThemeMode,
    /// Accent colour as `#rrggbb`.
    pub accent: String,
    /// Show a system tray icon (where supported). Takes effect after restart.
    pub tray: bool,
    /// Closing the window hides it to the tray instead of quitting.
    pub minimize_to_tray: bool,
    /// UI zoom factor.
    pub zoom: f32,
    /// Compact artwork rail; can be expanded from the library button.
    pub compact_library: bool,
    /// Windows: return unused memory to the OS at quiet moments.
    pub trim_memory: bool,
    /// Draw Lumen's own Spotify-style title bar instead of the system one.
    pub custom_title_bar: bool,
    /// Installed colour theme (Spicetify-compatible) id; empty = built-in theme.
    pub color_theme: String,
    /// Scheme within `color_theme` (a `[section]` of its color.ini).
    pub color_scheme: String,
    /// Animations (page fades, hover transitions, panel slides).
    pub animations: bool,
    /// Your Library sort: "recents", "alphabetical", "creator" or "custom".
    pub library_sort: String,
    /// Pinned library items (URIs), shown first.
    pub pinned: Vec<String>,
    /// Custom library order (URIs), from dragging rows.
    pub library_order: Vec<String>,
    /// Things you opened or played from search, newest first.
    pub recent_searches: Vec<RecentSearch>,
    /// Your Library "View as: Compact" (one text line per item, no covers).
    pub library_dense: bool,
    /// Friend Activity button in the top bar.
    pub show_friends: bool,
}

/// One entry in "Recent searches".
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RecentSearch {
    Collection(crate::model::Collection),
    Track(crate::model::Track),
}

impl RecentSearch {
    pub fn uri(&self) -> &str {
        match self {
            RecentSearch::Collection(c) => &c.uri,
            RecentSearch::Track(t) => &t.uri,
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Dark,
            accent: "#1ed760".into(),
            tray: true,
            minimize_to_tray: false,
            zoom: 1.0,
            compact_library: true,
            trim_memory: true,
            custom_title_bar: true,
            color_theme: String::new(),
            color_scheme: String::new(),
            animations: true,
            library_sort: "recents".into(),
            pinned: Vec::new(),
            library_order: Vec::new(),
            recent_searches: Vec::new(),
            library_dense: false,
            show_friends: true,
        }
    }
}

/// Keyboard shortcuts in `Modifier+Key` notation, e.g. `Ctrl+Right`, `Space`, `F12`.
/// `Ctrl` means Cmd on macOS.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Shortcuts {
    pub play_pause: String,
    pub next: String,
    pub previous: String,
    pub search: String,
    pub quit: String,
    pub debug_overlay: String,
    pub volume_up: String,
    pub volume_down: String,
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            play_pause: "Space".into(),
            next: "Ctrl+Right".into(),
            previous: "Ctrl+Left".into(),
            search: "Ctrl+L".into(),
            quit: "Ctrl+Q".into(),
            debug_overlay: "F12".into(),
            volume_up: "Ctrl+Up".into(),
            volume_down: "Ctrl+Down".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PluginsConfig {
    /// File names (without `.lua`) of plugins that are disabled.
    pub disabled: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LocalConfig {
    pub folders: Vec<PathBuf>,
    pub scan_on_start: bool,
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            scan_on_start: true,
        }
    }
}

/// Permanent downloads from matched YouTube audio, outside the evicting cache.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DownloadsConfig {
    /// Independent playback: skip Spotify streaming login; stream while saving.
    pub independent: bool,
    pub automatic: bool,
    /// Show attended in-app verification on demand; never run a browser while idle.
    pub browser_connection: bool,
    /// Empty selects the user's Music/Lumen Downloads folder.
    pub folder: PathBuf,
}
impl Default for DownloadsConfig {
    fn default() -> Self {
        Self {
            independent: false,
            automatic: true,
            browser_connection: cfg!(any(windows, target_os = "linux")),
            folder: PathBuf::new(),
        }
    }
}

/// Custom Discord Rich Presence.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DiscordConfig {
    pub enabled: bool,
    /// Application ID from https://discord.com/developers/applications.
    /// The application's name is shown after "Listening to".
    pub client_id: String,
    /// Optional override for the activity name (leave empty to use the app name).
    pub activity_name: String,
    /// Templates: {title}, {artist}, {album}.
    pub details: String,
    pub state: String,
    pub large_text: String,
    pub show_album_art: bool,
    pub show_progress: bool,
    pub show_button: bool,
    pub show_when_paused: bool,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            client_id: String::new(),
            activity_name: String::new(),
            details: "{title}".into(),
            state: "{artist}".into(),
            large_text: "{album}".into(),
            show_album_art: true,
            show_progress: true,
            show_button: true,
            show_when_paused: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub provider: crate::providers::Settings,
    /// Check and download stable releases; installation uses the system installer.
    pub automatic_updates: bool,
    /// Client ID of *your own* Spotify developer app, used for Web API calls
    /// (library, search). Leave empty to try the experimental session-token mode.
    pub client_id: String,
    /// Loopback port for the Web API OAuth redirect: `http://127.0.0.1:<port>/login`.
    pub redirect_port: u16,
    /// Name shown in the Spotify Connect device list.
    pub device_name: String,
    /// Stable device id (generated on first run).
    pub device_id: String,
    pub audio: AudioConfig,
    pub cache: CacheConfig,
    pub ui: UiConfig,
    pub shortcuts: Shortcuts,
    pub plugins: PluginsConfig,
    pub local: LocalConfig,
    pub discord: DiscordConfig,
    pub downloads: DownloadsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            automatic_updates: true,
            provider: crate::providers::Settings::default(),
            client_id: crate::spotify_library::DEFAULT_CLIENT_ID.into(),
            redirect_port: 8989,
            device_name: "Lumen".into(),
            device_id: String::new(),
            audio: AudioConfig::default(),
            cache: CacheConfig::default(),
            ui: UiConfig::default(),
            shortcuts: Shortcuts::default(),
            plugins: PluginsConfig::default(),
            discord: DiscordConfig::default(),
            downloads: DownloadsConfig::default(),
            local: LocalConfig::default(),
        }
    }
}

/// Result of loading the config: the config plus a human-readable warning when the
/// file was unreadable and defaults were used instead.
pub struct Loaded {
    pub config: Config,
    pub warning: Option<String>,
}

impl Config {
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/login", self.redirect_port)
    }

    /// Parse from TOML text. Unknown keys are ignored, missing keys get defaults.
    pub fn from_toml(text: &str) -> Result<Self> {
        let mut cfg: Config = toml::from_str(text).context("invalid config.toml")?;
        cfg.sanitize();
        Ok(cfg)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Keep values within sane ranges so a hand-edited file can't break the app.
    pub fn sanitize(&mut self) {
        self.audio.bitrate = self.audio.normalized_bitrate();
        self.audio.volume = self.audio.volume.min(100);
        self.cache.audio_cache_mb = self.cache.audio_cache_mb.clamp(64, 1024 * 1024);
        self.cache.art_cache_mb = self.cache.art_cache_mb.clamp(4, 4096);
        self.cache.lyrics_cache_mb = self.cache.lyrics_cache_mb.clamp(1, 1024);
        self.cache.art_memory_items = self.cache.art_memory_items.clamp(16, 1000);
        self.ui.zoom = if self.ui.zoom.is_finite() {
            self.ui.zoom.clamp(0.5, 3.0)
        } else {
            1.0
        };
        if parse_hex_color(&self.ui.accent).is_none() {
            self.ui.accent = UiConfig::default().accent;
        }
        if self.redirect_port == 0 {
            self.redirect_port = 8989;
        }
        self.client_id = self.client_id.trim().to_string();
        if self.client_id.is_empty() {
            self.client_id = crate::spotify_library::DEFAULT_CLIENT_ID.into();
        }
        self.discord.client_id = self
            .discord
            .client_id
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect();
        if self.device_name.trim().is_empty() {
            self.device_name = "Lumen".into();
        }
        if self.device_id.len() < 8 {
            self.device_id = random_device_id();
        }
    }

    /// Load the config, creating it with defaults on first run. A broken file is
    /// backed up to `config.toml.bak` and replaced by defaults (never a crash).
    pub fn load(paths: &Paths) -> Loaded {
        let file = paths.config_file();
        match fs::read_to_string(&file) {
            Ok(text) => match Self::from_toml(&text) {
                Ok(config) => {
                    // Persist generated fields (device id) on first load.
                    if let Err(e) = config.save(paths) {
                        tracing::warn!("could not save config: {e:#}");
                    }
                    Loaded {
                        config,
                        warning: None,
                    }
                }
                Err(e) => {
                    let backup = file.with_extension("toml.bak");
                    let _ = fs::copy(&file, &backup);
                    let mut config = Config::default();
                    config.sanitize();
                    let _ = config.save(paths);
                    Loaded {
                        config,
                        warning: Some(format!(
                            "config.toml could not be parsed ({e:#}); defaults were used and the old file was saved as {}",
                            backup.display()
                        )),
                    }
                }
            },
            Err(_) => {
                let mut config = Config::default();
                config.sanitize();
                let warning = config
                    .save(paths)
                    .err()
                    .map(|e| format!("could not write config: {e:#}"));
                Loaded { config, warning }
            }
        }
    }

    /// Atomically write the config file.
    pub fn save(&self, paths: &Paths) -> Result<()> {
        fs::create_dir_all(&paths.config_dir)?;
        let text = self.to_toml()?;
        let file = paths.config_file();
        let tmp = file.with_extension("toml.tmp");
        fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &file).with_context(|| format!("replacing {}", file.display()))?;
        Ok(())
    }
}

fn random_device_id() -> String {
    use rand::Rng as _;
    let mut rng = rand::rng();
    (0..40)
        .map(|_| char::from_digit(rng.random_range(0..16), 16).unwrap_or('0'))
        .collect()
}

/// Parse `#rrggbb` (or `rrggbb`) into RGB bytes.
pub fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

pub fn to_hex_color(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// A parsed keyboard shortcut, independent of the UI toolkit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutSpec {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Key name in egui's naming (`Space`, `ArrowRight`, `L`, `F12`, ...).
    pub key: String,
}

/// Parse `Ctrl+Shift+Right` style shortcut strings.
pub fn parse_shortcut(s: &str) -> Option<ShortcutSpec> {
    let mut spec = ShortcutSpec {
        ctrl: false,
        shift: false,
        alt: false,
        key: String::new(),
    };
    let parts: Vec<&str> = s.split('+').map(str::trim).collect();
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    let (key, mods) = parts.split_last()?;
    for m in mods {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmd" | "command" | "super" => spec.ctrl = true,
            "shift" => spec.shift = true,
            "alt" | "option" => spec.alt = true,
            _ => return None,
        }
    }
    let k = key.to_string();
    let lower = k.to_ascii_lowercase();
    spec.key = match lower.as_str() {
        "right" | "arrowright" => "ArrowRight".into(),
        "left" | "arrowleft" => "ArrowLeft".into(),
        "up" | "arrowup" => "ArrowUp".into(),
        "down" | "arrowdown" => "ArrowDown".into(),
        "space" | "spacebar" => "Space".into(),
        "esc" | "escape" => "Escape".into(),
        "enter" | "return" => "Enter".into(),
        "tab" => "Tab".into(),
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "delete" | "del" => "Delete".into(),
        "backspace" => "Backspace".into(),
        _ if lower.len() == 1 && lower.chars().all(|c| c.is_ascii_alphanumeric()) => {
            k.to_ascii_uppercase()
        }
        _ if lower.starts_with('f')
            && lower.len() <= 3
            && lower[1..]
                .parse::<u8>()
                .is_ok_and(|n| (1..=24).contains(&n)) =>
        {
            k.to_ascii_uppercase()
        }
        _ => return None,
    };
    Some(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip_through_toml() {
        let mut cfg = Config::default();
        cfg.sanitize();
        let text = cfg.to_toml().unwrap();
        let back = Config::from_toml(&text).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn missing_keys_get_defaults_and_unknown_keys_are_ignored() {
        let cfg =
            Config::from_toml("client_id = 'abc'\nunknown = 3\n[audio]\nbitrate = 160\n").unwrap();
        assert_eq!(cfg.client_id, "abc");
        assert_eq!(cfg.audio.bitrate, 160);
        assert!(cfg.audio.gapless);
        assert_eq!(cfg.cache.audio_cache_mb, 2048);
        assert_eq!(cfg.shortcuts.play_pause, "Space");
        assert_eq!(cfg.device_id.len(), 40);
        assert!(cfg.local.scan_on_start);
        assert!(cfg.local.folders.is_empty());
    }

    #[test]
    fn sanitize_clamps_values() {
        let cfg = Config::from_toml(
            "[audio]\nbitrate = 1000\nvolume = 250\n[ui]\naccent = 'nope'\nzoom = 99.0\n[cache]\naudio_cache_mb = 1\n",
        )
        .unwrap();
        assert_eq!(cfg.audio.bitrate, 320);
        assert_eq!(cfg.audio.volume, 100);
        assert_eq!(cfg.ui.accent, "#1ed760");
        assert_eq!(cfg.ui.zoom, 3.0);
        assert_eq!(cfg.cache.audio_cache_mb, 64);
    }

    #[test]
    fn bitrate_snaps_to_supported_values() {
        for (input, want) in [(96, 96), (100, 96), (160, 160), (200, 160), (320, 320)] {
            let a = AudioConfig {
                bitrate: input,
                ..Default::default()
            };
            assert_eq!(a.normalized_bitrate(), want);
        }
    }

    #[test]
    fn invalid_toml_is_an_error() {
        assert!(Config::from_toml("client_id = ").is_err());
        assert!(Config::from_toml("[audio]\nbitrate = 'loud'").is_err());
    }

    #[test]
    fn load_and_save_in_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        paths.create_all().unwrap();
        let loaded = Config::load(&paths);
        assert!(loaded.warning.is_none());
        assert!(paths.config_file().exists());
        let mut cfg = loaded.config;
        cfg.audio.volume = 42;
        cfg.save(&paths).unwrap();
        assert_eq!(Config::load(&paths).config.audio.volume, 42);

        // Corrupt file → defaults + backup + warning, never a panic.
        fs::write(paths.config_file(), "[[[").unwrap();
        let loaded = Config::load(&paths);
        assert!(loaded.warning.is_some());
        assert!(paths.config_file().with_extension("toml.bak").exists());
    }

    #[test]
    fn hex_colors() {
        assert_eq!(parse_hex_color("#1ed760"), Some([0x1e, 0xd7, 0x60]));
        assert_eq!(parse_hex_color("FF0000"), Some([255, 0, 0]));
        assert_eq!(parse_hex_color("#12345"), None);
        assert_eq!(parse_hex_color("#gg0000"), None);
        assert_eq!(to_hex_color([1, 2, 255]), "#0102ff");
    }

    #[test]
    fn shortcut_parsing() {
        let s = parse_shortcut("Ctrl+Right").unwrap();
        assert!(s.ctrl && !s.shift && !s.alt);
        assert_eq!(s.key, "ArrowRight");
        assert_eq!(parse_shortcut("Space").unwrap().key, "Space");
        assert_eq!(parse_shortcut("ctrl+l").unwrap().key, "L");
        assert_eq!(parse_shortcut("F12").unwrap().key, "F12");
        let s = parse_shortcut("Ctrl + Shift + K").unwrap();
        assert!(s.ctrl && s.shift);
        assert!(parse_shortcut("Hyper+K").is_none());
        assert!(parse_shortcut("Ctrl+").is_none());
        assert!(parse_shortcut("F99").is_none());
        assert!(parse_shortcut("").is_none());
    }
}
