//! Read-only account-library OAuth, independent of librespot streaming.
use crate::{
    auth::{self, StoredWebToken, TokenResponse},
    backend::Shared,
    events::{AppEvent, UiTx},
    free_library::{Import, Playlist},
    model::{Track, UserInfo},
};
use anyhow::{Context as _, Result};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
const KEY: &str = "account-library-token";
const BASE: &str = "https://api.spotify.com/v1";
pub const DEFAULT_CLIENT_ID: &str = "d2ee30a24a16463fb73ed7cb4262fa89";
const SCOPES: &[&str] = &[
    "user-read-private",
    "playlist-read-private",
    "playlist-read-collaborative",
    "user-library-read",
];
pub struct Connected {
    pub profile: UserInfo,
    pub library: Import,
    pub restricted: usize,
}
#[derive(Debug, thiserror::Error)]
#[error("Spotify denied library access. The app owner must have Premium; add this account in User Management and reconnect. Account or endpoint restrictions may still apply.")]
struct AccessDenied;
pub fn stored(shared: &Shared) -> bool {
    shared.store.load(KEY).is_some()
}
pub fn disconnect(shared: &Shared) {
    shared.store.delete(KEY);
}
struct Client {
    base: String,
    shared: Shared,
    client_id: String,
    token: TokenResponse,
}
impl Client {
    fn save(&self) -> Result<()> {
        let refresh_token = self
            .token
            .refresh_token
            .clone()
            .context("Spotify did not provide a refresh token")?;
        self.shared.store.save(
            KEY,
            &serde_json::to_string(&StoredWebToken {
                client_id: self.client_id.clone(),
                refresh_token,
            })?,
        )
    }
    async fn refresh(&mut self) -> Result<()> {
        let refresh = self
            .token
            .refresh_token
            .clone()
            .context("Sign in to Spotify again")?;
        let mut token = auth::refresh_token(&self.shared.http, &self.client_id, &refresh).await?;
        if token.refresh_token.is_none() {
            token.refresh_token = Some(refresh);
        }
        self.token = token;
        self.save()
    }
    async fn get(&mut self, path: &str) -> Result<Value> {
        let url = if path.starts_with('/') {
            format!("{}{path}", self.base)
        } else {
            path.to_owned()
        };
        anyhow::ensure!(
            trusted_next(&url, &self.base),
            "Unexpected Spotify pagination URL"
        );
        for attempt in 0..2 {
            let mut response = self
                .shared
                .http
                .get(&url)
                .bearer_auth(&self.token.access_token)
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await?;
            match response.status().as_u16() {
                401 if attempt == 0 => {
                    self.refresh().await?;
                    continue;
                }
                403 => return Err(AccessDenied.into()),
                429 => anyhow::bail!(
                    "Spotify is rate limiting library imports. Wait before syncing again."
                ),
                _ => {
                    response.error_for_status_ref()?;
                }
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                anyhow::ensure!(
                    bytes.len() + chunk.len() <= 4 * 1024 * 1024,
                    "Spotify response exceeded 4 MB"
                );
                bytes.extend_from_slice(&chunk);
            }
            return Ok(serde_json::from_slice(&bytes)?);
        }
        anyhow::bail!("Spotify login expired; reconnect your account")
    }
    async fn pages(&mut self, path: &str, limit: usize) -> Result<Vec<Value>> {
        let mut next = Some(path.to_string());
        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();
        while let Some(url) = next {
            anyhow::ensure!(
                seen.insert(url.clone()) && seen.len() <= 1100,
                "Spotify returned invalid pagination"
            );
            let page = self.get(&url).await?;
            let batch = page["items"]
                .as_array()
                .context("Spotify did not return a library list")?;
            anyhow::ensure!(
                items.len() + batch.len() <= limit,
                "Account library exceeds Lumen's import limit"
            );
            items.extend(batch.iter().cloned());
            next = page["next"].as_str().map(str::to_owned);
        }
        Ok(items)
    }
}
fn trusted_next(value: &str, base: &str) -> bool {
    let (Ok(url), Ok(base)) = (url::Url::parse(value), url::Url::parse(base)) else {
        return false;
    };
    url.origin() == base.origin()
        && url.username().is_empty()
        && url.password().is_none()
        && url
            .path()
            .starts_with(&format!("{}/", base.path().trim_end_matches('/')))
}

