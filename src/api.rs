//! Spotify Web API access (direct HTTP via `reqwest`) plus librespot-metadata
//! fallbacks for data the Web API no longer returns to development-mode apps
//! (e.g. the tracks of playlists the user doesn't own, artist top tracks).
//!
//! Responses are deserialised into minimal private structs and immediately mapped
//! into the compact types in [`crate::model`].

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use futures_util::StreamExt as _;
use librespot_core::{Session, SpotifyUri};
use librespot_metadata::Metadata as _;
use protobuf::Message as _;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::auth::TokenManager;
use crate::model::{
    ArtistInfo, Collection, CollectionKind, Page, Profile, SearchResults, Track, TrackRef, UserInfo,
};

const BASE: &str = "https://api.spotify.com/v1";
const PAGE: u32 = 50;

pub struct Api {
    http: reqwest::Client,
    token: Arc<TokenManager>,
    session: Session,
    me: tokio::sync::OnceCell<UserInfo>,
    cooldown: tokio::sync::Mutex<Option<std::time::Instant>>,
    base: String,
}

/// Outcome of a Web API GET that may be forbidden for dev-mode apps.
enum Fetch<T> {
    Ok(T),
    Forbidden,
}

impl Api {
    pub fn new(http: reqwest::Client, token: Arc<TokenManager>, session: Session) -> Arc<Self> {
        Arc::new(Self {
            http,
            token,
            session,
            me: tokio::sync::OnceCell::new(),
            cooldown: tokio::sync::Mutex::new(None),
            base: BASE.into(),
        })
    }

    pub fn session_browsing(&self) -> bool {
        self.token.uses_session_token()
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        match self.get_inner(path).await? {
            Fetch::Ok(v) => Ok(v),
            Fetch::Forbidden => bail!("Spotify refused this request (403)"),
        }
    }

