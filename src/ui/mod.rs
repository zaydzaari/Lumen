//! egui front-end: application state, event handling, layout and repaint policy.

pub mod art;
mod bidi;
mod dialogs;
mod free_mode;
mod fullscreen;
mod login;
mod mini;
mod panels;
mod player_bar;
mod profile_view;
mod settings;
mod sidebar;
mod stats_view;
pub mod theme;
mod themes_view;
mod topbar;
pub(crate) mod verification;
mod views;
mod widgets;

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{KeyboardShortcut, Modifiers, ViewportCommand};
use lru::LruCache;

use crate::backend::{self, Backend, Shared};
use crate::cache::DiskCache;
use crate::config::{parse_hex_color, parse_shortcut, Config};
use crate::events::{AppEvent, ListKey, ToastKind, UiTx};
use crate::lyrics::{self, Lyrics};
use crate::media_controls::MediaControls;
use crate::model::{Collection, CollectionKind, Page, SearchResults, Track, TrackRef};
use crate::player::{PlayerCmd, Snapshot};
use crate::plugins::{EventTracker, PluginAction, PluginHost, ThemeOverride, TrackInfo};
use crate::tray::Tray;

use self::art::ArtCache;
use self::theme::Palette;

pub const SEARCH_ID: &str = "lumen-search-box";
const LIST_CACHE: usize = 12;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Login,
    Connecting,
    Main,
}

#[derive(Clone, Debug, PartialEq)]
pub enum View {
    Home,
    Search,
    Liked,
    Playlists,
    Albums,
    Artists,
    Collection(Collection),
    Settings,
    Local,
    Lyrics,
    /// Theme marketplace (Spicetify colour schemes).
    Themes,
    /// A user's profile page (username).
    Profile(String),
    /// Your listening stats.
    Stats,
}

/// Look up a scheme of an installed theme.
pub fn find_scheme<'a>(
    themes: &'a [crate::themes::InstalledTheme],
    id: &str,
    scheme: &str,
) -> Option<&'a crate::themes::ColorScheme> {
    if id.is_empty() {
        return None;
    }
    let t = themes.iter().find(|t| t.id == id)?;
    t.schemes
        .iter()
        .find(|s| s.name == scheme)
        .or_else(|| t.schemes.first())
}

/// Spotify's popover on a saved song's ✓: tick Liked Songs and your playlists.
pub struct SavePop {
    /// The button it belongs to (one per place the song is shown).
    pub id: egui::Id,
    pub track: TrackRef,
    pub liked: bool,
    pub liked_before: bool,
    /// Playlists ticked now / when opened (only ones known to hold the song).
    pub checked: HashSet<String>,
    pub checked_before: HashSet<String>,
    pub query: String,
}

/// How the open list is sorted (Spotify's sort menu on playlist pages).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListSort {
    #[default]
    Custom,
    Title,
    Artist,
    Album,
    Duration,
}

impl ListSort {
    pub fn label(self) -> &'static str {
        match self {
            ListSort::Custom => "Custom order",
            ListSort::Title => "Title",
            ListSort::Artist => "Artist",
            ListSort::Album => "Album",
            ListSort::Duration => "Duration",
        }
    }
}

/// "Find in playlist" and sort state for the open list page.
#[derive(Default)]
pub struct ListFind {
    /// Context key of the page this belongs to; reset when another page opens.
    pub ctx: String,
    pub open: bool,
    /// Focus the box as soon as it has grown in.
    pub focus: bool,
    pub query: String,
    pub sort: ListSort,
    pub desc: bool,
    /// (signature, row → list index) so the order isn't rebuilt every frame.
    pub cache: Option<(u64, Vec<usize>)>,
}

impl ListFind {
    pub fn active(&self) -> bool {
        self.sort != ListSort::Custom || !self.query.trim().is_empty()
    }
}

/// Rows to show for `tracks` under a filter and sort (indices into `tracks`).
pub fn find_order(tracks: &[TrackRef], query: &str, sort: ListSort, desc: bool) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let mut idx: Vec<usize> = (0..tracks.len())
        .filter(|&i| {
            if q.is_empty() {
                return true;
            }
            let t = &tracks[i];
            t.name.to_lowercase().contains(&q)
                || t.artists.to_lowercase().contains(&q)
                || t.album.to_lowercase().contains(&q)
        })
        .collect();
    let key = |i: &usize| -> String {
        let t = &tracks[*i];
        match sort {
            ListSort::Title => t.name.to_lowercase(),
            ListSort::Artist => t.artists.to_lowercase(),
            ListSort::Album => t.album.to_lowercase(),
            _ => String::new(),
        }
    };
    match sort {
        ListSort::Custom => {}
        ListSort::Duration => idx.sort_by_key(|&i| tracks[i].duration_ms),
        _ => idx.sort_by_cached_key(key),
    }
    if desc && sort != ListSort::Custom {
        idx.reverse();
    }
    idx
}

/// What `session.json` holds: the song to reopen on the next launch.
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedSession {
    name: String,
    key: Option<String>,
    tracks: Vec<crate::model::Track>,
    start: usize,
    position_ms: u32,
}

/// Plausible fake listening history for `--demo` screenshots.
fn demo_history(tracks: &[TrackRef], now: i64) -> Vec<crate::history::Play> {
    let mut out = Vec::new();
    if tracks.is_empty() {
        return out;
    }
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for day in 0..120i64 {
        let n = 6 + (rnd() % 30) as i64;
        for _ in 0..n {
            // Skewed choice so some songs/artists clearly dominate.
            let r = (rnd() % 1000) as f64 / 1000.0;
            let idx = ((r * r * r) * 80.0) as usize % tracks.len();
            let t = &tracks[idx];
            let hour = [8, 9, 13, 17, 18, 19, 20, 21, 22, 23][(rnd() % 10) as usize];
            out.push(crate::history::Play {
                ts: now
                    - day * 86_400
                    - ((now / 3600) % 24 - hour).rem_euclid(24) * 3600
                    - (rnd() % 3600) as i64,
                ms: t.duration_ms.saturating_sub((rnd() % 40_000) as u32),
                uri: t.uri.clone(),
                name: t.name.clone(),
                artists: t.artists.clone(),
                artist_uri: t.artist_uri.clone(),
                album: t.album.clone(),
                album_uri: t.album_uri.clone(),
                image: t.image_large.clone(),
                duration_ms: t.duration_ms,
            });
        }
    }
    out
}

fn startup_destination(online: bool) -> (Screen, View) {
    if online {
        (Screen::Login, View::Home)
    } else {
        (Screen::Main, View::Local)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RightPanel {
    None,
    Queue,
    Lyrics,
    NowPlaying,
    Friends,
}

/// A list of tracks being shown (liked songs, playlist, album, artist top tracks).
pub struct TrackList {
    pub title: String,
    pub subtitle: String,
    pub image: Option<String>,
    pub tracks: Vec<TrackRef>,
    pub total: u32,
    pub next: Option<u32>,
    pub loading: bool,
    pub error: Option<String>,
    /// Metadata chunks (of 50 rows) already requested for stub tracks.
    pub meta_requested: HashSet<usize>,
    pub collection: Option<Collection>,
    /// Shown from the disk cache; a fresh first page is on its way.
    pub revalidating: bool,
}

impl TrackList {
    fn new(title: String) -> Self {
        Self {
            title,
            subtitle: String::new(),
            image: None,
            tracks: Vec::new(),
            total: 0,
            next: Some(0),
            loading: false,
            error: None,
            meta_requested: HashSet::new(),
            collection: None,
            revalidating: false,
        }
    }
}

/// A track list as saved on disk (so lists open instantly next time).
#[derive(serde::Serialize, serde::Deserialize)]
struct CachedList {
    title: String,
    subtitle: String,
    image: Option<String>,
    total: u32,
    complete: bool,
    tracks: Vec<Track>,
    collection: Option<Collection>,
}

impl TrackList {
    fn to_cache(&self) -> CachedList {
        CachedList {
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            image: self.image.clone(),
            total: self.total,
            complete: self.next.is_none(),
            tracks: self.tracks.iter().map(|t| (**t).clone()).collect(),
            collection: self.collection.clone(),
        }
    }

    fn from_cache(c: CachedList) -> Self {
        let n = c.tracks.len();
        let mut l = TrackList::new(c.title);
        l.subtitle = c.subtitle;
        l.image = c.image;
        l.total = c.total.max(n as u32);
        l.tracks = c.tracks.into_iter().map(Arc::new).collect();
        l.next = (!c.complete).then_some(n as u32);
        l.collection = c.collection;
        l.revalidating = true;
        l
    }
}

/// Paginated collection list (playlists / albums / artists).
pub struct Paged<T> {
    pub items: Vec<T>,
    pub total: u32,
    pub next: Option<u32>,
    pub cursor: Option<String>,
    pub loading: bool,
    pub started: bool,
    pub error: Option<String>,
    /// Items came from the disk cache and get replaced by the first fresh page.
    pub cached: bool,
}

impl<T> Default for Paged<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            total: 0,
            next: None,
            cursor: None,
            loading: false,
            started: false,
            error: None,
            cached: false,
        }
    }
}

#[derive(Default)]
pub struct HomeState {
    pub requested: bool,
    pub loaded: bool,
    pub artists: Vec<Collection>,
    /// From local listening history.
    pub mix: crate::history::HomeMix,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct MarketState {
    pub items: Vec<crate::themes::MarketTheme>,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub query: String,
    /// Ids being installed right now.
    pub installing: HashSet<String>,
    /// Theme whose details (schemes) are open in the browser.
    pub open: Option<String>,
}

#[derive(Default)]
pub struct SearchState {
    pub query: String,
    pub edited_at: Option<Instant>,
    pub sent: String,
    pub results: Option<SearchResults>,
    pub loading: bool,
    pub error: Option<String>,
}

pub enum LyricsState {
    Idle,
    Loading,
    Ready(Lyrics),
    Error(String),
}

/// Something being dragged with the mouse.
#[derive(Clone)]
pub enum DragItem {
    /// A library row (to reorder Your Library).
    Library(Collection),
    /// Songs, optionally from a playlist position (to reorder that playlist).
    Tracks {
        tracks: Vec<TrackRef>,
        from: Option<(ListKey, usize)>,
    },
}

/// Time spent on the current song, counted while it actually plays.
struct Listening {
    track: TrackRef,
    ms: u64,
    playing: bool,
    since: Instant,
}

/// State of the "Edit details" dialog for a playlist.
pub struct EditPlaylist {
    pub uri: String,
    pub name: String,
    pub description: String,
    /// Only send the description if it was touched (Lumen may not know the
    /// current one, and an untouched empty box must not erase it).
    pub description_dirty: bool,
    pub image: Option<String>,
    pub saving: bool,
}

pub struct LyricsView {
    pub uri: Option<String>,
    pub state: LyricsState,
    pub last_line: Option<usize>,
    /// Who provided the lyrics ("Musixmatch", "LRCLIB", …).
    pub source: String,
}

pub struct LoginState {
    pub client_id: String,
    pub status: String,
    pub url: Option<String>,
    pub error: Option<String>,
    pub busy: bool,
}

pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    pub until: Instant,
    pub born: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    PlayPause,
    Next,
    Previous,
    Search,
    Quit,
    Debug,
    VolumeUp,
    VolumeDown,
}

#[derive(Default)]
struct DebugStats {
    on: bool,
    frames: u32,
    window_start: Option<Instant>,
    fps: f32,
    rss: Option<usize>,
    sampled: Option<Instant>,
}

pub struct App {
    pub forward: Vec<View>,
    pub artist_info:
        std::collections::HashMap<String, Option<Result<crate::model::ArtistInfo, String>>>,
    /// Artists you follow (from the library and your own follow clicks).
    pub followed: HashSet<String>,
    /// Full artist bio dialog (name, text).
    pub about_open: Option<(String, String)>,
    /// "View credits" dialog: the song and its credits (None while loading).
    pub credits: Option<(TrackRef, Option<Result<crate::model::Credits, String>>)>,
    /// The open "Add to playlist" popover of a saved song's ✓ button.
    pub save_pop: Option<SavePop>,
    /// The keyboard shortcuts overview (Ctrl+/).
    pub shortcuts_open: bool,
    /// Friend Activity and when it was last fetched.
    pub friends: Option<Result<Vec<crate::model::Friend>, String>>,
    pub friends_at: Option<Instant>,
    pub album_info:
        std::collections::HashMap<String, Option<Result<crate::model::AlbumInfo, String>>>,
    /// "Recommended" songs under your own playlists (None while loading).
    pub recommended: std::collections::HashMap<String, Option<Result<Vec<TrackRef>, String>>>,
    /// Credits per song for the Now playing panel (None while loading).
    pub track_credits:
        std::collections::HashMap<String, Option<Result<crate::model::Credits, String>>>,
    /// Spotify Connect devices (from cluster updates).
    pub devices: Vec<crate::model::Device>,
    /// List whose rows are being drawn (for drag and drop).
    pub row_source: Option<ListKey>,
    /// Seed song autoplay was last requested for (once per song).
    autoplay_seed: Option<String>,
    /// Collection whose songs get queued once loaded (library menu).
    pending_queue: Option<ListKey>,
    /// The song being listened to right now (for the history).
    listening: Option<Listening>,
    pub stats: Option<crate::history::Stats>,
    pub stats_range: crate::history::Range,
    pub stats_loading: bool,
    pub edit_playlist: Option<EditPlaylist>,
    /// Owned playlist whose page is being drawn (enables "Remove from this playlist").
    pub row_playlist: Option<String>,
    /// Playlist pending delete confirmation.
    pub confirm_delete: Option<Collection>,
    /// Loaded user profiles by username (`None` while loading).
    pub profiles: std::collections::HashMap<String, Option<Result<crate::model::Profile, String>>>,
    /// Full-screen now playing view, and when it was opened (fade-in).
    pub fullscreen: bool,
    pub fullscreen_at: Option<Instant>,
    /// Mini player mode and the window size to go back to.
    pub mini: bool,
    mini_restore: Option<egui::Vec2>,
    /// Private session: no listening history, no Discord status, no recent searches.
    pub private_session: bool,
    /// Selected song row on the open page (click or arrow keys; Enter plays it).
    pub row_sel: Option<usize>,
    /// Set while the current page's rows are drawn, so only they show the selection.
    pub row_sel_page: bool,
    session_restored: bool,
    pub list_find: ListFind,
    /// Selected song rows (list indices), for Ctrl/Shift multi-select.
    pub row_multi: std::collections::BTreeSet<usize>,
    /// List length when the selection was made (a changed list drops it).
    pub row_multi_len: usize,
    /// Set by a row click (display row, modifiers); handled by the page.
    pub row_click: Option<(usize, egui::Modifiers)>,
    /// Whether the row being drawn is selected.
    pub row_hl: bool,
    /// The selected songs while more than one is selected (drag / menu use them).
    pub multi_tracks: Vec<TrackRef>,
    /// Installed colour themes.
    pub themes: Vec<crate::themes::InstalledTheme>,
    pub market: MarketState,
    pub market_tab: themes_view::MarketTab,
    /// Theme to preview while the pointer is over it (set each frame by the UI).
    pub theme_hover: Option<(String, String)>,
    theme_hover_active: Option<(String, String)>,
    /// Palette being animated towards, and where the animation started.
    palette_target: Palette,
    theme_anim: Option<(Palette, Instant)>,
    /// Library forced to the artwork rail because the window is narrow.
    pub auto_compact: bool,
    /// When the main view last changed (drives a short fade-in).
    nav_at: Option<std::time::Instant>,
    /// Track URIs known to be in Liked Songs (from the Liked list, save/remove
    /// actions and per-track checks of the playing song).
    pub liked: std::collections::HashSet<String>,
    /// URIs whose liked-state was already asked for (avoid repeat requests).
    liked_checked: std::collections::HashSet<String>,
    pub avatar: Option<String>,
    pub user_id: Option<String>,
    pub settings_scroll_to_discord: bool,
    pub settings_scroll_to: Option<String>,
    pub settings_query: String,
    pub library_filter: sidebar::LibraryFilter,
    pub library_query: String,
    pub library_search_open: bool,
    pub library_focus: bool,
    pub search_tab: views::SearchTab,
    /// Play this list as soon as its tracks arrive (card / shortcut play buttons).
    pub autoplay: Option<ListKey>,
    pub home_hover_color: Option<egui::Color32>,
    pub local_filter_changed: bool,
    pub discord: crate::discord::Discord,
    trimmer: crate::memory::Trimmer,
    demo: bool,
    pub player: Option<crate::player::PlayerHandle>,
    pub local_tracks: Vec<TrackRef>,
    pub download_tracks: Vec<TrackRef>,
    pub free_library: Vec<crate::free_library::Playlist>,
    pub free_import_path: String,
    pub free_importing: bool,
    pub downloader: Option<crate::downloads::Downloader>,
    pub download_status: String,
    pub download_token_input: String,
    pub download_token_saved: bool,
    pub download_connection: Arc<crate::verification::Connection>,
    pub download_verification: Option<verification::Panel>,
    #[cfg(windows)]
    pub native_hwnd: Option<isize>,
    pub local_filtered: Vec<TrackRef>,
    pub local_filter: String,
    pub local_folder_input: String,
    pub local_scanning: bool,
    online: Option<bool>,
    retry_login_at: Instant,
    pub shared: Shared,
    pub config: Config,
    rx: Receiver<AppEvent>,
    pub ui_tx: UiTx,
    pub screen: Screen,
    pub login: LoginState,
    pub backend: Option<Backend>,
    pub snapshot: Arc<Snapshot>,
    pub view: View,
    pub back: Vec<View>,
    pub right: RightPanel,
    /// Right panel last shown (drawn while it slides closed).
    last_right: RightPanel,
    pub playlists: Paged<Collection>,
    pub albums: Paged<Collection>,
    pub artists: Paged<Collection>,
    pub lists: LruCache<ListKey, TrackList>,
    pub artist_albums: LruCache<String, Vec<crate::model::Release>>,
    pub home: HomeState,
    pub search: SearchState,
    pub lyrics: LyricsView,
    pub lyrics_cache: Arc<DiskCache>,
    /// Track lists and the library saved on disk for instant opening.
    list_disk: Arc<DiskCache>,
    dirty_lists: HashSet<ListKey>,
    library_dirty: bool,
    last_list_save: Instant,
    pub art: ArtCache,
    pub toasts: Vec<Toast>,
    pub plugins: PluginHost,
    events: EventTracker,
    theme_override: ThemeOverride,
    pub palette: Palette,
    shortcuts: Vec<(Action, KeyboardShortcut)>,
    media: Option<MediaControls>,
    tray: Option<Tray>,
    debug: DebugStats,
    pub cache_sizes: Option<(u64, u64, u64)>,
    pub seek_drag: Option<f32>,
    pub volume_before_mute: u8,
    pub username: String,
    quitting: bool,
    pub connect_enabled: bool,
}

