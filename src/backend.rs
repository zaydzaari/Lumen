//! Login orchestration: turns stored or fresh credentials into a running backend
//! (connected session, Spotify Connect device, playback engine, Web API client).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context as _, Result};
use librespot_core::authentication::Credentials;
use librespot_core::cache::Cache;
use librespot_core::{Session, SessionConfig};

use crate::api::Api;
use crate::auth::{self, SecretStore, StoredWebToken, TokenManager, KEY_SESSION, KEY_WEB_TOKEN};
use crate::config::{Config, Paths};
use crate::events::{AppEvent, UiTx};
use crate::player::{self, AudioSettings, PlayerHandle, StartOptions};

/// Everything the UI needs once logged in.
pub struct Backend {
    pub api: Arc<Api>,
    pub player: PlayerHandle,
    pub session: Session,
    pub connect_enabled: bool,
    pub username: String,
    pub avatar: Option<String>,
    pub user_id: Option<String>,
}

impl Backend {
    pub fn shutdown(&self) {
        self.player.send(player::PlayerCmd::DetachConnection);
        if !self.session.is_invalid() {
            self.session.shutdown();
        }
    }
}

/// Process-wide services.
#[derive(Clone)]
pub struct Shared {
    pub rt: tokio::runtime::Handle,
    pub http: reqwest::Client,
    pub paths: Paths,
    pub store: SecretStore,
}

impl Shared {
    pub fn new(rt: tokio::runtime::Handle, paths: Paths) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!(
                "Lumen/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/lumen-player/lumen)"
            ))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(2)
            .build()
            .context("creating HTTP client")?;
        let store = SecretStore::new(paths.secrets_dir());
        Ok(Self {
            rt,
            http,
            paths,
            store,
        })
    }

    pub fn has_stored_login(&self) -> bool {
        self.store.load(KEY_SESSION).is_some()
    }

    pub fn logout(&self) {
        self.store.delete(KEY_SESSION);
        self.store.delete(KEY_WEB_TOKEN);
    }
}

fn audio_settings(cfg: &Config) -> AudioSettings {
    AudioSettings {
        bitrate: cfg.audio.normalized_bitrate(),
        normalisation: cfg.audio.normalisation,
        volume_level_db: match cfg.audio.volume_level.as_str() {
            "loud" => 3,
            "quiet" => -6,
            _ => 0,
        },
        gapless: cfg.audio.gapless,
    }
}

pub fn audio_settings_of(cfg: &Config) -> AudioSettings {
    audio_settings(cfg)
}

/// Connect the Web API without stopping playback or discarding session credentials.
pub async fn authorize_library(
    shared: Shared,
    cfg: Config,
    session: Session,
    ui: UiTx,
) -> Result<Arc<Api>> {
    anyhow::ensure!(
        cfg.client_id.len() == 32 && cfg.client_id.chars().all(|c| c.is_ascii_hexdigit()),
        "Enter your Spotify developer app's 32-character client ID in Settings."
    );
    ui.send(AppEvent::LoginStatus(
        "Approve library access in your browser…".into(),
    ));
    let ui2 = ui.clone();
    let token = auth::pkce_login(
        &shared.http,
        &cfg.client_id,
        cfg.redirect_port,
        auth::WEB_API_SCOPES,
        |url| ui2.send(AppEvent::LoginUrl(url)),
    )
    .await?;
    let refresh = token
        .refresh_token
        .context("Spotify did not return a refresh token")?;
    let manager = TokenManager::with_client(
        shared.http.clone(),
        shared.store.clone(),
        cfg.client_id.clone(),
        refresh.clone(),
        Some((token.access_token, Duration::from_secs(token.expires_in))),
    );
    let api = Api::new(shared.http, manager, session);
    api.me()
        .await
        .context("Spotify library authorization was refused")?;
    shared.store.save(
        KEY_WEB_TOKEN,
        &serde_json::to_string(&StoredWebToken {
            client_id: cfg.client_id,
            refresh_token: refresh,
        })?,
    )?;
    Ok(api)
}

