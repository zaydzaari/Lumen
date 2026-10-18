//! Authentication.
//!
//! Mirrors what current `ncspot` / `spotify-player` do (librespot removed password
//! login; OAuth is required):
//!
//! 1. **Streaming session** – OAuth 2.0 authorization-code + PKCE against Spotify's
//!    official desktop client ID with a loopback redirect
//!    (`http://127.0.0.1:<random port>/login`). The access token is handed to
//!    librespot (`Credentials::with_access_token`); after the first successful
//!    connection librespot returns *reusable* credentials which we persist so the
//!    user only logs in once.
//! 2. **Web API token** – Spotify no longer lets third-party apps use the official
//!    client's token reliably for the Web API, so (like spotify-player) a second
//!    PKCE flow runs against the user's *own* developer-app client ID. Its refresh
//!    token is persisted and refreshed automatically. If no client ID is configured
//!    library browsing uses librespot's authenticated metadata/context endpoints,
//!    rather than sending the shared login5 token to the public Web API.
//!
//! Secrets are stored in the OS keychain on Windows (Credential Manager) and macOS
//! (Keychain); on Linux in files readable only by the user (mode 0600).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context as _, Result};
use base64::Engine as _;
use librespot_core::authentication::Credentials;
use librespot_core::Session;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

/// Spotify's official desktop client ID (the one librespot itself uses).
pub const SPOTIFY_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";

const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

/// Scopes requested for the librespot streaming session.
pub const STREAMING_SCOPES: &[&str] = &[
    "streaming",
    "user-read-playback-state",
    "user-modify-playback-state",
    "user-read-currently-playing",
    "app-remote-control",
    "playlist-read-private",
    "playlist-read-collaborative",
    "user-library-read",
    "user-library-modify",
    "user-follow-read",
    "user-read-private",
    "user-read-email",
    "user-top-read",
    "user-read-recently-played",
    "user-read-playback-position",
    "user-personalized",
];

/// Scopes requested for the Web API client (user's own client ID).
pub const WEB_API_SCOPES: &[&str] = &[
    "user-read-private",
    "playlist-read-private",
    "playlist-read-collaborative",
    "user-library-read",
    "user-library-modify",
    "user-follow-read",
    "user-top-read",
    "user-read-recently-played",
    "user-read-playback-state",
    "user-modify-playback-state",
];

const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------- PKCE / OAuth

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn new() -> Self {
        use rand::Rng as _;
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
        let mut rng = rand::rng();
        let verifier: String = (0..96)
            .map(|_| CHARS[rng.random_range(0..CHARS.len())] as char)
            .collect();
        let challenge = Self::challenge_for(&verifier);
        Self {
            verifier,
            challenge,
        }
    }

    pub fn challenge_for(verifier: &str) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
    }
}

fn random_state() -> String {
    use rand::Rng as _;
    let mut rng = rand::rng();
    (0..24)
        .map(|_| char::from_digit(rng.random_range(0..36), 36).unwrap_or('x'))
        .collect()
}

pub fn authorize_url(
    client_id: &str,
    redirect_uri: &str,
    scopes: &[&str],
    challenge: &str,
    state: &str,
) -> Result<String> {
    let mut url = url::Url::parse(AUTHORIZE_URL)?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", challenge)
        .append_pair("state", state)
        .append_pair("scope", &scopes.join(" "));
    Ok(url.into())
}

/// Result of parsing one HTTP request on the loopback callback server.
#[derive(Debug, PartialEq, Eq)]
pub enum Callback {
    Code(String),
    Denied(String),
    /// Unrelated request (favicon etc.) – keep waiting.
    Ignore,
}

/// Parse the request target (`/login?code=...&state=...`).
pub fn parse_callback(target: &str, expected_state: &str) -> Callback {
    let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Callback::Ignore;
    };
    if url.path() != "/login" {
        return Callback::Ignore;
    }
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }
    if state.as_deref() != Some(expected_state) {
        return Callback::Ignore;
    }
    match (code, error) {
        (Some(c), _) if !c.is_empty() => Callback::Code(c),
        (_, Some(e)) => Callback::Denied(e),
        _ => Callback::Ignore,
    }
}

const SUCCESS_PAGE: &str = "<!doctype html><html><head><meta charset=utf-8><title>Lumen</title>\
<style>body{font-family:system-ui,sans-serif;background:#121212;color:#eee;display:grid;place-items:center;height:100vh;margin:0}\
div{text-align:center}h1{color:#1ed760}</style></head><body><div><h1>Logged in</h1>\
<p>You can close this tab and return to Lumen.</p></div></body></html>";

async fn wait_for_code(listener: TcpListener, state: &str) -> Result<String> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let n = match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            _ => continue,
        };
        let req = String::from_utf8_lossy(&buf[..n]);
        let target = req
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("");
        let result = parse_callback(target, state);
        let (status, body) = match &result {
            Callback::Code(_) => ("200 OK", SUCCESS_PAGE.to_string()),
            Callback::Denied(e) => ("200 OK", format!("<h1>Login cancelled</h1><p>{e}</p>")),
            Callback::Ignore => ("404 Not Found", String::new()),
        };
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes()).await;
        let _ = stream.shutdown().await;
        match result {
            Callback::Code(c) => return Ok(c),
            Callback::Denied(e) => bail!("Spotify login was denied: {e}"),
            Callback::Ignore => continue,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default = "default_expiry")]
    pub expires_in: u64,
}

