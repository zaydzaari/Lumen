//! Client for SpotiDownloader's token-authenticated download service.
//! Credentials and signed audio URLs must never enter config, logs or the library index.
use std::{
    io::Write as _,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use anyhow::{bail, Context as _, Result};
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    Client, StatusCode,
};
use serde::Deserialize;

pub const KEY: &str = "spotidownloader-token";
pub const SITE: &str = "https://spotidownloader.com/";
const ENDPOINT: &str = "https://api.spotidownloader.com/download";
const MAX_AUDIO: usize = 100 * 1024 * 1024;

/// Expiry is only a scheduling hint; the service still authenticates the token.
pub fn token_needs_refresh(token: &str, now: u64) -> bool {
    use base64::Engine as _;
    let Some(payload) = token.split('.').nth(1) else {
        return false;
    };
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    value
        .get("exp")
        .and_then(|v| v.as_u64())
        .is_some_and(|exp| exp <= now.saturating_add(15))
}

pub fn needs_verification(error: &anyhow::Error) -> bool {
    error
        .to_string()
        .contains("SpotiDownloader token expired or was rejected")
}

pub fn normalize_token(input: &str) -> Result<String> {
    let input = input.trim();
    let value = if input
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("bearer "))
    {
        &input[7..]
    } else {
        input
    };
    anyhow::ensure!(
        !value.is_empty() && value.len() <= 8192 && value.bytes().all(|b| b.is_ascii_graphic()),
        "Paste only the SpotiDownloader token or Bearer value."
    );
    Ok(value.to_owned())
}

pub(crate) fn authorization(token: &str) -> Result<HeaderValue> {
    let mut value = HeaderValue::from_str(&format!("Bearer {}", normalize_token(token)?))
        .map_err(|_| anyhow::anyhow!("Invalid SpotiDownloader token."))?;
    value.set_sensitive(true);
    Ok(value)
}

fn audio_url(raw: &str) -> Result<url::Url> {
    let url = url::Url::parse(raw)
        .map_err(|_| anyhow::anyhow!("SpotiDownloader returned an invalid audio link."))?;
    let domain = url.domain().unwrap_or_default().to_ascii_lowercase();
    anyhow::ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && domain.contains('.')
            && !domain.ends_with('.')
            && ![".localhost", ".local", ".internal", ".test", ".invalid"]
                .iter()
                .any(|suffix| domain.ends_with(suffix)),
        "SpotiDownloader returned an unsupported audio link."
    );
    Ok(url)
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    success: bool,
    link: Option<String>,
}

pub(crate) fn api_status(status: StatusCode) -> Result<()> {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => bail!(
            "SpotiDownloader token expired or was rejected. Replace it in Settings > Storage."
        ),
        StatusCode::TOO_MANY_REQUESTS => bail!("SpotiDownloader is rate limited. Try again later."),
        s if !s.is_success() => bail!("SpotiDownloader service returned HTTP {}.", s.as_u16()),
        _ => Ok(()),
    }
}

fn client() -> Result<Client> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(12))
        .timeout(Duration::from_secs(180))
        .user_agent(concat!("Lumen/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("Could not start SpotiDownloader client")
}

async fn resolve(client: &Client, endpoint: &str, id: &str, token: &str) -> Result<url::Url> {
    let mut response = client
        .post(endpoint)
        .header(AUTHORIZATION, authorization(token)?)
        .header("Origin", "https://spotidownloader.com")
        .header("Referer", SITE)
        .timeout(Duration::from_secs(30))
        .json(&serde_json::json!({"id": id}))
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("SpotiDownloader service could not be reached."))?;
    api_status(response.status())?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("SpotiDownloader response was interrupted."))?
    {
        anyhow::ensure!(
            bytes.len() + chunk.len() <= 64 * 1024,
            "SpotiDownloader response is too large."
        );
        bytes.extend_from_slice(&chunk);
    }
    // Never surface the server body: it may echo credentials or signed URLs.
    let reply: Reply = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("SpotiDownloader returned an unsupported response."))?;
    anyhow::ensure!(
        reply.success,
        "SpotiDownloader could not provide this song. Check the token or try again later."
    );
    audio_url(
        reply
            .link
            .as_deref()
            .context("SpotiDownloader returned no audio link.")?,
    )
}