/// Log in. With `interactive == false` only stored credentials are used (auto-login
/// on startup); otherwise missing credentials are obtained through the browser.
pub async fn login(
    shared: Shared,
    cfg: Config,
    ui: UiTx,
    interactive: bool,
    existing: PlayerHandle,
) -> Result<Backend> {
    let http = &shared.http;
    let store = &shared.store;

    // 1. Streaming session credentials.
    let stored = store
        .load(KEY_SESSION)
        .and_then(|s| auth::decode_credentials(&s).ok());
    let had_stored = stored.is_some();
    let credentials = match stored {
        Some(c) => c,
        None => {
            if !interactive {
                return Err(anyhow!("not logged in"));
            }
            ui.send(AppEvent::LoginStatus(
                "Approve Lumen in your browser…".into(),
            ));
            let ui2 = ui.clone();
            let tok = auth::pkce_login(
                http,
                auth::SPOTIFY_CLIENT_ID,
                0,
                auth::STREAMING_SCOPES,
                |u| ui2.send(AppEvent::LoginUrl(u)),
            )
            .await
            .context("Spotify login")?;
            Credentials::with_access_token(tok.access_token)
        }
    };

    // Optional Web API authorization must not prevent streaming/session browsing.
    // Missing authorization is connected separately from Settings, while music plays.
    let web: Option<(String, Option<(String, Duration)>)> = if cfg.client_id.is_empty() {
        None
    } else {
        let stored: Option<StoredWebToken> = store
            .load(KEY_WEB_TOKEN)
            .and_then(|s| serde_json::from_str(&s).ok())
            .filter(|t: &StoredWebToken| t.client_id == cfg.client_id);
        stored.map(|t| (t.refresh_token, None))
    };

    // 3. Connect session + Spotify Connect + playback engine.
    ui.send(AppEvent::LoginStatus("Connecting to Spotify…".into()));
    let audio_dir = cfg
        .cache
        .audio_cache
        .then(|| shared.paths.audio_cache_dir());
    let limit = cfg.cache.audio_cache_mb * 1024 * 1024;
    let device_id = cfg.device_id.clone();
    let make_session = move || {
        let cache = match Cache::new(
            None::<std::path::PathBuf>,
            None,
            audio_dir.clone(),
            Some(limit),
        ) {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!("audio cache disabled: {e}");
                None
            }
        };
        let config = SessionConfig {
            device_id: device_id.clone(),
            ..SessionConfig::default()
        };
        Session::new(config, cache)
    };
    let started = match player::start(
        make_session,
        credentials,
        StartOptions {
            device_name: cfg.device_name.clone(),
            audio: audio_settings(&cfg),
            volume_percent: cfg.audio.volume,
        },
        ui.clone(),
        existing,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            if had_stored {
                // Saved credentials were rejected (revoked / expired).
                store.delete(KEY_SESSION);
                return Err(e.context("your saved login is no longer valid — please log in again"));
            }
            return Err(e);
        }
    };

    match auth::reusable_credentials(&started.session).and_then(|c| auth::encode_credentials(&c)) {
        Ok(s) => {
            if let Err(e) = store.save(KEY_SESSION, &s) {
                tracing::warn!("could not persist credentials: {e:#}");
            }
        }
        Err(e) => tracing::warn!("no reusable credentials: {e:#}"),
    }

    let token = match web {
        Some((refresh, initial)) => TokenManager::with_client(
            http.clone(),
            store.clone(),
            cfg.client_id.clone(),
            refresh,
            initial,
        ),
        None => TokenManager::with_session(http.clone(), store.clone(), started.session.clone()),
    };
    let api = Api::new(http.clone(), token, started.session.clone());
    let (username, avatar, user_id) = match api.me().await {
        Ok(me) => (me.display_name, me.avatar, Some(me.id)),
        Err(e) => {
            tracing::warn!("Web API check failed: {e:#}");
            ui.toast(
                crate::events::ToastKind::Error,
                format!("Library access failed: {e:#}"),
            );
            (started.session.username(), None, None)
        }
    };

    Ok(Backend {
        api,
        player: started.handle,
        session: started.session,
        connect_enabled: started.connect_enabled,
        username,
        avatar,
        user_id,
    })
}
