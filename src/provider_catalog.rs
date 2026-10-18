//! Import metadata through the user's attended download-provider connection.
//! This does not sign into Spotify or access private Spotify account data.
use crate::{
    events::UiTx,
    free_library::{Import, Playlist},
    model::Track,
};
use anyhow::{Context as _, Result};
use serde_json::{json, Value};

pub fn reference(input: &str) -> Option<(String, String)> {
    let parts: Vec<String> = if input.starts_with("spotify:") {
        input.split(':').map(str::to_string).collect()
    } else {
        let url = url::Url::parse(input).ok()?;
        if url.scheme() != "https"
            || url.host_str() != Some("open.spotify.com")
            || url.port_or_known_default() != Some(443)
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        let parts: Vec<_> = url.path_segments()?.collect();
        let parts = if parts.first().is_some_and(|p| p.starts_with("intl-")) {
            &parts[1..]
        } else {
            &parts[..]
        };
        if parts.len() != 2 {
            return None;
        }
        vec!["spotify".into(), parts[0].into(), parts[1].into()]
    };
    match parts.as_slice() {
        [prefix, kind, id]
            if prefix == "spotify"
                && ["track", "album", "playlist"].contains(&kind.as_str())
                && id.len() == 22
                && id.bytes().all(|b| b.is_ascii_alphanumeric()) =>
        {
            Some((kind.clone(), id.clone()))
        }
        _ => None,
    }
}

async fn post(client: &reqwest::Client, endpoint: &str, body: Value, token: &str) -> Result<Value> {
    let mut response = client
        .post(format!("https://api.spotidownloader.com/{endpoint}"))
        .header(
            reqwest::header::AUTHORIZATION,
            crate::spotidownloader::authorization(token)?,
        )
        .header("Origin", "https://spotidownloader.com")
        .header("Referer", crate::spotidownloader::SITE)
        .json(&body)
        .send()
        .await
        .context("Download provider metadata request failed")?;
    crate::spotidownloader::api_status(response.status())?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("Could not read provider metadata")?
    {
        anyhow::ensure!(
            bytes.len() + chunk.len() <= 2 * 1024 * 1024,
            "Provider metadata page exceeds 2 MB"
        );
        bytes.extend_from_slice(&chunk);
    }
    let value: Value =
        serde_json::from_slice(&bytes).context("Provider returned invalid metadata")?;
    anyhow::ensure!(value["success"] == true, "Provider could not import this link. Private playlists may require a Spotify account-data export.");
    Ok(value)
}

fn track(
    value: &Value,
    fallback_id: Option<&str>,
    album: &str,
    cover: Option<&str>,
) -> Option<Track> {
    let id = value.get("id").and_then(|v| v.as_str()).or(fallback_id)?;
    let uri = crate::free_library::track_uri(&format!("spotify:track:{id}"))?;
    let name = value["title"].as_str()?.trim();
    if name.is_empty() || name.len() > 4096 {
        return None;
    }
    let artists = match &value["artists"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    };
    if artists.len() > 4096 {
        return None;
    }
    // Unknown duration stays unknown; do not mistake seconds for milliseconds.
    Some(Track {
        uri,
        name: name.to_string(),
        artists,
        album: value["album"]
            .as_str()
            .unwrap_or(album)
            .chars()
            .take(4096)
            .collect(),
        image: value["cover"]
            .as_str()
            .or(cover)
            .filter(|s| s.len() <= 4096 && s.starts_with("https://"))
            .map(str::to_string),
        duration_ms: value["duration_ms"]
            .as_u64()
            .and_then(|v| v.try_into().ok())
            .unwrap_or(0),
        ..Default::default()
    })
}

