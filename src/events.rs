//! Messages from background tasks to the UI thread.

use std::sync::mpsc;
use std::sync::Arc;

use crate::backend::Backend;
use crate::lyrics::Lyrics;
use crate::model::{Collection, Page, SearchResults, TrackRef};
use crate::player::{PlayerCmd, Snapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Error,
}

/// Identifies which list a loaded page belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ListKey {
    Liked,
    Recent,
    Top,
    /// Playlist / album / artist by URI.
    Collection(String),
}

impl ListKey {
    /// Stable string used as the player's context key.
    pub fn context_key(&self) -> String {
        match self {
            ListKey::Liked => "liked".into(),
            ListKey::Recent => "recent".into(),
            ListKey::Top => "top".into(),
            ListKey::Collection(u) => u.clone(),
        }
    }
}

pub enum AppEvent {
    Provider(crate::providers::UiEvent),
    AccountLibrary {
        generation: u64,
        result: Result<crate::spotify_library::Connected, String>,
    },
    PlaybackDownload(TrackRef),
    PlaybackBuffer {
        uri: String,
        buffer: Arc<crate::progressive::Buffer>,
    },
    DownloadFinished {
        uri: String,
        success: bool,
    },
    FreeLibraryImported {
        result: Result<crate::free_library::Import, String>,
        activate: bool,
    },
    VerifyDownload(crate::verification::Request),
    DownloadsChanged(Vec<TrackRef>),
    DownloadStatus(String),
    PlaylistArtwork(String, String),
    LibraryAuthorized(Result<Arc<crate::api::Api>, String>),
    LocalScan(Result<crate::local::Scan, String>),
    LocalReady(Vec<TrackRef>),
    Network(bool),
    /// Liked-state of tracks: (uri, saved). Sent after save/remove/contains.
    Saved(Vec<(String, bool)>),
    /// A playlist was created; the tracks are added to it next.
    PlaylistCreated(Result<Collection, String>, Vec<String>),
    /// Name/description saved (or failed).
    PlaylistEdited {
        uri: String,
        name: Option<String>,
        result: Result<(), String>,
    },
    PlaylistDeleted(String, Result<(), String>),
    /// Tracks added/removed: playlist uri and a message for the toast.
    PlaylistTracks(String, Result<String, String>),
    /// Radio playlist found for a seed (seed name, playlist uri).
    Radio(String, Result<String, String>),
    /// Autoplay songs to continue after `seed` (the last song).
    Autoplay(String, Result<Vec<TrackRef>, String>),
    /// Artist bio / picture / related artists.
    ArtistInfo(String, Result<crate::model::ArtistInfo, String>),
    /// Follow state changed (artist uri, following) or failed (error).
    Followed(String, Result<bool, String>),
    /// Spotify Connect devices from a cluster update.
    Devices(Vec<crate::model::Device>),
    /// Listening stats computed in the background.
    Stats(Box<crate::history::Stats>),
    /// A user's public profile was loaded.
    Profile(String, Result<crate::model::Profile, String>),
    /// Theme marketplace listing.
    Themes(Result<Vec<crate::themes::MarketTheme>, String>),
    /// A theme finished installing; `apply` = scheme to switch to.
    ThemeInstalled(
        Result<crate::themes::InstalledTheme, String>,
        Option<String>,
    ),
    Toast(ToastKind, String),
    /// Authorize URL, shown in case the browser did not open.
    LoginUrl(String),
    LoginStatus(String),
    LoginFailed(String),
    LoggedIn(Box<Backend>),
    Playback(Arc<Snapshot>),
    /// A command from OS media keys or the tray.
    Remote(PlayerCmd),
    /// Relative seek in ms (media keys).
    SeekBy(i64),
    /// Show / quit requests from the tray.
    TrayShow,
    TrayQuit,
    Playlists(Result<Page<Collection>, String>),
    Albums(Result<Page<Collection>, String>),
    Artists(Result<(Vec<Collection>, Option<String>, u32), String>),
    /// Header info for a collection view (name, subtitle, image).
    CollectionInfo(ListKey, Collection),
    /// A page of tracks for a list. `total` lets the UI show placeholder rows.
    Tracks {
        key: ListKey,
        page: Result<Page<TrackRef>, String>,
    },
    /// All URIs of a list loaded through librespot (metadata filled lazily).
    TrackUris {
        key: ListKey,
        name: String,
        uris: Vec<String>,
    },
    /// Metadata for stub tracks (any list may use them).
    TrackMeta(Vec<TrackRef>),
    ArtistAlbums(String, Result<Vec<crate::model::Release>, String>),
    Search(Result<SearchResults, String>),
    Home {
        recent: Result<Vec<TrackRef>, String>,
        top: Result<Vec<TrackRef>, String>,
        artists: Result<Vec<Collection>, String>,
    },
    /// Add to / remove from Your Library finished (item, added, result).
    LibraryChanged(crate::model::Collection, bool, Result<(), String>),
    /// Friend Activity list.
    Friends(Result<Vec<crate::model::Friend>, String>),
    /// Album page footer details.
    AlbumInfo(String, Result<crate::model::AlbumInfo, String>),
    /// Recommended songs for one of your playlists.
    Recommended(String, Result<Vec<TrackRef>, String>),
    /// Credits for a song (uri, result).
    Credits(String, Result<crate::model::Credits, String>),
    /// Home shelves built from local listening history.
    HomeMix(Box<crate::history::HomeMix>),
    /// Lyrics and their provider.
    Lyrics(String, Result<(Lyrics, String), String>),
    Image(String, Option<egui::ColorImage>),
    CacheSizes {
        audio: u64,
        art: u64,
        lyrics: u64,
    },
}

/// Sender that also wakes the UI thread.
#[derive(Clone)]
pub struct UiTx {
    tx: mpsc::Sender<AppEvent>,
    ctx: egui::Context,
}

impl UiTx {
    pub fn new(tx: mpsc::Sender<AppEvent>, ctx: egui::Context) -> Self {
        Self { tx, ctx }
    }

    pub fn send(&self, e: AppEvent) {
        if self.tx.send(e).is_ok() {
            self.ctx.request_repaint();
        }
    }

    pub fn toast(&self, kind: ToastKind, msg: impl Into<String>) {
        self.send(AppEvent::Toast(kind, msg.into()));
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }
}