fn text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(4096)
        .collect()
}
fn track(value: &Value) -> Option<Track> {
    let value = value
        .get("item")
        .filter(|v| v.is_object())
        .or_else(|| value.get("track").filter(|v| v.is_object()))
        .unwrap_or(value);
    let uri = crate::free_library::track_uri(value["uri"].as_str()?)?;
    if value["is_local"].as_bool() == Some(true) {
        return None;
    }
    let album = &value["album"];
    let image = album["images"]
        .as_array()
        .and_then(|a| a.last())
        .and_then(|i| i["url"].as_str())
        .map(str::to_owned);
    let image_large = album["images"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|i| i["url"].as_str())
        .map(str::to_owned);
    let name = text(&value["name"]);
    if name.is_empty() {
        return None;
    }
    Some(Track {
        uri,
        name,
        artists: value["artists"]
            .as_array()
            .map(|a| {
                a.iter()
                    .take(30)
                    .map(|a| text(&a["name"]))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        artist_uri: value["artists"][0]["uri"].as_str().map(str::to_owned),
        album: text(&album["name"]),
        album_uri: album["uri"].as_str().map(str::to_owned),
        duration_ms: value["duration_ms"]
            .as_u64()
            .unwrap_or(0)
            .min(u32::MAX as u64) as u32,
        image,
        image_large,
        explicit: value["explicit"].as_bool().unwrap_or(false),
    })
}
pub async fn connect(
    shared: Shared,
    config: crate::config::Config,
    ui: UiTx,
    interactive: bool,
) -> Result<Connected> {
    let client_id = config.client_id.trim().to_string();
    anyhow::ensure!(
        client_id.len() == 32 && client_id.bytes().all(|b| b.is_ascii_hexdigit()),
        "Enter Lumen's public Spotify Client ID in Settings first"
    );
    let stored = shared
        .store
        .load(KEY)
        .and_then(|s| serde_json::from_str::<StoredWebToken>(&s).ok())
        .filter(|s| s.client_id == client_id);
    let token = if interactive {
        ui.send(AppEvent::LoginStatus(
            "Approve library access in your browser…".into(),
        ));
        let ui2 = ui.clone();
        auth::pkce_login(
            &shared.http,
            &client_id,
            config.redirect_port,
            SCOPES,
            |url| ui2.send(AppEvent::LoginUrl(url)),
        )
        .await?
    } else {
        let previous = stored.context("Sign in to connect your Spotify library")?;
        let mut token =
            auth::refresh_token(&shared.http, &client_id, &previous.refresh_token).await?;
        if token.refresh_token.is_none() {
            token.refresh_token = Some(previous.refresh_token);
        }
        token
    };
    let mut client = Client {
        base: BASE.into(),
        shared,
        client_id,
        token,
    };
    let me = client.get("/me").await?;
    let profile = UserInfo {
        id: text(&me["id"]),
        display_name: text(&me["display_name"]),
        avatar: me["images"][0]["url"].as_str().map(str::to_owned),
    };
    anyhow::ensure!(
        !profile.id.is_empty(),
        "Spotify did not identify the account"
    );
    client.save()?;
    let prefix = format!(
        "lumen:playlist:account:{:x}",
        Sha256::digest(profile.id.as_bytes())
    );
    ui.send(AppEvent::LoginStatus("Importing your playlists…".into()));
    let lists = client.pages("/me/playlists?limit=50", 999).await?;
    let mut playlists = Vec::new();
    let mut skipped = 0;
    let mut total = 0;
    let mut restricted = 0;
    for list in lists {
        let id = list["id"]
            .as_str()
            .context("Spotify returned a playlist without an ID")?;
        anyhow::ensure!(
            id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric()),
            "Invalid Spotify playlist ID"
        );
        let name = text(&list["name"]);
        ui.send(AppEvent::LoginStatus(format!("Importing {name}…")));
        let items = match client
            .pages(&format!("/playlists/{id}/items?limit=50"), 50_000 - total)
            .await
        {
            Ok(items) => items,
            Err(error)
                if error.downcast_ref::<AccessDenied>().is_some()
                    || error
                        .downcast_ref::<reqwest::Error>()
                        .is_some_and(|e| e.status() == Some(reqwest::StatusCode::NOT_FOUND)) =>
            {
                restricted += 1;
                continue;
            }
            Err(error) => {
                return Err(error.context(format!(
                    "Could not import playlist '{name}'. Your previous library is unchanged"
                )))
            }
        };
        total += items.len();
        let tracks: Vec<_> = items.iter().filter_map(track).collect();
        skipped += items.len() - tracks.len();
        playlists.push(Playlist {
            id: format!("{prefix}:{id}"),
            name,
            tracks,
        });
    }
    ui.send(AppEvent::LoginStatus("Importing your liked songs…".into()));
    let liked = client.pages("/me/tracks?limit=50", 50_000 - total).await?;
    let tracks: Vec<_> = liked.iter().filter_map(track).collect();
    skipped += liked.len() - tracks.len();
    playlists.push(Playlist {
        id: format!("{prefix}:liked"),
        name: "Liked Songs".into(),
        tracks,
    });
    Ok(Connected {
        profile,
        library: Import { playlists, skipped },
        restricted,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pagination_never_sends_token_to_another_origin() {
        assert!(trusted_next(
            "https://api.spotify.com/v1/me/tracks?offset=50",
            BASE
        ));
        assert!(!trusted_next("https://evil.test/v1/me", BASE));
        assert!(!trusted_next("http://api.spotify.com/v1/me", BASE));
    }
    #[test]
    fn maps_both_spotify_item_shapes_and_skips_non_music() {
        let t = serde_json::json!({"uri":"spotify:track:76gtwXQM0Un36Lq1JzNh8I", "name":"Song", "artists":[{"name":"Artist"}], "duration_ms":123000});
        assert_eq!(
            track(&serde_json::json!({"item":t.clone()})).unwrap().name,
            "Song"
        );
        assert_eq!(
            track(&serde_json::json!({"track":t})).unwrap().duration_ms,
            123000
        );
        assert!(
            track(&serde_json::json!({"uri":"spotify:episode:76gtwXQM0Un36Lq1JzNh8I"})).is_none()
        );
    }
    #[tokio::test]
    async fn imports_every_page_with_authorization_and_a_total_limit() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let bodies = vec![serde_json::json!({"items":[{"id":"one"}],"next":format!("{base}/me/playlists?offset=1")}).to_string(), serde_json::json!({"items":[{"id":"two"}],"next":null}).to_string()];
        let server = std::thread::spawn(move || {
            for body in bodies {
                let started = std::time::Instant::now();
                let mut socket = loop {
                    if let Ok((socket, _)) = listener.accept() {
                        break socket;
                    }
                    assert!(
                        started.elapsed() < std::time::Duration::from_secs(5),
                        "Missing pagination request"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                };
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = [0u8; 4096];
                let count = socket.read(&mut bytes).unwrap();
                assert!(String::from_utf8_lossy(&bytes[..count])
                    .to_ascii_lowercase()
                    .contains("authorization: bearer fixture-only"));
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared::new(
            tokio::runtime::Handle::current(),
            crate::config::Paths::under(dir.path()),
        )
        .unwrap();
        let mut client = Client {
            base,
            shared,
            client_id: DEFAULT_CLIENT_ID.into(),
            token: TokenResponse {
                access_token: "fixture-only".into(),
                refresh_token: None,
                expires_in: 3600,
            },
        };
        // The second page must not silently bypass the overall limit.
        let error = client.pages("/me/playlists", 1).await.unwrap_err();
        assert!(error.to_string().contains("import limit"));
        server.join().unwrap();
    }
}
