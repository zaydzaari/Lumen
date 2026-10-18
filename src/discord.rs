//! Custom Discord Rich Presence ("Listening to …" with album art, progress and
//! a button). Runs on its own thread; the UI only sends small update messages.
//!
//! Users create their own Discord application (the application's name is what
//! Discord shows after "Listening to") and paste its Application ID in
//! Settings → Discord. Text fields are templates with `{title}`, `{artist}`,
//! `{album}` placeholders.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{
    Activity, ActivityType, Assets, Button, StatusDisplayType, Timestamps,
};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

use crate::config::DiscordConfig;
use crate::player::Snapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presence {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub image: Option<String>,
    /// `spotify:track:…` (or `local:…`).
    pub uri: String,
    pub playing: bool,
    /// Track start / end as Unix milliseconds (for Discord's progress bar).
    pub start_ms: i64,
    pub end_ms: i64,
}

impl Presence {
    /// Build from a playback snapshot; `None` when nothing is loaded.
    pub fn from_snapshot(s: &Snapshot) -> Option<Self> {
        let t = s.track.as_ref().filter(|t| !t.is_stub())?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let start = now - s.position_now() as i64;
        Some(Self {
            title: t.name.clone(),
            artist: t.artists.clone(),
            album: t.album.clone(),
            // Discord can only show images it can fetch over https.
            image: t
                .image_large
                .clone()
                .or_else(|| t.image.clone())
                .filter(|u| u.starts_with("https://")),
            uri: t.uri.clone(),
            playing: s.playing,
            start_ms: start,
            end_ms: start + t.duration_ms as i64,
        })
    }

    /// Whether a new presence differs enough to be worth sending (Discord
    /// rate-limits updates, so ignore sub-3-second timestamp drift).
    pub fn differs(&self, other: &Presence) -> bool {
        self.uri != other.uri
            || self.playing != other.playing
            || (self.start_ms - other.start_ms).abs() > 3000
    }
}

/// Fill `{title}`, `{artist}`, `{album}` and clamp to Discord's 2..=128 chars.
pub fn render(template: &str, p: &Presence) -> String {
    let s = template
        .replace("{title}", &p.title)
        .replace("{artist}", &p.artist)
        .replace("{album}", &p.album);
    let mut s: String = s.trim().chars().take(128).collect();
    while s.chars().count() < 2 {
        s.push(' ');
    }
    s
}

/// Spotify web link for a track URI (for the presence button).
pub fn open_url(uri: &str) -> Option<String> {
    let id = uri.strip_prefix("spotify:track:")?;
    Some(format!("https://open.spotify.com/track/{id}"))
}

enum Msg {
    Update(Option<Presence>),
    Config(DiscordConfig),
}

/// Handle to the presence thread.
pub struct Discord {
    tx: Sender<Msg>,
    last: Option<Presence>,
    pub status: std::sync::Arc<std::sync::Mutex<String>>,
}

impl Discord {
    pub fn start(cfg: DiscordConfig) -> Self {
        let (tx, rx) = mpsc::channel();
        let status = std::sync::Arc::new(std::sync::Mutex::new(String::from("Not connected")));
        let st = status.clone();
        let spawned = std::thread::Builder::new()
            .name("discord-rpc".into())
            .spawn(move || worker(rx, cfg, st));
        if let Err(e) = spawned {
            tracing::warn!("discord presence thread failed to start: {e}");
        }
        Self {
            tx,
            last: None,
            status,
        }
    }