async fn audio_response(
    client: &Client,
    mut url: url::Url,
    token: &str,
) -> Result<(url::Url, reqwest::Response)> {
    let mut redirects = 0;
    let response = loop {
        let response = audio_request(client, &url, token)?
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("SpotiDownloader audio could not be reached."))?;
        if response.status().is_redirection() {
            anyhow::ensure!(
                redirects < 5,
                "SpotiDownloader audio redirected too many times."
            );
            let location = response
                .headers()
                .get("location")
                .and_then(|h| h.to_str().ok())
                .context("SpotiDownloader audio redirect has no destination.")?;
            let next = url
                .join(location)
                .map_err(|_| anyhow::anyhow!("SpotiDownloader audio redirect is invalid."))?;
            url = audio_url(next.as_str())?;
            redirects += 1;
        } else {
            break response;
        }
    };
    anyhow::ensure!(
        response.status() == StatusCode::OK,
        "SpotiDownloader audio returned HTTP {}.",
        response.status().as_u16()
    );
    anyhow::ensure!(
        response
            .content_length()
            .is_none_or(|n| n <= MAX_AUDIO as u64),
        "SpotiDownloader audio exceeds 100 MB."
    );
    Ok((url, response))
}

async fn audio_bytes(response: &mut reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("SpotiDownloader audio download was interrupted."))?
    {
        anyhow::ensure!(
            bytes.len() + chunk.len() <= limit,
            "SpotiDownloader audio exceeds the download limit."
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn write_audio(
    response: &mut reqwest::Response,
    file: &mut std::fs::File,
    size: &mut usize,
) -> Result<()> {
    let start = *size;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("SpotiDownloader audio download was interrupted."))?
    {
        anyhow::ensure!(
            *size + chunk.len() <= MAX_AUDIO,
            "SpotiDownloader audio exceeds 100 MB."
        );
        *size += chunk.len();
        file.write_all(&chunk)
            .context("Could not write downloaded audio")?;
    }
    anyhow::ensure!(*size > start, "SpotiDownloader audio is empty.");
    Ok(())
}

/// Only finite, unencrypted media playlists. Network URLs are fetched by our
/// bounded HTTP client; FFmpeg never receives a remote playlist or credentials.
pub(crate) fn hls_parts(bytes: &[u8], base: &url::Url) -> Result<Vec<url::Url>> {
    let text = std::str::from_utf8(bytes).context("Invalid audio playlist")?;
    anyhow::ensure!(
        bytes.len() <= 256 * 1024
            && text.starts_with("#EXTM3U")
            && text.lines().any(|l| l.trim() == "#EXT-X-ENDLIST"),
        "The provider returned an unsupported or live audio playlist."
    );
    let mut parts = Vec::new();
    let mut segments = 0;
    for line in text.lines().map(str::trim) {
        anyhow::ensure!(
            ![
                "#EXT-X-KEY:",
                "#EXT-X-SESSION-KEY:",
                "#EXT-X-BYTERANGE:",
                "#EXT-X-STREAM-INF:",
                "#EXT-X-DISCONTINUITY"
            ]
            .iter()
            .any(|tag| line.starts_with(tag)),
            "The provider returned an encrypted or unsupported audio playlist."
        );
        let reference = if let Some(map) = line.strip_prefix("#EXT-X-MAP:") {
            anyhow::ensure!(
                parts.is_empty() && !map.contains("BYTERANGE"),
                "Unsupported audio initialization segment"
            );
            map.strip_prefix("URI=\"")
                .and_then(|v| v.strip_suffix('"'))
                .context("Invalid audio initialization segment")?
        } else if !line.is_empty() && !line.starts_with('#') {
            segments += 1;
            line
        } else {
            continue;
        };
        anyhow::ensure!(parts.len() < 1001, "Audio playlist has too many segments");
        let joined = base
            .join(reference)
            .map_err(|_| anyhow::anyhow!("Invalid audio segment link"))?;
        parts.push(audio_url(joined.as_str())?);
    }
    anyhow::ensure!(segments > 0, "Audio playlist contains no segments");
    Ok(parts)
}

async fn fetch(id: &str, token: &str, destination: &Path) -> Result<bool> {
    let client = client()?;
    let url = resolve(&client, ENDPOINT, id, token).await?;
    let (url, mut response) = audio_response(&client, url, token).await?;
    let mut bytes = Vec::new();
    while bytes.len() < 7 {
        let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("SpotiDownloader audio download was interrupted."))?
        else {
            break;
        };
        bytes.extend_from_slice(&chunk);
    }
    anyhow::ensure!(!bytes.is_empty(), "SpotiDownloader audio is empty.");
    let hls = bytes.starts_with(b"#EXTM3U");
    let mut file =
        std::fs::File::create(destination).context("Could not create downloaded audio")?;
    if hls {
        anyhow::ensure!(bytes.len() <= 256 * 1024, "Audio playlist is too large");
        bytes.extend(audio_bytes(&mut response, 256 * 1024 - bytes.len()).await?);
        let parts = hls_parts(&bytes, &url)?;
        let mut size = 0;
        for part in parts {
            let (_, mut response) = audio_response(&client, part, token).await?;
            write_audio(&mut response, &mut file, &mut size).await?;
        }
    } else {
        file.write_all(&bytes)
            .context("Could not write downloaded audio")?;
        let mut size = bytes.len();
        // The first response chunk may already contain the complete file.
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("SpotiDownloader audio download was interrupted."))?
        {
            anyhow::ensure!(
                size + chunk.len() <= MAX_AUDIO,
                "SpotiDownloader audio exceeds 100 MB."
            );
            size += chunk.len();
            file.write_all(&chunk)
                .context("Could not write downloaded audio")?;
        }
    }
    file.sync_all()?;
    Ok(hls)
}