async fn fetch(kind: &str, id: &str, token: &str) -> Result<Import> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(12))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let meta = post(&client, "metadata", json!({"type": kind, "id": id}), token).await?;
    let name = meta["title"]
        .as_str()
        .unwrap_or("Imported music")
        .chars()
        .take(4096)
        .collect::<String>();
    let album = meta["album"].as_str().unwrap_or(&name);
    let cover = meta["cover"].as_str();
    let mut skipped = 0;
    let mut tracks = Vec::new();
    if kind == "track" {
        tracks.push(
            track(&meta, Some(id), album, cover)
                .context("Provider returned incomplete track metadata")?,
        );
    } else {
        let mut offset = 0;
        loop {
            let page = post(
                &client,
                "tracks",
                json!({"type": kind, "id": id, "offset": offset}),
                token,
            )
            .await?;
            let items = page["trackList"]
                .as_array()
                .context("Provider did not return a track list")?;
            anyhow::ensure!(
                tracks.len() + skipped + items.len() <= 50_000,
                "Import at most 50,000 tracks"
            );
            for item in items {
                if let Some(t) = track(item, None, album, cover) {
                    tracks.push(t);
                } else {
                    skipped += 1;
                }
            }
            let next = page["nextOffset"]
                .as_u64()
                .or_else(|| page["nextOffset"].as_str().and_then(|s| s.parse().ok()));
            match next {
                None | Some(0) => break,
                Some(next) => {
                    anyhow::ensure!(
                        !items.is_empty() && next > offset,
                        "Provider pagination stopped progressing"
                    );
                    offset = next;
                }
            }
        }
    }
    anyhow::ensure!(
        !tracks.is_empty(),
        "No usable music tracks returned for this link"
    );
    // Round-trip through the bounded personal-library parser for consistent identifiers/validation.
    let mut imported = crate::free_library::parse(&serde_json::to_vec(&vec![Playlist {
        id: String::new(),
        name,
        tracks,
    }])?)?;
    imported.playlists[0].id = format!("lumen:playlist:provider-{kind}-{id}");
    imported.skipped += skipped;
    Ok(imported)
}

pub async fn import(
    shared: &crate::backend::Shared,
    connection: std::sync::Arc<crate::verification::Connection>,
    ui: UiTx,
    input: &str,
) -> Result<Import> {
    let (kind, id) = reference(input).context("Paste a Spotify track, album or playlist link")?;
    let mut verified = false;
    loop {
        let token = shared.store.load(crate::spotidownloader::KEY);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if token
            .as_ref()
            .is_none_or(|t| crate::spotidownloader::token_needs_refresh(t, now))
        {
            anyhow::ensure!(
                !verified && connection.enabled(),
                "Connect the download provider in Settings > Storage before importing links"
            );
            let (connection, ui) = (connection.clone(), ui.clone());
            tokio::task::spawn_blocking(move || connection.verify(&ui, "", &Default::default()))
                .await??;
            verified = true;
            continue;
        }
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(180),
            fetch(&kind, &id, &token.unwrap()),
        )
        .await
        .context("Playlist import timed out")?;
        if !verified
            && connection.enabled()
            && result
                .as_ref()
                .is_err_and(crate::spotidownloader::needs_verification)
        {
            let (connection, ui) = (connection.clone(), ui.clone());
            tokio::task::spawn_blocking(move || connection.verify(&ui, "", &Default::default()))
                .await??;
            verified = true;
            continue;
        }
        return result;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_spotify_references_are_accepted() {
        assert!(
            reference("https://open.spotify.com/intl-fr/track/76gtwXQM0Un36Lq1JzNh8I?si=abc")
                .is_some()
        );
        assert!(reference("spotify:playlist:0123456789abcdefghijkl").is_some());
        for input in [
            "https://open.spotify.com.evil.test/track/76gtwXQM0Un36Lq1JzNh8I",
            "https://127.0.0.1/track/76gtwXQM0Un36Lq1JzNh8I",
            "spotify:artist:0123456789abcdefghijkl",
            "spotify:track:../../secrets",
        ] {
            assert!(reference(input).is_none());
        }
    }
    #[test]
    fn metadata_preserves_unknown_duration_and_recording_identity() {
        let t = track(&json!({"id":"76gtwXQM0Un36Lq1JzNh8I","title":"Ennahla","artists":["Nass El Ghiwane"],"duration":200}), None, "Album", None).unwrap();
        assert_eq!(t.uri, "spotify:track:76gtwXQM0Un36Lq1JzNh8I");
        assert_eq!(t.artists, "Nass El Ghiwane");
        assert_eq!(t.duration_ms, 0);
    }
}