    pub fn update(&mut self, p: Option<Presence>) {
        let changed = match (&self.last, &p) {
            (Some(a), Some(b)) => a.differs(b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            self.last = p.clone();
            let _ = self.tx.send(Msg::Update(p));
        }
    }

    pub fn set_config(&mut self, cfg: DiscordConfig) {
        let _ = self.tx.send(Msg::Config(cfg));
        // Re-send current presence with the new settings.
        let last = self.last.clone();
        let _ = self.tx.send(Msg::Update(last));
    }

    pub fn status(&self) -> String {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

fn set_status(st: &std::sync::Mutex<String>, s: impl Into<String>) {
    if let Ok(mut g) = st.lock() {
        *g = s.into();
    }
}

fn worker(
    rx: Receiver<Msg>,
    mut cfg: DiscordConfig,
    status: std::sync::Arc<std::sync::Mutex<String>>,
) {
    let mut client: Option<DiscordIpcClient> = None;
    let mut current: Option<Presence> = None;
    let mut dirty = true;
    loop {
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Msg::Update(p)) => {
                current = p;
                dirty = true;
            }
            Ok(Msg::Config(c)) => {
                if c.client_id != cfg.client_id || !c.enabled {
                    if let Some(mut cl) = client.take() {
                        let _ = cl.clear_activity();
                        let _ = cl.close();
                    }
                }
                cfg = c;
                dirty = true;
            }
            Err(RecvTimeoutError::Timeout) => {} // retry a pending update / reconnect
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if !dirty {
            continue;
        }
        let id = cfg.client_id.trim().to_string();
        if !cfg.enabled || id.is_empty() {
            set_status(
                &status,
                if cfg.enabled {
                    "Add your Discord Application ID"
                } else {
                    "Off"
                },
            );
            dirty = false;
            continue;
        }
        if client.is_none() {
            let mut c = DiscordIpcClient::new(&id);
            match c.connect() {
                Ok(()) => {
                    tracing::info!("connected to Discord");
                    set_status(&status, "Connected");
                    client = Some(c);
                }
                Err(e) => {
                    tracing::debug!("Discord not reachable: {e}");
                    set_status(&status, "Discord is not running");
                    continue; // keep dirty, retry on next tick
                }
            }
        }
        let Some(c) = client.as_mut() else { continue };
        let res = match current
            .as_ref()
            .filter(|p| p.playing || cfg.show_when_paused)
        {
            None => c.clear_activity(),
            Some(p) => {
                let details = render(&cfg.details, p);
                let state = render(&cfg.state, p);
                let large_text = render(&cfg.large_text, p);
                let mut act = Activity::new()
                    .activity_type(ActivityType::Listening)
                    .status_display_type(StatusDisplayType::Details)
                    .details(details.as_str())
                    .state(state.as_str());
                if !cfg.activity_name.trim().is_empty() {
                    act = act.name(cfg.activity_name.trim());
                }
                let mut assets = Assets::new().large_text(large_text.as_str());
                if cfg.show_album_art {
                    if let Some(img) = &p.image {
                        assets = assets.large_image(img.as_str());
                    }
                }
                if !p.playing {
                    assets = assets.small_text("Paused");
                }
                act = act.assets(assets);
                if cfg.show_progress && p.playing && p.end_ms > p.start_ms {
                    act = act.timestamps(Timestamps::new().start(p.start_ms).end(p.end_ms));
                }
                let url = open_url(&p.uri);
                if cfg.show_button {
                    if let Some(u) = &url {
                        act = act.buttons(vec![Button::new("Open in Spotify", u.as_str())]);
                    }
                }
                c.set_activity(act)
            }
        };
        match res {
            Ok(()) => dirty = false,
            Err(e) => {
                tracing::debug!("Discord update failed, reconnecting: {e}");
                set_status(&status, "Reconnecting…");
                if let Some(mut cl) = client.take() {
                    let _ = cl.close();
                }
            }
        }
    }
    if let Some(mut cl) = client.take() {
        let _ = cl.clear_activity();
        let _ = cl.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> Presence {
        Presence {
            title: "Baida mon amour".into(),
            artist: "Cheb Hasni".into(),
            album: "L'album d'or".into(),
            image: None,
            uri: "spotify:track:abc".into(),
            playing: true,
            start_ms: 1000,
            end_ms: 200_000,
        }
    }

    #[test]
    fn templates_fill_and_clamp() {
        assert_eq!(render("{title}", &p()), "Baida mon amour");
        assert_eq!(
            render("by {artist} — {album}", &p()),
            "by Cheb Hasni — L'album d'or"
        );
        assert_eq!(render("", &p()).chars().count(), 2);
        let long = "x".repeat(500);
        assert_eq!(render(&long, &p()).chars().count(), 128);
    }

    #[test]
    fn change_detection_ignores_small_drift() {
        let a = p();
        let mut b = p();
        b.start_ms += 1500;
        assert!(!a.differs(&b));
        b.start_ms += 5000;
        assert!(a.differs(&b));
        let mut c = p();
        c.playing = false;
        assert!(a.differs(&c));
    }

    #[test]
    fn spotify_links() {
        assert_eq!(
            open_url("spotify:track:abc").as_deref(),
            Some("https://open.spotify.com/track/abc")
        );
        assert_eq!(open_url("local:/music/a.mp3"), None);
    }
}