    /// GET with one retry on 401 (token refresh) and up to two waits on 429.
    async fn get_inner<T: DeserializeOwned>(&self, path: &str) -> Result<Fetch<T>> {
        if self.session_browsing() {
            bail!("Connect your Spotify library in Settings to use this feature. Playback, playlists and track search are available with your Spotify session.");
        }
        if self
            .cooldown
            .lock()
            .await
            .is_some_and(|until| until > std::time::Instant::now())
        {
            bail!("Spotify is rate limiting your library. Please wait before retrying.");
        }
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{path}", self.base)
        };
        let mut refreshed = false;
        loop {
            let token = self.token.get().await?;
            let resp = self
                .http
                .get(&url)
                .bearer_auth(&token)
                .timeout(Duration::from_secs(20))
                .send()
                .await
                .with_context(|| format!("GET {path}"))?;
            match resp.status() {
                s if s.is_success() => {
                    let bytes = resp.bytes().await?;
                    return serde_json::from_slice(&bytes)
                        .map(Fetch::Ok)
                        .with_context(|| format!("unexpected response from {path}"));
                }
                StatusCode::UNAUTHORIZED if !refreshed => {
                    refreshed = true;
                    self.token.invalidate().await;
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    let wait = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(30)
                        .clamp(1, 86400);
                    *self.cooldown.lock().await =
                        Some(std::time::Instant::now() + Duration::from_secs(wait));
                    bail!("Spotify is rate limiting your library. Try again in {wait} seconds.");
                }
                StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => return Ok(Fetch::Forbidden),
                s => {
                    let body = resp.text().await.unwrap_or_default();
                    bail!(
                        "GET {path} failed: {s} {}",
                        body.chars().take(200).collect::<String>()
                    );
                }
            }
        }
    }

    async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: serde_json::Value,
    ) -> Result<()> {
        let token = self.token.get().await?;
        let resp = self
            .http
            .request(method, format!("{BASE}{path}"))
            .bearer_auth(token)
            .json(&body)
            .timeout(Duration::from_secs(20))
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("{path} failed: {}", resp.status());
        }
        Ok(())
    }

    pub async fn me(&self) -> Result<UserInfo> {
        if self.session_browsing() {
            let id = self.session.username();
            // Spotify's own profile service gives the real display name and
            // photo without a developer app.
            let p = self.profile(&id).await.ok();
            return Ok(UserInfo {
                display_name: p
                    .as_ref()
                    .map(|p| p.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| id.clone()),
                avatar: p.and_then(|p| p.image),
                id,
            });
        }
        self.me
            .get_or_try_init(|| async {
                let u: JUser = self.get("/me").await?;
                Ok::<_, anyhow::Error>(UserInfo {
                    display_name: u.display_name.unwrap_or_else(|| u.id.clone()),
                    avatar: pick_image(&u.images, 64),
                    id: u.id,
                })
            })
            .await
            .cloned()
    }

    /// Public profile of `username` through Spotify's profile service (the same
    /// one the official clients use; works with the streaming session alone).
    pub async fn profile(&self, username: &str) -> Result<Profile> {
        let bytes = tokio::time::timeout(
            Duration::from_secs(15),
            self.session
                .spclient()
                .get_user_profile(username, Some(50), Some(12)),
        )
        .await
        .context("profile request timed out")??;
        parse_profile(&bytes, username)
    }

    /// Spotify's own lyrics for a track (`None` if Spotify has none).
    pub async fn spotify_lyrics(
        &self,
        uri: &str,
    ) -> Result<Option<(crate::lyrics::Lyrics, String)>> {
        let SpotifyUri::Track { id } = SpotifyUri::from_uri(uri)? else {
            return Ok(None);
        };
        let r = tokio::time::timeout(
            Duration::from_secs(10),
            self.session.spclient().get_lyrics(&id),
        )
        .await
        .context("lyrics request timed out")?;
        match r {
            Ok(bytes) => Ok(crate::lyrics::parse_spotify(&bytes)),
            // 404 = Spotify has no lyrics for this song.
            Err(e) if e.kind == librespot_core::error::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // ------------------------------------------------------------ library

    pub async fn my_playlists(&self, offset: u32) -> Result<Page<Collection>> {
        if self.session_browsing() {
            return self.playlists_via_session(offset).await;
        }
        let me = self.me().await.ok();
        let p: JPage<Option<JPlaylist>> = self
            .get(&format!("/me/playlists?limit={PAGE}&offset={offset}"))
            .await?;
        let items = p
            .items
            .into_iter()
            .flatten()
            .map(|pl| map_playlist(pl, me.as_ref()))
            .collect();
        Ok(page(items, offset, p.total, p.next.is_some()))
    }

    pub async fn saved_tracks(&self, offset: u32) -> Result<Page<TrackRef>> {
        if self.session_browsing() {
            let uri = format!("spotify:user:{}:collection", self.session.username());
            return self.context_tracks(&uri, offset, 50).await;
        }
        let p: JPage<JSavedTrack> = self
            .get(&format!("/me/tracks?limit={PAGE}&offset={offset}"))
            .await?;
        let items = p
            .items
            .into_iter()
            .filter_map(|s| s.track)
            .map(map_track)
            .collect();
        Ok(page(items, offset, p.total, p.next.is_some()))
    }

    pub async fn saved_albums(&self, offset: u32) -> Result<Page<Collection>> {
        if self.session_browsing() {
            // One shot: the collection service returns everything.
            if offset > 0 {
                return Ok(page(Vec::new(), offset, offset, false));
            }
            let items = self.library_via_session(CollectionKind::Album).await?;
            let n = items.len() as u32;
            return Ok(page(items, 0, n, false));
        }
        let p: JPage<JSavedAlbum> = self
            .get(&format!("/me/albums?limit={PAGE}&offset={offset}"))
            .await?;
        let items = p.items.into_iter().map(|a| map_album(a.album)).collect();
        Ok(page(items, offset, p.total, p.next.is_some()))
    }

    /// Followed artists (cursor based); returns items and the next cursor.
    pub async fn followed_artists(
        &self,
        after: Option<&str>,
    ) -> Result<(Vec<Collection>, Option<String>, u32)> {
        if self.session_browsing() {
            if after.is_some() {
                return Ok((Vec::new(), None, 0));
            }
            let items = self.library_via_session(CollectionKind::Artist).await?;
            let n = items.len() as u32;
            return Ok((items, None, n));
        }
        let mut path = format!("/me/following?type=artist&limit={PAGE}");
        if let Some(a) = after {
            path.push_str("&after=");
            path.push_str(a);
        }
        let r: JFollowing = self.get(&path).await?;
        let next = r
            .artists
            .cursors
            .and_then(|c| c.after)
            .filter(|_| r.artists.next.is_some());
        let items = r.artists.items.into_iter().map(map_artist).collect();
        Ok((items, next, r.artists.total))
    }

    pub async fn recently_played(&self) -> Result<Vec<TrackRef>> {
        let r: JRecent = self.get("/me/player/recently-played?limit=50").await?;
        let mut seen = std::collections::HashSet::new();
        Ok(r.items
            .into_iter()
            .map(|i| map_track(i.track))
            .filter(|t| seen.insert(t.uri.clone()))
            .collect())
    }

    pub async fn top_tracks(&self) -> Result<Vec<TrackRef>> {
        let r: JPage<JTrack> = self
            .get("/me/top/tracks?limit=30&time_range=short_term")
            .await?;
        Ok(r.items.into_iter().map(map_track).collect())
    }

    pub async fn top_artists(&self) -> Result<Vec<Collection>> {
        let r: JPage<JArtist> = self
            .get("/me/top/artists?limit=20&time_range=medium_term")
            .await?;
        Ok(r.items.into_iter().map(map_artist).collect())
    }

    // ------------------------------------------------------------ collections

    /// Playlist tracks via the Web API. `Ok(None)` means Spotify withheld the items
    /// (playlist not owned by the user) and the caller should use
    /// [`Api::playlist_uris_via_session`].
    pub async fn playlist_tracks(
        &self,
        playlist_id: &str,
        offset: u32,
    ) -> Result<Option<Page<TrackRef>>> {
        if self.session_browsing() {
            return Ok(None);
        }
        let path = format!(
            "/playlists/{playlist_id}/items?limit={PAGE}&offset={offset}&additional_types=track"
        );
        match self.get_inner::<JPage<JPlaylistItem>>(&path).await? {
            Fetch::Ok(p) => {
                let items = p
                    .items
                    .into_iter()
                    .filter_map(|i| i.item.or(i.track))
                    .filter(|t| t.kind.as_deref().unwrap_or("track") == "track")
                    .map(map_track)
                    .collect();
                Ok(Some(page(items, offset, p.total, p.next.is_some())))
            }
            Fetch::Forbidden => Ok(None),
        }
    }

    pub async fn album(&self, album_id: &str) -> Result<(Collection, Page<TrackRef>)> {
        if self.session_browsing() {
            let uri = SpotifyUri::from_uri(&format!("spotify:album:{album_id}"))?;
            let album = tokio::time::timeout(
                Duration::from_secs(15),
                librespot_metadata::Album::get(&self.session, &uri),
            )
            .await??;
            let image = album.covers.iter().min_by_key(|i| i.width).map(|i| {
                format!(
                    "https://i.scdn.co/image/{}",
                    i.id.to_base16().unwrap_or_default()
                )
            });
            let uris: Vec<_> = album
                .tracks()
                .filter_map(|t| t.to_uri().ok())
                .take(2000)
                .collect();
            let collection = Collection {
                kind: CollectionKind::Album,
                uri: format!("spotify:album:{album_id}"),
                name: album.name,
                subtitle: album
                    .artists
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                image,
                total_tracks: uris.len() as u32,
                owned: false,
            };
            let tracks = self
                .tracks_via_session(uris.into_iter().take(PAGE as usize).collect())
                .await;
            let page = page(
                tracks,
                0,
                collection.total_tracks,
                collection.total_tracks > PAGE,
            );
            return Ok((collection, page));
        }
        let a: JFullAlbum = self.get(&format!("/albums/{album_id}")).await?;
        let images = a.images.clone();
        let col = Collection {
            kind: CollectionKind::Album,
            uri: a.uri.clone(),
            name: a.name.clone(),
            subtitle: join_artists(&a.artists),
            image: pick_image(&images, 64),
            total_tracks: a.tracks.total,
            owned: false,
        };
        let tracks = a
            .tracks
            .items
            .into_iter()
            .map(|t| map_simple_track(t, &a.name, &a.uri, &images))
            .collect();
        let has_next = a.tracks.next.is_some();
        Ok((col.clone(), page(tracks, 0, col.total_tracks, has_next)))
    }

    pub async fn album_tracks(&self, album: &Collection, offset: u32) -> Result<Page<TrackRef>> {
        if self.session_browsing() {
            return self.context_tracks(&album.uri, offset, PAGE as usize).await;
        }
        let p: JPage<JSimpleTrack> = self
            .get(&format!(
                "/albums/{}/tracks?limit={PAGE}&offset={offset}",
                album.id()
            ))
            .await?;
        let img: Vec<JImage> = album
            .image
            .iter()
            .map(|u| JImage {
                url: u.clone(),
                width: Some(64),
            })
            .collect();
        let items = p
            .items
            .into_iter()
            .map(|t| map_simple_track(t, &album.name, &album.uri, &img))
            .collect();
        Ok(page(items, offset, p.total, p.next.is_some()))
    }

    /// An artist's releases (albums, singles & EPs, compilations, appears on),
    /// newest first within each group.
    pub async fn artist_albums(&self, artist_id: &str) -> Result<Vec<crate::model::Release>> {
        use crate::model::{Release, ReleaseGroup};
        if self.session_browsing() {
            let uri = SpotifyUri::from_uri(&format!("spotify:artist:{artist_id}"))?;
            let artist = tokio::time::timeout(
                Duration::from_secs(15),
                librespot_metadata::Artist::get(&self.session, &uri),
            )
            .await??;
            let pick =
                |groups: &librespot_metadata::artist::AlbumGroups, n: usize| -> Vec<String> {
                    groups
                        .iter()
                        .filter_map(|group| group.first())
                        .filter_map(|uri| uri.to_uri().ok())
                        .take(n)
                        .collect()
                };
            let mut wanted: Vec<(ReleaseGroup, String)> = Vec::new();
            for (g, uris) in [
                (ReleaseGroup::Album, pick(&artist.albums, 30)),
                (ReleaseGroup::Single, pick(&artist.singles, 30)),
                (ReleaseGroup::Compilation, pick(&artist.compilations, 10)),
                (ReleaseGroup::AppearsOn, pick(&artist.appears_on_albums, 12)),
            ] {
                wanted.extend(uris.into_iter().map(|u| (g, u)));
            }
            let mut out: Vec<Release> = futures_util::stream::iter(wanted)
                .map(|(group, uri)| async move {
                    let id = SpotifyUri::from_uri(&uri).ok()?;
                    let album = tokio::time::timeout(
                        Duration::from_secs(10),
                        librespot_metadata::Album::get(&self.session, &id),
                    )
                    .await
                    .ok()?
                    .ok()?;
                    let year = album.date.as_utc().year();
                    use librespot_metadata::album::AlbumType;
                    let kind = match album.album_type {
                        AlbumType::SINGLE => "Single",
                        AlbumType::EP => "EP",
                        AlbumType::COMPILATION => "Compilation",
                        _ => "Album",
                    };
                    let subtitle = if group == ReleaseGroup::AppearsOn {
                        album
                            .artists
                            .iter()
                            .map(|a| a.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    } else {
                        format!("{year} • {kind}")
                    };
                    let mut covers: Vec<_> = album.covers.iter().collect();
                    covers.sort_by_key(|i| i.width);
                    let image = covers
                        .iter()
                        .find(|i| i.width >= 200)
                        .or(covers.last())
                        .map(|i| {
                            format!(
                                "https://i.scdn.co/image/{}",
                                i.id.to_base16().unwrap_or_default()
                            )
                        });
                    Some(Release {
                        group,
                        year,
                        popularity: album.popularity,
                        coll: Collection {
                            kind: CollectionKind::Album,
                            uri,
                            name: album.name,
                            subtitle,
                            image,
                            total_tracks: album.discs.iter().map(|d| d.tracks.len() as u32).sum(),
                            owned: false,
                        },
                    })
                })
                .buffered(8)
                .filter_map(|r| async move { r })
                .collect()
                .await;
            out.sort_by_key(|r| (r.group as u8, std::cmp::Reverse(r.year)));
            return Ok(out);
        }
        let p: JPage<JSimpleAlbum> = self
            .get(&format!(
                "/artists/{artist_id}/albums?include_groups=album,single,compilation,appears_on&limit=50"
            ))
            .await?;
        let mut out: Vec<crate::model::Release> = p
            .items
            .into_iter()
            .map(|a| {
                let year: i32 = a
                    .release_date
                    .as_deref()
                    .and_then(|d| d.get(..4))
                    .and_then(|y| y.parse().ok())
                    .unwrap_or(0);
                let group = match a.album_group.as_deref() {
                    Some("single") => ReleaseGroup::Single,
                    Some("compilation") => ReleaseGroup::Compilation,
                    Some("appears_on") => ReleaseGroup::AppearsOn,
                    _ => ReleaseGroup::Album,
                };
                let kind = match a.album_type.as_deref() {
                    Some("single") if a.total_tracks > 3 => "EP",
                    Some("single") => "Single",
                    Some("compilation") => "Compilation",
                    _ => "Album",
                };
                let mut coll = map_album(a);
                if group != ReleaseGroup::AppearsOn {
                    coll.subtitle = format!("{year} • {kind}");
                }
                crate::model::Release {
                    group,
                    year,
                    popularity: 0,
                    coll,
                }
            })
            .collect();
        out.sort_by_key(|r| (r.group as u8, std::cmp::Reverse(r.year)));
        Ok(out)
    }

    // ------------------------------------------------------------ search

    pub async fn search(&self, query: &str) -> Result<SearchResults> {
        if self.session_browsing() {
            let q = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
            let page = self
                .context_tracks(&format!("spotify:search:{q}"), 0, 30)
                .await?;
            return Ok(SearchResults {
                query: query.into(),
                tracks: page.items,
                ..SearchResults::default()
            });
        }
        let q: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        // Development-mode apps are limited to 10 results per type (Feb 2026).
        let r: JSearch = self
            .get(&format!(
                "/search?q={q}&type=track,album,artist,playlist&limit=10"
            ))
            .await?;
        let me = self.me().await.ok();
        Ok(SearchResults {
            query: query.to_string(),
            tracks: r
                .tracks
                .map(|p| p.items.into_iter().map(map_track).collect())
                .unwrap_or_default(),
            albums: r
                .albums
                .map(|p| p.items.into_iter().map(map_album).collect())
                .unwrap_or_default(),
            artists: r
                .artists
                .map(|p| p.items.into_iter().map(map_artist).collect())
                .unwrap_or_default(),
            playlists: r
                .playlists
                .map(|p| {
                    p.items
                        .into_iter()
                        .flatten()
                        .map(|pl| map_playlist(pl, me.as_ref()))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// Save tracks to Liked Songs.
    pub async fn save_to_library(&self, uris: &[String]) -> Result<()> {
        if self.session_browsing() {
            return self.collection_write("collection", uris, true).await;
        }
        let joined = uris.join(",");
        let q: String = url::form_urlencoded::byte_serialize(joined.as_bytes()).collect();
        self.send_json(
            reqwest::Method::PUT,
            &format!("/me/library?uris={q}"),
            serde_json::json!({}),
        )
        .await
    }

    /// Remove tracks from Liked Songs (counterpart of [`Self::save_to_library`]).
    pub async fn remove_from_library(&self, uris: &[String]) -> Result<()> {
        if self.session_browsing() {
            return self.collection_write("collection", uris, false).await;
        }
        let joined = uris.join(",");
        let q: String = url::form_urlencoded::byte_serialize(joined.as_bytes()).collect();
        self.send_json(
            reqwest::Method::DELETE,
            &format!("/me/library?uris={q}"),
            serde_json::json!({}),
        )
        .await
    }

    /// Whether each URI is in the user's library (Liked Songs for tracks).
    pub async fn library_contains(&self, uris: &[String]) -> Result<Vec<bool>> {
        let joined = uris.join(",");
        let q: String = url::form_urlencoded::byte_serialize(joined.as_bytes()).collect();
        let v: Vec<bool> = self.get(&format!("/me/library/contains?uris={q}")).await?;
        if v.len() != uris.len() {
            bail!("unexpected library/contains response");
        }
        Ok(v)
    }

    // ------------------------------------------------------------ collection service
    //
    // Spotify's own "Your Library" storage (sets: "collection" = liked songs
    // and saved albums, "artist" = followed artists). Works with the streaming
    // session alone, so liking songs, following artists and saved albums work
    // without a developer app.

    async fn collection_request(&self, endpoint: &str, body: Vec<u8>) -> Result<Vec<u8>> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static(
                "application/vnd.collection-v2.spotify.proto",
            ),
        );
        let bytes = tokio::time::timeout(
            Duration::from_secs(20),
            self.session.spclient().request(
                &reqwest::Method::POST,
                endpoint,
                Some(headers),
                Some(&body),
            ),
        )
        .await
        .context("request timed out")??;
        Ok(bytes.to_vec())
    }

    /// All URIs in a collection set (newest first), following pagination.
    pub async fn collection_uris(&self, set: &str) -> Result<Vec<String>> {
        let user = self.session.username();
        let mut out = Vec::new();
        let mut token = String::new();
        for _ in 0..40 {
            let body = crate::collection_pb::page_request(&user, set, &token, 300);
            let bytes = self
                .collection_request("/collection/v2/paging", body)
                .await?;
            let (items, next) = crate::collection_pb::parse_page(&bytes)
                .ok_or_else(|| anyhow!("unexpected collection response"))?;
            out.extend(items.into_iter().filter(|i| !i.is_removed).map(|i| i.uri));
            if next.is_empty() {
                break;
            }
            token = next;
        }
        Ok(out)
    }

    /// Add (`add = true`) or remove URIs from a collection set.
    pub async fn collection_write(&self, set: &str, uris: &[String], add: bool) -> Result<()> {
        let now = (now_ms() / 1000) as i32;
        let items: Vec<crate::collection_pb::Item> = uris
            .iter()
            .map(|u| crate::collection_pb::Item {
                uri: u.clone(),
                added_at: now,
                is_removed: !add,
            })
            .collect();
        let body = crate::collection_pb::write_request(&self.session.username(), set, &items);
        self.collection_request("/collection/v2/write", body)
            .await
            .map(|_| ())
    }

    /// Follow / unfollow an artist.
    pub async fn follow_artist(&self, uri: &str, follow: bool) -> Result<()> {
        if self.web_api() {
            let id = crate::model::uri_id(uri);
            let method = if follow {
                reqwest::Method::PUT
            } else {
                reqwest::Method::DELETE
            };
            if self
                .send_json_value(
                    method,
                    &format!("/me/following?type=artist&ids={id}"),
                    serde_json::json!({}),
                )
                .await
                .is_ok()
            {
                return Ok(());
            }
        }
        self.collection_write("artist", &[uri.to_string()], follow)
            .await
    }

    /// Save or remove an album in Your Library.
    pub async fn save_album(&self, uri: &str, add: bool) -> Result<()> {
        if self.web_api() {
            let id = crate::model::uri_id(uri);
            let method = if add {
                reqwest::Method::PUT
            } else {
                reqwest::Method::DELETE
            };
            if self
                .send_json_value(
                    method,
                    &format!("/me/albums?ids={id}"),
                    serde_json::json!({}),
                )
                .await
                .is_ok()
            {
                return Ok(());
            }
        }
        self.collection_write("collection", &[uri.to_string()], add)
            .await
    }

    /// Follow someone else's playlist (adds it to Your Library).
    pub async fn follow_playlist(&self, uri: &str) -> Result<()> {
        let id = crate::model::uri_id(uri);
        if self.web_api() {
            match self
                .send_json_value(
                    reqwest::Method::PUT,
                    &format!("/playlists/{id}/followers"),
                    serde_json::json!({}),
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) => tracing::warn!("Web API follow failed, using Spotify's service: {e:#}"),
            }
        }
        let full = format!("spotify:playlist:{id}");
        self.rootlist_change(vec![add_op(&[full], true)]).await
    }

    /// Saved albums or followed artists through the collection service, with
    /// names and pictures from Spotify's metadata.
    pub async fn library_via_session(&self, kind: CollectionKind) -> Result<Vec<Collection>> {
        let (set, prefix) = match kind {
            CollectionKind::Album => ("collection", "spotify:album:"),
            CollectionKind::Artist => ("artist", "spotify:artist:"),
            CollectionKind::Playlist => bail!("playlists come from the rootlist"),
        };
        let uris: Vec<String> = self
            .collection_uris(set)
            .await?
            .into_iter()
            .filter(|u| u.starts_with(prefix))
            .take(500)
            .collect();
        let items = futures_util::stream::iter(uris)
            .map(|uri| async move { self.collection_meta(&uri, kind).await.ok() })
            .buffered(8)
            .collect::<Vec<_>>()
            .await;
        Ok(items.into_iter().flatten().collect())
    }

    async fn collection_meta(&self, uri: &str, kind: CollectionKind) -> Result<Collection> {
        let id = SpotifyUri::from_uri(uri).map_err(|e| anyhow!("bad uri: {e}"))?;
        let img = |imgs: &librespot_metadata::image::Images| -> Option<String> {
            let mut v: Vec<_> = imgs.iter().collect();
            v.sort_by_key(|i| i.width);
            v.iter().find(|i| i.width >= 200).or(v.last()).map(|i| {
                format!(
                    "https://i.scdn.co/image/{}",
                    i.id.to_base16().unwrap_or_default()
                )
            })
        };
        Ok(match kind {
            CollectionKind::Album => {
                let a = tokio::time::timeout(
                    Duration::from_secs(10),
                    librespot_metadata::Album::get(&self.session, &id),
                )
                .await
                .context("timed out")?
                .map_err(|e| anyhow!("{e}"))?;
                Collection {
                    kind,
                    uri: uri.to_string(),
                    subtitle: a
                        .artists
                        .iter()
                        .map(|x| x.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    image: img(&a.covers),
                    total_tracks: a.discs.iter().map(|d| d.tracks.len() as u32).sum(),
                    owned: false,
                    name: a.name.clone(),
                }
            }
            _ => {
                let a = tokio::time::timeout(
                    Duration::from_secs(10),
                    librespot_metadata::Artist::get(&self.session, &id),
                )
                .await
                .context("timed out")?
                .map_err(|e| anyhow!("{e}"))?;
                Collection {
                    kind,
                    uri: uri.to_string(),
                    subtitle: "Artist".into(),
                    image: img(&a.portraits).or_else(|| {
                        a.portrait_group
                            .first()
                            .and_then(|_| img(&a.portrait_group))
                    }),
                    total_tracks: 0,
                    owned: false,
                    name: a.name.clone(),
                }
            }
        })
    }

    /// Bio, picture and "Fans also like" for an artist page.
    pub async fn artist_info(&self, uri: &str) -> Result<ArtistInfo> {
        let id = SpotifyUri::from_uri(uri).map_err(|e| anyhow!("bad artist uri: {e}"))?;
        let a = tokio::time::timeout(
            Duration::from_secs(15),
            librespot_metadata::Artist::get(&self.session, &id),
        )
        .await
        .context("timed out")?
        .map_err(|e| anyhow!("{e}"))?;
        let bio = a
            .biographies
            .iter()
            .map(|b| strip_tags(&b.text))
            .find(|t| !t.trim().is_empty());
        let portrait = self
            .collection_meta(uri, CollectionKind::Artist)
            .await
            .ok()
            .and_then(|c| c.image);
        let related_uris: Vec<String> = a
            .related
            .iter()
            .take(10)
            .filter_map(|r| r.id.to_uri().ok())
            .collect();
        let related = futures_util::stream::iter(related_uris)
            .map(|u| async move { self.collection_meta(&u, CollectionKind::Artist).await.ok() })
            .buffered(6)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .flatten()
            .collect();
        Ok(ArtistInfo {
            bio,
            portrait,
            related,
            popularity: a.popularity,
        })
    }

    // ------------------------------------------------------------ devices

    /// Watch Spotify Connect cluster updates (sent whenever any of your devices
    /// changes) and report the device list.
    pub fn watch_devices(&self, ui: crate::events::UiTx) {
        use librespot_core::dealer::protocol::Message;
        use librespot_protocol::connect::ClusterUpdate;
        let stream = match self.session.dealer().listen_for(
            "hm://connect-state/v1/cluster",
            Message::from_raw::<ClusterUpdate>,
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("device updates unavailable: {e}");
                return;
            }
        };
        let me = self.session.device_id().to_string();
        tokio::spawn(async move {
            let mut stream = stream;
            while let Some(update) = stream.next().await {
                let Ok(update) = update else { continue };
                let Some(cluster) = update.cluster.as_ref() else {
                    continue;
                };
                ui.send(crate::events::AppEvent::Devices(devices_of(cluster, &me)));
            }
        });
    }

    /// Move playback that runs on another device (or on Lumen's Connect
    /// device) to `to`.
    pub async fn transfer_playback(&self, from: &str, to: &str) -> Result<()> {
        tokio::time::timeout(
            Duration::from_secs(15),
            self.session.spclient().transfer(from, to, None),
        )
        .await
        .context("transfer timed out")??;
        Ok(())
    }

    /// Start `track` (inside `context`) at `position_ms` on device `to` — used
    /// to hand music that Lumen plays itself over to a phone or speaker.
    pub async fn play_on_device(
        &self,
        to: &str,
        context: &str,
        track: &str,
        position_ms: u32,
    ) -> Result<()> {
        let me = self.session.device_id().to_string();
        let body = serde_json::json!({
            "command": {
                "endpoint": "play",
                "context": { "uri": context, "url": format!("context://{context}"), "metadata": {} },
                "play_origin": { "feature_identifier": "lumen" },
                "options": {
                    "skip_to": { "track_uri": track },
                    "seek_to": position_ms,
                    "player_options_override": {}
                },
                "logging_params": {}
            }
        })
        .to_string();
        tokio::time::timeout(
            Duration::from_secs(15),
            self.session.spclient().request(
                &reqwest::Method::POST,
                &format!("/connect-state/v1/player/command/from/{me}/to/{to}"),
                None,
                Some(body.as_bytes()),
            ),
        )
        .await
        .context("request timed out")??;
        Ok(())
    }

    pub fn device_id(&self) -> String {
        self.session.device_id().to_string()
    }

    // ------------------------------------------------------------ radio

    /// Spotify's radio playlist for a song, artist, album or playlist
    /// (the "Go to song radio" mix). Returns the playlist URI.
    pub async fn radio_playlist(&self, seed_uri: &str) -> Result<String> {
        let bytes = tokio::time::timeout(
            Duration::from_secs(15),
            self.session.spclient().request_as_json(
                &reqwest::Method::GET,
                &format!("/inspiredby-mix/v2/seed_to_playlist/{seed_uri}?response-format=json"),
                None,
                None,
            ),
        )
        .await
        .context("radio request timed out")??;
        find_playlist_uri(&serde_json::from_slice(&bytes)?)
            .ok_or_else(|| anyhow!("Spotify has no radio for this"))
    }

    /// Friend Activity: what the people you follow are listening to.
    pub async fn friend_activity(&self) -> Result<Vec<crate::model::Friend>> {
        let bytes = tokio::time::timeout(
            Duration::from_secs(15),
            self.session.spclient().request_as_json(
                &reqwest::Method::GET,
                "/presence-view/v1/buddylist",
                None,
                None,
            ),
        )
        .await
        .context("friend activity timed out")??;
        Ok(parse_friends(&serde_json::from_slice(&bytes)?))
    }

    /// Release date, label and copyrights of an album.
    pub async fn album_info(&self, uri: &str) -> Result<crate::model::AlbumInfo> {
        let id = SpotifyUri::from_uri(uri).map_err(|e| anyhow!("bad album uri: {e}"))?;
        let a = tokio::time::timeout(
            Duration::from_secs(15),
            librespot_metadata::Album::get(&self.session, &id),
        )
        .await
        .context("timed out")?
        .map_err(|e| anyhow!("{e}"))?;
        let d = a.date.as_utc();
        let released = if u8::from(d.month()) == 1 && d.day() == 1 {
            d.year().to_string()
        } else {
            format!("{} {}, {}", d.month(), d.day(), d.year())
        };
        let copyrights = a
            .copyrights
            .iter()
            .map(|c| {
                let sign = match c.copyright_type {
                    librespot_metadata::copyright::CopyrightType::P => "\u{2117}",
                    _ => "©",
                };
                let t = c.text.trim();
                if t.starts_with('©')
                    || t.starts_with('℗')
                    || t.starts_with("(C)")
                    || t.starts_with("(P)")
                {
                    t.to_string()
                } else {
                    format!("{sign} {t}")
                }
            })
            .collect();
        let first = a.artists.first();
        Ok(crate::model::AlbumInfo {
            released,
            label: a.label.clone(),
            copyrights,
            artist_uri: first.and_then(|x| x.id.to_uri().ok()),
            artist_name: first.map(|x| x.name.clone()).unwrap_or_default(),
        })
    }

    /// Song credits (performers, writers, producers) from Spotify's credits view.
    pub async fn credits(&self, uri: &str) -> Result<crate::model::Credits> {
        let id = crate::model::uri_id(uri);
        let bytes = tokio::time::timeout(
            Duration::from_secs(15),
            self.session.spclient().request_as_json(
                &reqwest::Method::GET,
                &format!("/track-credits-view/v0/experimental/{id}/credits"),
                None,
                None,
            ),
        )
        .await
        .context("credits request timed out")??;
        parse_credits(&serde_json::from_slice(&bytes)?)
    }

    /// Songs for autoplay after `seed_uri` (radio of the last song, metadata
    /// loaded, the seed itself removed).
    pub async fn radio_tracks(&self, seed_uri: &str, max: usize) -> Result<Vec<TrackRef>> {
        let pl = self.radio_playlist(seed_uri).await?;
        let (_, uris) = self.playlist_uris_via_session(&pl).await?;
        let uris: Vec<String> = uris
            .into_iter()
            .filter(|u| u != seed_uri)
            .take(max)
            .collect();
        Ok(self
            .tracks_via_session(uris)
            .await
            .into_iter()
            .filter(|t| t.name != "Unavailable track")
            .collect())
    }

    // ------------------------------------------------------------ playlist editing
    //
    // With a developer client ID the documented Web API is used. Without one
    // (session browsing) Lumen speaks Spotify's own playlist service: the same
    // protobuf "list changes" the official apps send.

    async fn send_json_value(
        &self,
        method: reqwest::Method,
        path: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let token = self.token.get().await?;
        let resp = self
            .http
            .request(method, format!("{BASE}{path}"))
            .bearer_auth(token)
            .json(&body)
            .timeout(Duration::from_secs(20))
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!(
                "{path} failed: {status} {}",
                text.chars().take(160).collect::<String>()
            );
        }
        Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
    }

    fn web_api(&self) -> bool {
        !self.session_browsing()
    }

    /// Create a playlist in the user's library. Returns it as a collection.
    pub async fn create_playlist(&self, name: &str) -> Result<Collection> {
        let uri = if self.web_api() {
            match self
                .send_json_value(
                    reqwest::Method::POST,
                    "/me/playlists",
                    serde_json::json!({ "name": name }),
                )
                .await
            {
                Ok(v) => v["uri"].as_str().map(str::to_owned),
                Err(e) => {
                    tracing::warn!(
                        "Web API create playlist failed, using Spotify's service: {e:#}"
                    );
                    None
                }
            }
        } else {
            None
        };
        let uri = match uri {
            Some(u) => u,
            None => self.create_playlist_via_session(name).await?,
        };
        Ok(Collection {
            kind: CollectionKind::Playlist,
            uri: format!("spotify:playlist:{}", crate::model::uri_id(&uri)),
            name: name.to_string(),
            subtitle: self.session.username(),
            image: None,
            total_tracks: 0,
            owned: true,
        })
    }

    async fn create_playlist_via_session(&self, name: &str) -> Result<String> {
        use librespot_protocol::playlist4_external::{
            CreateListReply, ListAttributes, ListUpdateRequest,
        };
        let mut req = ListUpdateRequest::new();
        let mut attrs = ListAttributes::new();
        attrs.set_name(name.to_string());
        req.attributes = protobuf::MessageField::some(attrs);
        req.info = protobuf::MessageField::some(change_info());
        let bytes = self
            .spclient_proto(reqwest::Method::POST, "/playlist/v2/playlist", &req)
            .await
            .context("creating playlist")?;
        let reply = CreateListReply::parse_from_bytes(&bytes)?;
        let uri = reply.uri().to_string();
        anyhow::ensure!(!uri.is_empty(), "Spotify didn't return the new playlist");
        // New playlists only show up in the library once added to the rootlist.
        let op = add_op(std::slice::from_ref(&uri), true);
        self.rootlist_change(vec![op])
            .await
            .context("adding playlist to your library")?;
        Ok(uri)
    }

    /// Rename / change the description of a playlist (only fields that are `Some`).
    pub async fn edit_playlist(
        &self,
        uri: &str,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<()> {
        let id = crate::model::uri_id(uri);
        if self.web_api() {
            let mut body = serde_json::Map::new();
            if let Some(n) = name {
                body.insert("name".into(), n.into());
            }
            if let Some(d) = description {
                body.insert("description".into(), d.into());
            }
            match self
                .send_json_value(
                    reqwest::Method::PUT,
                    &format!("/playlists/{id}"),
                    body.into(),
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) => {
                    tracing::warn!("Web API edit playlist failed, using Spotify's service: {e:#}")
                }
            }
        }
        use librespot_protocol::playlist4_external::{
            op, ListAttributeKind, ListAttributes, ListAttributesPartialState, Op,
            UpdateListAttributes,
        };
        let mut values = ListAttributes::new();
        let mut no_value = Vec::new();
        if let Some(n) = name {
            values.set_name(n.to_string());
        }
        if let Some(d) = description {
            if d.is_empty() {
                no_value.push(protobuf::EnumOrUnknown::new(
                    ListAttributeKind::LIST_DESCRIPTION,
                ));
            } else {
                values.set_description(d.to_string());
            }
        }
        let mut state = ListAttributesPartialState::new();
        state.values = protobuf::MessageField::some(values);
        state.no_value = no_value;
        let mut upd = UpdateListAttributes::new();
        upd.new_attributes = protobuf::MessageField::some(state);
        let mut o = Op::new();
        o.set_kind(op::Kind::UPDATE_LIST_ATTRIBUTES);
        o.update_list_attributes = protobuf::MessageField::some(upd);
        self.playlist_change(uri, vec![o]).await
    }

    /// Add tracks to the end of a playlist.
    pub async fn add_to_playlist(&self, uri: &str, tracks: &[String]) -> Result<()> {
        let id = crate::model::uri_id(uri);
        if self.web_api() {
            let body = serde_json::json!({ "uris": tracks });
            let r = match self
                .send_json_value(
                    reqwest::Method::POST,
                    &format!("/playlists/{id}/items"),
                    body.clone(),
                )
                .await
            {
                Ok(_) => Ok(()),
                Err(_) => self
                    .send_json_value(
                        reqwest::Method::POST,
                        &format!("/playlists/{id}/tracks"),
                        body,
                    )
                    .await
                    .map(|_| ()),
            };
            match r {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::warn!("Web API add to playlist failed, using Spotify's service: {e:#}")
                }
            }
        }
        self.playlist_change(uri, vec![add_op(tracks, false)]).await
    }

    /// Remove every occurrence of the given tracks from a playlist.
    pub async fn remove_from_playlist(&self, uri: &str, tracks: &[String]) -> Result<()> {
        let id = crate::model::uri_id(uri);
        if self.web_api() {
            let items: Vec<serde_json::Value> = tracks
                .iter()
                .map(|t| serde_json::json!({ "uri": t }))
                .collect();
            let r = match self
                .send_json_value(
                    reqwest::Method::DELETE,
                    &format!("/playlists/{id}/items"),
                    serde_json::json!({ "items": items }),
                )
                .await
            {
                Ok(_) => Ok(()),
                Err(_) => self
                    .send_json_value(
                        reqwest::Method::DELETE,
                        &format!("/playlists/{id}/tracks"),
                        serde_json::json!({ "tracks": items }),
                    )
                    .await
                    .map(|_| ()),
            };
            match r {
                Ok(()) => return Ok(()),
                Err(e) => tracing::warn!(
                    "Web API remove from playlist failed, using Spotify's service: {e:#}"
                ),
            }
        }
        self.playlist_change(uri, vec![rem_op(tracks)]).await
    }

    /// Move the song at `from` so it lands at position `to` (indices in the
    /// playlist before the move, like Spotify's drag and drop).
    pub async fn move_in_playlist(&self, uri: &str, from: usize, to: usize) -> Result<()> {
        let id = crate::model::uri_id(uri);
        // Web API semantics: insert_before is an index in the original list.
        let insert_before = if to > from { to + 1 } else { to };
        if self.web_api() {
            let body = serde_json::json!({ "range_start": from, "insert_before": insert_before, "range_length": 1 });
            let r = match self
                .send_json_value(
                    reqwest::Method::PUT,
                    &format!("/playlists/{id}/items"),
                    body.clone(),
                )
                .await
            {
                Ok(_) => Ok(()),
                Err(_) => self
                    .send_json_value(
                        reqwest::Method::PUT,
                        &format!("/playlists/{id}/tracks"),
                        body,
                    )
                    .await
                    .map(|_| ()),
            };
            match r {
                Ok(()) => return Ok(()),
                Err(e) => tracing::warn!("Web API reorder failed, using Spotify's service: {e:#}"),
            }
        }
        use librespot_protocol::playlist4_external::{op, Mov, Op};
        let mut m = Mov::new();
        m.set_from_index(from as i32);
        m.set_length(1);
        m.set_to_index(insert_before as i32);
        let mut o = Op::new();
        o.set_kind(op::Kind::MOV);
        o.mov = protobuf::MessageField::some(m);
        self.playlist_change(uri, vec![o]).await
    }

    /// Remove a playlist from the library (deletes it if you own it, like Spotify).
    pub async fn delete_playlist(&self, uri: &str) -> Result<()> {
        let id = crate::model::uri_id(uri);
        if self.web_api() {
            match self
                .send_json_value(
                    reqwest::Method::DELETE,
                    &format!("/playlists/{id}/followers"),
                    serde_json::json!({}),
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) => tracing::warn!("Web API unfollow failed, using Spotify's service: {e:#}"),
            }
        }
        let full = format!("spotify:playlist:{id}");
        self.rootlist_change(vec![rem_op(&[full])]).await
    }

    async fn spclient_proto<M: protobuf::Message>(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        msg: &M,
    ) -> Result<Vec<u8>> {
        let body = msg.write_to_bytes()?;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/x-protobuf"),
        );
        let bytes = tokio::time::timeout(
            Duration::from_secs(20),
            self.session
                .spclient()
                .request(&method, endpoint, Some(headers), Some(&body)),
        )
        .await
        .context("request timed out")??;
        Ok(bytes.to_vec())
    }

    async fn playlist_change(
        &self,
        uri: &str,
        ops: Vec<librespot_protocol::playlist4_external::Op>,
    ) -> Result<()> {
        let id = crate::model::uri_id(uri);
        // Changes are applied on top of the current revision.
        let current = tokio::time::timeout(
            Duration::from_secs(20),
            self.session.spclient().request(
                &reqwest::Method::GET,
                &format!("/playlist/v2/playlist/{id}?decorate=revision"),
                None,
                None,
            ),
        )
        .await
        .context("request timed out")??;
        let rev = librespot_protocol::playlist4_external::SelectedListContent::parse_from_bytes(
            &current,
        )?
        .revision()
        .to_vec();
        let changes = list_changes(rev, ops);
        self.spclient_proto(
            reqwest::Method::POST,
            &format!("/playlist/v2/playlist/{id}/changes"),
            &changes,
        )
        .await
        .map(|_| ())
    }

    async fn rootlist_change(
        &self,
        ops: Vec<librespot_protocol::playlist4_external::Op>,
    ) -> Result<()> {
        let current = tokio::time::timeout(
            Duration::from_secs(20),
            self.session.spclient().get_rootlist(0, Some(1)),
        )
        .await
        .context("request timed out")??;
        let rev = librespot_protocol::playlist4_external::SelectedListContent::parse_from_bytes(
            &current,
        )?
        .revision()
        .to_vec();
        let user = self.session.username();
        self.spclient_proto(
            reqwest::Method::POST,
            &format!("/playlist/v2/user/{user}/rootlist/changes"),
            &list_changes(rev, ops),
        )
        .await
        .map(|_| ())
    }

    // ------------------------------------------------------------ librespot fallbacks

    async fn playlists_via_session(&self, offset: u32) -> Result<Page<Collection>> {
        let data = tokio::time::timeout(
            Duration::from_secs(20),
            self.session
                .spclient()
                .get_rootlist(offset as usize, Some(PAGE as usize)),
        )
        .await
        .context("playlist request timed out")??;
        map_rootlist(&data, offset, &self.session.username())
    }

    pub async fn playlist_image(&self, uri: &str) -> Result<Option<String>> {
        let id = SpotifyUri::from_uri(uri)?;
        let playlist = tokio::time::timeout(
            Duration::from_secs(10),
            librespot_metadata::Playlist::get(&self.session, &id),
        )
        .await??;
        if let Some(url) = playlist
            .attributes
            .picture_sizes
            .iter()
            .find_map(|picture| {
                let url = picture.url.as_str();
                if url.starts_with("https://") {
                    Some(url.to_owned())
                } else {
                    url.strip_prefix("spotify:image:")
                        .map(|id| format!("https://i.scdn.co/image/{id}"))
                }
            })
        {
            return Ok(Some(url));
        }
        if let Some(uri) = playlist.tracks().find_map(|track| {
            if matches!(track, SpotifyUri::Track { .. }) {
                track.to_uri().ok()
            } else {
                None
            }
        }) {
            return Ok(self.track_via_session(&uri).await?.image_large.clone());
        }
        Ok(None)
    }

    async fn context_tracks(&self, uri: &str, offset: u32, limit: usize) -> Result<Page<TrackRef>> {
        anyhow::ensure!(offset < 20_000, "Library view is limited to 20,000 tracks");
        let context = tokio::time::timeout(
            Duration::from_secs(20),
            self.session.spclient().get_context(uri),
        )
        .await
        .context("track request timed out")??;
        let mut uris = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut next = None;
        for page in context.pages {
            next = page.next_page_url.clone().filter(|u| !u.is_empty());
            for track in page.tracks {
                if track.uri().starts_with("spotify:track:") && seen.insert(track.uri().to_owned())
                {
                    uris.push(track.uri().to_owned());
                }
            }
        }
        let needed = offset as usize + limit + 1;
        let mut requests = 0;
        while uris.len() < needed && requests < 40 && uris.len() < 20_000 {
            let Some(url) = next.take() else {
                break;
            };
            anyhow::ensure!(
                url.starts_with("hm://") || url.starts_with("/"),
                "unsupported context page URL"
            );
            let data = tokio::time::timeout(
                Duration::from_secs(15),
                self.session.spclient().get_next_page(&url),
            )
            .await
            .context("track page timed out")??;
            let page: librespot_protocol::context_page::ContextPage =
                protobuf_json_mapping::parse_from_str(std::str::from_utf8(&data)?)?;
            next = page
                .next_page_url
                .clone()
                .filter(|u| !u.is_empty() && u != &url);
            for track in page.tracks {
                if track.uri().starts_with("spotify:track:") && seen.insert(track.uri().to_owned())
                {
                    uris.push(track.uri().to_owned());
                }
            }
            requests += 1;
        }
        uris.truncate(20_000);
        let has_next = (uris.len() > offset as usize + limit || next.is_some())
            && offset as usize + limit < 20_000;
        let total = uris.len() as u32;
        let tracks = self
            .tracks_via_session(uris.into_iter().skip(offset as usize).take(limit).collect())
            .await;
        let next = (has_next && !tracks.is_empty()).then_some(offset + tracks.len() as u32);
        Ok(Page {
            items: tracks,
            offset,
            total,
            next,
        })
    }

    /// All track URIs (and the name) of any playlist, via Spotify's internal API.
    pub async fn playlist_uris_via_session(&self, uri: &str) -> Result<(String, Vec<String>)> {
        let id = SpotifyUri::from_uri(uri).map_err(|e| anyhow!("bad playlist uri: {e}"))?;
        let pl = tokio::time::timeout(
            Duration::from_secs(20),
            librespot_metadata::Playlist::get(&self.session, &id),
        )
        .await
        .context("playlist tracks timed out")?
        .map_err(|e| anyhow!("loading playlist: {e}"))?;
        let uris = pl
            .tracks()
            .filter(|u| matches!(u, SpotifyUri::Track { .. }))
            .filter_map(|u| u.to_uri().ok())
            .collect();
        Ok((pl.attributes.name.clone(), uris))
    }

    /// Artist top tracks via Spotify's internal API (the Web API endpoint was
    /// removed for development-mode apps). Returns URIs only.
    pub async fn artist_top_tracks_via_session(&self, uri: &str) -> Result<(String, Vec<String>)> {
        let id = SpotifyUri::from_uri(uri).map_err(|e| anyhow!("bad artist uri: {e}"))?;
        let artist = librespot_metadata::Artist::get(&self.session, &id)
            .await
            .map_err(|e| anyhow!("loading artist: {e}"))?;
        let country = self.session.country();
        let top = artist
            .top_tracks
            .iter()
            .find(|t| t.country == country)
            .or_else(|| artist.top_tracks.first());
        let uris = top
            .map(|t| t.tracks.iter().filter_map(|u| u.to_uri().ok()).collect())
            .unwrap_or_default();
        Ok((artist.name.clone(), uris))
    }

    /// Resolve track metadata for URIs via librespot (8 concurrent requests).
    pub async fn tracks_via_session(&self, uris: Vec<String>) -> Vec<TrackRef> {
        futures_util::stream::iter(uris)
            .map(|uri| async move {
                match self.track_via_session(&uri).await {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::debug!("metadata for {uri}: {e:#}");
                        Arc::new(Track {
                            uri,
                            name: "Unavailable track".into(),
                            ..Default::default()
                        })
                    }
                }
            })
            .buffered(8)
            .collect()
            .await
    }

    async fn track_via_session(&self, uri: &str) -> Result<TrackRef> {
        let id = SpotifyUri::from_uri(uri).map_err(|e| anyhow!("bad track uri: {e}"))?;
        let t = tokio::time::timeout(
            Duration::from_secs(10),
            librespot_metadata::Track::get(&self.session, &id),
        )
        .await
        .context("track metadata timed out")?
        .map_err(|e| anyhow!("{e}"))?;
        let cover = |min: i32| -> Option<String> {
            let mut imgs: Vec<_> = t.album.covers.iter().collect();
            imgs.sort_by_key(|i| i.width);
            imgs.iter()
                .find(|i| i.width >= min)
                .or(imgs.last())
                .map(|i| {
                    format!(
                        "https://i.scdn.co/image/{}",
                        i.id.to_base16().unwrap_or_default()
                    )
                })
        };
        Ok(Arc::new(Track {
            uri: uri.to_string(),
            name: t.name.clone(),
            artists: t
                .artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            artist_uri: t.artists.first().and_then(|a| a.id.to_uri().ok()),
            album: t.album.name.clone(),
            album_uri: t.album.id.to_uri().ok(),
            duration_ms: t.duration.max(0) as u32,
            image: cover(64),
            image_large: cover(300),
            explicit: t.is_explicit,
        }))
    }
}

