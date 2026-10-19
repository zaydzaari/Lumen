//! A personal library imported from Spotify's account-data JSON export.
//! No Spotify session, streaming credentials, or private endpoints are involved.
use crate::model::{Collection, CollectionKind, Track, TrackRef};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_TRACKS: usize = 50_000;

#[derive(Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub tracks: Vec<Track>,
}
impl Playlist {
    pub fn collection(&self) -> Collection {
        Collection {
            kind: CollectionKind::Playlist,
            uri: self.id.clone(),
            name: self.name.clone(),
            subtitle: "Imported · stored on this PC".into(),
            image: self.tracks.iter().find_map(|t| t.image.clone()),
            total_tracks: self.tracks.len() as u32,
            owned: false,
        }
    }
}

pub fn track_uri(raw: &str) -> Option<String> {
    let id = if let Some(id) = raw.strip_prefix("spotify:track:") {
        id.to_string()
    } else {
        let url = url::Url::parse(raw).ok()?;
        if url.scheme() != "https"
            || url.host_str() != Some("open.spotify.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port_or_known_default() != Some(443)
        {
            return None;
        }
        let segments: Vec<_> = url.path_segments()?.collect();
        match segments.as_slice() {
            ["track", id] => id.to_string(),
            [locale, "track", id] if locale.starts_with("intl-") => id.to_string(),
            _ => return None,
        }
    };
    (id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| format!("spotify:track:{id}"))
}

pub struct Import {
    pub playlists: Vec<Playlist>,
    pub skipped: usize,
}

/// Accept Spotify Playlist*.json and YourLibrary.json, plus Lumen's own backup.
pub fn parse(bytes: &[u8]) -> Result<Import> {
    anyhow::ensure!(bytes.len() <= MAX_BYTES, "Library file exceeds 64 MB");
    let data: serde_json::Value = serde_json::from_slice(bytes)
        .context("Choose an extracted Spotify playlist/library JSON file")?;
    let native = data.is_array();
    let lists = if let Some(lists) = data.get("playlists").and_then(|v| v.as_array()) {
        lists.clone()
    } else if let Some(lists) = data.as_array() {
        lists.clone()
    } else if data.get("tracks").is_some() {
        vec![serde_json::json!({"name":"Imported Liked Songs", "tracks": data["tracks"]})]
    } else {
        anyhow::bail!("No playlists or saved tracks found in this JSON file");
    };
    anyhow::ensure!(
        lists.len() <= 1000,
        "Import at most 1,000 playlists per file"
    );
    let mut names = HashMap::<String, usize>::new();
    let mut playlists = Vec::new();
    let mut skipped = 0;
    let mut total = 0;
    for list in lists {
        let name = list
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Imported playlist")
            .trim();
        anyhow::ensure!(
            !name.is_empty() && name.len() <= 4096,
            "Invalid playlist name"
        );
        let occurrence = names.entry(name.to_owned()).or_default();
        let id = format!(
            "lumen:playlist:{:x}",
            Sha256::digest(format!("{name}\0{occurrence}"))
        );
        *occurrence += 1;
        let id = if native {
            list.get("id")
                .and_then(|v| v.as_str())
                .filter(|id| {
                    id.starts_with("lumen:playlist:")
                        && id.len() <= 256
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b':' || b == b'-')
                })
                .unwrap_or(&id)
                .to_string()
        } else {
            id
        };
        let items = list
            .get("items")
            .or_else(|| list.get("tracks"))
            .and_then(|v| v.as_array())
            .context("Playlist does not contain a track list")?;
        let mut tracks = Vec::new();
        for item in items {
            total += 1;
            anyhow::ensure!(total <= MAX_TRACKS, "Import at most 50,000 tracks per file");
            let item = item.get("track").filter(|v| v.is_object()).unwrap_or(item);
            let parsed = if native {
                serde_json::from_value::<Track>(item.clone()).ok()
            } else {
                let uri = item
                    .get("trackUri")
                    .or_else(|| item.get("uri"))
                    .and_then(|v| v.as_str())
                    .and_then(track_uri);
                uri.map(|uri| Track {
                    uri,
                    name: item
                        .get("trackName")
                        .or_else(|| item.get("name"))
                        .or_else(|| item.get("track"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    artists: item
                        .get("artistName")
                        .or_else(|| item.get("artist"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    album: item
                        .get("albumName")
                        .or_else(|| item.get("album"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    duration_ms: item
                        .get("duration_ms")
                        .and_then(|v| v.as_u64())
                        .and_then(|v| v.try_into().ok())
                        .unwrap_or(0),
                    ..Default::default()
                })
            };
            if let Some(track) = parsed.filter(|t| {
                (track_uri(&t.uri).is_some() || (native && crate::providers::valid_uri(&t.uri)))
                    && !t.is_stub()
                    && t.name.len() <= 4096
                    && t.artists.len() <= 4096
                    && t.album.len() <= 4096
            }) {
                tracks.push(track);
            } else {
                skipped += 1;
            }
        }
        playlists.push(Playlist {
            id,
            name: name.to_string(),
            tracks,
        });
    }
    anyhow::ensure!(
        native || playlists.iter().any(|p| !p.tracks.is_empty()),
        "No usable Spotify music tracks found (podcasts and local-file entries are skipped)"
    );
    Ok(Import { playlists, skipped })
}

pub fn read(path: &Path) -> Result<Import> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).context("Could not open library file")?;
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    parse(&bytes)
}

pub fn save(path: &Path, playlists: &[Playlist]) -> Result<()> {
    let bytes = serde_json::to_vec(playlists)?;
    anyhow::ensure!(
        bytes.len() <= MAX_BYTES
            && playlists.len() <= 1000
            && playlists.iter().map(|p| p.tracks.len()).sum::<usize>() <= MAX_TRACKS,
        "Imported library is full"
    );
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path).context("Could not save imported library")
}

pub fn search(
    playlists: &[Playlist],
    extra: impl Iterator<Item = TrackRef>,
    query: &str,
) -> Vec<TrackRef> {
    let q = query.trim().to_lowercase();
    let uri = track_uri(query.trim());
    let mut seen = HashSet::new();
    playlists
        .iter()
        .flat_map(|p| p.tracks.iter().cloned().map(Arc::new))
        .chain(extra)
        .filter(|t| {
            (uri.as_deref() == Some(t.uri.as_str())
                || format!("{} {} {}", t.name, t.artists, t.album)
                    .to_lowercase()
                    .contains(&q))
                && seen.insert(t.uri.clone())
        })
        .take(500)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "0123456789abcdefghijkl";
    #[test]
    fn native_provider_library_survives_restart() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("library.json");
        let playlists = vec![Playlist {
            id: "lumen:playlist:native:youtube:songs".into(),
            name: "Provider songs".into(),
            tracks: vec![Track {
                uri: "native:youtube:opaque-id".into(),
                name: "Song".into(),
                artists: "Artist".into(),
                ..Default::default()
            }],
        }];
        save(&path, &playlists).unwrap();
        let imported = read(&path).unwrap();
        assert_eq!(imported.playlists[0].tracks, playlists[0].tracks);
        assert_eq!(imported.playlists[0].id, playlists[0].id);
    }
    #[test]
    fn provider_playlist_identity_and_order_survive_restart() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("free-library.json");
        let playlists = vec![Playlist {
            id: format!("lumen:playlist:provider-track-{ID}"),
            name: "My playlist".into(),
            tracks: vec![Track {
                uri: format!("spotify:track:{ID}"),
                name: "Song".into(),
                ..Default::default()
            }],
        }];
        save(&path, &playlists).unwrap();
        let imported = read(&path).unwrap();
        assert_eq!(imported.playlists[0].id, playlists[0].id);
        assert_eq!(imported.playlists[0].tracks, playlists[0].tracks);
    }

    #[test]
    fn imports_export_with_duplicates_and_skips_episodes() {
        let json = serde_json::json!({"playlists":[{"name":"My mix","items":[
            {"track":{"trackUri":format!("spotify:track:{ID}"),"trackName":"Song","artistName":"Artist","albumName":"Album"}},
            {"episode":{"episodeName":"Podcast"}},
            {"track":{"trackUri":format!("spotify:track:{ID}"),"trackName":"Song","artistName":"Artist"}}
        ]}]});
        let result = parse(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(result.skipped, 1);
        assert_eq!(result.playlists[0].tracks.len(), 2);
        assert_eq!(result.playlists[0].tracks[0].duration_ms, 0);
        assert_eq!(
            search(&result.playlists, std::iter::empty(), "artist").len(),
            1
        );
        let roundtrip = parse(&serde_json::to_vec(&result.playlists).unwrap()).unwrap();
        assert_eq!(roundtrip.playlists[0].id, result.playlists[0].id);
    }
    #[test]
    fn imports_saved_tracks_and_validates_links() {
        let result = parse(format!(r#"{{"tracks":[{{"uri":"spotify:track:{ID}","track":"Song","artist":"Artist","album":"Album"}}]}}"#).as_bytes());
        // Spotify's saved-library export calls the title "track".
        assert!(result.is_ok());
        assert!(track_uri(&format!(
            "https://open.spotify.com/intl-fr/track/{ID}?si=abc"
        ))
        .is_some());
        assert!(track_uri(&format!("https://open.spotify.com.evil.test/track/{ID}")).is_none());
        assert!(track_uri("spotify:track:../../file").is_none());
        assert!(parse(b"{\"playlists\":[]}").is_err());
    }
}
