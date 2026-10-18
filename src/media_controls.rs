//! OS media keys and "now playing" integration via `souvlaki`
//! (SMTC on Windows, MPRIS on Linux, MPNowPlayingInfoCenter on macOS).

use std::time::Duration;

use crate::events::{AppEvent, UiTx};
use crate::player::{PlayerCmd, Snapshot};

#[cfg(feature = "media-controls")]
pub struct MediaControls {
    inner: souvlaki::MediaControls,
    last_uri: Option<String>,
    last_playing: Option<bool>,
}

#[cfg(feature = "media-controls")]
impl MediaControls {
    /// `hwnd` is required on Windows (the main window handle).
    pub fn new(ui: UiTx, hwnd: Option<*mut std::ffi::c_void>) -> Option<Self> {
        // souvlaki's MPRIS thread panics if no D-Bus session bus exists (and
        // release builds abort on panic), so check for one first.
        #[cfg(target_os = "linux")]
        if !session_bus_available() {
            tracing::info!("no D-Bus session bus; media keys (MPRIS) disabled");
            return None;
        }
        let config = souvlaki::PlatformConfig {
            display_name: "Lumen",
            dbus_name: "lumen",
            hwnd,
        };
        let mut inner = match souvlaki::MediaControls::new(config) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("media controls unavailable: {e:?}");
                return None;
            }
        };
        let res = inner.attach(move |event: souvlaki::MediaControlEvent| {
            use souvlaki::{MediaControlEvent as E, SeekDirection};
            let cmd = match event {
                E::Play => PlayerCmd::Play,
                E::Pause | E::Stop => PlayerCmd::Pause,
                E::Toggle => PlayerCmd::TogglePlay,
                E::Next => PlayerCmd::Next,
                E::Previous => PlayerCmd::Previous,
                E::SetPosition(p) => PlayerCmd::Seek(p.0.as_millis() as u32),
                E::SetVolume(v) => PlayerCmd::SetVolume((v.clamp(0.0, 1.0) * 65535.0) as u16),
                E::Seek(dir) | E::SeekBy(dir, _) => {
                    // Relative seeks are resolved by the UI (it knows the position).
                    let delta: i64 = if matches!(dir, SeekDirection::Forward) {
                        10_000
                    } else {
                        -10_000
                    };
                    ui.send(AppEvent::SeekBy(delta));
                    return;
                }
                E::Raise => {
                    ui.send(AppEvent::TrayShow);
                    return;
                }
                E::Quit => {
                    ui.send(AppEvent::TrayQuit);
                    return;
                }
                E::OpenUri(_) => return,
            };
            ui.send(AppEvent::Remote(cmd));
        });
        if let Err(e) = res {
            tracing::warn!("media controls attach failed: {e:?}");
            return None;
        }
        Some(Self {
            inner,
            last_uri: None,
            last_playing: None,
        })
    }

    /// Push the current state to the OS (only when something changed).
    pub fn update(&mut self, snap: &Snapshot) {
        let uri = snap.track.as_ref().map(|t| t.uri.clone());
        if uri != self.last_uri {
            if let Some(t) = &snap.track {
                let _ = self.inner.set_metadata(souvlaki::MediaMetadata {
                    title: Some(t.display_name()),
                    artist: Some(&t.artists),
                    album: Some(&t.album),
                    cover_url: t.image_large.as_deref(),
                    duration: Some(Duration::from_millis(t.duration_ms as u64)),
                });
            }
            self.last_uri = uri;
            self.last_playing = None;
        }
        if self.last_playing != Some(snap.playing) {
            let progress = Some(souvlaki::MediaPosition(Duration::from_millis(
                snap.position_now() as u64,
            )));
            let pb = if snap.track.is_none() {
                souvlaki::MediaPlayback::Stopped
            } else if snap.playing {
                souvlaki::MediaPlayback::Playing { progress }
            } else {
                souvlaki::MediaPlayback::Paused { progress }
            };
            let _ = self.inner.set_playback(pb);
            self.last_playing = Some(snap.playing);
        }
    }
}

#[cfg(not(feature = "media-controls"))]
pub struct MediaControls;

#[cfg(not(feature = "media-controls"))]
impl MediaControls {
    pub fn new(_ui: UiTx, _hwnd: Option<*mut std::ffi::c_void>) -> Option<Self> {
        None
    }
    pub fn update(&mut self, _snap: &Snapshot) {}
}

/// Whether a D-Bus session bus is reachable (Linux MPRIS).
#[cfg(all(target_os = "linux", feature = "media-controls"))]
fn session_bus_available() -> bool {
    use std::os::unix::net::UnixStream;
    let addr = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
    let path = match addr.as_deref() {
        Some(a) => {
            if let Some(p) = a
                .split(';')
                .filter_map(|part| part.strip_prefix("unix:"))
                .flat_map(|kv| kv.split(','))
                .find_map(|kv| kv.strip_prefix("path="))
            {
                std::path::PathBuf::from(p)
            } else {
                // abstract sockets / tcp: can't check cheaply, assume usable
                return true;
            }
        }
        None => match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(d) => std::path::Path::new(&d).join("bus"),
            None => return false,
        },
    };
    UnixStream::connect(path).is_ok()
}