fn page<T>(items: Vec<T>, offset: u32, total: u32, has_next: bool) -> Page<T> {
    let next = if has_next {
        Some(offset + items.len().max(1) as u32)
    } else {
        None
    };
    Page {
        items,
        offset,
        total,
        next,
    }
}

fn map_rootlist(data: &[u8], offset: u32, username: &str) -> Result<Page<Collection>> {
    anyhow::ensure!(data.len() <= 8 << 20, "playlist response too large");
    let root = librespot_protocol::playlist4_external::SelectedListContent::parse_from_bytes(data)?;
    let contents = root.contents.get_or_default();
    let consumed = contents.items.len() as u32;
    let mut items = Vec::new();
    for (i, entry) in contents.items.iter().enumerate() {
        if !matches!(
            SpotifyUri::from_uri(entry.uri()),
            Ok(SpotifyUri::Playlist { .. })
        ) {
            continue;
        }
        let meta = contents.meta_items.get(i);
        let name = meta
            .and_then(|m| m.attributes.as_ref())
            .map(|a| a.name())
            .filter(|n| !n.is_empty())
            .unwrap_or("Playlist");
        let owner = meta.map(|m| m.owner_username()).unwrap_or_default();
        let image = meta.and_then(|m| m.attributes.as_ref()).and_then(|a| {
            a.picture_size.iter().find_map(|p| {
                let url = p.url();
                if url.starts_with("https://") {
                    Some(url.to_owned())
                } else {
                    url.strip_prefix("spotify:image:")
                        .map(|id| format!("https://i.scdn.co/image/{id}"))
                }
            })
        });
        items.push(Collection {
            kind: CollectionKind::Playlist,
            uri: format!("spotify:playlist:{}", crate::model::uri_id(entry.uri())),
            name: name.into(),
            subtitle: owner.into(),
            image,
            total_tracks: meta.map_or(0, |m| m.length().max(0) as u32),
            owned: owner == username,
        });
    }
    let total = root.length().max(0) as u32;
    let next = (consumed > 0 && (contents.truncated() || offset + consumed < total))
        .then_some(offset + consumed);
    Ok(Page {
        items,
        offset,
        total,
        next,
    })
}

