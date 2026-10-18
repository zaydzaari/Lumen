//! Compact data structures shared by the API layer, the player and the UI.
//! Full JSON responses are never kept; everything is mapped into these.

use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Track {
    /// `spotify:track:...`
    pub uri: String,
    /// Empty while only the URI is known (metadata not yet loaded).
    pub name: String,
    /// Artist names joined with ", ".
    pub artists: String,
    pub artist_uri: Option<String>,
    pub album: String,
    pub album_uri: Option<String>,
    pub duration_ms: u32,
    /// Smallest suitable cover image URL (~64px).
    pub image: Option<String>,
    /// Larger cover (~300px) used for the now-playing view.
    pub image_large: Option<String>,
    pub explicit: bool,
}

pub type TrackRef = Arc<Track>;

impl Track {
    /// A placeholder with only the URI known.
    pub fn stub(uri: impl Into<String>) -> TrackRef {
        Arc::new(Track {
            uri: uri.into(),
            ..Default::default()
        })
    }

    pub fn is_stub(&self) -> bool {
        self.name.is_empty()
    }

    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            "Loading…"
        } else {
            &self.name
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CollectionKind {
    Playlist,
    Album,
    Artist,
}

/// A playlist, album or artist as shown in lists and grids.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Collection {
    pub kind: CollectionKind,
    /// `spotify:playlist:...`, `spotify:album:...`, `spotify:artist:...`
    pub uri: String,
    pub name: String,
    /// Owner / artist names / "Artist".
    pub subtitle: String,
    pub image: Option<String>,
    pub total_tracks: u32,
    /// For playlists: whether the user owns or collaborates (Web API returns items).
    pub owned: bool,
}

impl Collection {
    pub fn id(&self) -> &str {
        self.uri.rsplit(':').next().unwrap_or(&self.uri)
    }
}

/// One page of a paginated listing.
#[derive(Clone, Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub offset: u32,
    pub total: u32,
    /// Offset of the next page, if any.
    pub next: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct SearchResults {
    pub query: String,
    pub tracks: Vec<TrackRef>,
    pub albums: Vec<Collection>,
    pub artists: Vec<Collection>,
    pub playlists: Vec<Collection>,
}

/// A Spotify user's public profile (from Spotify's own profile service).
#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub name: String,
    pub image: Option<String>,
    pub followers: u32,
    pub following: u32,
    pub total_public_playlists: u32,
    pub public_playlists: Vec<Collection>,
    pub recent_artists: Vec<Collection>,
}

#[derive(Clone, Debug, Default)]
pub struct UserInfo {
    pub id: String,
    pub display_name: String,
    /// Profile picture URL (smallest available), if the account has one.
    pub avatar: Option<String>,
}

/// Extract the base62 id from a `spotify:kind:id` URI.
pub fn uri_id(uri: &str) -> &str {
    uri.rsplit(':').next().unwrap_or(uri)
}

/// Format milliseconds as `m:ss` (or `h:mm:ss`).
pub fn fmt_ms(ms: u32) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(fmt_ms(0), "0:00");
        assert_eq!(fmt_ms(61_000), "1:01");
        assert_eq!(fmt_ms(3_723_000), "1:02:03");
    }

    #[test]
    fn uri_ids() {
        assert_eq!(uri_id("spotify:track:abc"), "abc");
        assert_eq!(uri_id("abc"), "abc");
        assert!(Track::stub("spotify:track:x").is_stub());
    }
}

/// A Spotify Connect device (phone, computer, speaker, TV…).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// "computer", "smartphone", "speaker", "tv", "tablet", …
    pub kind: String,
    pub active: bool,
    /// 0..=65535
    pub volume: u32,
}

/// Extra artist details for the artist page.
#[derive(Clone, Debug, Default)]
pub struct ArtistInfo {
    pub bio: Option<String>,
    pub portrait: Option<String>,
    pub related: Vec<Collection>,
    pub popularity: i32,
}

/// Song credits: (role title, people) groups and the sources Spotify got them from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Credits {
    pub roles: Vec<(String, Vec<CreditPerson>)>,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreditPerson {
    pub name: String,
    pub uri: Option<String>,
    /// e.g. "Composer, Lyricist".
    pub roles: String,
}

/// Which shelf of an artist's discography a release belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseGroup {
    Album,
    Single,
    Compilation,
    AppearsOn,
}

/// One release on an artist page.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub group: ReleaseGroup,
    pub year: i32,
    pub popularity: i32,
    /// Card data; the subtitle is Spotify's "2024 • Album".
    pub coll: Collection,
}

/// Details shown at the bottom of an album page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlbumInfo {
    /// "March 3, 2023" (or just the year when that's all Spotify has).
    pub released: String,
    pub label: String,
    /// "© 2023 …", "℗ 2023 …"
    pub copyrights: Vec<String>,
    pub artist_uri: Option<String>,
    pub artist_name: String,
}

/// One entry of Friend Activity: what a friend is (or was last) listening to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Friend {
    pub name: String,
    pub user_uri: String,
    pub image: Option<String>,
    pub track_uri: String,
    pub track: String,
    pub artist: String,
    pub artist_uri: Option<String>,
    pub context: String,
    pub context_uri: Option<String>,
    /// When they played it (Unix ms).
    pub ts: i64,
}