fn default_expiry() -> u64 {
    3600
}

async fn token_request(http: &reqwest::Client, form: &[(&str, &str)]) -> Result<TokenResponse> {
    let resp = http
        .post(TOKEN_URL)
        .form(form)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("contacting accounts.spotify.com")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!(
            "token request failed ({status}): {}",
            body.chars().take(300).collect::<String>()
        );
    }
    Ok(resp.json().await?)
}

/// Run a full PKCE flow: listen on loopback, open the browser, wait for the
/// redirect, exchange the code. `port == 0` picks a free port.
/// `on_url` receives the authorize URL (so the UI can show it if the browser
/// did not open).
pub async fn pkce_login(
    http: &reqwest::Client,
    client_id: &str,
    port: u16,
    scopes: &[&str],
    on_url: impl FnOnce(String),
) -> Result<TokenResponse> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("could not listen on 127.0.0.1:{port} for the login redirect"))?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/login");
    let pkce = Pkce::new();
    let state = random_state();
    let url = authorize_url(client_id, &redirect_uri, scopes, &pkce.challenge, &state)?;
    if let Err(e) = open::that_detached(&url) {
        tracing::warn!("could not open browser: {e}");
    }
    on_url(url);
    let code = tokio::time::timeout(LOGIN_TIMEOUT, wait_for_code(listener, &state))
        .await
        .map_err(|_| anyhow!("timed out waiting for the browser login (5 minutes)"))??;
    token_request(
        http,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
            ("client_id", client_id),
            ("code_verifier", &pkce.verifier),
        ],
    )
    .await
}

pub async fn refresh_token(
    http: &reqwest::Client,
    client_id: &str,
    refresh: &str,
) -> Result<TokenResponse> {
    token_request(
        http,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("client_id", client_id),
        ],
    )
    .await
}

// ---------------------------------------------------------------- secret storage

pub const KEY_SESSION: &str = "librespot-credentials";
pub const KEY_WEB_TOKEN: &str = "webapi-token";

/// Stores small secrets: OS keychain on Windows/macOS, 0600 files on Linux
/// (and as a fallback if the keychain is unavailable).
#[derive(Clone)]
pub struct SecretStore {
    dir: PathBuf,
}

impl SecretStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn file(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    pub fn save(&self, key: &str, value: &str) -> Result<()> {
        #[cfg(any(windows, target_os = "macos"))]
        {
            match keyring::Entry::new("Lumen", key).and_then(|e| e.set_password(value)) {
                Ok(()) => {
                    let _ = std::fs::remove_file(self.file(key));
                    return Ok(());
                }
                Err(e) => tracing::warn!("keychain unavailable ({e}); using a protected file"),
            }
        }
        self.save_file(key, value)
    }

    fn save_file(&self, key: &str, value: &str) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.file(key);
        let tmp = path.with_extension("tmp");
        #[cfg(unix)]
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(value.as_bytes())?;
        }
        #[cfg(not(unix))]
        std::fs::write(&tmp, value)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn load(&self, key: &str) -> Option<String> {
        #[cfg(any(windows, target_os = "macos"))]
        {
            if let Ok(v) = keyring::Entry::new("Lumen", key).and_then(|e| e.get_password()) {
                return Some(v);
            }
        }
        std::fs::read_to_string(self.file(key)).ok()
    }

    pub fn delete(&self, key: &str) {
        #[cfg(any(windows, target_os = "macos"))]
        {
            if let Ok(e) = keyring::Entry::new("Lumen", key) {
                let _ = e.delete_credential();
            }
        }
        let _ = std::fs::remove_file(self.file(key));
    }
}

/// Encode librespot's reusable credentials for storage.
pub fn encode_credentials(c: &Credentials) -> Result<String> {
    Ok(serde_json::to_string(c)?)
}

pub fn decode_credentials(s: &str) -> Result<Credentials> {
    Ok(serde_json::from_str(s)?)
}