impl App {
    pub fn connect_library(&mut self) {
        if self.login.busy {
            return;
        }
        let Some(backend) = &self.backend else {
            self.screen = Screen::Login;
            return;
        };
        let session = backend.session.clone();
        let (shared, cfg, ui) = (self.shared.clone(), self.config.clone(), self.ui_tx.clone());
        self.login.busy = true;
        self.login.error = None;
        self.login.url = None;
        self.shared.rt.spawn(async move {
            let result = backend::authorize_library(shared, cfg, session, ui.clone())
                .await
                .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::LibraryAuthorized(result));
        });
    }

    pub fn refresh_library(&mut self) {
        self.playlists = Paged::default();
        self.albums = Paged::default();
        self.artists = Paged::default();
        self.home = HomeState::default();
        self.lists.clear();
        self.artist_albums.clear();
        self.load_playlists();
        self.on_view_opened();
    }
    fn probe_network(&self) {
        let http = self.shared.http.clone();
        let ui = self.ui_tx.clone();
        self.shared.rt.spawn(async move {
            let online = http
                .head("https://accounts.spotify.com")
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .is_ok();
            ui.send(AppEvent::Network(online));
        });
    }

    fn load_local(&mut self) {
        if self.config.local.scan_on_start {
            self.rescan_local();
            return;
        }
        let file = self.shared.paths.cache_dir.join("local-index.json");
        let folders = self.config.local.folders.clone();
        let ui = self.ui_tx.clone();
        self.shared
            .rt
            .spawn_blocking(move || match crate::local::Index::load(&file) {
                Ok(mut index) => {
                    index.retain_folders(&folders);
                    ui.send(AppEvent::LocalReady(index.tracks()));
                }
                Err(e) if file.exists() => {
                    ui.toast(ToastKind::Error, format!("Local index: {e:#}"))
                }
                Err(_) => {}
            });
    }

    pub fn rescan_local(&mut self) {
        if self.local_scanning {
            return;
        }
        self.local_scanning = true;
        let file = self.shared.paths.cache_dir.join("local-index.json");
        let folders = self.config.local.folders.clone();
        let ui = self.ui_tx.clone();
        self.shared.rt.spawn_blocking(move || {
            let result = (|| -> anyhow::Result<crate::local::Scan> {
                let previous = match crate::local::Index::load(&file) {
                    Ok(index) => index,
                    Err(e) => {
                        if file.exists() {
                            ui.toast(ToastKind::Error, format!("Rebuilding local index: {e:#}"));
                        }
                        crate::local::Index::default()
                    }
                };
                let scan = crate::local::scan(&folders, previous);
                scan.index.save(&file)?;
                Ok(scan)
            })();
            ui.send(AppEvent::LocalScan(result.map_err(|e| format!("{e:#}"))));
        });
    }

    pub fn filter_local(&mut self) {
        let filter = self.local_filter.to_lowercase();
        self.local_filtered = self
            .local_tracks
            .iter()
            .chain(self.download_tracks.iter())
            .filter(|t| {
                filter.is_empty()
                    || t.name.to_lowercase().contains(&filter)
                    || t.artists.to_lowercase().contains(&filter)
                    || t.album.to_lowercase().contains(&filter)
            })
            .cloned()
            .collect();
    }

    pub fn show_local(&mut self) {
        self.screen = Screen::Main;
        self.navigate(View::Local);
    }

    pub fn new(
        cc: &eframe::CreationContext<'_>,
        shared: Shared,
        config: Config,
        warning: Option<String>,
        rx: Receiver<AppEvent>,
        tx: std::sync::mpsc::Sender<AppEvent>,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        ctx.set_zoom_factor(config.ui.zoom);
        let ui_tx = UiTx::new(tx, ctx.clone());
        let paths = shared.paths.clone();

        let art_disk = DiskCache::new(
            paths.art_cache_dir(),
            config.cache.art_cache_mb * 1024 * 1024,
        )
        .or_else(|_| DiskCache::new(std::env::temp_dir().join("lumen-art"), 16 << 20));
        let lyrics_disk = DiskCache::new(
            paths.lyrics_cache_dir(),
            config.cache.lyrics_cache_mb * 1024 * 1024,
        )
        .or_else(|_| DiskCache::new(std::env::temp_dir().join("lumen-lyrics"), 4 << 20));
        let (art_disk, lyrics_disk) = match (art_disk, lyrics_disk) {
            (Ok(a), Ok(l)) => (Arc::new(a), Arc::new(l)),
            // Both fallbacks failing means no writable temp dir; use a dummy location.
            (a, l) => {
                let dummy = || {
                    DiskCache::new(std::env::temp_dir(), 1 << 20)
                        .map(Arc::new)
                        .unwrap_or_else(|_| {
                            Arc::new(DiskCache::new(".", 1 << 20).unwrap_or_else(|e| {
                                tracing::error!("no writable cache dir: {e}");
                                std::process::exit(1)
                            }))
                        })
                };
                (
                    a.map(Arc::new).unwrap_or_else(|_| dummy()),
                    l.map(Arc::new).unwrap_or_else(|_| dummy()),
                )
            }
        };

        let art = ArtCache::new(
            config.cache.art_memory_items,
            art_disk,
            shared.http.clone(),
            shared.rt.clone(),
            ui_tx.clone(),
        );

        // Plugins (example plugins installed on first run).
        let mut config = config;
        let newly_disabled = crate::plugins::install_examples(
            &paths.plugins_dir(),
            &paths.config_dir.join(".examples-installed"),
        );
        if !newly_disabled.is_empty() {
            for n in newly_disabled {
                if !config.plugins.disabled.contains(&n) {
                    config.plugins.disabled.push(n);
                }
            }
            let _ = config.save(&paths);
        }
        let plugins = PluginHost::load_dir(
            &paths.plugins_dir(),
            &paths.plugin_data_root(),
            &config.plugins.disabled,
        );

        widgets::install_fonts(&ctx);
        let theme_override = ThemeOverride::default();
        let shared_paths_themes = paths.themes_dir();
        let installed = crate::themes::load_installed(&shared_paths_themes);
        let palette = theme::palette(
            &config.ui,
            &theme_override,
            find_scheme(&installed, &config.ui.color_theme, &config.ui.color_scheme),
        );
        theme::apply(&ctx, &palette);

        // OS integrations.
        let hwnd = window_handle(cc);
        let media = MediaControls::new(ui_tx.clone(), hwnd);
        let tray = if config.ui.tray {
            Tray::new(
                ui_tx.clone(),
                parse_hex_color(&config.ui.accent).unwrap_or([30, 215, 96]),
            )
        } else {
            None
        };

        let discord = crate::discord::Discord::start(config.discord.clone());
        let mut app = Self {
            forward: Vec::new(),
            artist_info: Default::default(),
            followed: HashSet::new(),
            about_open: None,
            credits: None,
            save_pop: None,
            shortcuts_open: false,
            friends: None,
            friends_at: None,
            album_info: Default::default(),
            recommended: Default::default(),
            track_credits: Default::default(),
            devices: Vec::new(),
            row_source: None,
            autoplay_seed: None,
            pending_queue: None,
            listening: None,
            stats: None,
            stats_range: Default::default(),
            stats_loading: false,
            edit_playlist: None,
            row_playlist: None,
            confirm_delete: None,
            profiles: Default::default(),
            fullscreen: false,
            fullscreen_at: None,
            mini: false,
            mini_restore: None,
            private_session: false,
            row_sel: None,
            row_sel_page: false,
            session_restored: false,
            list_find: ListFind::default(),
            row_multi: Default::default(),
            row_multi_len: 0,
            row_click: None,
            row_hl: false,
            multi_tracks: Vec::new(),
            themes: installed,
            market: MarketState::default(),
            market_tab: Default::default(),
            theme_hover: None,
            theme_hover_active: None,
            palette_target: palette,
            theme_anim: None,
            auto_compact: false,
            nav_at: None,
            liked: Default::default(),
            liked_checked: Default::default(),
            avatar: None,
            user_id: None,
            settings_scroll_to_discord: false,
            settings_scroll_to: None,
            settings_query: String::new(),
            library_filter: sidebar::LibraryFilter::default(),
            library_query: String::new(),
            library_search_open: false,
            library_focus: false,
            search_tab: views::SearchTab::default(),
            autoplay: None,
            home_hover_color: None,
            local_filter_changed: false,
            discord,
            trimmer: crate::memory::Trimmer::new(Instant::now()),
            demo: false,
            player: None,
            local_tracks: Vec::new(),
            download_tracks: Vec::new(),
            free_library: Vec::new(),
            free_import_path: String::new(),
            free_importing: false,
            downloader: None,
            download_status: "Starting…".into(),
            download_token_input: String::new(),
            download_token_saved: shared.store.load(crate::spotidownloader::KEY).is_some(),
            download_connection: crate::verification::Connection::new(
                config.downloads.browser_connection,
            ),
            download_verification: None,
            #[cfg(windows)]
            native_hwnd: hwnd.map(|h| h as isize),
            local_filtered: Vec::new(),
            local_filter: String::new(),
            local_folder_input: String::new(),
            local_scanning: false,
            online: None,
            retry_login_at: Instant::now() + Duration::from_secs(30),
            login: LoginState {
                client_id: config.client_id.clone(),
                status: String::new(),
                url: None,
                error: None,
                busy: false,
            },
            shared,
            rx,
            ui_tx,
            screen: Screen::Login,
            backend: None,
            snapshot: Arc::new(Snapshot::default()),
            view: View::Home,
            back: Vec::new(),
            right: RightPanel::NowPlaying,
            last_right: RightPanel::NowPlaying,
            playlists: Paged::default(),
            albums: Paged::default(),
            artists: Paged::default(),
            lists: LruCache::new(NonZeroUsize::new(LIST_CACHE).unwrap_or(NonZeroUsize::MIN)),
            artist_albums: LruCache::new(NonZeroUsize::new(8).unwrap_or(NonZeroUsize::MIN)),
            home: HomeState::default(),
            search: SearchState::default(),
            lyrics: LyricsView {
                uri: None,
                state: LyricsState::Idle,
                last_line: None,
                source: String::new(),
            },
            lyrics_cache: lyrics_disk,
            list_disk: Arc::new(
                DiskCache::new(paths.cache_dir.join("lists"), 64 << 20)
                    .or_else(|_| DiskCache::new(std::env::temp_dir().join("lumen-lists"), 16 << 20))
                    .expect("no writable cache directory"),
            ),
            dirty_lists: HashSet::new(),
            library_dirty: false,
            last_list_save: Instant::now(),
            art,
            toasts: Vec::new(),
            plugins,
            events: EventTracker::default(),
            theme_override,
            palette,
            shortcuts: Vec::new(),
            media,
            tray,
            debug: DebugStats::default(),
            cache_sizes: None,
            seek_drag: None,
            volume_before_mute: 50,
            username: String::new(),
            quitting: false,
            connect_enabled: false,
            config,
        };
        app.compile_shortcuts();
        {
            let _entered = app.shared.rt.enter();
            match crate::player::start_offline(
                backend::audio_settings_of(&app.config),
                app.config.audio.volume,
                app.ui_tx.clone(),
            ) {
                Ok(player) => app.player = Some(player),
                Err(e) => app
                    .ui_tx
                    .toast(ToastKind::Error, format!("Playback unavailable: {e:#}")),
            }
        }
        if !std::env::args().any(|a| a == "--demo") {
            match crate::downloads::Downloader::start(
                crate::downloads::folder(&app.config.downloads, &app.shared.paths),
                app.config.downloads.automatic,
                app.ui_tx.clone(),
                app.shared.store.clone(),
                app.shared.rt.clone(),
                app.download_connection.clone(),
            ) {
                Ok(downloader) => {
                    app.send(PlayerCmd::DownloadLibrary(downloader.library.clone()));
                    app.downloader = Some(downloader);
                }
                Err(e) => app.download_status = format!("Downloads unavailable: {e:#}"),
            }
        }
        app.send(PlayerCmd::Independent(app.config.downloads.independent));
        let free_path = app.shared.paths.data_dir.join("free-library.json");
        if free_path.exists() {
            app.free_importing = true;
            let ui = app.ui_tx.clone();
            app.shared.rt.spawn_blocking(move || {
                ui.send(AppEvent::FreeLibraryImported {
                    result: crate::free_library::read(&free_path).map_err(|e| format!("{e:#}")),
                    activate: false,
                });
            });
        }
        if app.config.downloads.independent {
            app.screen = Screen::Main;
            app.load_cached_library();
            app.view = View::Local;
        }
        app.load_local();
        app.probe_network();
        if let Some(w) = warning {
            app.toast(ToastKind::Error, w);
        }
        for info in app.plugins.infos().to_vec() {
            if let Some(e) = info.error {
                app.toast(
                    ToastKind::Error,
                    format!("Plugin {} failed: {e}", info.name),
                );
            }
        }
        if std::env::args().any(|a| a == "--demo") {
            app.load_demo();
        }
        app
    }

    /// `--demo`: show the main UI with generated sample data and no Spotify
    /// connection (used for screenshots and memory measurements of the UI).
    fn load_demo(&mut self) {
        self.demo = true;
        let words = [
            "Midnight",
            "Neon",
            "Echo",
            "Velvet",
            "Static",
            "Golden",
            "Paper",
            "Glass",
            "River",
            "Satellite",
        ];
        let demo_art = std::env::var("LUMEN_DEMO_ART").ok();
        let covers = demo_art
            .as_ref()
            .and_then(|d| std::fs::read_dir(d).ok())
            .map(|rd| rd.count())
            .unwrap_or(0)
            .max(1);
        let art = |i: usize| -> Option<String> {
            demo_art
                .as_ref()
                .map(|d| format!("file://{d}/cover{}.png", i % covers))
        };
        let mk = |i: usize| -> TrackRef {
            Arc::new(Track {
                uri: format!("spotify:track:demo{i}"),
                name: if i % 17 == 5 {
                    format!("بيضاء مون امور (Live {i})")
                } else {
                    format!("{} {} {}", words[i % 10], words[(i / 10) % 10], i)
                },
                artists: if i % 17 == 5 {
                    "الشاب حسني".to_string()
                } else {
                    format!("Demo Artist {}", i % 37)
                },
                artist_uri: Some(format!("spotify:artist:demo{}", i % 37)),
                album: format!("Demo Album {}", i % 113),
                album_uri: Some(format!("spotify:album:demo{}", i % 113)),
                duration_ms: 150_000 + (i as u32 * 7919) % 120_000,
                image: art(i),
                image_large: art(i),
                explicit: i.is_multiple_of(9),
            })
        };
        let mut liked = TrackList::new("Liked Songs".into());
        liked.tracks = (0..2000).map(mk).collect();
        liked.total = 2000;
        liked.next = None;
        self.liked
            .extend(liked.tracks.iter().map(|t| t.uri.clone()));
        self.lists.put(ListKey::Liked, liked);
        self.playlists.started = true;
        self.playlists.items = (0..60)
            .map(|i| Collection {
                kind: CollectionKind::Playlist,
                uri: format!("spotify:playlist:demo{i}"),
                name: format!("{} mix #{i}", words[i % 10]),
                subtitle: "by Demo User".into(),
                image: art(i + 3),
                total_tracks: 20 + i as u32,
                owned: true,
            })
            .collect();
        self.playlists.total = 60;
        self.home.requested = true;
        self.home.loaded = true;
        self.home.artists = (0..10)
            .map(|i| Collection {
                kind: CollectionKind::Artist,
                uri: format!("spotify:artist:demo{i}"),
                name: format!("Demo Artist {i}"),
                subtitle: "Artist".into(),
                image: art(i + 5),
                total_tracks: 0,
                owned: false,
            })
            .collect();
        let mut top = TrackList::new("Your top tracks".into());
        top.tracks = (40..52).map(mk).collect();
        top.next = None;
        self.lists.put(ListKey::Top, top);
        let mut recent = TrackList::new("Recently played".into());
        recent.tracks = (0..10).map(mk).collect();
        recent.next = None;
        self.lists.put(ListKey::Recent, recent);
        self.artists.started = true;
        self.artists.items = self.home.artists.clone();
        self.artists.total = self.artists.items.len() as u32;
        let albums: Vec<Collection> = (0..12)
            .map(|i| Collection {
                kind: CollectionKind::Album,
                uri: format!("spotify:album:demo{i}"),
                name: format!("Demo Album {i}"),
                subtitle: format!("{} • Demo Artist {}", 2014 + i % 12, i % 37),
                image: art(i + 1),
                total_tracks: 12,
                owned: false,
            })
            .collect();
        self.home.mix.albums = albums.iter().rev().take(8).cloned().collect();
        self.search.query = "midnight".into();
        self.search.sent = "midnight".into();
        self.search.results = Some(crate::model::SearchResults {
            query: "midnight".into(),
            tracks: (0..10).map(|i| mk(i * 10)).collect(),
            albums,
            artists: self.home.artists.clone(),
            playlists: self.playlists.items[..8].to_vec(),
        });
        let current = mk(3);
        self.snapshot = Arc::new(Snapshot {
            track: Some(current.clone()),
            playing: true,
            position_ms: 42_000,
            at: Some(Instant::now()),
            volume: 45_000,
            queue: (100..103).map(mk).collect(),
            upcoming: (4..30).map(mk).collect(),
            upcoming_total: 1996,
            context_name: "Liked Songs".into(),
            ..Default::default()
        });
        let lines = (0..40)
            .map(|i| {
                format!(
                    "[{:02}:{:02}.00]Demo lyric line number {i}",
                    (i * 5) / 60,
                    (i * 5) % 60
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.lyrics = LyricsView {
            uri: Some(current.uri.clone()),
            state: LyricsState::Ready(Lyrics::Synced(lyrics::parse_lrc(&lines))),
            last_line: None,
            source: String::new(),
        };
        self.username = "Demo".into();
        self.user_id = Some("demo".into());
        self.avatar = art(7);
        self.profiles.insert(
            "demo".into(),
            Some(Ok(crate::model::Profile {
                name: "Demo".into(),
                image: art(7),
                followers: 128,
                following: 42,
                total_public_playlists: 12,
                public_playlists: self.playlists.items[..12].to_vec(),
                recent_artists: self.home.artists.clone(),
            })),
        );
        self.screen = Screen::Main;
        self.view = View::Liked;
        if let Some(v) = std::env::args().skip_while(|a| a != "--demo-panel").nth(1) {
            self.right = match v.as_str() {
                "queue" => RightPanel::Queue,
                "lyrics" => RightPanel::Lyrics,
                "now" => RightPanel::NowPlaying,
                "friends" => RightPanel::Friends,
                _ => RightPanel::None,
            };
        }
        if let Some(v) = std::env::args().skip_while(|a| a != "--demo-view").nth(1) {
            self.view = match v.as_str() {
                "home" => View::Home,
                "lyrics" => View::Lyrics,
                "local" => View::Local,
                "albums" => View::Albums,
                "settings" => View::Settings,
                "playlists" => View::Playlists,
                "search" => View::Search,
                "artists" => View::Artists,
                "themes" => View::Themes,
                "profile" => View::Profile("demo".into()),
                "stats" => View::Stats,
                "artist" => {
                    let a = self.home.artists[0].clone();
                    self.artist_info.insert(
                        a.uri.clone(),
                        Some(Ok(crate::model::ArtistInfo {
                            bio: Some("A four-piece from the coast who write long, patient songs about night drives and small towns. Their second record went gold in three countries and they have toured with friends ever since. This text is demo filler.".into()),
                            portrait: a.image.clone(),
                            related: self.home.artists.iter().skip(1).take(8).cloned().collect(),
                            popularity: 74,
                        })),
                    );
                    // Releases and popular songs for the demo artist page.
                    let demo_albums: Vec<Collection> = self
                        .search
                        .results
                        .as_ref()
                        .map(|r| r.albums.clone())
                        .unwrap_or_default();
                    let releases: Vec<crate::model::Release> = demo_albums
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            use crate::model::ReleaseGroup::*;
                            let group = [Album, Single, Single, AppearsOn][i % 4];
                            let mut coll = c.clone();
                            let year = 2024 - i as i32;
                            if group != AppearsOn {
                                coll.subtitle = format!(
                                    "{year} • {}",
                                    if group == Album { "Album" } else { "Single" }
                                );
                            }
                            crate::model::Release {
                                group,
                                year,
                                popularity: (i * 37 % 100) as i32,
                                coll,
                            }
                        })
                        .collect();
                    self.artist_albums.put(a.uri.clone(), releases);
                    let mut top = TrackList::new(a.name.clone());
                    top.tracks = self
                        .lists
                        .peek(&ListKey::Liked)
                        .map(|l| l.tracks.iter().take(10).cloned().collect())
                        .unwrap_or_default();
                    top.next = None;
                    self.lists.put(ListKey::Collection(a.uri.clone()), top);
                    View::Collection(a)
                }
                _ => View::Liked,
            };
        }
        if self.view == View::Themes {
            self.load_market(false);
        }
        if self.view == View::Stats {
            self.load_stats();
        }
        self.fullscreen = std::env::args().any(|a| a == "--demo-fullscreen");
        self.private_session = std::env::args().any(|a| a == "--demo-private");
        if std::env::args().any(|a| a == "--demo-loading") {
            self.home.loaded = false;
        }
        if std::env::args().any(|a| a == "--demo-recent") {
            self.search.query.clear();
            self.config.ui.recent_searches = vec![
                crate::config::RecentSearch::Collection(self.home.artists[2].clone()),
                crate::config::RecentSearch::Track((*mk(7)).clone()),
                crate::config::RecentSearch::Collection(self.playlists.items[4].clone()),
                crate::config::RecentSearch::Collection(self.home.mix.albums[1].clone()),
            ];
        }
        if std::env::args().any(|a| a == "--demo-mini") {
            self.set_mini(true);
        }
        self.debug.on = std::env::args().any(|a| a == "--debug-overlay");
        self.discord
            .update(crate::discord::Presence::from_snapshot(&self.snapshot));
    }

    // ------------------------------------------------------------ helpers

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        let text = text.into();
        if self.toasts.iter().any(|t| t.text == text) {
            return;
        }
        let secs = if kind == ToastKind::Error { 7 } else { 4 };
        self.toasts.push(Toast {
            kind,
            text,
            until: Instant::now() + Duration::from_secs(secs),
            born: Instant::now(),
        });
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    pub fn send(&self, cmd: PlayerCmd) {
        if let Some(player) = &self.player {
            player.send(cmd);
        }
    }

    pub fn save_config(&mut self) {
        self.config.sanitize();
        if let Err(e) = self.config.save(&self.shared.paths) {
            self.toast(ToastKind::Error, format!("Could not save settings: {e:#}"));
        }
    }

    pub fn compile_shortcuts(&mut self) {
        let s = &self.config.shortcuts;
        let entries = [
            (Action::PlayPause, &s.play_pause),
            (Action::Next, &s.next),
            (Action::Previous, &s.previous),
            (Action::Search, &s.search),
            (Action::Quit, &s.quit),
            (Action::Debug, &s.debug_overlay),
            (Action::VolumeUp, &s.volume_up),
            (Action::VolumeDown, &s.volume_down),
        ];
        let mut out = Vec::new();
        let mut bad = Vec::new();
        for (action, text) in entries {
            match parse_shortcut(text).and_then(|spec| {
                let key = egui::Key::from_name(&spec.key)?;
                let mut m = Modifiers::NONE;
                if spec.ctrl {
                    m |= Modifiers::COMMAND;
                }
                if spec.shift {
                    m |= Modifiers::SHIFT;
                }
                if spec.alt {
                    m |= Modifiers::ALT;
                }
                Some(KeyboardShortcut::new(m, key))
            }) {
                Some(sc) => out.push((action, sc)),
                None => bad.push(text.clone()),
            }
        }
        // Longer modifier combos first so Ctrl+X wins over X.
        out.sort_by_key(|(_, sc)| {
            std::cmp::Reverse(
                sc.modifiers.ctrl as u8
                    + sc.modifiers.command as u8
                    + sc.modifiers.shift as u8
                    + sc.modifiers.alt as u8,
            )
        });
        self.shortcuts = out;
        for b in bad {
            self.toast(
                ToastKind::Error,
                format!("Invalid shortcut in config: '{b}'"),
            );
        }
    }

    pub fn navigate(&mut self, v: View) {
        if let (View::Search, View::Collection(c)) = (&self.view, &v) {
            if !self.search.query.trim().is_empty() {
                self.remember_search(crate::config::RecentSearch::Collection(c.clone()));
            }
        }
        if self.view != v {
            let old = std::mem::replace(&mut self.view, v);
            self.nav_at = Some(std::time::Instant::now());
            self.back.push(old);
            if self.back.len() > 50 {
                self.back.remove(0);
            }
            self.forward.clear();
        }
        self.on_view_opened();
    }

    /// Re-open a view without touching history (used by "Reload").
    pub fn navigate_replace(&mut self, v: View) {
        self.view = v;
        self.on_view_opened();
    }

    pub fn go_back(&mut self) {
        if let Some(v) = self.back.pop() {
            let cur = std::mem::replace(&mut self.view, v);
            self.nav_at = Some(std::time::Instant::now());
            self.forward.push(cur);
            if self.forward.len() > 50 {
                self.forward.remove(0);
            }
            self.on_view_opened();
        }
    }

    pub fn go_forward(&mut self) {
        if let Some(v) = self.forward.pop() {
            let cur = std::mem::replace(&mut self.view, v);
            self.nav_at = Some(std::time::Instant::now());
            self.back.push(cur);
            self.on_view_opened();
        }
    }

    /// Start a pending "play this list" once its first tracks are loaded.
    /// Play a playlist/album without opening it (library menu, card button).
    pub fn play_collection(&mut self, c: &Collection) {
        let key = ListKey::Collection(c.uri.clone());
        self.autoplay = Some(key.clone());
        if self.lists.peek(&key).is_none() {
            self.open_list(key, c.name.clone(), Some(c.clone()));
        }
        self.try_autoplay();
    }

    /// Add a whole playlist/album to the queue (loads it first if needed).
    pub fn queue_collection(&mut self, c: &Collection) {
        let key = ListKey::Collection(c.uri.clone());
        self.pending_queue = Some(key.clone());
        if self.lists.peek(&key).is_none() {
            self.open_list(key, c.name.clone(), Some(c.clone()));
        }
        self.try_autoplay();
    }

    /// Open Spotify's radio for a song/artist/album/playlist and start it.
    pub fn start_radio(&mut self, seed_uri: &str, name: &str) {
        if self.demo {
            self.toast(ToastKind::Info, "Radio needs a Spotify login");
            return;
        }
        let Some(b) = &self.backend else { return };
        let (api, ui, seed, name) = (
            b.api.clone(),
            self.ui_tx.clone(),
            seed_uri.to_string(),
            name.to_string(),
        );
        self.toast(ToastKind::Info, format!("Starting {name} Radio…"));
        self.spawn(async move {
            let r = api
                .radio_playlist(&seed)
                .await
                .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::Radio(name, r));
        });
    }

    /// When the last song of the queue is playing, fetch similar songs so the
    /// music doesn't stop (Spotify's Autoplay).
    fn check_autoplay(&mut self) {
        if !self.config.audio.autoplay || self.demo {
            return;
        }
        let s = &self.snapshot;
        if s.mode != crate::player::Mode::Local
            || s.repeat != crate::player::Repeat::Off
            || !s.queue.is_empty()
            || !s.upcoming.is_empty()
            || s.upcoming_total > 0
        {
            return;
        }
        let Some(t) = s
            .track
            .as_ref()
            .filter(|t| t.uri.starts_with("spotify:track:"))
        else {
            return;
        };
        if self.autoplay_seed.as_deref() == Some(t.uri.as_str()) {
            return;
        }
        let Some(b) = &self.backend else { return };
        self.autoplay_seed = Some(t.uri.clone());
        let (api, ui, seed) = (b.api.clone(), self.ui_tx.clone(), t.uri.clone());
        self.spawn(async move {
            let r = api
                .radio_tracks(&seed, 40)
                .await
                .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::Autoplay(seed, r));
        });
    }

    /// Move playback to another Spotify Connect device (or back to Lumen).
    pub fn transfer_to(&mut self, target: &crate::model::Device) {
        let Some(b) = &self.backend else { return };
        let api = b.api.clone();
        let me = api.device_id();
        let active = self.devices.iter().find(|d| d.active).map(|d| d.id.clone());
        let s = self.snapshot.clone();
        let local_track = (s.mode == crate::player::Mode::Local)
            .then(|| s.track.clone())
            .flatten()
            .filter(|t| t.uri.starts_with("spotify:track:"));
        let ui = self.ui_tx.clone();
        let name = target.name.clone();
        let to = target.id.clone();
        if to == me && active.as_deref() == Some(me.as_str()) {
            return;
        }
        let user = self.user_id.clone().unwrap_or_default();
        if let (Some(t), true) = (local_track, to != me) {
            // Lumen plays this itself: start the same song on the other device.
            let context = match s.context_key.as_deref() {
                Some("liked") => format!("spotify:user:{user}:collection"),
                Some(k) if k.starts_with("spotify:") => k.to_string(),
                _ => t.uri.clone(),
            };
            let pos = s.position_now();
            self.send(PlayerCmd::Pause);
            self.spawn(async move {
                match api.play_on_device(&to, &context, &t.uri, pos).await {
                    Ok(()) => ui.toast(ToastKind::Info, format!("Playing on {name}")),
                    Err(e) => ui.toast(
                        ToastKind::Error,
                        format!("Couldn't switch to {name}: {e:#}"),
                    ),
                }
            });
            return;
        }
        let from = active.unwrap_or_else(|| me.clone());
        self.spawn(async move {
            match api.transfer_playback(&from, &to).await {
                Ok(()) => ui.toast(ToastKind::Info, format!("Playing on {name}")),
                Err(e) => ui.toast(
                    ToastKind::Error,
                    format!("Couldn't switch to {name}: {e:#}"),
                ),
            }
        });
    }

    pub fn load_artist_info(&mut self, uri: &str) {
        if self.artist_info.contains_key(uri) || self.demo {
            return;
        }
        let Some(b) = &self.backend else { return };
        if self.artist_info.len() > 40 {
            self.artist_info.clear();
        }
        self.artist_info.insert(uri.to_string(), None);
        let (api, ui, uri) = (b.api.clone(), self.ui_tx.clone(), uri.to_string());
        self.spawn(async move {
            let r = api.artist_info(&uri).await.map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::ArtistInfo(uri, r));
        });
        // Followed artists decide the Follow / Following button.
        if !self.artists.started {
            self.load_artists();
        }
    }

    pub fn is_following(&self, uri: &str) -> bool {
        self.followed.contains(uri) || self.artists.items.iter().any(|a| a.uri == uri)
    }

    pub fn open_save_pop(&mut self, id: egui::Id, t: &TrackRef) {
        let checked: HashSet<String> = self
            .owned_playlists()
            .into_iter()
            .filter(|c| {
                self.lists
                    .peek(&ListKey::Collection(c.uri.clone()))
                    .is_some_and(|l| l.tracks.iter().any(|x| x.uri == t.uri))
            })
            .map(|c| c.uri)
            .collect();
        let liked = self.is_liked(&t.uri);
        self.save_pop = Some(SavePop {
            id,
            track: t.clone(),
            liked,
            liked_before: liked,
            checked_before: checked.clone(),
            checked,
            query: String::new(),
        });
    }

    /// Apply the popover's ticks: like/unlike, add to / remove from playlists.
    pub fn apply_save_pop(&mut self) {
        let Some(p) = self.save_pop.take() else {
            return;
        };
        let uri = p.track.uri.clone();
        if p.liked != p.liked_before {
            self.toggle_liked(&uri);
        }
        let names: std::collections::HashMap<String, String> = self
            .owned_playlists()
            .into_iter()
            .map(|c| (c.uri, c.name))
            .collect();
        for pl in p.checked.difference(&p.checked_before) {
            let name = names.get(pl).cloned().unwrap_or_default();
            self.add_to_playlist(pl, &name, vec![uri.clone()]);
        }
        for pl in p.checked_before.difference(&p.checked) {
            self.remove_from_playlist(pl, std::slice::from_ref(&uri));
        }
    }

    /// Whether a playlist, album or artist is in Your Library.
    pub fn in_library(&self, c: &Collection) -> bool {
        match c.kind {
            CollectionKind::Playlist => self.playlists.items.iter().any(|p| p.uri == c.uri),
            CollectionKind::Album => self.albums.items.iter().any(|p| p.uri == c.uri),
            CollectionKind::Artist => self.is_following(&c.uri),
        }
    }

    /// Add to / remove from Your Library (save album, follow artist or playlist).
    pub fn toggle_in_library(&mut self, c: &Collection) {
        if c.kind == CollectionKind::Artist {
            self.toggle_follow(&c.uri);
            return;
        }
        if c.kind == CollectionKind::Playlist && c.owned {
            // Your own playlist: removing it means deleting, which asks first.
            self.confirm_delete = Some(c.clone());
            return;
        }
        let add = !self.in_library(c);
        self.apply_library_change(c, add);
        if self.demo {
            self.toast(
                ToastKind::Info,
                if add {
                    "Added to Your Library"
                } else {
                    "Removed from Your Library"
                },
            );
            return;
        }
        let Some(b) = &self.backend else { return };
        let (api, ui, c2) = (b.api.clone(), self.ui_tx.clone(), c.clone());
        self.spawn(async move {
            let r = match c2.kind {
                CollectionKind::Album => api.save_album(&c2.uri, add).await,
                CollectionKind::Playlist if add => api.follow_playlist(&c2.uri).await,
                _ => api.delete_playlist(&c2.uri).await,
            };
            ui.send(AppEvent::LibraryChanged(
                c2,
                add,
                r.map_err(|e| format!("{e:#}")),
            ));
        });
    }

    /// Optimistic library update (undone if Spotify refuses).
    fn apply_library_change(&mut self, c: &Collection, add: bool) {
        let list = match c.kind {
            CollectionKind::Playlist => &mut self.playlists,
            CollectionKind::Album => &mut self.albums,
            CollectionKind::Artist => return,
        };
        if add {
            if !list.items.iter().any(|p| p.uri == c.uri) {
                list.items.insert(0, c.clone());
                list.total += 1;
            }
        } else {
            let before = list.items.len();
            list.items.retain(|p| p.uri != c.uri);
            list.total = list
                .total
                .saturating_sub((before - list.items.len()) as u32);
        }
        self.library_dirty = true;
    }

    /// Refresh Friend Activity (at most every two minutes while it's open).
    pub fn load_friends(&mut self, force: bool) {
        if !force
            && self
                .friends_at
                .is_some_and(|t| t.elapsed() < Duration::from_secs(120))
        {
            return;
        }
        self.friends_at = Some(Instant::now());
        if self.demo {
            let now = chrono::Local::now().timestamp_millis();
            let tracks = self
                .lists
                .peek(&ListKey::Liked)
                .map(|l| l.tracks.clone())
                .unwrap_or_default();
            let names = ["Amine", "Sara", "Youssef", "Lina", "Omar"];
            self.friends = Some(Ok(names
                .iter()
                .enumerate()
                .filter_map(|(i, n)| {
                    let t = tracks.get(i * 7)?;
                    Some(crate::model::Friend {
                        name: n.to_string(),
                        user_uri: format!("spotify:user:{n}"),
                        image: self.home.artists.get(i + 2).and_then(|a| a.image.clone()),
                        track_uri: t.uri.clone(),
                        track: t.name.clone(),
                        artist: t.artists.clone(),
                        artist_uri: t.artist_uri.clone(),
                        context: [
                            "Liked Songs",
                            "Midnight mix #0",
                            "Neon mix #1",
                            "Gym",
                            "Chill",
                        ][i]
                            .into(),
                        context_uri: None,
                        ts: now - [0, 3, 25, 140, 60 * 26][i] * 60_000,
                    })
                })
                .collect()));
            return;
        }
        let Some(b) = &self.backend else { return };
        let (api, ui) = (b.api.clone(), self.ui_tx.clone());
        self.spawn(async move {
            let r = api.friend_activity().await.map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::Friends(r));
        });
    }

    pub fn load_album_info(&mut self, uri: &str) {
        if self.album_info.contains_key(uri) || !uri.starts_with("spotify:album:") {
            return;
        }
        if self.demo {
            self.album_info.insert(
                uri.to_string(),
                Some(Ok(crate::model::AlbumInfo {
                    released: "March 3, 2023".into(),
                    label: "Demo Records".into(),
                    copyrights: vec!["© 2023 Demo Records".into(), "℗ 2023 Demo Records".into()],
                    artist_uri: Some(self.home.artists[0].uri.clone()),
                    artist_name: self.home.artists[0].name.clone(),
                })),
            );
            return;
        }
        let Some(b) = &self.backend else { return };
        if self.album_info.len() > 64 {
            self.album_info.clear();
        }
        self.album_info.insert(uri.to_string(), None);
        let (api, ui, uri) = (b.api.clone(), self.ui_tx.clone(), uri.to_string());
        self.spawn(async move {
            let r = api.album_info(&uri).await.map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::AlbumInfo(uri, r));
        });
    }

    /// Songs Spotify suggests for one of your playlists (its radio, minus what's in it).
    pub fn load_recommended(&mut self, uri: &str, refresh: bool) {
        if !refresh && self.recommended.contains_key(uri) {
            return;
        }
        if self.demo {
            let tracks: Vec<TrackRef> = self
                .lists
                .peek(&ListKey::Liked)
                .map(|l| {
                    l.tracks
                        .iter()
                        .skip(if refresh { 120 } else { 100 })
                        .take(10)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            self.recommended.insert(uri.to_string(), Some(Ok(tracks)));
            return;
        }
        let Some(b) = &self.backend else { return };
        self.recommended.insert(uri.to_string(), None);
        let have: HashSet<String> = self
            .lists
            .peek(&ListKey::Collection(uri.to_string()))
            .map(|l| l.tracks.iter().map(|t| t.uri.clone()).collect())
            .unwrap_or_default();
        let (api, ui, uri) = (b.api.clone(), self.ui_tx.clone(), uri.to_string());
        self.spawn(async move {
            let r = api
                .radio_tracks(&uri, 40)
                .await
                .map(|v| {
                    let mut v: Vec<TrackRef> =
                        v.into_iter().filter(|t| !have.contains(&t.uri)).collect();
                    if refresh {
                        use rand::seq::SliceRandom;
                        v.shuffle(&mut rand::rng());
                    }
                    v.truncate(10);
                    v
                })
                .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::Recommended(uri, r));
        });
    }

    /// Load credits for the Now playing panel (once per song).
    pub fn load_track_credits(&mut self, uri: &str) {
        if self.demo || self.track_credits.contains_key(uri) || !uri.starts_with("spotify:track:") {
            return;
        }
        let Some(b) = &self.backend else { return };
        if self.track_credits.len() > 64 {
            self.track_credits.clear();
        }
        self.track_credits.insert(uri.to_string(), None);
        let (api, ui, uri) = (b.api.clone(), self.ui_tx.clone(), uri.to_string());
        self.spawn(async move {
            let r = api.credits(&uri).await.map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::Credits(uri, r));
        });
    }

    pub fn show_credits(&mut self, t: &TrackRef) {
        if self.demo {
            let people = |n: &str, r: &str| crate::model::CreditPerson {
                name: n.into(),
                uri: None,
                roles: r.into(),
            };
            self.credits = Some((
                t.clone(),
                Some(Ok(crate::model::Credits {
                    roles: vec![
                        (
                            "Performed by".into(),
                            vec![people(&t.artists, "Main Artist")],
                        ),
                        (
                            "Written by".into(),
                            vec![people("Demo Writer", "Composer, Lyricist")],
                        ),
                        (
                            "Produced by".into(),
                            vec![people("Demo Producer", "Producer")],
                        ),
                    ],
                    sources: vec!["Demo Records".into()],
                })),
            ));
            return;
        }
        if let Some(Some(r)) = self.track_credits.get(&t.uri) {
            self.credits = Some((t.clone(), Some(r.clone())));
            return;
        }
        let Some(b) = &self.backend else { return };
        self.credits = Some((t.clone(), None));
        let (api, ui, uri) = (b.api.clone(), self.ui_tx.clone(), t.uri.clone());
        self.spawn(async move {
            let r = api.credits(&uri).await.map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::Credits(uri, r));
        });
    }

    pub fn toggle_follow(&mut self, uri: &str) {
        let follow = !self.is_following(uri);
        // Optimistic.
        if follow {
            self.followed.insert(uri.to_string());
        } else {
            self.followed.remove(uri);
            self.artists.items.retain(|a| a.uri != uri);
        }
        if self.demo {
            return;
        }
        let Some(b) = &self.backend else { return };
        let (api, ui, uri) = (b.api.clone(), self.ui_tx.clone(), uri.to_string());
        self.spawn(async move {
            let r = api
                .follow_artist(&uri, follow)
                .await
                .map(|_| follow)
                .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::Followed(uri, r));
        });
    }

    pub fn toggle_pin(&mut self, uri: &str) {
        let pins = &mut self.config.ui.pinned;
        if let Some(i) = pins.iter().position(|p| p == uri) {
            pins.remove(i);
            self.toast(ToastKind::Info, "Unpinned");
        } else {
            pins.insert(0, uri.to_string());
            self.toast(ToastKind::Info, "Pinned to the top of Your Library");
        }
        self.save_config();
    }

    /// Start index for playing a whole list: a random song when shuffle is on
    /// (like Spotify's play button), otherwise the first.
    fn start_index(len: usize, shuffle: bool) -> usize {
        if shuffle && len > 1 {
            use rand::Rng;
            rand::rng().random_range(0..len)
        } else {
            0
        }
    }

    /// Play a whole list from the top (or a random song with shuffle on),
    /// loading it first if needed.
    pub fn play_whole(&mut self, key: &ListKey, shuffle: bool) {
        match self.lists.peek(key).map(|l| l.tracks.len()) {
            Some(n) if n > 0 => self.play_list(key, Self::start_index(n, shuffle)),
            _ => {
                self.autoplay = Some(key.clone());
                if self.lists.peek(key).is_none() {
                    match key {
                        ListKey::Liked => {
                            self.open_list(ListKey::Liked, "Liked Songs".into(), None)
                        }
                        ListKey::Collection(uri) => {
                            let c = self.find_collection(uri);
                            let title = c.as_ref().map(|c| c.name.clone()).unwrap_or_default();
                            self.open_list(key.clone(), title, c);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Play Local Files from the top (or shuffled).
    pub fn play_local_whole(&mut self, shuffle: bool) {
        let tracks = self.local_filtered.clone();
        if tracks.is_empty() {
            self.toast(
                ToastKind::Info,
                "No local songs yet — add a folder in Settings",
            );
            return;
        }
        let start = Self::start_index(tracks.len(), shuffle);
        self.send(PlayerCmd::PlayContext {
            name: "Local Files".into(),
            key: Some("local".into()),
            tracks,
            start,
        });
    }

    /// Whether `view` (Liked Songs, Local Files, a playlist/album) is what's playing now.
    pub fn view_playing(&self, view: &View) -> bool {
        let ctx = match view {
            View::Liked => "liked",
            View::Local => "local",
            View::Collection(c) => c.uri.as_str(),
            _ => return false,
        };
        self.snapshot.playing && self.snapshot.context_key.as_deref() == Some(ctx)
    }

    /// Play/pause what a library row or page stands for: resume or pause if it
    /// is already the current context, otherwise start it.
    pub fn toggle_play_view(&mut self, view: &View) {
        let ctx = match view {
            View::Liked => "liked".to_string(),
            View::Local => "local".to_string(),
            View::Collection(c) => c.uri.clone(),
            _ => return,
        };
        if self.snapshot.context_key.as_deref() == Some(ctx.as_str())
            && self.snapshot.track.is_some()
        {
            self.send(if self.snapshot.playing {
                PlayerCmd::Pause
            } else {
                PlayerCmd::Play
            });
            return;
        }
        let shuffle = self.snapshot.shuffle;
        match view {
            View::Liked => self.play_whole(&ListKey::Liked, shuffle),
            View::Local => self.play_local_whole(shuffle),
            View::Collection(c) if c.kind == CollectionKind::Artist => {
                // Artist pages play their popular songs.
                self.navigate(view.clone());
                self.play_whole(&ListKey::Collection(c.uri.clone()), shuffle);
            }
            View::Collection(c) => self.play_collection(c),
            _ => {}
        }
    }

    fn find_collection(&self, uri: &str) -> Option<Collection> {
        [
            &self.playlists.items,
            &self.albums.items,
            &self.artists.items,
        ]
        .into_iter()
        .flat_map(|v| v.iter())
        .find(|c| c.uri == uri)
        .cloned()
    }

    /// Row order for the open list when find/sort is active (None = as is).
    pub fn list_order(&mut self, key: &ListKey) -> Option<Vec<usize>> {
        if !self.list_find.active() {
            return None;
        }
        let list = self.lists.peek(key)?;
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.list_find.query.trim().to_lowercase().hash(&mut h);
        (self.list_find.sort as u8).hash(&mut h);
        self.list_find.desc.hash(&mut h);
        list.tracks.len().hash(&mut h);
        list.tracks
            .iter()
            .filter(|t| t.is_stub())
            .count()
            .hash(&mut h);
        let sig = h.finish();
        if let Some((s, o)) = &self.list_find.cache {
            if *s == sig {
                return Some(o.clone());
            }
        }
        let o = find_order(
            &list.tracks,
            &self.list_find.query,
            self.list_find.sort,
            self.list_find.desc,
        );
        self.list_find.cache = Some((sig, o.clone()));
        Some(o)
    }

    pub fn try_autoplay(&mut self) {
        if let Some(k) = self.pending_queue.clone() {
            if let Some(l) = self.lists.peek(&k).filter(|l| !l.tracks.is_empty()) {
                let mut tracks: Vec<TrackRef> =
                    l.tracks.iter().filter(|t| !t.is_stub()).cloned().collect();
                if tracks.is_empty() {
                    tracks = l.tracks.clone();
                }
                let n = tracks.len();
                self.pending_queue = None;
                if n > 0 {
                    self.send(PlayerCmd::AddToQueue(tracks));
                    self.toast(ToastKind::Info, format!("Added {n} songs to queue"));
                }
            }
        }
        let Some(k) = self.autoplay.clone() else {
            return;
        };
        let ready = self.lists.peek(&k).is_some_and(|l| !l.tracks.is_empty());
        if ready {
            self.autoplay = None;
            let shuffle = self.snapshot.shuffle;
            self.play_whole(&k, shuffle);
        }
    }

    fn on_view_opened(&mut self) {
        self.row_sel = None;
        self.row_multi.clear();
        self.multi_tracks.clear();
        match self.view.clone() {
            View::Home => self.load_home(),
            View::Liked => self.open_list(ListKey::Liked, "Liked Songs".into(), None),
            View::Playlists => {
                if !self.playlists.started {
                    self.load_playlists();
                }
            }
            View::Albums => {
                if !self.albums.started {
                    self.load_albums();
                }
            }
            View::Artists => {
                if !self.artists.started {
                    self.load_artists();
                }
            }
            View::Collection(c) => {
                self.open_list(ListKey::Collection(c.uri.clone()), c.name.clone(), Some(c))
            }
            View::Settings => self.refresh_cache_sizes(),
            View::Themes => self.load_market(false),
            View::Profile(u) => self.load_profile(&u, false),
            View::Stats => self.load_stats(),
            View::Search | View::Local | View::Lyrics => {}
        }
    }

    /// Open the playlist / album / artist that's playing.
    pub fn open_context(&mut self) {
        let Some(k) = self.snapshot.context_key.clone() else {
            return;
        };
        let name = self.snapshot.context_name.clone();
        match k.as_str() {
            "liked" => self.navigate(View::Liked),
            "local" => self.navigate(View::Local),
            uri if uri.starts_with("spotify:") => {
                if let Some(c) = self.find_collection(uri) {
                    self.navigate(View::Collection(c));
                    return;
                }
                let kind = if uri.starts_with("spotify:album:") {
                    CollectionKind::Album
                } else if uri.starts_with("spotify:artist:") {
                    CollectionKind::Artist
                } else {
                    CollectionKind::Playlist
                };
                self.open_uri(kind, uri, &name);
            }
            _ => {}
        }
    }

    pub fn open_uri(&mut self, kind: CollectionKind, uri: &str, name: &str) {
        let c = Collection {
            kind,
            uri: uri.to_string(),
            name: name.to_string(),
            subtitle: String::new(),
            image: None,
            total_tracks: 0,
            owned: false,
        };
        self.navigate(View::Collection(c));
    }

    // ------------------------------------------------------------ login

    pub fn start_login(&mut self, interactive: bool) {
        if self.config.downloads.independent {
            if !interactive {
                return;
            }
            self.config.downloads.independent = false;
            self.send(PlayerCmd::Independent(false));
            self.save_config();
        }
        if !interactive {
            self.load_cached_library();
        }
        if self.login.busy {
            return;
        }
        self.login.busy = true;
        self.login.error = None;
        self.login.url = None;
        self.login.status = if interactive {
            "Opening your browser…".into()
        } else {
            "Signing in…".into()
        };
        if interactive {
            self.screen = Screen::Connecting;
        }
        let (shared, cfg, ui) = (self.shared.clone(), self.config.clone(), self.ui_tx.clone());
        let Some(player) = self.player.clone() else {
            self.login.busy = false;
            self.toast(ToastKind::Error, "Playback engine unavailable");
            return;
        };
        self.shared.rt.spawn(async move {
            match backend::login(shared, cfg, ui.clone(), interactive, player).await {
                Ok(b) => ui.send(AppEvent::LoggedIn(Box::new(b))),
                Err(e) => ui.send(AppEvent::LoginFailed(format!("{e:#}"))),
            }
        });
    }

    pub fn logout(&mut self) {
        let local_snapshot = self
            .snapshot
            .track
            .as_ref()
            .filter(|t| crate::local::path(&t.uri).is_some())
            .map(|_| self.snapshot.clone());
        if let Some(b) = self.backend.take() {
            b.shutdown();
        }
        self.shared.logout();
        self.reset_session_state();
        if let Some(snapshot) = local_snapshot {
            self.snapshot = snapshot;
        }
        self.screen = Screen::Main;
        self.view = View::Local;
        self.toast(ToastKind::Info, "Logged out");
    }

    fn reset_session_state(&mut self) {
        self.snapshot = Arc::new(Snapshot::default());
        self.playlists = Paged::default();
        self.albums = Paged::default();
        self.artists = Paged::default();
        self.lists.clear();
        self.artist_albums.clear();
        self.home = HomeState::default();
        self.search = SearchState::default();
        self.view = View::Home;
        self.back.clear();
        self.username.clear();
        self.events = EventTracker::default();
    }

    // ------------------------------------------------------------ loading

    fn spawn<F>(&self, f: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.shared.rt.spawn(f);
    }

    pub fn load_home(&mut self) {
        if self.home.requested {
            return;
        }
        let Some(b) = &self.backend else { return };
        self.home.requested = true;
        {
            let ui = self.ui_tx.clone();
            let file = crate::history::path(&self.shared.paths.data_dir);
            std::thread::spawn(move || {
                let plays = crate::history::load(&file);
                let now = chrono::Local::now().timestamp();
                ui.send(AppEvent::HomeMix(Box::new(crate::history::home_mix(
                    &plays, now,
                ))));
                crate::memory::release_heap();
            });
        }
        if b.api.session_browsing() {
            self.home.loaded = true;
            self.home.error = None;
            return;
        }
        let (api, ui) = (b.api.clone(), self.ui_tx.clone());
        self.spawn(async move {
            let (recent, top, artists) =
                tokio::join!(api.recently_played(), api.top_tracks(), api.top_artists());
            let s = |e: anyhow::Error| format!("{e:#}");
            ui.send(AppEvent::Home {
                recent: recent.map_err(s),
                top: top.map_err(s),
                artists: artists.map_err(s),
            });
        });
    }

    pub fn load_playlists(&mut self) {
        let Some(b) = &self.backend else { return };
        let Some(offset) = self
            .playlists
            .next
            .or((!self.playlists.started).then_some(0))
        else {
            return;
        };
        if self.playlists.loading {
            return;
        }
        self.playlists.started = true;
        self.playlists.loading = true;
        let (api, ui) = (b.api.clone(), self.ui_tx.clone());
        self.spawn(async move {
            let r = api.my_playlists(offset).await.map_err(|e| format!("{e:#}"));
            let missing: Vec<_> = r
                .as_ref()
                .map(|page| {
                    page.items
                        .iter()
                        .filter(|p| p.image.is_none())
                        .map(|p| p.uri.clone())
                        .collect()
                })
                .unwrap_or_default();
            ui.send(AppEvent::Playlists(r));
            use futures_util::StreamExt as _;
            futures_util::stream::iter(missing)
                .for_each_concurrent(4, |uri| {
                    let (api, ui) = (api.clone(), ui.clone());
                    async move {
                        if let Ok(Some(image)) = api.playlist_image(&uri).await {
                            ui.send(AppEvent::PlaylistArtwork(uri, image));
                        }
                    }
                })
                .await;
        });
    }

    pub fn load_albums(&mut self) {
        let Some(b) = &self.backend else { return };
        let Some(offset) = self.albums.next.or((!self.albums.started).then_some(0)) else {
            return;
        };
        if self.albums.loading {
            return;
        }
        self.albums.started = true;
        self.albums.loading = true;
        let (api, ui) = (b.api.clone(), self.ui_tx.clone());
        self.spawn(async move {
            let r = api.saved_albums(offset).await.map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::Albums(r));
        });
    }

    pub fn load_artists(&mut self) {
        let Some(b) = &self.backend else { return };
        if self.artists.loading || (self.artists.started && self.artists.cursor.is_none()) {
            return;
        }
        self.artists.started = true;
        self.artists.loading = true;
        let cursor = self.artists.cursor.clone();
        let (api, ui) = (b.api.clone(), self.ui_tx.clone());
        self.spawn(async move {
            let r = api
                .followed_artists(cursor.as_deref())
                .await
                .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::Artists(r));
        });
    }

    /// Start loading a playlist/album before it's opened (on hover).
    pub fn prefetch(&mut self, c: &Collection) {
        if c.kind == CollectionKind::Artist || self.backend.is_none() {
            return;
        }
        let key = ListKey::Collection(c.uri.clone());
        if self.lists.peek(&key).is_none() {
            self.open_list(key, c.name.clone(), Some(c.clone()));
        }
    }

    fn open_list(&mut self, key: ListKey, title: String, col: Option<Collection>) {
        if self.open_free_list(&key) {
            return;
        }
        if self.lists.get(&key).is_none() && !self.demo {
            // Show the saved copy instantly, then check it against Spotify.
            if let Some(cached) = self
                .list_disk
                .get(&key.context_key(), "json")
                .and_then(|b| serde_json::from_slice::<CachedList>(&b).ok())
            {
                let mut list = TrackList::from_cache(cached);
                if key == ListKey::Liked {
                    self.liked.extend(list.tracks.iter().map(|t| t.uri.clone()));
                }
                if let Some(c) = &col {
                    if c.image.is_some() {
                        list.image = c.image.clone();
                    }
                }
                self.lists.put(key.clone(), list);
                self.load_more(&key);
            }
        }
        if self.lists.get(&key).is_none() {
            let mut list = TrackList::new(title);
            if let Some(c) = &col {
                list.subtitle = c.subtitle.clone();
                list.image = c.image.clone();
                list.total = c.total_tracks;
            }
            list.collection = col;
            if self.demo {
                // Demo: fill playlists and albums with some of the demo songs.
                let n = list.total.clamp(1, 40) as usize;
                let skip = key.context_key().len() * 7 % 200;
                list.tracks = self
                    .lists
                    .peek(&ListKey::Liked)
                    .map(|l| l.tracks.iter().skip(skip).take(n).cloned().collect())
                    .unwrap_or_default();
                list.total = list.tracks.len() as u32;
                list.next = None;
            }
            self.lists.put(key.clone(), list);
            self.load_more(&key);
        }
        if let Some(ListKey::Collection(uri)) = Some(&key) {
            if uri.starts_with("spotify:artist:") && !self.artist_albums.contains(uri) {
                self.load_artist_albums(uri.clone());
            }
        }
    }

    fn load_artist_albums(&mut self, uri: String) {
        let Some(b) = &self.backend else { return };
        self.artist_albums.put(uri.clone(), Vec::new());
        let (api, ui) = (b.api.clone(), self.ui_tx.clone());
        self.spawn(async move {
            let r = api
                .artist_albums(crate::model::uri_id(&uri))
                .await
                .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::ArtistAlbums(uri, r));
        });
    }

    /// Request the next page of a track list.
    pub fn load_more(&mut self, key: &ListKey) {
        let Some(b) = &self.backend else { return };
        let Some(list) = self.lists.get_mut(key) else {
            return;
        };
        let offset = if list.revalidating {
            0
        } else {
            let Some(offset) = list.next else { return };
            offset
        };
        if list.loading {
            return;
        }
        list.loading = true;
        list.error = None;
        let (api, ui, key) = (b.api.clone(), self.ui_tx.clone(), key.clone());
        let collection = list.collection.clone();
        self.spawn(async move {
            let err = |e: anyhow::Error| format!("{e:#}");
            match &key {
                ListKey::Liked => {
                    let page = api.saved_tracks(offset).await.map_err(err);
                    ui.send(AppEvent::Tracks { key, page });
                }
                ListKey::Recent | ListKey::Top => {
                    let r = if key == ListKey::Recent {
                        api.recently_played().await
                    } else {
                        api.top_tracks().await
                    };
                    let page = r
                        .map(|items| Page {
                            total: items.len() as u32,
                            items,
                            offset: 0,
                            next: None,
                        })
                        .map_err(err);
                    ui.send(AppEvent::Tracks { key, page });
                }
                ListKey::Collection(uri) => {
                    let uri = uri.clone();
                    if uri.starts_with("spotify:playlist:") {
                        match api
                            .playlist_tracks(crate::model::uri_id(&uri), offset)
                            .await
                        {
                            Ok(Some(page)) => ui.send(AppEvent::Tracks {
                                key,
                                page: Ok(page),
                            }),
                            Ok(None) => match api.playlist_uris_via_session(&uri).await {
                                Ok((name, uris)) => {
                                    ui.send(AppEvent::TrackUris { key, name, uris })
                                }
                                Err(e) => ui.send(AppEvent::Tracks {
                                    key,
                                    page: Err(err(e)),
                                }),
                            },
                            Err(e) => ui.send(AppEvent::Tracks {
                                key,
                                page: Err(err(e)),
                            }),
                        }
                    } else if uri.starts_with("spotify:album:") {
                        if offset == 0 {
                            match api.album(crate::model::uri_id(&uri)).await {
                                Ok((col, page)) => {
                                    ui.send(AppEvent::CollectionInfo(key.clone(), col));
                                    ui.send(AppEvent::Tracks {
                                        key,
                                        page: Ok(page),
                                    });
                                }
                                Err(e) => ui.send(AppEvent::Tracks {
                                    key,
                                    page: Err(err(e)),
                                }),
                            }
                        } else {
                            let col = collection.unwrap_or(Collection {
                                kind: CollectionKind::Album,
                                uri: uri.clone(),
                                name: String::new(),
                                subtitle: String::new(),
                                image: None,
                                total_tracks: 0,
                                owned: false,
                            });
                            let page = api.album_tracks(&col, offset).await.map_err(err);
                            ui.send(AppEvent::Tracks { key, page });
                        }
                    } else if uri.starts_with("spotify:artist:") {
                        match api.artist_top_tracks_via_session(&uri).await {
                            Ok((name, uris)) => ui.send(AppEvent::TrackUris { key, name, uris }),
                            Err(e) => ui.send(AppEvent::Tracks {
                                key,
                                page: Err(err(e)),
                            }),
                        }
                    }
                }
            }
        });
    }

    /// Fetch metadata for stub rows around `row` (chunks of 50).
    pub fn ensure_meta(&mut self, key: &ListKey, row: usize) {
        let Some(b) = &self.backend else { return };
        let Some(list) = self.lists.get_mut(key) else {
            return;
        };
        let chunk = row / 50;
        if list.meta_requested.contains(&chunk) {
            return;
        }
        list.meta_requested.insert(chunk);
        let uris: Vec<String> = list
            .tracks
            .iter()
            .skip(chunk * 50)
            .take(50)
            .filter(|t| t.is_stub())
            .map(|t| t.uri.clone())
            .collect();
        if uris.is_empty() {
            return;
        }
        let (api, ui) = (b.api.clone(), self.ui_tx.clone());
        self.spawn(async move {
            let tracks = api.tracks_via_session(uris).await;
            ui.send(AppEvent::TrackMeta(tracks));
        });
    }

    pub fn run_search(&mut self) {
        let q = self.search.query.trim().to_string();
        if q.is_empty() || q == self.search.sent {
            return;
        }
        if self.backend.is_none() {
            self.search_free_library(q);
            return;
        }
        let Some(b) = &self.backend else { return };
        self.search.sent = q.clone();
        self.search.loading = true;
        let (api, ui) = (b.api.clone(), self.ui_tx.clone());
        self.spawn(async move {
            let r = api.search(&q).await.map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::Search(r));
        });
    }

    pub fn request_lyrics(&mut self) {
        let Some(track) = self.snapshot.track.clone() else {
            self.lyrics = LyricsView {
                uri: None,
                state: LyricsState::Idle,
                last_line: None,
                source: String::new(),
            };
            return;
        };
        if track.is_stub() || self.lyrics.uri.as_deref() == Some(track.uri.as_str()) {
            return;
        }
        self.lyrics = LyricsView {
            uri: Some(track.uri.clone()),
            state: LyricsState::Loading,
            last_line: None,
            source: String::new(),
        };
        if let Some(l) = (crate::local::path(&track.uri).is_none())
            .then(|| lyrics::from_cache(&self.lyrics_cache, &track))
            .flatten()
        {
            self.lyrics.source = lyrics::cached_source(&self.lyrics_cache, &track)
                .unwrap_or_else(|| "LRCLIB".into());
            self.lyrics.state = LyricsState::Ready(l);
            return;
        }
        let (http, cache, ui) = (
            self.shared.http.clone(),
            self.lyrics_cache.clone(),
            self.ui_tx.clone(),
        );
        let spotify = self.backend.as_ref().map(|b| b.api.clone());
        self.spawn(async move {
            let r = lyrics::fetch(&http, cache, &track, spotify)
                .await
                .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::Lyrics(track.uri.clone(), r));
        });
    }

    pub fn refresh_cache_sizes(&mut self) {
        let paths = self.shared.paths.clone();
        let ui = self.ui_tx.clone();
        self.shared.rt.spawn_blocking(move || {
            ui.send(AppEvent::CacheSizes {
                audio: crate::cache::dir_size(&paths.audio_cache_dir()),
                art: crate::cache::dir_size(&paths.art_cache_dir()),
                lyrics: crate::cache::dir_size(&paths.lyrics_cache_dir()),
            });
        });
    }

    /// Play `index` of a loaded list, using the whole list as context.
    pub fn play_list(&mut self, key: &ListKey, index: usize) {
        let Some(list) = self.lists.get(key) else {
            return;
        };
        if list.tracks.is_empty() {
            return;
        }
        let cmd = PlayerCmd::PlayContext {
            name: list.title.clone(),
            key: Some(key.context_key()),
            tracks: list.tracks.clone(),
            start: index.min(list.tracks.len() - 1),
        };
        self.send(cmd);
    }

    pub fn play_tracks(&mut self, name: &str, tracks: Vec<TrackRef>, index: usize) {
        if tracks.is_empty() {
            return;
        }
        self.send(PlayerCmd::PlayContext {
            name: name.to_string(),
            key: None,
            tracks,
            start: index,
        });
    }

    pub fn save_tracks(&mut self, uris: Vec<String>) {
        if uris.is_empty() {
            return;
        }
        if self.demo {
            self.liked.extend(uris);
            self.toast(ToastKind::Info, "Added to Liked Songs");
            return;
        }
        let Some(b) = &self.backend else { return };
        let (api, ui) = (b.api.clone(), self.ui_tx.clone());
        // Optimistic: show the check right away, roll back on failure.
        self.liked.extend(uris.iter().cloned());
        self.spawn(async move {
            match api.save_to_library(&uris).await {
                Ok(()) => ui.toast(ToastKind::Info, "Added to Liked Songs"),
                Err(e) => {
                    ui.send(AppEvent::Saved(
                        uris.into_iter().map(|u| (u, false)).collect(),
                    ));
                    ui.toast(ToastKind::Error, format!("Could not save: {e:#}"))
                }
            }
        });
    }

    pub fn is_liked(&self, uri: &str) -> bool {
        self.liked.contains(uri)
    }

    /// Save or un-save one track (the heart/check button).
    pub fn toggle_liked(&mut self, uri: &str) {
        if !self.is_liked(uri) {
            self.save_tracks(vec![uri.to_string()]);
            return;
        }
        self.unsave_tracks(vec![uri.to_string()]);
    }

    /// Like several songs at once, or unlike them all if all are liked already.
    pub fn toggle_liked_many(&mut self, uris: Vec<String>) {
        let all = uris.iter().all(|u| self.is_liked(u));
        if all {
            self.unsave_tracks(uris);
        } else {
            let new: Vec<String> = uris.into_iter().filter(|u| !self.is_liked(u)).collect();
            self.save_tracks(new);
        }
    }

    pub fn unsave_tracks(&mut self, uris: Vec<String>) {
        if uris.is_empty() {
            return;
        }
        for u in &uris {
            self.liked.remove(u);
        }
        let gone: HashSet<String> = uris.iter().cloned().collect();
        if let Some(list) = self.lists.get_mut(&ListKey::Liked) {
            let before = list.tracks.len();
            list.tracks.retain(|t| !gone.contains(&t.uri));
            list.total = list
                .total
                .saturating_sub((before - list.tracks.len()) as u32);
        }
        let msg = if uris.len() == 1 {
            "Removed from Liked Songs".to_string()
        } else {
            format!("Removed {} songs from Liked Songs", uris.len())
        };
        if self.demo {
            self.toast(ToastKind::Info, msg);
            return;
        }
        let Some(b) = &self.backend else { return };
        let (api, ui) = (b.api.clone(), self.ui_tx.clone());
        self.spawn(async move {
            match api.remove_from_library(&uris).await {
                Ok(()) => ui.toast(ToastKind::Info, msg),
                Err(e) => {
                    ui.send(AppEvent::Saved(
                        uris.into_iter().map(|u| (u, true)).collect(),
                    ));
                    ui.toast(ToastKind::Error, format!("Could not remove: {e:#}"))
                }
            }
        });
    }

    /// Ask Spotify whether the playing track is liked (once per track).
    fn check_liked(&mut self, uri: &str) {
        if self.demo || !uri.starts_with("spotify:track:") || self.liked_checked.contains(uri) {
            return;
        }
        let Some(b) = &self.backend else { return };
        if b.api.session_browsing() {
            return;
        }
        self.liked_checked.insert(uri.to_string());
        if self.liked_checked.len() > 4096 {
            self.liked_checked.clear();
        }
        let (api, ui, uri) = (b.api.clone(), self.ui_tx.clone(), uri.to_string());
        self.spawn(async move {
            match api.library_contains(std::slice::from_ref(&uri)).await {
                Ok(v) => ui.send(AppEvent::Saved(vec![(uri, v[0])])),
                Err(e) => tracing::debug!("library contains: {e:#}"),
            }
        });
    }

    pub fn reload_plugins(&mut self) {
        let paths = &self.shared.paths;
        self.plugins = PluginHost::load_dir(
            &paths.plugins_dir(),
            &paths.plugin_data_root(),
            &self.config.plugins.disabled,
        );
        self.events = EventTracker::default();
        self.theme_override = ThemeOverride::default();
        self.apply_theme_now();
        for info in self.plugins.infos().to_vec() {
            if let Some(e) = info.error {
                self.toast(
                    ToastKind::Error,
                    format!("Plugin {} failed: {e}", info.name),
                );
            }
        }
        // Let freshly loaded plugins see the current state.
        self.sync_plugins();
    }

    pub fn apply_theme_now(&mut self) {
        let (id, scheme) = match &self.theme_hover_active {
            Some((i, s)) => (i.clone(), s.clone()),
            None => (
                self.config.ui.color_theme.clone(),
                self.config.ui.color_scheme.clone(),
            ),
        };
        let target = theme::palette(
            &self.config.ui,
            &self.theme_override,
            find_scheme(&self.themes, &id, &scheme),
        );
        if target == self.palette_target && self.theme_anim.is_none() && target == self.palette {
            return;
        }
        self.palette_target = target;
        if self.config.ui.animations {
            // Cross-fade from whatever is on screen now.
            self.theme_anim = Some((self.palette, Instant::now()));
        } else {
            self.palette = target;
            theme::apply(self.ui_tx.ctx(), &self.palette);
        }
        self.ui_tx.ctx().request_repaint();
    }

    pub fn set_mini(&mut self, on: bool) {
        if self.mini == on {
            return;
        }
        let ctx = self.ui_tx.ctx().clone();
        if on {
            self.set_fullscreen(false);
            self.mini_restore = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size()));
            ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(egui::vec2(220.0, 220.0)));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(mini::SIZE));
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        } else {
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
            ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(egui::vec2(800.0, 560.0)));
            let size = self
                .mini_restore
                .take()
                .filter(|s| s.x >= 800.0 && s.y >= 560.0)
                .unwrap_or(egui::vec2(1280.0, 820.0));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        }
        self.mini = on;
        ctx.request_repaint();
    }

    pub fn set_private_session(&mut self, on: bool) {
        self.private_session = on;
        if on {
            self.listening = None;
            self.discord.update(None);
            self.toast(
                ToastKind::Info,
                "Private session on: nothing you play is recorded",
            );
        } else {
            self.discord
                .update(crate::discord::Presence::from_snapshot(&self.snapshot));
            self.toast(ToastKind::Info, "Private session off");
        }
    }

    /// Remember something opened or played from search ("Recent searches").
    pub fn remember_search(&mut self, item: crate::config::RecentSearch) {
        if self.private_session || self.demo {
            return;
        }
        let list = &mut self.config.ui.recent_searches;
        list.retain(|r| r.uri() != item.uri());
        list.insert(0, item);
        list.truncate(10);
        self.save_config();
    }

    pub fn forget_search(&mut self, uri: Option<&str>) {
        match uri {
            Some(u) => self.config.ui.recent_searches.retain(|r| r.uri() != u),
            None => self.config.ui.recent_searches.clear(),
        }
        self.save_config();
    }

    pub fn set_fullscreen(&mut self, on: bool) {
        if self.fullscreen == on {
            return;
        }
        self.fullscreen = on;
        self.fullscreen_at = (on && self.config.ui.animations).then(Instant::now);
        let ctx = self.ui_tx.ctx().clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
        if on {
            self.request_lyrics();
        }
        ctx.request_repaint();
    }

    /// Write changed lists (and the library) to disk, at most every few seconds,
    /// on a background thread.
    fn save_lists(&mut self, force: bool) {
        if self.demo || (!force && self.last_list_save.elapsed() < Duration::from_secs(4)) {
            return;
        }
        if self.dirty_lists.is_empty() && !self.library_dirty {
            return;
        }
        self.last_list_save = Instant::now();
        let mut out: Vec<(String, Vec<u8>)> = Vec::new();
        for key in std::mem::take(&mut self.dirty_lists) {
            if let Some(list) = self.lists.peek(&key) {
                if list.tracks.is_empty() || list.error.is_some() {
                    continue;
                }
                if let Ok(b) = serde_json::to_vec(&list.to_cache()) {
                    out.push((key.context_key(), b));
                }
            }
        }
        if std::mem::take(&mut self.library_dirty)
            && self.playlists.next.is_none()
            && !self.playlists.items.is_empty()
        {
            if let Ok(b) = serde_json::to_vec(&self.playlists.items) {
                out.push(("library:playlists".into(), b));
            }
        }
        let disk = self.list_disk.clone();
        let write = move || {
            for (k, b) in out {
                if let Err(e) = disk.put(&k, "json", &b) {
                    tracing::debug!("list cache write: {e:#}");
                }
            }
        };
        if force {
            write();
        } else {
            self.shared.rt.spawn_blocking(write);
        }
    }

    /// Show the library from the last session right away (refreshed after login).
    fn load_cached_library(&mut self) {
        if self.demo || !self.playlists.items.is_empty() {
            return;
        }
        if let Some(items) = self
            .list_disk
            .get("library:playlists", "json")
            .and_then(|b| serde_json::from_slice::<Vec<Collection>>(&b).ok())
        {
            self.playlists.total = items.len() as u32;
            self.playlists.items = items;
            self.playlists.cached = true;
        }
    }

    /// Advance a running palette cross-fade (called once per frame).
    fn step_theme_anim(&mut self, ctx: &egui::Context) {
        let hover = self.theme_hover.take();
        if hover != self.theme_hover_active {
            self.theme_hover_active = hover;
            self.apply_theme_now();
        }
        let Some((from, at)) = self.theme_anim else {
            return;
        };
        let t = (at.elapsed().as_secs_f32() / 0.35).min(1.0);
        if t >= 1.0 {
            self.palette = self.palette_target;
            self.theme_anim = None;
        } else {
            let e = 1.0 - (1.0 - t).powi(3);
            self.palette = from.lerp(&self.palette_target, e);
            ctx.request_repaint();
        }
        theme::apply(ctx, &self.palette);
    }

    /// Switch to an installed theme's scheme (empty id = built-in theme).
    pub fn set_color_theme(&mut self, id: &str, scheme: &str) {
        self.config.ui.color_theme = id.to_string();
        self.config.ui.color_scheme = scheme.to_string();
        self.save_config();
        self.apply_theme_now();
    }

    /// Playlists the user can add songs to.
    pub fn owned_playlists(&self) -> Vec<Collection> {
        self.playlists
            .items
            .iter()
            .filter(|c| c.owned)
            .cloned()
            .collect()
    }

    /// Create "My Playlist #N" (like Spotify), optionally with songs in it.
    pub fn create_playlist(&mut self, tracks: Vec<String>) {
        let n = self.playlists.items.iter().filter(|c| c.owned).count() + 1;
        let name = format!("My Playlist #{n}");
        if self.demo {
            let c = Collection {
                kind: CollectionKind::Playlist,
                uri: format!("spotify:playlist:demo-new{n}"),
                name,
                subtitle: self.username.clone(),
                image: None,
                total_tracks: tracks.len() as u32,
                owned: true,
            };
            self.ui_tx
                .send(AppEvent::PlaylistCreated(Ok(c), Vec::new()));
            return;
        }
        let Some(b) = &self.backend else { return };
        let (api, ui) = (b.api.clone(), self.ui_tx.clone());
        self.spawn(async move {
            let r = api
                .create_playlist(&name)
                .await
                .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::PlaylistCreated(r, tracks));
        });
    }

    /// Open the "Edit details" dialog for a playlist.
    pub fn edit_playlist_details(&mut self, c: &Collection) {
        self.edit_playlist = Some(EditPlaylist {
            uri: c.uri.clone(),
            name: c.name.clone(),
            description: String::new(),
            description_dirty: false,
            image: c.image.clone(),
            saving: false,
        });
    }

    pub fn save_playlist_edit(&mut self) {
        let Some(e) = &mut self.edit_playlist else {
            return;
        };
        let name = e.name.trim().to_string();
        if name.is_empty() {
            self.toast(ToastKind::Error, "Playlist name is required");
            return;
        }
        e.saving = true;
        let uri = e.uri.clone();
        let desc = e
            .description_dirty
            .then(|| e.description.trim().to_string());
        if self.demo {
            self.ui_tx.send(AppEvent::PlaylistEdited {
                uri,
                name: Some(name),
                result: Ok(()),
            });
            return;
        }
        let Some(b) = &self.backend else { return };
        let (api, ui) = (b.api.clone(), self.ui_tx.clone());
        self.spawn(async move {
            let result = api
                .edit_playlist(&uri, Some(&name), desc.as_deref())
                .await
                .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::PlaylistEdited {
                uri,
                name: Some(name),
                result,
            });
        });
    }

    pub fn delete_playlist(&mut self, uri: &str) {
        if self.demo {
            self.ui_tx
                .send(AppEvent::PlaylistDeleted(uri.to_string(), Ok(())));
            return;
        }
        let Some(b) = &self.backend else { return };
        let (api, ui, uri) = (b.api.clone(), self.ui_tx.clone(), uri.to_string());
        self.spawn(async move {
            let r = api
                .delete_playlist(&uri)
                .await
                .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::PlaylistDeleted(uri, r));
        });
    }

    pub fn add_to_playlist(&mut self, uri: &str, name: &str, tracks: Vec<String>) {
        let n = tracks.len();
        let msg = if n == 1 {
            format!("Added to {name}")
        } else {
            format!("Added {n} songs to {name}")
        };
        if self.demo {
            self.ui_tx
                .send(AppEvent::PlaylistTracks(uri.to_string(), Ok(msg)));
            return;
        }
        let Some(b) = &self.backend else { return };
        let (api, ui, uri) = (b.api.clone(), self.ui_tx.clone(), uri.to_string());
        self.spawn(async move {
            let r = api
                .add_to_playlist(&uri, &tracks)
                .await
                .map(|_| msg)
                .map_err(|e| format!("Couldn't add: {e:#}"));
            ui.send(AppEvent::PlaylistTracks(uri, r));
        });
    }

    /// Drag-and-drop reorder inside one of your playlists.
    pub fn move_in_playlist(&mut self, uri: &str, from: usize, to: usize) {
        if from == to {
            return;
        }
        let key = ListKey::Collection(uri.to_string());
        if let Some(list) = self.lists.get_mut(&key) {
            if from < list.tracks.len() && to < list.tracks.len() {
                let t = list.tracks.remove(from);
                list.tracks.insert(to, t);
                self.dirty_lists.insert(key.clone());
            }
        }
        if self.demo {
            return;
        }
        let Some(b) = &self.backend else { return };
        let (api, ui, uri) = (b.api.clone(), self.ui_tx.clone(), uri.to_string());
        self.spawn(async move {
            if let Err(e) = api.move_in_playlist(&uri, from, to).await {
                ui.send(AppEvent::PlaylistTracks(
                    uri,
                    Err(format!("Couldn't move the song: {e:#}")),
                ));
            }
        });
    }

    pub fn remove_from_playlist(&mut self, uri: &str, tracks: &[String]) {
        if tracks.is_empty() {
            return;
        }
        // Optimistic: drop the rows now.
        let key = ListKey::Collection(uri.to_string());
        let gone: HashSet<&str> = tracks.iter().map(String::as_str).collect();
        if let Some(list) = self.lists.get_mut(&key) {
            let before = list.tracks.len();
            list.tracks.retain(|t| !gone.contains(t.uri.as_str()));
            list.total = list
                .total
                .saturating_sub((before - list.tracks.len()) as u32);
        }
        let msg = if tracks.len() == 1 {
            "Removed from this playlist".to_string()
        } else {
            format!("Removed {} songs from this playlist", tracks.len())
        };
        if self.demo {
            self.toast(ToastKind::Info, msg);
            return;
        }
        let Some(b) = &self.backend else { return };
        let (api, ui, uri, tracks) = (
            b.api.clone(),
            self.ui_tx.clone(),
            uri.to_string(),
            tracks.to_vec(),
        );
        self.spawn(async move {
            let r = api
                .remove_from_playlist(&uri, &tracks)
                .await
                .map(|_| msg)
                .map_err(|e| format!("Couldn't remove: {e:#}"));
            ui.send(AppEvent::PlaylistTracks(uri, r));
        });
    }

    fn rename_playlist_locally(&mut self, uri: &str, name: &str) {
        for c in self.playlists.items.iter_mut().filter(|c| c.uri == uri) {
            c.name = name.to_string();
        }
        if let Some(list) = self.lists.get_mut(&ListKey::Collection(uri.to_string())) {
            list.title = name.to_string();
            if let Some(c) = &mut list.collection {
                c.name = name.to_string();
            }
        }
        if let View::Collection(c) = &mut self.view {
            if c.uri == uri {
                c.name = name.to_string();
            }
        }
    }

    pub fn load_profile(&mut self, username: &str, force: bool) {
        if !force && self.profiles.contains_key(username) {
            return;
        }
        let Some(b) = &self.backend else { return };
        self.profiles.insert(username.to_string(), None);
        let (api, ui, u) = (b.api.clone(), self.ui_tx.clone(), username.to_string());
        self.spawn(async move {
            let r = api.profile(&u).await.map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::Profile(u, r));
        });
    }

    /// Open the signed-in user's profile page.
    pub fn open_my_profile(&mut self) {
        if let Some(id) = self.user_id.clone() {
            self.navigate(View::Profile(id));
        }
    }

    pub fn load_market(&mut self, force: bool) {
        if self.market.loading || (self.market.loaded && !force) {
            return;
        }
        self.market.loading = true;
        self.market.error = None;
        let (http, dir, ui) = (
            self.shared.http.clone(),
            self.shared.paths.cache_dir.clone(),
            self.ui_tx.clone(),
        );
        self.spawn(async move {
            let r = crate::themes::marketplace(&http, &dir, force)
                .await
                .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::Themes(r));
        });
    }

    /// Download and install a marketplace theme; `apply` switches to that scheme
    /// (or the first one) once installed.
    pub fn install_theme(&mut self, t: crate::themes::MarketTheme, apply: Option<String>) {
        if !self.market.installing.insert(t.id.clone()) {
            return;
        }
        let (http, dir, ui) = (
            self.shared.http.clone(),
            self.shared.paths.themes_dir(),
            self.ui_tx.clone(),
        );
        self.spawn(async move {
            let r = crate::themes::install(&http, dir, t)
                .await
                .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::ThemeInstalled(r, apply));
        });
    }

    pub fn remove_theme(&mut self, id: &str) {
        if let Err(e) = crate::themes::remove_installed(&self.shared.paths.themes_dir(), id) {
            self.toast(ToastKind::Error, format!("Could not remove theme: {e:#}"));
            return;
        }
        self.themes.retain(|t| t.id != id);
        if self.config.ui.color_theme == id {
            self.set_color_theme("", "");
        }
        self.toast(ToastKind::Info, "Theme removed");
    }

    fn import_theme_file(&mut self, path: &std::path::Path) {
        match crate::themes::import_file(&self.shared.paths.themes_dir(), path) {
            Ok(t) => {
                let (id, scheme) = (t.id.clone(), t.schemes[0].name.clone());
                let n = t.schemes.len();
                self.themes.retain(|x| x.id != t.id);
                self.themes.push(t);
                self.set_color_theme(&id, &scheme);
                self.toast(
                    ToastKind::Info,
                    format!(
                        "Theme imported ({n} colour scheme{})",
                        if n == 1 { "" } else { "s" }
                    ),
                );
            }
            Err(e) => self.toast(ToastKind::Error, format!("{e:#}")),
        }
    }

    pub fn run_plugin_command(&mut self, p: usize, c: usize) {
        self.plugins.run_command(p, c);
    }

    // ------------------------------------------------------------ events

    fn handle_event(&mut self, ctx: &egui::Context, ev: AppEvent) {
        match ev {
            AppEvent::PlaybackDownload(track) => {
                let uri = track.uri.clone();
                let result = self
                    .downloader
                    .as_ref()
                    .ok_or("Download worker is unavailable")
                    .and_then(|d| d.request_playback(track));
                if let Err(message) = result {
                    self.toast(ToastKind::Error, message);
                    self.send(PlayerCmd::DownloadFinished {
                        uri,
                        success: false,
                    });
                }
            }
            AppEvent::DownloadFinished { uri, success } => {
                self.send(PlayerCmd::DownloadFinished { uri, success })
            }
            AppEvent::FreeLibraryImported { result, activate } => {
                self.free_importing = false;
                match result {
                    Ok(imported) => {
                        self.free_library = imported.playlists;
                        self.attach_free_playlists();
                        if activate && self.backend.is_none() {
                            self.config.downloads.independent = true;
                            self.send(PlayerCmd::Independent(true));
                            self.save_config();
                            self.screen = Screen::Main;
                        }
                        self.toast(ToastKind::Info, format!("{} imported playlists / {} tracks / {} unsupported entries skipped",
                            self.free_library.len(), self.free_library.iter().map(|p| p.tracks.len()).sum::<usize>(), imported.skipped));
                    }
                    Err(error) => self.toast(ToastKind::Error, error),
                }
            }
            AppEvent::DownloadsChanged(tracks) => {
                self.download_tracks = tracks;
                self.filter_local();
                self.restore_session(false);
            }
            AppEvent::DownloadStatus(status) => self.download_status = status,
            AppEvent::VerifyDownload(request) => self.start_download_verification(request),

            AppEvent::PlaylistArtwork(uri, image) => {
                if let Some(playlist) = self.playlists.items.iter_mut().find(|p| p.uri == uri) {
                    playlist.image = Some(image.clone());
                }
                if let Some(list) = self.lists.get_mut(&ListKey::Collection(uri)) {
                    list.image = Some(image);
                }
            }
            AppEvent::LibraryAuthorized(result) => {
                self.login.busy = false;
                self.login.url = None;
                match result {
                    Ok(api) => {
                        if let Some(backend) = &mut self.backend {
                            backend.api = api;
                        }
                        self.refresh_library();
                        self.toast(ToastKind::Info, "Spotify library connected");
                    }
                    Err(e) => {
                        self.login.error = Some(e.clone());
                        self.toast(ToastKind::Error, e);
                    }
                }
            }
            AppEvent::Toast(k, m) => self.toast(k, m),
            AppEvent::LoginUrl(u) => self.login.url = Some(u),
            AppEvent::LoginStatus(s) => self.login.status = s,
            AppEvent::LoginFailed(e) => {
                self.login.busy = false;
                self.screen = Screen::Main;
                self.view = View::Local;
                self.toast(ToastKind::Info, "Offline — Spotify features unavailable");
                self.retry_login_at = Instant::now() + Duration::from_secs(30);
                tracing::warn!("login failed: {e}");
                if e != "not logged in" {
                    self.toast(ToastKind::Error, format!("Spotify login: {e}"));
                    self.login.error = Some(e);
                }
            }
            AppEvent::LoggedIn(b) => {
                self.login.busy = false;
                self.login.url = None;
                self.username = b.username.clone();
                self.avatar = b.avatar.clone();
                self.user_id = b.user_id.clone();
                self.connect_enabled = b.connect_enabled;
                if b.connect_enabled {
                    let _entered = self.shared.rt.enter();
                    b.api.watch_devices(self.ui_tx.clone());
                }
                self.backend = Some(*b);
                self.screen = Screen::Main;
                self.load_playlists();
                self.on_view_opened();
                self.restore_session(true);
            }
            AppEvent::Playback(s) => {
                if !self.demo {
                    self.on_snapshot(s)
                }
            }
            AppEvent::LocalReady(tracks) => {
                self.local_tracks = tracks;
                self.filter_local();
                self.restore_session(false);
            }
            AppEvent::LocalScan(result) => {
                self.local_scanning = false;
                match result {
                    Ok(scan) => {
                        self.local_tracks = scan.index.tracks();
                        self.filter_local();
                        self.art.clear_memory();
                        self.toast(
                            ToastKind::Info,
                            format!(
                                "{} local files — {} updated, {} unchanged",
                                self.local_tracks.len(),
                                scan.parsed,
                                scan.reused
                            ),
                        );
                        for warning in scan.warnings {
                            self.toast(ToastKind::Error, warning);
                        }
                    }
                    Err(e) => self.toast(ToastKind::Error, format!("Local scan: {e}")),
                }
            }
            AppEvent::Network(online) => {
                let changed = self.online != Some(online);
                self.online = Some(online);
                if !online && changed {
                    if self.screen != Screen::Main {
                        (self.screen, self.view) = startup_destination(false);
                    }
                    self.toast(ToastKind::Info, "Offline — Spotify features unavailable");
                } else if online && self.backend.is_none() && self.shared.has_stored_login() {
                    self.start_login(false);
                }
            }
            AppEvent::Remote(cmd) => self.send(cmd),
            AppEvent::SeekBy(d) => {
                let pos = self.snapshot.position_now() as i64 + d;
                self.send(PlayerCmd::Seek(pos.max(0) as u32));
            }
            AppEvent::TrayShow => {
                ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
            AppEvent::TrayQuit => self.quit(ctx),
            AppEvent::Playlists(r) => {
                merge_page(&mut self.playlists, r);
                self.attach_free_playlists();
                self.library_dirty = true;
            }
            AppEvent::Albums(r) => merge_page(&mut self.albums, r),
            AppEvent::Artists(r) => {
                self.artists.loading = false;
                match r {
                    Ok((items, cursor, total)) => {
                        self.artists.items.extend(items);
                        self.artists.cursor = cursor;
                        self.artists.total = total;
                    }
                    Err(e) => self.artists.error = Some(e),
                }
            }
            AppEvent::CollectionInfo(key, col) => {
                if let Some(list) = self.lists.get_mut(&key) {
                    list.title = col.name.clone();
                    list.subtitle = col.subtitle.clone();
                    if col.image.is_some() {
                        list.image = col.image.clone();
                    }
                    list.total = col.total_tracks;
                    list.collection = Some(col);
                }
            }
            AppEvent::Tracks { key, page } => {
                if key == ListKey::Liked {
                    if let Ok(p) = &page {
                        self.liked.extend(p.items.iter().map(|t| t.uri.clone()));
                    }
                }
                self.on_tracks(key, page)
            }
            AppEvent::PlaylistCreated(r, tracks) => match r {
                Ok(c) => {
                    self.playlists.items.insert(0, c.clone());
                    self.playlists.total += 1;
                    if tracks.is_empty() {
                        self.toast(ToastKind::Info, format!("Created {}", c.name));
                        self.navigate(View::Collection(c));
                    } else {
                        self.add_to_playlist(&c.uri, &c.name, tracks);
                    }
                }
                Err(e) => self.toast(
                    ToastKind::Error,
                    format!("Couldn't create the playlist: {e}"),
                ),
            },
            AppEvent::PlaylistEdited { uri, name, result } => match result {
                Ok(()) => {
                    if let Some(n) = name {
                        self.rename_playlist_locally(&uri, &n);
                    }
                    self.edit_playlist = None;
                    self.toast(ToastKind::Info, "Playlist updated");
                }
                Err(e) => {
                    if let Some(ed) = &mut self.edit_playlist {
                        ed.saving = false;
                    }
                    self.toast(ToastKind::Error, format!("Couldn't save: {e}"));
                }
            },
            AppEvent::PlaylistDeleted(uri, r) => match r {
                Ok(()) => {
                    self.playlists.items.retain(|c| c.uri != uri);
                    self.playlists.total = self.playlists.total.saturating_sub(1);
                    self.lists.pop(&ListKey::Collection(uri.clone()));
                    if matches!(&self.view, View::Collection(c) if c.uri == uri) {
                        self.navigate_replace(View::Home);
                    }
                    self.toast(ToastKind::Info, "Removed from Your Library");
                }
                Err(e) => self.toast(ToastKind::Error, format!("Couldn't delete: {e}")),
            },
            AppEvent::PlaylistTracks(uri, r) => match r {
                Ok(msg) => {
                    // Reload that playlist next time it's opened (or now, if open).
                    let key = ListKey::Collection(uri.clone());
                    if !self.demo && !msg.starts_with("Removed") {
                        self.lists.pop(&key);
                        if matches!(&self.view, View::Collection(c) if c.uri == uri) {
                            self.on_view_opened();
                        }
                    }
                    self.toast(ToastKind::Info, msg);
                }
                Err(e) => {
                    self.toast(ToastKind::Error, e);
                    self.lists.pop(&ListKey::Collection(uri));
                }
            },
            AppEvent::Radio(name, r) => match r {
                Ok(uri) => {
                    let c = Collection {
                        kind: CollectionKind::Playlist,
                        uri,
                        name: format!("{name} Radio"),
                        subtitle: "Spotify".into(),
                        image: None,
                        total_tracks: 0,
                        owned: false,
                    };
                    self.autoplay = Some(ListKey::Collection(c.uri.clone()));
                    self.navigate(View::Collection(c));
                    self.try_autoplay();
                }
                Err(e) => self.toast(ToastKind::Error, format!("No radio: {e}")),
            },
            AppEvent::Autoplay(seed, r) => match r {
                Ok(tracks) if !tracks.is_empty() => {
                    // Only if that song is still the last one playing.
                    let s = &self.snapshot;
                    let still_last = s.track.as_ref().is_some_and(|t| t.uri == seed)
                        && s.queue.is_empty()
                        && s.upcoming.is_empty();
                    if still_last {
                        match s.context_key.clone() {
                            Some(key) => self.send(PlayerCmd::ExtendContext { key, tracks }),
                            None => self.send(PlayerCmd::AddToQueue(tracks)),
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::debug!("autoplay: {e}"),
            },
            AppEvent::Devices(d) => self.devices = d,
            AppEvent::ArtistInfo(uri, r) => {
                self.artist_info.insert(uri, Some(r));
            }
            AppEvent::Followed(uri, r) => match r {
                Ok(true) => self.toast(ToastKind::Info, "Added to Your Library"),
                Ok(false) => self.toast(ToastKind::Info, "Removed from Your Library"),
                Err(e) => {
                    // Roll back.
                    if self.followed.remove(&uri) {
                    } else {
                        self.followed.insert(uri);
                    }
                    self.toast(ToastKind::Error, format!("Couldn't update: {e}"));
                }
            },
            AppEvent::Stats(st) => {
                self.stats_loading = false;
                if st.range == self.stats_range {
                    self.stats = Some(*st);
                } else {
                    self.load_stats();
                }
            }
            AppEvent::Profile(u, r) => {
                if let Ok(p) = &r {
                    // Keep the top-bar name/photo in sync with the real profile.
                    if self.user_id.as_deref() == Some(u.as_str()) {
                        if !p.name.is_empty() {
                            self.username = p.name.clone();
                        }
                        if p.image.is_some() {
                            self.avatar = p.image.clone();
                        }
                    }
                }
                self.profiles.insert(u, Some(r));
            }
            AppEvent::Themes(r) => {
                self.market.loading = false;
                self.market.loaded = true;
                match r {
                    Ok(v) => self.market.items = v,
                    Err(e) => self.market.error = Some(e),
                }
            }
            AppEvent::ThemeInstalled(r, apply) => match r {
                Ok(t) => {
                    self.market.installing.remove(&t.id);
                    let (id, name) = (t.id.clone(), t.name.clone());
                    let first = t.schemes[0].name.clone();
                    self.themes.retain(|x| x.id != t.id);
                    self.themes.push(t);
                    self.themes.sort_by_key(|t| t.name.to_lowercase());
                    if let Some(s) = apply {
                        let s = if s.is_empty() { first } else { s };
                        self.set_color_theme(&id, &s);
                    }
                    self.toast(ToastKind::Info, format!("{name} installed"));
                }
                Err(e) => {
                    self.market.installing.clear();
                    self.toast(ToastKind::Error, e);
                }
            },
            AppEvent::Saved(states) => {
                for (uri, saved) in states {
                    self.liked_checked.insert(uri.clone());
                    if saved {
                        self.liked.insert(uri);
                    } else {
                        self.liked.remove(&uri);
                    }
                }
            }
            AppEvent::TrackUris { key, name, uris } => {
                if let Some(list) = self.lists.get_mut(&key) {
                    list.loading = false;
                    list.revalidating = false;
                    if !name.is_empty() {
                        list.title = name;
                    }
                    // Keep metadata we already have (e.g. from the disk cache).
                    let known: std::collections::HashMap<&str, &TrackRef> = list
                        .tracks
                        .iter()
                        .filter(|t| !t.is_stub())
                        .map(|t| (t.uri.as_str(), t))
                        .collect();
                    let tracks: Vec<TrackRef> = uris
                        .into_iter()
                        .map(|u| {
                            known
                                .get(u.as_str())
                                .map(|t| (*t).clone())
                                .unwrap_or_else(|| Track::stub(u))
                        })
                        .collect();
                    list.total = tracks.len() as u32;
                    list.tracks = tracks;
                    list.next = None;
                    list.meta_requested.clear();
                    self.dirty_lists.insert(key.clone());
                }
                self.try_autoplay();
            }
            AppEvent::TrackMeta(tracks) => self.on_track_meta(tracks),
            AppEvent::ArtistAlbums(uri, r) => match r {
                Ok(v) => {
                    self.artist_albums.put(uri, v);
                }
                Err(e) => tracing::debug!("artist albums: {e}"),
            },
            AppEvent::Search(r) => {
                self.search.loading = false;
                match r {
                    Ok(res) => {
                        self.search.error = None;
                        self.search.results = Some(res);
                    }
                    Err(e) => self.search.error = Some(e),
                }
            }
            AppEvent::Home {
                recent,
                top,
                artists,
            } => {
                self.home.loaded = true;
                let mut errors = Vec::new();
                for (key, title, r) in [
                    (ListKey::Recent, "Recently played", recent),
                    (ListKey::Top, "Your top tracks", top),
                ] {
                    match r {
                        Ok(items) => {
                            let mut list = TrackList::new(title.into());
                            list.total = items.len() as u32;
                            list.tracks = items;
                            list.next = None;
                            self.lists.put(key, list);
                        }
                        Err(e) => errors.push(e),
                    }
                }
                match artists {
                    Ok(a) => self.home.artists = a,
                    Err(e) => errors.push(e),
                }
                self.home.error = errors.into_iter().next();
            }
            AppEvent::Friends(r) => self.friends = Some(r),
            AppEvent::AlbumInfo(uri, r) => {
                self.album_info.insert(uri, Some(r));
            }
            AppEvent::Recommended(uri, r) => {
                self.recommended.insert(uri, Some(r));
            }
            AppEvent::LibraryChanged(c, add, r) => match r {
                Ok(()) => self.toast(
                    ToastKind::Info,
                    if add {
                        "Added to Your Library"
                    } else {
                        "Removed from Your Library"
                    },
                ),
                Err(e) => {
                    self.apply_library_change(&c, !add);
                    self.toast(
                        ToastKind::Error,
                        format!("Couldn't update Your Library: {e}"),
                    );
                }
            },
            AppEvent::Credits(uri, r) => {
                if let Some((t, slot)) = &mut self.credits {
                    if t.uri == uri {
                        *slot = Some(r.clone());
                    }
                }
                if self.track_credits.contains_key(&uri) {
                    self.track_credits.insert(uri, Some(r));
                }
            }
            AppEvent::HomeMix(mix) => {
                // Fill the shelves the Web API couldn't (session mode or errors).
                for (key, title, items) in [
                    (ListKey::Recent, "Recently played", &mix.recent),
                    (ListKey::Top, "On repeat", &mix.repeat),
                ] {
                    let empty = self.lists.peek(&key).is_none_or(|l| l.tracks.is_empty());
                    if empty && !items.is_empty() {
                        let mut list = TrackList::new(title.into());
                        list.total = items.len() as u32;
                        list.tracks = items.clone();
                        list.next = None;
                        self.lists.put(key, list);
                    }
                }
                if self.home.artists.is_empty() {
                    self.home.artists = mix.artists.clone();
                }
                self.home.mix = *mix;
            }
            AppEvent::Lyrics(uri, r) => {
                if self.lyrics.uri.as_deref() == Some(uri.as_str()) {
                    self.lyrics.state = match r {
                        Ok((l, src)) => {
                            self.lyrics.source = src;
                            LyricsState::Ready(l)
                        }
                        Err(e) => LyricsState::Error(e),
                    };
                    self.lyrics.last_line = None;
                }
            }
            AppEvent::Image(k, img) => self.art.insert(ctx, k, img),
            AppEvent::CacheSizes { audio, art, lyrics } => {
                self.cache_sizes = Some((audio, art, lyrics))
            }
        }
    }

    fn on_tracks(&mut self, key: ListKey, page: Result<Page<TrackRef>, String>) {
        let playing_key = self.snapshot.context_key.clone();
        let Some(list) = self.lists.get_mut(&key) else {
            return;
        };
        list.loading = false;
        if list.revalidating {
            list.revalidating = false;
            match &page {
                Ok(p) if p.offset == 0 => {
                    let same = p.total == list.total
                        && p.items.len() <= list.tracks.len()
                        && p.items
                            .iter()
                            .zip(&list.tracks)
                            .all(|(a, b)| a.uri == b.uri);
                    if same {
                        // Cache is current: keep it (with its loaded metadata).
                        return;
                    }
                    // Changed: start over from the fresh first page.
                    list.tracks.clear();
                    list.meta_requested.clear();
                    list.next = Some(0);
                    self.dirty_lists.insert(key.clone());
                }
                // Offline or failed: keep showing the cached copy quietly.
                Err(_) => return,
                _ => {}
            }
        }
        let Some(list) = self.lists.get_mut(&key) else {
            return;
        };
        match page {
            Ok(p) => {
                if p.offset as usize != list.tracks.len() {
                    // Stale/duplicate page.
                    return;
                }
                let new_items = p.items.clone();
                list.tracks.extend(p.items);
                self.dirty_lists.insert(key.clone());
                list.total = p.total.max(list.tracks.len() as u32);
                list.next = p.next;
                if playing_key.as_deref() == Some(key.context_key().as_str())
                    && !new_items.is_empty()
                {
                    if let Some(b) = &self.backend {
                        b.player.send(PlayerCmd::ExtendContext {
                            key: key.context_key(),
                            tracks: new_items,
                        });
                    }
                }
                self.try_autoplay();
            }
            Err(e) => {
                list.error = Some(e.clone());
                list.next = None;
                self.toast(ToastKind::Error, e);
            }
        }
    }

    fn on_track_meta(&mut self, tracks: Vec<TrackRef>) {
        let map: std::collections::HashMap<&str, &TrackRef> =
            tracks.iter().map(|t| (t.uri.as_str(), t)).collect();
        for (key, list) in self.lists.iter_mut() {
            let mut changed = false;
            for t in list.tracks.iter_mut() {
                if t.is_stub() {
                    if let Some(n) = map.get(t.uri.as_str()) {
                        *t = (*n).clone();
                        changed = true;
                    }
                }
            }
            if changed {
                self.dirty_lists.insert(key.clone());
            }
        }
        self.send(PlayerCmd::UpdateTracks(tracks));
    }

    fn sync_plugins(&mut self) {
        let s = &self.snapshot;
        let info = s
            .track
            .as_ref()
            .filter(|t| !t.is_stub())
            .map(|t| TrackInfo {
                title: t.name.clone(),
                artist: t.artists.clone(),
                album: t.album.clone(),
                uri: t.uri.clone(),
                duration_ms: t.duration_ms,
            });
        let uri = info.as_ref().map(|i| i.uri.clone());
        self.plugins.set_state(info, s.playing, s.position_now());
        for ev in self.events.events(uri.as_deref(), s.playing) {
            self.plugins.emit(ev);
        }
    }

    /// Count listening time and record finished plays (30 s or more).
    fn track_listening(&mut self) {
        let now = Instant::now();
        let s = self.snapshot.clone();
        if let Some(l) = &mut self.listening {
            if l.playing {
                l.ms += now.duration_since(l.since).as_millis() as u64;
            }
            l.since = now;
            l.playing = s.playing;
        }
        let cur = s.track.as_ref().filter(|t| !t.is_stub());
        let changed = match (&self.listening, cur) {
            (Some(l), Some(t)) => l.track.uri != t.uri,
            (None, Some(_)) => true,
            (Some(_), None) => true,
            (None, None) => false,
        };
        if changed {
            self.finish_listening();
            self.listening = cur.map(|t| Listening {
                track: t.clone(),
                ms: 0,
                playing: s.playing,
                since: now,
            });
        }
    }

    fn finish_listening(&mut self) {
        let Some(l) = self.listening.take() else {
            return;
        };
        let mut ms = l.ms;
        if l.playing {
            ms += l.since.elapsed().as_millis() as u64;
        }
        let t = &l.track;
        let ms = ms.min(t.duration_ms.max(1) as u64) as u32;
        if self.demo || self.private_session || ms < crate::history::MIN_PLAY_MS {
            return;
        }
        let play = crate::history::Play {
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            ms,
            uri: t.uri.clone(),
            name: t.name.clone(),
            artists: t.artists.clone(),
            artist_uri: t.artist_uri.clone(),
            album: t.album.clone(),
            album_uri: t.album_uri.clone(),
            image: t.image_large.clone().or(t.image.clone()),
            duration_ms: t.duration_ms,
        };
        let file = crate::history::path(&self.shared.paths.data_dir);
        self.shared.rt.spawn_blocking(move || {
            if let Err(e) = crate::history::append(&file, &play) {
                tracing::warn!("could not save listening history: {e:#}");
            }
        });
        self.stats = None; // recompute next time
    }

    pub fn load_stats(&mut self) {
        if self.stats_loading {
            return;
        }
        self.stats_loading = true;
        let range = self.stats_range;
        let ui = self.ui_tx.clone();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let plays = if self.demo {
            Some(demo_history(
                &self
                    .lists
                    .peek(&ListKey::Liked)
                    .map(|l| l.tracks.clone())
                    .unwrap_or_default(),
                now,
            ))
        } else {
            None
        };
        let file = crate::history::path(&self.shared.paths.data_dir);
        self.shared.rt.spawn_blocking(move || {
            let plays = plays.unwrap_or_else(|| crate::history::load(&file));
            ui.send(AppEvent::Stats(Box::new(crate::history::compute(
                &plays, range, now,
            ))));
        });
    }

    fn on_snapshot(&mut self, s: Arc<Snapshot>) {
        let old_uri = self.snapshot.track.as_ref().map(|t| t.uri.clone());
        let old_stub = self.snapshot.track.as_ref().is_some_and(|t| t.is_stub());
        let started = !self.snapshot.playing && s.playing;
        self.snapshot = s;
        // Volume may be changed remotely (Spotify Connect).
        let vol = ((self.snapshot.volume as u32 * 100 + 32767) / 65535) as u8;
        if self.backend.is_some() && vol != self.config.audio.volume {
            self.config.audio.volume = vol;
        }
        let new_track = self.snapshot.track.clone();
        self.sync_plugins();
        if let Some(m) = &mut self.media {
            m.update(&self.snapshot);
        }
        if !self.private_session {
            self.discord
                .update(crate::discord::Presence::from_snapshot(&self.snapshot));
        }
        self.track_listening();
        self.check_autoplay();
        let changed = new_track.as_ref().map(|t| t.uri.clone()) != old_uri
            || (old_stub && new_track.as_ref().is_some_and(|t| !t.is_stub()));
        if changed && new_track.is_some() {
            // So a crash or forced close still reopens on this song.
            self.save_session(false);
        }
        if (changed || started) && self.snapshot.playing && !self.private_session {
            if let (Some(downloader), Some(track)) = (
                &self.downloader,
                new_track.as_ref().filter(|t| !t.is_stub()),
            ) {
                downloader.request(track.clone());
            }
        }
        if changed && (self.right == RightPanel::Lyrics || self.view == View::Lyrics) {
            self.request_lyrics();
        }
        if changed {
            if let Some(t) = new_track.as_ref().filter(|t| !t.is_stub()) {
                let uri = t.uri.clone();
                self.check_liked(&uri);
            }
        }
    }

    fn process_plugin_actions(&mut self) {
        let actions = self.plugins.drain_actions();
        let mut theme_changed = false;
        for a in actions {
            match a {
                PluginAction::Notify { plugin, text } => {
                    self.toast(ToastKind::Info, format!("{plugin}: {text}"))
                }
                PluginAction::Theme(ov) => {
                    self.theme_override = ov;
                    theme_changed = true;
                }
            }
        }
        if theme_changed {
            self.apply_theme_now();
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Close);
        // Safety net if the window is hidden and the close request isn't processed.
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(3));
            std::process::exit(0);
        });
    }

    /// Spotify's fixed navigation keys: Alt+←/→ and the mouse side buttons go
    /// back / forward, Alt+Shift+B likes the current
    /// song, Ctrl+Shift+M opens the mini player, Esc leaves a text box.
    fn handle_nav_keys(&mut self, ctx: &egui::Context) {
        if self.screen != Screen::Main {
            return;
        }
        let typing = ctx.egui_wants_keyboard_input();
        if ctx.input_mut(|i| {
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::Slash,
            ))
        }) {
            self.shortcuts_open = !self.shortcuts_open;
        }
        // Ctrl+K opens search too, like the current Spotify app.
        if ctx.input_mut(|i| {
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::K,
            ))
        }) {
            self.navigate(View::Search);
            ctx.memory_mut(|m| m.request_focus(egui::Id::new(SEARCH_ID)));
        }
        let (back, fwd, like, mini) = ctx.input_mut(|i| {
            use egui::{Key, KeyboardShortcut as K, Modifiers as M};
            let back = i.consume_shortcut(&K::new(M::ALT, Key::ArrowLeft))
                || i.pointer.button_pressed(egui::PointerButton::Extra1);
            let fwd = i.consume_shortcut(&K::new(M::ALT, Key::ArrowRight))
                || i.pointer.button_pressed(egui::PointerButton::Extra2);
            let like = i.consume_shortcut(&K::new(M::ALT | M::SHIFT, Key::B));
            let mini = i.consume_shortcut(&K::new(M::COMMAND | M::SHIFT, Key::M));
            (back, fwd, like, mini)
        });
        if back && !typing {
            self.go_back();
        }
        if fwd && !typing {
            self.go_forward();
        }
        if like {
            if let Some(t) = self.snapshot.track.clone() {
                if !t.uri.starts_with("local:") {
                    self.toggle_liked(&t.uri);
                }
            }
        }
        if mini {
            let on = !self.mini;
            self.set_mini(on);
        }
        if typing && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        self.handle_nav_keys(ctx);
        let typing = ctx.egui_wants_keyboard_input();
        let mut fired = Vec::new();
        ctx.input_mut(|i| {
            for (action, sc) in &self.shortcuts {
                let plain = sc.modifiers.is_none()
                    || matches!(
                        action,
                        Action::Next | Action::Previous | Action::VolumeUp | Action::VolumeDown
                    );
                if typing && plain && !matches!(sc.logical_key, egui::Key::F12) {
                    continue;
                }
                if i.consume_shortcut(sc) {
                    fired.push(*action);
                }
            }
        });
        for a in fired {
            match a {
                Action::PlayPause => self.send(PlayerCmd::TogglePlay),
                Action::Next => self.send(PlayerCmd::Next),
                Action::Previous => self.send(PlayerCmd::Previous),
                Action::Search => {
                    if self.screen == Screen::Main {
                        self.navigate(View::Search);
                        ctx.memory_mut(|m| m.request_focus(egui::Id::new(SEARCH_ID)));
                    }
                }
                Action::Quit => self.quit(ctx),
                Action::Debug => self.debug.on = !self.debug.on,
                Action::VolumeUp | Action::VolumeDown => {
                    let step: i32 = if a == Action::VolumeUp { 5 } else { -5 };
                    let v = (self.config.audio.volume as i32 + step).clamp(0, 100) as u8;
                    self.set_volume(v);
                }
            }
        }
    }

    // ------------------------------------------------------------ resume

    fn session_path(&self) -> std::path::PathBuf {
        self.shared.paths.data_dir.join("session.json")
    }

    /// Remember the current song, its list and position (Spotify reopens
    /// where you left off). `now` writes on this thread (used at exit).
    fn save_session(&mut self, now: bool) {
        if self.demo {
            return;
        }
        let snap = self.snapshot.clone();
        let Some(cur) = snap.track.clone() else {
            return;
        };
        if cur.is_stub() {
            return;
        }
        let list_tracks = snap.context_key.as_deref().and_then(|k| {
            let key = match k {
                "liked" => Some(ListKey::Liked),
                "recent" => Some(ListKey::Recent),
                "top" => Some(ListKey::Top),
                "local" => None,
                uri => Some(ListKey::Collection(uri.to_string())),
            };
            match key {
                Some(key) => self.lists.peek(&key).map(|l| l.tracks.clone()),
                None => Some(self.local_filtered.clone()),
            }
        });
        let mut tracks: Vec<crate::model::Track> = list_tracks
            .unwrap_or_default()
            .iter()
            .take(5000)
            .map(|t| (**t).clone())
            .collect();
        let start = match tracks.iter().position(|t| t.uri == cur.uri) {
            Some(i) => i,
            None => {
                tracks = vec![(*cur).clone()];
                0
            }
        };
        let saved = SavedSession {
            name: snap.context_name.clone(),
            key: snap.context_key.clone(),
            tracks,
            start,
            position_ms: snap.position_now(),
        };
        let path = self.session_path();
        let write = move || {
            if let Ok(json) = serde_json::to_vec(&saved) {
                let tmp = path.with_extension("tmp");
                if std::fs::write(&tmp, json).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
        };
        if now {
            write();
        } else {
            std::thread::spawn(write);
        }
    }

    /// Load the last session's song, paused. Local songs can be restored before
    /// login; Spotify songs once the session is up.
    fn restore_session(&mut self, logged_in: bool) {
        if self.demo || self.session_restored || self.snapshot.track.is_some() {
            return;
        }
        let Some(saved) = std::fs::read(self.session_path())
            .ok()
            .and_then(|b| serde_json::from_slice::<SavedSession>(&b).ok())
        else {
            self.session_restored = true;
            return;
        };
        let Some(first) = saved.tracks.get(saved.start) else {
            self.session_restored = true;
            return;
        };
        let local = first.uri.starts_with("local:");
        let downloaded = self
            .downloader
            .as_ref()
            .and_then(|d| d.library.path(&first.uri))
            .is_some();
        if !local && !downloaded && !logged_in {
            return;
        }
        self.session_restored = true;
        self.send(PlayerCmd::Restore {
            name: saved.name,
            key: saved.key,
            tracks: saved.tracks.into_iter().map(Arc::new).collect(),
            start: saved.start,
            position_ms: saved.position_ms,
        });
    }

    pub fn toggle_debug(&mut self) {
        self.debug.on = !self.debug.on;
    }

    pub fn set_volume(&mut self, percent: u8) {
        self.config.audio.volume = percent.min(100);
        self.send(PlayerCmd::SetVolume(
            (self.config.audio.volume as u32 * 65535 / 100) as u16,
        ));
    }

    /// Small label following the pointer while dragging (like Spotify's).
    fn draw_drag_ghost(&self, ctx: &egui::Context) {
        let Some(item) = egui::DragAndDrop::payload::<DragItem>(ctx) else {
            return;
        };
        let Some(pos) = ctx.pointer_latest_pos() else {
            return;
        };
        let text = match &*item {
            DragItem::Library(c) => c.name.clone(),
            DragItem::Tracks { tracks, .. } => match tracks.as_slice() {
                [t] => format!("{} · {}", t.name, t.artists),
                ts => format!("{} songs", ts.len()),
            },
        };
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        egui::Area::new(egui::Id::new("drag-ghost"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos + egui::vec2(14.0, 10.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(widgets::mix(self.palette.bg, self.palette.text, 0.12))
                    .corner_radius(4)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(text)
                                .font(widgets::font(13.0, widgets::Weight::Semibold)),
                        );
                    });
            });
    }

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.toasts.retain(|t| t.until > now);
        if self.toasts.is_empty() {
            return;
        }
        let bottom = if self.screen == Screen::Main {
            -100.0
        } else {
            -32.0
        };
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, bottom))
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                let mut dismiss = None;
                for (i, t) in self.toasts.iter().enumerate() {
                    // Fade out during the last 300 ms.
                    let left = t.until.saturating_duration_since(now).as_secs_f32();
                    // …and fade/slide in over the first 200 ms.
                    let age = (t.born.elapsed().as_secs_f32() / 0.2).min(1.0);
                    if age < 1.0 {
                        ui.ctx().request_repaint();
                    }
                    let enter = 1.0 - (1.0 - age).powi(3);
                    let alpha = (left / 0.3).clamp(0.0, 1.0).min(enter);
                    ui.add_space((1.0 - enter) * 14.0);
                    let fill = if t.kind == ToastKind::Error {
                        self.palette.notification_error
                    } else {
                        self.palette.notification
                    };
                    let fg = theme::contrast_on(fill);
                    let resp = ui
                        .vertical_centered(|ui| {
                            egui::Frame::new()
                                .fill(widgets::with_alpha(fill, alpha))
                                .corner_radius(egui::CornerRadius::same(8))
                                .shadow(egui::epaint::Shadow {
                                    offset: [0, 4],
                                    blur: 16,
                                    spread: 0,
                                    color: egui::Color32::from_black_alpha((120.0 * alpha) as u8),
                                })
                                .inner_margin(egui::Margin::symmetric(16, 10))
                                .show(ui, |ui| {
                                    // Size the pill to its text (Spotify's toasts hug the label).
                                    let w = widgets::galley(
                                        ui,
                                        &t.text,
                                        14.0,
                                        widgets::Weight::Semibold,
                                        egui::Color32::WHITE,
                                        520.0,
                                    )
                                    .size()
                                    .x;
                                    ui.set_max_width(w.ceil() + 1.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&t.text)
                                                .font(widgets::font(
                                                    14.0,
                                                    widgets::Weight::Semibold,
                                                ))
                                                .color(widgets::with_alpha(fg, alpha)),
                                        )
                                        .wrap(),
                                    );
                                })
                                .response
                        })
                        .inner
                        .interact(egui::Sense::click());
                    if resp.on_hover_text("Click to dismiss").clicked() {
                        dismiss = Some(i);
                    }
                }
                if let Some(i) = dismiss {
                    self.toasts.remove(i);
                }
            });
        if let Some(next) = self.toasts.iter().map(|t| t.until).min() {
            let left = next.saturating_duration_since(now);
            // Repaint for the fade only near the end.
            ctx.request_repaint_after(if left > Duration::from_millis(300) {
                left - Duration::from_millis(300)
            } else {
                Duration::from_millis(30)
            });
        }
    }

    fn draw_debug(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let d = &mut self.debug;
        d.frames += 1;
        let start = *d.window_start.get_or_insert(now);
        if now.duration_since(start) >= Duration::from_secs(1) {
            d.fps = d.frames as f32 / now.duration_since(start).as_secs_f32();
            d.frames = 0;
            d.window_start = Some(now);
        }
        if !d.on {
            return;
        }
        if d.sampled
            .is_none_or(|s| now.duration_since(s) > Duration::from_millis(500))
        {
            d.rss = memory_stats::memory_stats().map(|m| m.physical_mem);
            d.sampled = Some(now);
        }
        let atlas = ctx.fonts(|f| f.font_image_size());
        let text = format!(
            "RSS {}   FPS {:.1}   art {}/{}   lists {}   font atlas {}×{}",
            d.rss
                .map(|b| format!("{:.1} MB", b as f64 / 1048576.0))
                .unwrap_or_else(|| "n/a".into()),
            d.fps,
            self.art.len(),
            self.art.cap(),
            self.lists.len(),
            atlas[0],
            atlas[1],
        );
        egui::Area::new(egui::Id::new("debug-overlay"))
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(8.0, -96.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(200))
                    .corner_radius(egui::CornerRadius::same(4))
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(text)
                                .monospace()
                                .color(egui::Color32::from_rgb(0x9f, 0xff, 0x9f)),
                        );
                    });
            });
        // Keep the overlay live (twice per second, not every frame).
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn schedule_repaint(&mut self, ctx: &egui::Context) {
        if self.backend.is_none() && !std::env::args().any(|a| a == "--demo") {
            if Instant::now() >= self.retry_login_at {
                self.retry_login_at = Instant::now() + Duration::from_secs(30);
                if !self.login.busy {
                    self.probe_network();
                }
            }
            ctx.request_repaint_after(
                self.retry_login_at
                    .saturating_duration_since(Instant::now()),
            );
        }
        if self.screen != Screen::Main {
            return;
        }
        let s = &self.snapshot;
        if s.playing && s.track.is_some() {
            let mut wait = Duration::from_millis(500);
            if self.right == RightPanel::Lyrics || self.view == View::Lyrics {
                if let LyricsState::Ready(Lyrics::Synced(lines)) = &self.lyrics.state {
                    if let Some(ms) = lyrics::ms_until_next(lines, s.position_now()) {
                        wait = wait.min(Duration::from_millis(ms as u64 + 15));
                    }
                }
            }
            ctx.request_repaint_after(wait);
        }
        if let Some(t) = self.search.edited_at {
            let el = t.elapsed();
            if el >= SEARCH_DEBOUNCE {
                self.search.edited_at = None;
                self.run_search();
            } else {
                ctx.request_repaint_after(SEARCH_DEBOUNCE - el);
            }
        }
    }

    fn on_close_requested(&mut self, ctx: &egui::Context) {
        self.download_verification.take();
        if self.quitting {
            return;
        }
        if self.config.ui.minimize_to_tray && self.tray.is_some() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }
    }

    fn shutdown(&mut self) {
        self.download_verification.take();
        self.save_session(true);
        self.finish_listening();
        self.save_config();
        self.save_lists(true);
        if let Some(b) = &self.backend {
            b.shutdown();
        }
        self.send(PlayerCmd::Shutdown);
    }
}