// ---------------------------------------------------------------- JSON shapes

#[derive(Deserialize)]
struct JPage<T> {
    items: Vec<T>,
    #[serde(default)]
    total: u32,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Deserialize)]
struct JUser {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    images: Vec<JImage>,
}

#[derive(Deserialize, Clone)]
struct JImage {
    url: String,
    #[serde(default)]
    width: Option<u32>,
}

#[derive(Deserialize)]
struct JArtistRef {
    name: String,
    #[serde(default)]
    uri: Option<String>,
}

#[derive(Deserialize)]
struct JAlbumRef {
    name: String,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    images: Vec<JImage>,
}

#[derive(Deserialize)]
struct JTrack {
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    duration_ms: u32,
    #[serde(default)]
    explicit: bool,
    #[serde(default)]
    artists: Vec<JArtistRef>,
    #[serde(default)]
    album: Option<JAlbumRef>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct JSimpleTrack {
    uri: String,
    name: String,
    #[serde(default)]
    duration_ms: u32,
    #[serde(default)]
    explicit: bool,
    #[serde(default)]
    artists: Vec<JArtistRef>,
}

#[derive(Deserialize)]
struct JSavedTrack {
    track: Option<JTrack>,
}

#[derive(Deserialize)]
struct JPlaylistItem {
    /// Renamed from `track` in the February 2026 API update; accept both.
    #[serde(default)]
    item: Option<JTrack>,
    #[serde(default)]
    track: Option<JTrack>,
}

#[derive(Deserialize)]
struct JOwner {
    #[serde(default)]
    id: String,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct JCount {
    #[serde(default)]
    total: u32,
}

#[derive(Deserialize)]
struct JPlaylist {
    uri: String,
    name: String,
    #[serde(default)]
    images: Option<Vec<JImage>>,
    #[serde(default)]
    owner: Option<JOwner>,
    #[serde(default)]
    collaborative: bool,
    /// `items` since Feb 2026, `tracks` before.
    #[serde(default)]
    items: Option<JCount>,
    #[serde(default)]
    tracks: Option<JCount>,
}

#[derive(Deserialize)]
struct JSimpleAlbum {
    uri: String,
    name: String,
    #[serde(default)]
    album_group: Option<String>,
    #[serde(default)]
    album_type: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    images: Vec<JImage>,
    #[serde(default)]
    artists: Vec<JArtistRef>,
    #[serde(default)]
    total_tracks: u32,
}

#[derive(Deserialize)]
struct JSavedAlbum {
    album: JSimpleAlbum,
}

#[derive(Deserialize)]
struct JFullAlbum {
    uri: String,
    name: String,
    #[serde(default)]
    images: Vec<JImage>,
    #[serde(default)]
    artists: Vec<JArtistRef>,
    tracks: JPage<JSimpleTrack>,
}

#[derive(Deserialize)]
struct JArtist {
    uri: String,
    name: String,
    #[serde(default)]
    images: Vec<JImage>,
}

#[derive(Deserialize)]
struct JCursors {
    after: Option<String>,
}

#[derive(Deserialize)]
struct JArtistsCursorPage {
    items: Vec<JArtist>,
    #[serde(default)]
    next: Option<String>,
    #[serde(default)]
    total: u32,
    #[serde(default)]
    cursors: Option<JCursors>,
}

#[derive(Deserialize)]
struct JFollowing {
    artists: JArtistsCursorPage,
}

#[derive(Deserialize)]
struct JRecentItem {
    track: JTrack,
}

#[derive(Deserialize)]
struct JRecent {
    items: Vec<JRecentItem>,
}

#[derive(Deserialize)]
struct JSearch {
    #[serde(default)]
    tracks: Option<JPage<JTrack>>,
    #[serde(default)]
    albums: Option<JPage<JSimpleAlbum>>,
    #[serde(default)]
    artists: Option<JPage<JArtist>>,
    #[serde(default)]
    playlists: Option<JPage<Option<JPlaylist>>>,
}

// ---------------------------------------------------------------- mapping

/// Smallest image at least `min` px wide (Spotify lists images largest first).
fn devices_of(c: &librespot_protocol::connect::Cluster, me: &str) -> Vec<crate::model::Device> {
    let mut v: Vec<crate::model::Device> = c
        .device
        .iter()
        .filter(|(_, d)| !d.is_offline && d.can_play)
        .map(|(id, d)| crate::model::Device {
            id: id.clone(),
            name: if d.name.is_empty() {
                "Unknown device".into()
            } else {
                d.name.clone()
            },
            kind: format!("{:?}", d.device_type.enum_value_or_default()).to_lowercase(),
            active: *id == c.active_device_id,
            volume: d.volume,
        })
        .collect();
    // This computer first, then the active device, then by name.
    v.sort_by_key(|d| (d.id != me, !d.active, d.name.to_lowercase()));
    v
}

/// Plain text from Spotify's bio markup (`<a href=…>Name</a>` → `Name`).
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_string()
}

/// First `spotify:playlist:` URI anywhere in a JSON document.
fn parse_friends(v: &serde_json::Value) -> Vec<crate::model::Friend> {
    let s = |v: &serde_json::Value, k: &str| {
        v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    let mut out: Vec<crate::model::Friend> = v
        .get("friends")
        .and_then(|f| f.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    let user = f.get("user")?;
                    let track = f.get("track")?;
                    let artist = track.get("artist");
                    let context = track.get("context");
                    Some(crate::model::Friend {
                        name: s(user, "name"),
                        user_uri: s(user, "uri"),
                        image: user
                            .get("imageUrl")
                            .and_then(|x| x.as_str())
                            .filter(|x| !x.is_empty())
                            .map(str::to_string),
                        track_uri: s(track, "uri"),
                        track: s(track, "name"),
                        artist: artist.map(|a| s(a, "name")).unwrap_or_default(),
                        artist_uri: artist.map(|a| s(a, "uri")).filter(|u| !u.is_empty()),
                        context: context.map(|c| s(c, "name")).unwrap_or_default(),
                        context_uri: context.map(|c| s(c, "uri")).filter(|u| !u.is_empty()),
                        ts: f.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|f| std::cmp::Reverse(f.ts));
    out
}

fn parse_credits(v: &serde_json::Value) -> Result<crate::model::Credits> {
    let roles = v
        .get("roleCredits")
        .and_then(|r| r.as_array())
        .ok_or_else(|| anyhow!("no credits"))?;
    let mut out = crate::model::Credits::default();
    for r in roles {
        let title = r
            .get("roleTitle")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let people: Vec<crate::model::CreditPerson> = r
            .get("artists")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|p| {
                        let name = p.get("name")?.as_str()?.to_string();
                        let uri = p
                            .get("uri")
                            .and_then(|u| u.as_str())
                            .filter(|u| u.starts_with("spotify:artist:"))
                            .map(str::to_string);
                        let roles = p
                            .get("subroles")
                            .and_then(|s| s.as_array())
                            .map(|s| {
                                s.iter()
                                    .filter_map(|x| x.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default();
                        Some(crate::model::CreditPerson { name, uri, roles })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !people.is_empty() {
            out.roles.push((title, people));
        }
    }
    out.sources = v
        .get("sourceNames")
        .and_then(|s| s.as_array())
        .map(|s| {
            s.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(out)
}

fn find_playlist_uri(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) if s.starts_with("spotify:playlist:") => Some(s.clone()),
        serde_json::Value::Array(a) => a.iter().find_map(find_playlist_uri),
        serde_json::Value::Object(o) => o.values().find_map(find_playlist_uri),
        _ => None,
    }
}

fn change_info() -> librespot_protocol::playlist4_external::ChangeInfo {
    use librespot_protocol::playlist4_external::{source_info, ChangeInfo, SourceInfo};
    let mut src = SourceInfo::new();
    src.set_client(source_info::Client::CLIENT);
    let mut info = ChangeInfo::new();
    info.set_timestamp(now_ms());
    info.source = protobuf::MessageField::some(src);
    info
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn add_op(uris: &[String], first: bool) -> librespot_protocol::playlist4_external::Op {
    use librespot_protocol::playlist4_external::{op, Add, Item, ItemAttributes, Op};
    let mut add = Add::new();
    for u in uris {
        let mut item = Item::new();
        item.set_uri(u.clone());
        let mut a = ItemAttributes::new();
        a.set_timestamp(now_ms());
        item.attributes = protobuf::MessageField::some(a);
        add.items.push(item);
    }
    if first {
        add.set_add_first(true);
    } else {
        add.set_add_last(true);
    }
    let mut o = Op::new();
    o.set_kind(op::Kind::ADD);
    o.add = protobuf::MessageField::some(add);
    o
}

fn rem_op(uris: &[String]) -> librespot_protocol::playlist4_external::Op {
    use librespot_protocol::playlist4_external::{op, Item, Op, Rem};
    let mut rem = Rem::new();
    for u in uris {
        let mut item = Item::new();
        item.set_uri(u.clone());
        rem.items.push(item);
    }
    rem.set_items_as_key(true);
    let mut o = Op::new();
    o.set_kind(op::Kind::REM);
    o.rem = protobuf::MessageField::some(rem);
    o
}

fn list_changes(
    base_revision: Vec<u8>,
    ops: Vec<librespot_protocol::playlist4_external::Op>,
) -> librespot_protocol::playlist4_external::ListChanges {
    use librespot_protocol::playlist4_external::{Delta, ListChanges};
    let mut delta = Delta::new();
    delta.ops = ops;
    delta.info = protobuf::MessageField::some(change_info());
    let mut c = ListChanges::new();
    if !base_revision.is_empty() {
        c.set_base_revision(base_revision);
    }
    c.deltas.push(delta);
    c.set_want_resulting_revisions(false);
    c.set_want_sync_result(false);
    c
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JProfile {
    name: Option<String>,
    image_url: Option<String>,
    followers_count: Option<u32>,
    following_count: Option<u32>,
    total_public_playlists_count: Option<u32>,
    public_playlists: Vec<JProfileItem>,
    recently_played_artists: Vec<JProfileItem>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JProfileItem {
    uri: String,
    name: String,
    image_url: Option<String>,
    owner_name: Option<String>,
    owner_uri: Option<String>,
}

/// Image references in Spotify's internal services: https URLs,
/// `spotify:image:<id>` or `spotify:mosaic:<id>:<id>:…`.
pub fn image_ref_url(r: &str) -> Option<String> {
    let r = r.trim();
    if r.starts_with("https://") {
        Some(r.to_string())
    } else if let Some(id) = r.strip_prefix("spotify:image:") {
        Some(format!("https://i.scdn.co/image/{id}"))
    } else if let Some(ids) = r.strip_prefix("spotify:mosaic:") {
        let ids: Vec<&str> = ids.split(':').filter(|s| !s.is_empty()).collect();
        if ids.len() == 1 {
            Some(format!("https://i.scdn.co/image/{}", ids[0]))
        } else {
            Some(format!("https://mosaic.scdn.co/300/{}", ids.concat()))
        }
    } else {
        None
    }
}

fn parse_profile(bytes: &[u8], username: &str) -> Result<Profile> {
    anyhow::ensure!(bytes.len() <= 4 << 20, "profile response too large");
    let p: JProfile = serde_json::from_slice(bytes).context("unexpected profile response")?;
    let items = |v: Vec<JProfileItem>, kind: CollectionKind| -> Vec<Collection> {
        v.into_iter()
            .filter(|i| !i.uri.is_empty())
            .map(|i| {
                let owned = i
                    .owner_uri
                    .as_deref()
                    .is_some_and(|o| o.ends_with(&format!(":{username}")));
                Collection {
                    kind,
                    uri: i.uri,
                    name: i.name,
                    subtitle: match kind {
                        CollectionKind::Artist => "Artist".into(),
                        _ => i.owner_name.unwrap_or_default(),
                    },
                    image: i.image_url.as_deref().and_then(image_ref_url),
                    total_tracks: 0,
                    owned,
                }
            })
            .collect()
    };
    let public_playlists = items(p.public_playlists, CollectionKind::Playlist);
    Ok(Profile {
        name: p.name.unwrap_or_else(|| username.to_string()),
        image: p.image_url.as_deref().and_then(image_ref_url),
        followers: p.followers_count.unwrap_or(0),
        following: p.following_count.unwrap_or(0),
        total_public_playlists: p
            .total_public_playlists_count
            .unwrap_or(public_playlists.len() as u32),
        public_playlists,
        recent_artists: items(p.recently_played_artists, CollectionKind::Artist),
    })
}

fn pick_image(images: &[JImage], min: u32) -> Option<String> {
    let mut sorted: Vec<&JImage> = images.iter().collect();
    sorted.sort_by_key(|i| i.width.unwrap_or(0));
    sorted
        .iter()
        .find(|i| i.width.unwrap_or(0) >= min)
        .or(sorted.last())
        .map(|i| i.url.clone())
}

fn join_artists(a: &[JArtistRef]) -> String {
    a.iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn map_track(t: JTrack) -> TrackRef {
    let (album, album_uri, image, image_large) = match t.album {
        Some(a) => (
            a.name,
            a.uri,
            pick_image(&a.images, 64),
            pick_image(&a.images, 300),
        ),
        None => (String::new(), None, None, None),
    };
    Arc::new(Track {
        uri: t.uri.unwrap_or_default(),
        name: t.name,
        artists: join_artists(&t.artists),
        artist_uri: t.artists.first().and_then(|a| a.uri.clone()),
        album,
        album_uri,
        duration_ms: t.duration_ms,
        image,
        image_large,
        explicit: t.explicit,
    })
}

fn map_simple_track(t: JSimpleTrack, album: &str, album_uri: &str, images: &[JImage]) -> TrackRef {
    Arc::new(Track {
        uri: t.uri,
        name: t.name,
        artists: join_artists(&t.artists),
        artist_uri: t.artists.first().and_then(|a| a.uri.clone()),
        album: album.to_string(),
        album_uri: Some(album_uri.to_string()),
        duration_ms: t.duration_ms,
        image: pick_image(images, 64),
        image_large: pick_image(images, 300),
        explicit: t.explicit,
    })
}

fn map_playlist(p: JPlaylist, me: Option<&crate::model::UserInfo>) -> Collection {
    let owner = p.owner.as_ref();
    let owned = p.collaborative || me.zip(owner).is_some_and(|(m, o)| m.id == o.id);
    Collection {
        kind: CollectionKind::Playlist,
        uri: p.uri,
        name: p.name,
        subtitle: owner
            .and_then(|o| o.display_name.clone().or(Some(o.id.clone())))
            .map(|o| format!("by {o}"))
            .unwrap_or_default(),
        image: p.images.as_deref().and_then(|i| pick_image(i, 64)),
        total_tracks: p.items.or(p.tracks).map(|c| c.total).unwrap_or(0),
        owned,
    }
}

fn map_album(a: JSimpleAlbum) -> Collection {
    Collection {
        kind: CollectionKind::Album,
        uri: a.uri,
        name: a.name,
        subtitle: join_artists(&a.artists),
        image: pick_image(&a.images, 64),
        total_tracks: a.total_tracks,
        owned: false,
    }
}

fn map_artist(a: JArtist) -> Collection {
    Collection {
        kind: CollectionKind::Artist,
        uri: a.uri,
        name: a.name,
        subtitle: "Artist".into(),
        image: pick_image(&a.images, 64),
        total_tracks: 0,
        owned: false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn friends_parse_newest_first() {
        let v = serde_json::json!({"friends": [
            {"timestamp": 1000, "user": {"uri": "spotify:user:a", "name": "Amine", "imageUrl": ""},
             "track": {"uri": "spotify:track:1", "name": "Siniya",
                       "artist": {"uri": "spotify:artist:x", "name": "Ahl El Ghiwane"},
                       "context": {"uri": "spotify:playlist:p", "name": "Classics"}}},
            {"timestamp": 5000, "user": {"uri": "spotify:user:b", "name": "Sara", "imageUrl": "https://i/x"},
             "track": {"uri": "spotify:track:2", "name": "Other", "artist": {"uri": "", "name": "Y"}}}
        ]});
        let f = super::parse_friends(&v);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].name, "Sara");
        assert_eq!(f[0].artist_uri, None);
        assert_eq!(f[1].image, None);
        assert_eq!(f[1].context, "Classics");
    }

    #[test]
    fn credits_parse() {
        let v = serde_json::json!({
            "trackUri": "spotify:track:x",
            "roleCredits": [
                {"roleTitle": "Performers", "artists": [
                    {"uri": "spotify:artist:a", "name": "Ahl El Ghiwane", "subroles": ["main artist"]}
                ]},
                {"roleTitle": "Writers", "artists": [
                    {"uri": "", "name": "Larbi Batma", "subroles": ["composer", "lyricist"]}
                ]},
                {"roleTitle": "Producers", "artists": []}
            ],
            "sourceNames": ["Label X"]
        });
        let c = super::parse_credits(&v).unwrap();
        assert_eq!(c.roles.len(), 2);
        assert_eq!(c.roles[0].1[0].uri.as_deref(), Some("spotify:artist:a"));
        assert_eq!(c.roles[1].1[0].uri, None);
        assert_eq!(c.roles[1].1[0].roles, "composer, lyricist");
        assert_eq!(c.sources, ["Label X"]);
    }

    use super::*;

    #[tokio::test]
    #[ignore = "requires the user's saved Spotify login and live network"]
    async fn live_session_library_smoke() {
        let paths = crate::config::Paths::resolve().unwrap();
        let store = crate::auth::SecretStore::new(paths.secrets_dir());
        let credentials = crate::auth::decode_credentials(
            &store
                .load(crate::auth::KEY_SESSION)
                .expect("saved login required"),
        )
        .unwrap();
        let session = Session::new(Default::default(), None);
        tokio::time::timeout(Duration::from_secs(25), session.connect(credentials, false))
            .await
            .unwrap()
            .unwrap();
        let http = reqwest::Client::new();
        let token = TokenManager::with_session(http.clone(), store, session.clone());
        let api = Api::new(http, token, session.clone());
        let playlists = api.my_playlists(0).await.unwrap();
        eprintln!(
            "Session playlist count: {} (total root entries: {})",
            playlists.items.len(),
            playlists.total
        );
        eprintln!(
            "Playlist covers: {}",
            playlists.items.iter().filter(|p| p.image.is_some()).count()
        );
        if let Some(playlist) = playlists.items.iter().find(|p| p.image.is_none()) {
            eprintln!(
                "Fallback playlist cover: {}",
                api.playlist_image(&playlist.uri).await.unwrap().is_some()
            );
        }
        if let Some(playlist) = playlists.items.first() {
            let (_, uris) = api.playlist_uris_via_session(&playlist.uri).await.unwrap();
            eprintln!("First playlist track count: {}", uris.len());
        }
        let liked = api.saved_tracks(0).await.unwrap();
        eprintln!("Liked songs first page: {}", liked.items.len());
        if let Some(next) = liked.next {
            let more = api.saved_tracks(next).await.unwrap();
            eprintln!("Liked songs second page: {}", more.items.len());
            assert!(!more.items.is_empty());
        }
        let results = api.search("Daft Punk Get Lucky").await.unwrap();
        eprintln!("Search result tracks: {}", results.tracks.len());
        if let Some(track) = results.tracks.first() {
            crate::player::smoke_stream(&session, track).await.unwrap();
            eprintln!("Muted Spotify playback: started successfully");
            let album_uri = track.album_uri.as_deref().unwrap();
            let (_, tracks) = api.album(crate::model::uri_id(album_uri)).await.unwrap();
            eprintln!("Album first page: {}", tracks.items.len());
            let dir = tempfile::tempdir().unwrap();
            let cache = Arc::new(crate::cache::DiskCache::new(dir.path(), 4 << 20).unwrap());
            let (lyrics, _src) = crate::lyrics::fetch(&reqwest::Client::new(), cache, track, None)
                .await
                .unwrap();
            eprintln!(
                "Lyrics available: {}",
                lyrics != crate::lyrics::Lyrics::NotFound
            );
        }
        session.shutdown();
    }

    #[test]
    fn rootlist_pagination_uses_raw_entries_when_folders_are_filtered() {
        use librespot_protocol::playlist4_external::{
            Item, ListAttributes, ListItems, MetaItem, SelectedListContent,
        };
        let mut contents = ListItems::new();
        contents.set_pos(0);
        contents.set_truncated(true);
        let mut folder = Item::new();
        folder.set_uri("spotify:start-group:folder".into());
        let mut playlist = Item::new();
        playlist.set_uri("spotify:user:owner:playlist:3cEYpjA9oz9GiPac4AsH4n".into());
        contents.items = vec![folder, playlist];
        let mut attributes = ListAttributes::new();
        attributes.set_name("My playlist".into());
        let mut meta = MetaItem::new();
        meta.attributes = protobuf::MessageField::some(attributes);
        meta.set_owner_username("owner".into());
        meta.set_length(15);
        contents.meta_items = vec![MetaItem::new(), meta];
        let mut root = SelectedListContent::new();
        root.set_length(25);
        root.contents = protobuf::MessageField::some(contents);
        let mapped = map_rootlist(&root.write_to_bytes().unwrap(), 0, "owner").unwrap();
        assert_eq!(mapped.items.len(), 1);
        assert_eq!(mapped.next, Some(2));
        assert_eq!(mapped.items[0].name, "My playlist");
        assert!(mapped.items[0].owned);
        assert_eq!(
            mapped.items[0].uri,
            "spotify:playlist:3cEYpjA9oz9GiPac4AsH4n"
        );
    }

    #[tokio::test]
    async fn rate_limit_returns_immediately_and_prevents_retry_storms() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0; 2048];
            assert!(stream.read(&mut buf).await.unwrap() > 0);
            stream.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let http = reqwest::Client::new();
        let token = TokenManager::with_client(
            http.clone(),
            crate::auth::SecretStore::new(dir.path().into()),
            "test".into(),
            "refresh".into(),
            Some(("fake-token".into(), Duration::from_secs(3600))),
        );
        let mut api = Api::new(http, token, Session::new(Default::default(), None));
        Arc::get_mut(&mut api).unwrap().base = format!("http://{address}");
        let started = std::time::Instant::now();
        let first = api
            .get::<serde_json::Value>("/test")
            .await
            .unwrap_err()
            .to_string();
        assert!(first.contains("120 seconds"));
        assert!(started.elapsed() < Duration::from_secs(2));
        let second = api
            .get::<serde_json::Value>("/test")
            .await
            .unwrap_err()
            .to_string();
        assert!(second.contains("wait before retrying"));
        server.await.unwrap();
    }

    #[test]
    fn maps_playlist_items_with_new_and_old_field_names() {
        let new = r#"{"items":[{"item":{"uri":"spotify:track:1","name":"A","duration_ms":1000,"artists":[{"name":"X","uri":"spotify:artist:x"}],"album":{"name":"Al","uri":"spotify:album:a","images":[{"url":"big","width":640},{"url":"small","width":64}]}}}],"total":1,"next":null}"#;
        let old = new.replace("\"item\"", "\"track\"");
        for json in [new.to_string(), old] {
            let p: JPage<JPlaylistItem> = serde_json::from_str(&json).unwrap();
            let t = p.items.into_iter().next().unwrap();
            let t = map_track(t.item.or(t.track).unwrap());
            assert_eq!(t.name, "A");
            assert_eq!(t.artists, "X");
            assert_eq!(t.image.as_deref(), Some("small"));
            assert_eq!(t.image_large.as_deref(), Some("big"));
        }
    }

    #[test]
    fn picks_smallest_suitable_image() {
        let imgs = vec![
            JImage {
                url: "640".into(),
                width: Some(640),
            },
            JImage {
                url: "300".into(),
                width: Some(300),
            },
            JImage {
                url: "64".into(),
                width: Some(64),
            },
        ];
        assert_eq!(pick_image(&imgs, 64).as_deref(), Some("64"));
        assert_eq!(pick_image(&imgs, 100).as_deref(), Some("300"));
        assert_eq!(pick_image(&imgs, 1000).as_deref(), Some("640"));
        assert_eq!(pick_image(&[], 64), None);
    }

    #[test]
    fn playlist_ownership() {
        let json = r#"{"uri":"spotify:playlist:p","name":"P","owner":{"id":"me","display_name":"Me"},"items":{"total":5}}"#;
        let me = UserInfo {
            id: "me".into(),
            display_name: "Me".into(),
            avatar: None,
        };
        let c = map_playlist(serde_json::from_str(json).unwrap(), Some(&me));
        assert!(c.owned);
        assert_eq!(c.total_tracks, 5);
        let other = UserInfo {
            id: "you".into(),
            display_name: String::new(),
            avatar: None,
        };
        assert!(!map_playlist(serde_json::from_str(json).unwrap(), Some(&other)).owned);
    }

    #[test]
    fn page_offsets() {
        let p = page(vec![1, 2, 3], 50, 100, true);
        assert_eq!(p.next, Some(53));
        assert_eq!(page(vec![1], 0, 1, false).next, None);
    }

    #[test]
    fn parses_user_profile_view() {
        let json = br#"{"uri":"spotify:user:31ho","name":"Zayd","image_url":"https://i.scdn.co/image/ab67","followers_count":12,"following_count":3,
            "public_playlists":[{"uri":"spotify:playlist:x","name":"Gym","image_url":"spotify:mosaic:aa:bb:cc:dd","owner_name":"Zayd","owner_uri":"spotify:user:31ho"}],
            "total_public_playlists_count":7,
            "recently_played_artists":[{"uri":"spotify:artist:y","name":"Cheb Hasni","image_url":"spotify:image:ab12"}]}"#;
        let p = parse_profile(json, "31ho").unwrap();
        assert_eq!(p.name, "Zayd");
        assert_eq!(p.image.as_deref(), Some("https://i.scdn.co/image/ab67"));
        assert_eq!(
            (p.followers, p.following, p.total_public_playlists),
            (12, 3, 7)
        );
        assert!(p.public_playlists[0].owned);
        assert_eq!(
            p.public_playlists[0].image.as_deref(),
            Some("https://mosaic.scdn.co/300/aabbccdd")
        );
        assert_eq!(
            p.recent_artists[0].image.as_deref(),
            Some("https://i.scdn.co/image/ab12")
        );
        let empty = parse_profile(b"{}", "u").unwrap();
        assert_eq!(empty.name, "u");
    }

    #[test]
    fn playlist_change_messages_encode() {
        use librespot_protocol::playlist4_external::{op, ListChanges};
        let c = list_changes(
            vec![1, 2],
            vec![
                add_op(&["spotify:track:a".into()], false),
                rem_op(&["spotify:track:b".into()]),
            ],
        );
        let bytes = c.write_to_bytes().unwrap();
        let back = ListChanges::parse_from_bytes(&bytes).unwrap();
        assert_eq!(back.base_revision(), &[1, 2]);
        let ops = &back.deltas[0].ops;
        assert_eq!(ops[0].kind(), op::Kind::ADD);
        assert!(ops[0].add.add_last());
        assert_eq!(ops[0].add.items[0].uri(), "spotify:track:a");
        assert_eq!(ops[1].kind(), op::Kind::REM);
        assert!(ops[1].rem.items_as_key());
    }

    #[test]
    fn radio_response_finds_playlist() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"mediaItems":[{"uri":"spotify:playlist:37i9dQZF1E8abc"}],"total":1}"#,
        )
        .unwrap();
        assert_eq!(
            find_playlist_uri(&v).as_deref(),
            Some("spotify:playlist:37i9dQZF1E8abc")
        );
        assert!(find_playlist_uri(&serde_json::json!({"x": "spotify:track:1"})).is_none());
    }

    #[test]
    fn bio_markup_is_stripped() {
        assert_eq!(
            strip_tags("Born in <a href=\"spotify:artist:x\">Oran</a> &amp; raised"),
            "Born in Oran & raised"
        );
    }
}