/// Reusable credentials of a connected session (what librespot would write to
/// `credentials.json`).
pub fn reusable_credentials(session: &Session) -> Result<Credentials> {
    let auth_data = session.auth_data();
    if auth_data.is_empty() {
        bail!("session has no reusable credentials");
    }
    let json = serde_json::json!({
        "username": session.username(),
        "auth_type": 1, // AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS
        "auth_data": base64::engine::general_purpose::STANDARD.encode(&auth_data),
    });
    Ok(serde_json::from_value(json)?)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredWebToken {
    pub client_id: String,
    pub refresh_token: String,
}

// ---------------------------------------------------------------- Web API token

enum Source {
    /// User's own developer app (refresh-token flow).
    Pkce {
        client_id: String,
        refresh: tokio::sync::Mutex<String>,
    },
    /// librespot session login5 token (experimental).
    Session(Session),
}

/// Hands out valid Web API access tokens, refreshing them shortly before expiry.
pub struct TokenManager {
    source: Source,
    http: reqwest::Client,
    store: SecretStore,
    cached: tokio::sync::Mutex<Option<(String, Instant)>>,
}

impl TokenManager {
    pub fn with_client(
        http: reqwest::Client,
        store: SecretStore,
        client_id: String,
        refresh: String,
        initial: Option<(String, Duration)>,
    ) -> Arc<Self> {
        Arc::new(Self {
            source: Source::Pkce {
                client_id,
                refresh: tokio::sync::Mutex::new(refresh),
            },
            http,
            store,
            cached: tokio::sync::Mutex::new(initial.map(|(t, d)| (t, Instant::now() + d))),
        })
    }

    pub fn with_session(http: reqwest::Client, store: SecretStore, session: Session) -> Arc<Self> {
        Arc::new(Self {
            source: Source::Session(session),
            http,
            store,
            cached: tokio::sync::Mutex::new(None),
        })
    }

    pub fn uses_session_token(&self) -> bool {
        matches!(self.source, Source::Session(_))
    }

    /// A token valid for at least another minute.
    pub async fn get(&self) -> Result<String> {
        let mut cached = self.cached.lock().await;
        if let Some((tok, exp)) = cached.as_ref() {
            if *exp > Instant::now() + Duration::from_secs(60) {
                return Ok(tok.clone());
            }
        }
        let (tok, ttl) = match &self.source {
            Source::Pkce { client_id, refresh } => {
                let mut refresh = refresh.lock().await;
                let resp = refresh_token(&self.http, client_id, &refresh)
                    .await
                    .context("refreshing the Web API token (try logging out and in again)")?;
                if let Some(new_refresh) = resp.refresh_token.filter(|r| !r.is_empty()) {
                    if *refresh != new_refresh {
                        *refresh = new_refresh;
                        let stored = StoredWebToken {
                            client_id: client_id.clone(),
                            refresh_token: refresh.clone(),
                        };
                        if let Err(e) = serde_json::to_string(&stored)
                            .map_err(anyhow::Error::from)
                            .and_then(|s| self.store.save(KEY_WEB_TOKEN, &s))
                        {
                            tracing::warn!("could not persist refreshed token: {e:#}");
                        }
                    }
                }
                (resp.access_token, Duration::from_secs(resp.expires_in))
            }
            Source::Session(session) => {
                let tok =
                    tokio::time::timeout(Duration::from_secs(10), session.login5().auth_token())
                        .await
                        .map_err(|_| anyhow!("timed out getting a session token"))?
                        .map_err(|e| anyhow!("session token: {e}"))?;
                (tok.access_token, tok.expires_in)
            }
        };
        *cached = Some((tok.clone(), Instant::now() + ttl));
        Ok(tok)
    }

    /// Forget the cached access token (e.g. after a 401).
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc7636_example() {
        // RFC 7636 appendix B
        assert_eq!(
            Pkce::challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let p = Pkce::new();
        assert!(p.verifier.len() >= 43 && p.verifier.len() <= 128);
    }

    #[test]
    fn callback_parsing() {
        assert_eq!(
            parse_callback("/login?code=abc&state=s1", "s1"),
            Callback::Code("abc".into())
        );
        assert_eq!(
            parse_callback("/login?code=abc&state=bad", "s1"),
            Callback::Ignore
        );
        assert_eq!(parse_callback("/favicon.ico", "s1"), Callback::Ignore);
        assert_eq!(
            parse_callback("/login?error=access_denied&state=s1", "s1"),
            Callback::Denied("access_denied".into())
        );
        assert_eq!(parse_callback("garbage", "s1"), Callback::Ignore);
    }

    #[test]
    fn authorize_url_contains_pkce_params() {
        let u = authorize_url("cid", "http://127.0.0.1:1/login", &["a", "b"], "ch", "st").unwrap();
        assert!(u.starts_with(AUTHORIZE_URL));
        assert!(u.contains("code_challenge=ch"));
        assert!(u.contains("code_challenge_method=S256"));
        assert!(u.contains("scope=a+b"));
        assert!(u.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A1%2Flogin"));
    }

    #[test]
    fn credentials_roundtrip() {
        let json = serde_json::json!({"username": "me", "auth_type": 1, "auth_data": "aGVsbG8="});
        let c: Credentials = serde_json::from_value(json).unwrap();
        assert_eq!(c.auth_data, b"hello");
        let s = encode_credentials(&c).unwrap();
        assert_eq!(decode_credentials(&s).unwrap(), c);
    }

    #[cfg(unix)]
    #[test]
    fn secret_files_are_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::new(dir.path().join("s"));
        store.save("k", "v").unwrap();
        assert_eq!(store.load("k").as_deref(), Some("v"));
        let mode = std::fs::metadata(dir.path().join("s/k.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        store.delete("k");
        assert!(store.load("k").is_none());
    }
}