fn merge_page(p: &mut Paged<Collection>, r: Result<Page<Collection>, String>) {
    p.loading = false;
    match r {
        Ok(page) => {
            if p.cached && page.offset == 0 {
                // Fresh data replaces what was shown from the cache.
                p.cached = false;
                p.items.clear();
                p.next = Some(0);
            }
            if page.offset == p.next.unwrap_or(0) {
                p.items.extend(page.items);
                p.total = page.total;
                p.next = page.next;
            }
        }
        Err(e) => {
            p.error = Some(e);
            p.next = None;
        }
    }
}

#[cfg(windows)]
fn window_handle(cc: &eframe::CreationContext<'_>) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut std::ffi::c_void),
        _ => None,
    }
}

#[cfg(not(windows))]
fn window_handle(_cc: &eframe::CreationContext<'_>) -> Option<*mut std::ffi::c_void> {
    None
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Bounded per frame so a flood of events can't stall rendering.
        for _ in 0..256 {
            match self.rx.try_recv() {
                Ok(ev) => self.handle_event(ctx, ev),
                Err(_) => break,
            }
        }
        self.process_plugin_actions();
        self.step_theme_anim(ctx);
        self.save_lists(false);
        // Drop a Spicetify color.ini onto the window to import it as a theme.
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| {
                    p.extension().is_some_and(|e| {
                        e.eq_ignore_ascii_case("ini") || e.eq_ignore_ascii_case("json")
                    })
                })
                .collect()
        });
        for p in dropped {
            if p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
            {
                self.free_import_path = p.display().to_string();
                self.import_free_library();
            } else {
                self.import_theme_file(&p);
            }
        }
        // Popups/menus measure themselves on the first frame and only move to
        // a position that fits (e.g. flip above a row near the bottom) on the
        // next one; the app is reactive, so ask for that one extra frame.
        if ctx.input(|i| i.pointer.any_click() || i.pointer.any_released()) {
            ctx.request_repaint();
        }
        if self.config.ui.trim_memory {
            let (had_input, hidden) = ctx.input(|i| {
                (
                    !i.events.is_empty() || i.pointer.is_moving(),
                    i.viewport().minimized == Some(true)
                        || i.viewport().visible() == Some(false)
                        || i.viewport().occluded == Some(true),
                )
            });
            match self.trimmer.tick(Instant::now(), had_input, hidden) {
                crate::memory::Decision::Trim => {
                    if crate::memory::trim() {
                        tracing::debug!("working set trimmed");
                    }
                }
                crate::memory::Decision::Wait(d) => ctx.request_repaint_after(d),
                crate::memory::Decision::Nothing => {}
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.on_close_requested(ctx);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_shortcuts(&ctx);
        if ctx.input(|i| i.key_pressed(egui::Key::F11)) && self.screen == Screen::Main {
            self.set_fullscreen(!self.fullscreen);
        }
        if self.mini && self.screen == Screen::Main {
            mini::show(self, ui);
            self.show_download_verification(&ctx);
            self.schedule_repaint(&ctx);
            return;
        }
        if self.fullscreen && self.screen == Screen::Main {
            fullscreen::show(self, ui);
            self.show_download_verification(&ctx);
            self.draw_toasts(&ctx);
            self.draw_debug(&ctx);
            self.schedule_repaint(&ctx);
            return;
        }
        match self.screen {
            Screen::Login | Screen::Connecting => login::show(self, ui),
            Screen::Main => {
                let pal = self.palette;
                ui.painter().rect_filled(ui.max_rect(), 0.0, pal.base);
                egui::Panel::top("top-bar")
                    .exact_size(topbar::HEIGHT)
                    .frame(
                        egui::Frame::new()
                            .fill(pal.base)
                            .inner_margin(egui::Margin::symmetric(8, 0)),
                    )
                    .show(ui, |ui| topbar::show(self, ui));
                if self.login.busy || self.login.url.is_some() {
                    egui::Panel::top("library-status")
                        .frame(
                            egui::Frame::new()
                                .fill(pal.base)
                                .inner_margin(egui::Margin::symmetric(18, 6)),
                        )
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if self.login.busy {
                                    ui.spinner();
                                    ui.label(&self.login.status);
                                }
                                if let Some(url) = &self.login.url {
                                    ui.hyperlink_to("Continue in your browser", url);
                                }
                            });
                        });
                }
                // Spotify's green strip when music plays on another device.
                let me = self.backend.as_ref().map(|b| b.api.device_id());
                if let Some(d) = self
                    .devices
                    .iter()
                    .find(|d| d.active && Some(&d.id) != me.as_ref())
                    .cloned()
                {
                    egui::Panel::bottom("listening-on")
                        .exact_size(26.0)
                        .frame(egui::Frame::new().fill(pal.accent))
                        .show(ui, |ui| {
                            let r = ui.max_rect();
                            widgets::text(
                                ui,
                                egui::pos2(r.right() - 24.0, r.center().y),
                                egui::Align2::RIGHT_CENTER,
                                &format!("Playing on {}", d.name),
                                13.0,
                                widgets::Weight::Bold,
                                pal.on_accent,
                                r.width() - 48.0,
                            );
                        });
                }
                egui::Panel::bottom("player-bar")
                    .exact_size(84.0)
                    .frame(
                        egui::Frame::new()
                            .fill(pal.base)
                            .inner_margin(egui::Margin {
                                left: 8,
                                right: 8,
                                top: 4,
                                bottom: 8,
                            }),
                    )
                    .show(ui, |ui| player_bar::show(self, ui));
                // Narrow window: fold the library into the artwork rail so the
                // main view keeps a usable width (like Spotify does).
                let right_w = if self.right == RightPanel::None {
                    0.0
                } else {
                    340.0
                };
                self.auto_compact = !self.config.ui.compact_library
                    && ui.max_rect().width() - 300.0 - right_w < 480.0;
                let compact = self.config.ui.compact_library || self.auto_compact;
                egui::Panel::left(if compact {
                    "library-rail"
                } else {
                    "library-expanded"
                })
                .default_size(if compact { 72.0 } else { 300.0 })
                .size_range(if compact { 72.0..=72.0 } else { 260.0..=420.0 })
                .resizable(!compact)
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(pal.sidebar)
                        .corner_radius(8)
                        .outer_margin(egui::Margin {
                            left: 8,
                            right: 4,
                            top: 0,
                            bottom: 0,
                        })
                        .inner_margin(egui::Margin::symmetric(4, 8)),
                )
                .show(ui, |ui| sidebar::show(self, ui));
                let open = self.right != RightPanel::None;
                if open {
                    self.last_right = self.right;
                }
                let narrow = ui.max_rect().width() < 1100.0;
                let panel = egui::Panel::right("right-panel")
                    .default_size(330.0)
                    .size_range(if narrow { 260.0..=280.0 } else { 280.0..=440.0 })
                    .resizable(true)
                    .show_separator_line(false)
                    .frame(
                        egui::Frame::new()
                            .fill(pal.bg)
                            .corner_radius(8)
                            .outer_margin(egui::Margin {
                                left: 4,
                                right: 8,
                                top: 0,
                                bottom: 0,
                            })
                            .inner_margin(egui::Margin::symmetric(16, 12)),
                    );
                if self.config.ui.animations {
                    // Slides in/out; keeps drawing the last panel while closing.
                    let mut expanded = open;
                    let shown = self.right;
                    self.right = self.last_right;
                    panel.show_collapsible(ui, &mut expanded, |ui| panels::show(self, ui));
                    self.right = if open && !expanded {
                        RightPanel::None
                    } else {
                        shown
                    };
                } else if open {
                    panel.show(ui, |ui| panels::show(self, ui));
                }
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(pal.bg)
                            .corner_radius(8)
                            .outer_margin(egui::Margin {
                                left: 4,
                                right: if self.right == RightPanel::None { 8 } else { 4 },
                                top: 0,
                                bottom: 0,
                            }),
                    )
                    .show(ui, |ui| {
                        ui.set_clip_rect(ui.max_rect());
                        // Quick fade-in when switching pages (ease-out, 180 ms).
                        if let Some(at) = self.nav_at.filter(|_| self.config.ui.animations) {
                            let t = (at.elapsed().as_secs_f32() / 0.18).min(1.0);
                            if t < 1.0 {
                                let e = 1.0 - (1.0 - t).powi(3);
                                ui.multiply_opacity(0.25 + 0.75 * e);
                                ui.ctx().request_repaint();
                            } else {
                                self.nav_at = None;
                            }
                        }
                        views::show(self, ui)
                    });
            }
        }
        if self.screen == Screen::Main && topbar::custom_chrome(self) {
            topbar::resize_handles(&ctx);
        }
        if self.screen == Screen::Main {
            dialogs::show(self, &ctx);
            self.draw_drag_ghost(&ctx);
        }
        self.draw_toasts(&ctx);
        self.draw_debug(&ctx);
        self.show_download_verification(&ctx);
        self.schedule_repaint(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shutdown();
    }
}