fn audio_request(client: &Client, url: &url::Url, token: &str) -> Result<reqwest::RequestBuilder> {
    let mut request = client
        .get(url.clone())
        .header("Origin", "https://spotidownloader.com")
        .header("Referer", SITE);
    // Some links point back to the authenticated API. Never forward its bearer
    // credential to a CDN or another host, including after redirects.
    if url.origin() == url::Url::parse(ENDPOINT)?.origin() {
        request = request.header(AUTHORIZATION, authorization(token)?);
    }
    Ok(request)
}

pub async fn download(
    id: &str,
    token: &str,
    destination: &Path,
    stop: &AtomicBool,
) -> Result<bool> {
    let job = fetch(id, token, destination);
    tokio::pin!(job);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        if stop.load(Ordering::Relaxed) {
            bail!("Download cancelled");
        }
        tokio::select! {
            result = &mut job => return result,
            _ = tokio::time::sleep_until(deadline) => bail!("SpotiDownloader audio download timed out."),
            _ = tokio::time::sleep(Duration::from_millis(200)) => {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_audio_playlists_resolve_segments_and_reject_unsafe_variants() {
        let base = url::Url::parse("https://cdn.example.com/song/list.m3u8").unwrap();
        let parts = hls_parts(b"#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:10,\npart1.m4s\n#EXTINF:2,\nhttps://other.example.com/part2.m4s\n#EXT-X-ENDLIST\n", &base).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].as_str(), "https://cdn.example.com/song/init.mp4");
        assert_eq!(parts[1].as_str(), "https://cdn.example.com/song/part1.m4s");
        for invalid in [
            "#EXTM3U\npart.ts\n",
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\npart.ts\n#EXT-X-ENDLIST",
            "#EXTM3U\n#EXT-X-BYTERANGE:10@0\npart.ts\n#EXT-X-ENDLIST",
            "#EXTM3U\nfile:///private\n#EXT-X-ENDLIST",
            "#EXTM3U\nhttps://127.0.0.1/private\n#EXT-X-ENDLIST",
            "#EXTM3U\n#EXT-X-ENDLIST",
        ] {
            assert!(hls_parts(invalid.as_bytes(), &base).is_err());
        }
    }

    #[test]
    fn expired_sessions_refresh_only_when_needed() {
        use base64::Engine as _;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"exp":1600}"#);
        let token = format!("fixture.{payload}.fixture");
        assert!(!token_needs_refresh(&token, 1000));
        assert!(token_needs_refresh(&token, 1585));
        assert!(token_needs_refresh(&token, 2000));
        assert!(!token_needs_refresh("opaque-service-token", 1000));
        assert!(!token_needs_refresh("malformed.!!!.fixture", 1000));
        assert!(needs_verification(
            &api_status(StatusCode::UNAUTHORIZED).unwrap_err()
        ));
        assert!(!needs_verification(
            &api_status(StatusCode::TOO_MANY_REQUESTS).unwrap_err()
        ));
    }

    #[test]
    fn token_normalization_and_redaction() {
        assert_eq!(
            normalize_token(" Bearer example-token ").unwrap(),
            "example-token"
        );
        assert!(normalize_token("abc\r\nX-Header: secret").is_err());
        assert!(normalize_token("").is_err());
        let header = authorization("example-token").unwrap();
        assert!(header.is_sensitive());
        assert!(!format!("{header:?}").contains("example-token"));
    }

    #[test]
    fn rejects_unsafe_download_urls() {
        assert!(audio_url("https://audio.example.com/song?signature=private").is_ok());
        for raw in [
            "http://audio.example.com/song",
            "file:///secret",
            "https://127.0.0.1/song",
            "https://[::1]/song",
            "https://localhost/song",
            "https://host.local/song",
            "https://user:password@audio.example.com/song",
            "https://audio.example.com:8080/song",
        ] {
            let error = audio_url(raw).unwrap_err().to_string();
            assert!(!error.contains(raw));
        }
    }

    #[test]
    fn audio_auth_is_confined_to_the_api_origin() {
        let client = client().unwrap();
        for (url, authenticated) in [
            ("https://api.spotidownloader.com/file", true),
            ("https://cdn.spotidownloader.com/file", false),
            ("https://audio.example.com/file", false),
            ("https://api.spotidownloader.com.evil.example/file", false),
        ] {
            let request = audio_request(&client, &url::Url::parse(url).unwrap(), "fixture-token")
                .unwrap()
                .build()
                .unwrap();
            assert_eq!(request.headers().contains_key(AUTHORIZATION), authenticated);
        }
    }

    #[tokio::test]
    async fn api_reply_accepts_links_and_hides_error_bodies() {
        use tokio::{
            io::{AsyncReadExt as _, AsyncWriteExt as _},
            net::TcpListener,
        };
        for (body, valid) in [
            (r#"{"success":true,"link":"https://cdn.example.com/audio?signature=private-fixture-token"}"#.to_owned(), true),
            (r#"{"success":false,"error":"private-fixture-token"}"#.to_owned(), false),
            ("private-fixture-token".to_owned(), false),
            ("x".repeat(65537), false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/download", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 16384];
                let _ = socket.read(&mut bytes).await.unwrap();
                let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = socket.write_all(response.as_bytes()).await;
            });
            let result = resolve(&client().unwrap(), &endpoint, "track-id", "fixture-token").await;
            assert_eq!(result.is_ok(), valid);
            if let Err(error) = result { assert!(!format!("{error:#}").contains("private-fixture-token")); }
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn stopped_download_does_not_create_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("audio.mp3");
        assert!(
            download("track-id", "fixture-token", &file, &AtomicBool::new(true))
                .await
                .is_err()
        );
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn api_contract_and_expired_token_do_not_expose_secrets() {
        use tokio::{
            io::{AsyncReadExt as _, AsyncWriteExt as _},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/download", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 16384];
            let mut size = 0;
            loop {
                size += socket.read(&mut bytes[size..]).await.unwrap();
                if bytes[..size].ends_with(b"{\"id\":\"track-id\"}") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&bytes[..size]).to_ascii_lowercase();
            assert!(request.contains("authorization: bearer private-fixture-token"));
            assert!(request.contains("origin: https://spotidownloader.com"));
            socket.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 21\r\nConnection: close\r\n\r\nprivate-fixture-token").await.unwrap();
        });
        let error = resolve(
            &client().unwrap(),
            &endpoint,
            "track-id",
            "private-fixture-token",
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("token expired"));
        assert!(!error.contains("private-fixture-token"));
        server.await.unwrap();
    }
}