#[cfg(test)]
mod startup_tests {
    #[test]
    fn list_cache_roundtrip_marks_revalidation() {
        let mut l = super::TrackList::new("Gym".into());
        l.tracks = vec![std::sync::Arc::new(crate::model::Track {
            uri: "spotify:track:a".into(),
            name: "A".into(),
            ..Default::default()
        })];
        l.total = 3;
        l.next = Some(1);
        let bytes = serde_json::to_vec(&l.to_cache()).unwrap();
        let back = super::TrackList::from_cache(serde_json::from_slice(&bytes).unwrap());
        assert!(back.revalidating);
        assert_eq!(back.title, "Gym");
        assert_eq!(back.total, 3);
        assert_eq!(
            back.next,
            Some(1),
            "partial lists continue paging after the cached rows"
        );
        assert_eq!(back.tracks[0].name, "A");
    }

    use super::*;

    #[test]
    fn collection_pagination_accepts_raw_offsets_after_folder_filtering() {
        let collection = Collection {
            kind: CollectionKind::Playlist,
            uri: "spotify:playlist:test".into(),
            name: "Test".into(),
            subtitle: String::new(),
            image: None,
            total_tracks: 1,
            owned: true,
        };
        let mut paged = Paged::default();
        merge_page(
            &mut paged,
            Ok(Page {
                items: vec![collection.clone()],
                offset: 0,
                total: 4,
                next: Some(2),
            }),
        );
        merge_page(
            &mut paged,
            Ok(Page {
                items: vec![collection],
                offset: 2,
                total: 4,
                next: None,
            }),
        );
        assert_eq!(paged.items.len(), 2);
        assert_eq!(paged.next, None);
    }
    #[test]
    fn offline_startup_skips_login_and_opens_local_files() {
        assert_eq!(startup_destination(false), (Screen::Main, View::Local));
        assert_eq!(startup_destination(true), (Screen::Login, View::Home));
    }

    #[test]
    fn find_order_filters_and_sorts() {
        use super::{find_order, ListSort};
        let t = |name: &str, artist: &str, ms: u32| {
            std::sync::Arc::new(crate::model::Track {
                uri: format!("spotify:track:{name}"),
                name: name.into(),
                artists: artist.into(),
                duration_ms: ms,
                ..Default::default()
            })
        };
        let v = vec![
            t("Bravo", "Zed", 300),
            t("alpha", "Amy", 100),
            t("Charlie", "Amy", 200),
        ];
        assert_eq!(find_order(&v, "", ListSort::Custom, false), [0, 1, 2]);
        assert_eq!(find_order(&v, "", ListSort::Title, false), [1, 0, 2]);
        assert_eq!(find_order(&v, "", ListSort::Title, true), [2, 0, 1]);
        assert_eq!(find_order(&v, "", ListSort::Duration, false), [1, 2, 0]);
        assert_eq!(find_order(&v, "amy", ListSort::Custom, false), [1, 2]);
        assert_eq!(find_order(&v, " ZED ", ListSort::Custom, false), [0]);
    }
}
