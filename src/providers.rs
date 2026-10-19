//! Lumen's adapter for the GPL Sonora provider engines. Protected providers are
//! streamed through their account/CDM paths, never exported by the download worker.
use crate::model::{Track, TrackRef};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use sonora_music::{MusicApi, MusicProvider, PlaybackEvent, SignIn};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Downloads,
    Youtube,
    Deezer,
    Apple,
    Subsonic,
}
impl Kind {
    pub const ALL: [Self; 5] = [
        Self::Downloads,
        Self::Youtube,
        Self::Deezer,
        Self::Apple,
        Self::Subsonic,
    ];
    pub fn slug(self) -> &'static str {
        match self {
            Self::Downloads => "downloads",
            Self::Youtube => "youtube",
            Self::Deezer => "deezer",
            Self::Apple => "apple",
            Self::Subsonic => "subsonic",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Downloads => "Download connection / offline files",
            Self::Youtube => "YouTube Music",
            Self::Deezer => "Deezer",
            Self::Apple => "Apple Music",
            Self::Subsonic => "Subsonic / Navidrome",
        }
    }
    pub fn provider(self) -> Option<Box<dyn MusicProvider>> {
        Some(match self {
            Self::Downloads => return None,
            Self::Youtube => Box::new(sonora_music::youtube::YouTubeProvider::new()),
            Self::Deezer => Box::new(sonora_music::deezer::DeezerProvider::new()),
            Self::Apple => Box::new(sonora_music::apple::AppleProvider::new()),
            Self::Subsonic => Box::new(sonora_music::subsonic::SubsonicProvider::new()),
        })
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub kind: Kind,
    pub server: String,
    pub username: String,
}
pub struct Connected {
    pub kind: Kind,
    pub session: sonora_music::ProviderSession,
}

pub async fn connect(
    settings: Settings,
    secret: String,
    restore: bool,
    prompt: sonora_music::PromptSink,
    input: sonora_music::InputSource,
) -> Result<Connected> {
    let kind = settings.kind;
    let provider = kind.provider().context("Choose a streaming provider")?;
    anyhow::ensure!(secret.len() <= 32768, "Provider credential is too large");
    if restore || (secret.is_empty() && kind != Kind::Youtube) {
        if let Some(session) = provider.restore().await? {
            return Ok(Connected { kind, session });
        }
    }
    let method = match kind {
        Kind::Youtube if secret.is_empty() => SignIn::Anonymous,
        Kind::Youtube | Kind::Deezer | Kind::Apple => {
            anyhow::ensure!(
                !secret.trim().is_empty(),
                "Sign in to the service, then enter its account credential locally"
            );
            SignIn::Secret
        }
        Kind::Subsonic => {
            let url = url::Url::parse(settings.server.trim())
                .context("Enter a complete server URL, including https://")?;
            anyhow::ensure!(
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "Enter the server address without credentials or query parameters"
            );
            SignIn::Credentials {
                server: settings.server,
                username: settings.username,
                password: secret.clone(),
            }
        }
        Kind::Downloads => unreachable!(),
    };
    let session = provider.sign_in(method, prompt, input).await?;
    Ok(Connected { kind, session })
}

pub fn track(kind: Kind, source: sonora_music::Track) -> Option<TrackRef> {
    let id = source.id.filter(|_| source.playable)?;
    Some(Arc::new(Track {
        uri: format!("native:{}:{id}", kind.slug()),
        name: source.name,
        artists: source.artists,
        album: source.album,
        duration_ms: source.duration.as_millis().min(u128::from(u32::MAX)) as u32,
        image: if kind == Kind::Subsonic {
            None
        } else {
            source.cover.clone()
        },
        image_large: if kind == Kind::Subsonic {
            None
        } else {
            source.cover
        },
        explicit: source.explicit,
        ..Default::default()
    }))
}
pub enum UiEvent {
    Prompt {
        generation: u64,
        prompt: sonora_music::SignInPrompt,
    },
    Connected {
        generation: u64,
        result: Result<Arc<Connected>, String>,
    },
    Search {
        generation: u64,
        query: String,
        result: Result<Vec<TrackRef>, String>,
    },
    Library {
        generation: u64,
        result: Result<crate::free_library::Playlist, String>,
    },
    CdmOffer(Result<sonora_music::drm::Offer, String>),
    CdmInstalled(bool),
}
fn direct_id(kind: Kind, uri: &str) -> Option<&str> {
    uri.strip_prefix("native:")?
        .strip_prefix(kind.slug())?
        .strip_prefix(':')
        .filter(|id| !id.is_empty())
}
pub fn valid_uri(uri: &str) -> bool {
    uri.len() <= 4096
        && !uri.chars().any(char::is_control)
        && Kind::ALL
            .into_iter()
            .filter(|k| *k != Kind::Downloads)
            .any(|kind| direct_id(kind, uri).is_some())
}
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn matches(original: &Track, candidate: &sonora_music::Track) -> bool {
    if !candidate.playable || candidate.id.is_none() {
        return false;
    }
    let name = words(&original.name);
    let other = words(&candidate.name);
    let artist = words(
        original
            .artists
            .split(',')
            .next()
            .unwrap_or(&original.artists),
    );
    let authors = words(&candidate.artists);
    if name.is_empty()
        || artist.is_empty()
        || !name.iter().all(|w| other.contains(w))
        || !artist.iter().all(|w| authors.contains(w))
    {
        return false;
    }
    if [
        "cover",
        "karaoke",
        "live",
        "remix",
        "instrumental",
        "slowed",
        "sped",
    ]
    .iter()
    .any(|tag| other.iter().any(|w| w == tag) && !name.iter().any(|w| w == tag))
    {
        return false;
    }
    original.duration_ms == 0
        || (candidate.duration.as_secs_f64() - f64::from(original.duration_ms) / 1000.).abs() <= 12.
}
async fn resolve(api: &dyn MusicApi, kind: Kind, original: &Track) -> Result<String> {
    if let Some(id) = direct_id(kind, &original.uri) {
        return Ok(id.to_owned());
    }
    anyhow::ensure!(
        !original.uri.starts_with("native:"),
        "Select the matching music provider for this track"
    );
    anyhow::ensure!(
        !original.is_stub(),
        "This song needs its title and artist before matching"
    );
    let found = api
        .search(&format!("{} {}", original.name, original.artists))
        .await?;
    let candidate = found
        .into_iter()
        .filter(|t| matches(original, t))
        .min_by_key(|t| {
            t.duration
                .as_millis()
                .abs_diff(u128::from(original.duration_ms))
        });
    candidate
        .and_then(|t| t.id)
        .context("No matching full-length recording was found on this provider")
}

pub enum Event {
    Resolved {
        generation: u64,
        result: Result<String>,
    },
    Prefetched {
        generation: u64,
        uri: String,
        result: Result<String>,
    },
    Playback(PlaybackEvent),
}
pub struct Engine {
    pub player: Box<dyn sonora_music::Player>,
    pub rx: mpsc::UnboundedReceiver<Event>,
    tx: mpsc::UnboundedSender<Event>,
    api: Arc<dyn MusicApi>,
    pub kind: Kind,
    pub generation: u64,
    pub uri: Option<String>,
    pub id: Option<String>,
    pub playing: bool,
    pub position_ms: u32,
    cache: HashMap<String, String>,
    resolve: Option<JoinHandle<()>>,
    prefetch: Option<JoinHandle<()>>,
    prefetched: Option<String>,
    events: JoinHandle<()>,
}
impl Engine {
    pub fn new(connected: &Connected, audio: &crate::player::AudioSettings, volume: u16) -> Self {
        let (player, mut events) = connected
            .session
            .playback
            .start(sonora_music::PlaybackConfig {
                normalisation: audio.normalisation,
                gapless: audio.gapless,
                position_interval: Duration::from_millis(500),
                gain: f32::from(volume) / 65535.,
                equalizer: Default::default(),
            });
        let (tx, rx) = mpsc::unbounded_channel();
        let output = tx.clone();
        let events = tokio::spawn(async move {
            while let Some(event) = events.next().await {
                if output.send(Event::Playback(event)).is_err() {
                    break;
                }
            }
        });
        Self {
            player,
            rx,
            tx,
            api: connected.session.api.clone(),
            kind: connected.kind,
            generation: 0,
            uri: None,
            id: None,
            playing: false,
            position_ms: 0,
            cache: HashMap::new(),
            resolve: None,
            prefetch: None,
            prefetched: None,
            events,
        }
    }
    pub fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.player.pause();
        self.uri = None;
        self.id = None;
        if let Some(task) = self.resolve.take() {
            task.abort();
        }
        if let Some(task) = self.prefetch.take() {
            task.abort();
        }
        self.prefetched = None;
    }
    pub fn begin(&mut self, track: TrackRef, playing: bool, position_ms: u32) {
        self.reset();
        self.uri = Some(track.uri.clone());
        self.playing = playing;
        self.position_ms = position_ms;
        let generation = self.generation;
        if let Some(id) = self.cache.get(&track.uri) {
            let _ = self.tx.send(Event::Resolved {
                generation,
                result: Ok(id.clone()),
            });
            return;
        }
        let (api, kind, tx) = (self.api.clone(), self.kind, self.tx.clone());
        self.resolve = Some(tokio::spawn(async move {
            let result =
                tokio::time::timeout(Duration::from_secs(25), resolve(&*api, kind, &track))
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Provider lookup timed out")));
            let _ = tx.send(Event::Resolved { generation, result });
        }));
    }
    pub fn resolved(&mut self, generation: u64, id: String) -> Result<()> {
        anyhow::ensure!(self.generation == generation, "Stale provider lookup");
        if self.cache.len() >= 256 {
            self.cache.clear();
        }
        if let Some(uri) = &self.uri {
            self.cache.insert(uri.clone(), id.clone());
        }
        let at = Duration::from_millis(u64::from(self.position_ms));
        self.id = Some(id.clone());
        if self.playing {
            self.player.load(&id, at, false)
        } else {
            self.player.load_paused_at(&id, at)
        }
    }
    pub fn prefetch(&mut self, track: TrackRef) {
        if self.prefetched.as_ref() == Some(&track.uri) {
            return;
        }
        self.prefetched = Some(track.uri.clone());
        let (api, kind, tx, generation) = (
            self.api.clone(),
            self.kind,
            self.tx.clone(),
            self.generation,
        );
        self.prefetch = Some(tokio::spawn(async move {
            let result =
                tokio::time::timeout(Duration::from_secs(20), resolve(&*api, kind, &track))
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Provider preload timed out")));
            let _ = tx.send(Event::Prefetched {
                generation,
                uri: track.uri.clone(),
                result,
            });
        }));
    }
    pub fn prefetched(&mut self, generation: u64, uri: String, id: String) {
        if self.generation != generation {
            return;
        }
        if self.cache.len() >= 256 {
            self.cache.clear();
        }
        self.cache.insert(uri, id.clone());
        let _ = self.player.preload(&id, false);
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.reset();
        self.events.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_uris_preserve_opaque_ids_and_cannot_cross_providers() {
        assert_eq!(direct_id(Kind::Youtube, "native:youtube:a:b"), Some("a:b"));
        assert_eq!(direct_id(Kind::Apple, "native:youtube:abc"), None);
        assert_eq!(direct_id(Kind::Youtube, "native:youtube:"), None);
    }
    #[test]
    fn configuration_never_serializes_a_service_secret() {
        let text = toml::to_string(&Settings {
            kind: Kind::Deezer,
            ..Default::default()
        })
        .unwrap();
        assert!(!text.contains("password"));
        assert!(!text.contains("cookie"));
        assert!(text.contains("deezer"));
    }

    fn candidate(name: &str, artist: &str, seconds: u64) -> sonora_music::Track {
        sonora_music::Track {
            id: Some("recording".into()),
            name: name.into(),
            artists: artist.into(),
            playable: true,
            duration: Duration::from_secs(seconds),
            album: "Album".into(),
            artist_refs: vec![],
            album_id: None,
            cover: None,
            added_at: None,
            added_by: None,
            playcount: None,
            popularity: 0,
            explicit: false,
            track_number: 0,
            disc_number: 0,
            tags: vec![],
            languages: vec![],
            credits: vec![],
        }
    }

    #[test]
    fn matching_rejects_wrong_recordings_and_accepts_unicode_titles() {
        let original = Track {
            name: "The Night We Met".into(),
            artists: "Lord Huron".into(),
            duration_ms: 208_000,
            ..Default::default()
        };
        assert!(matches(
            &original,
            &candidate("The Night We Met (Official Audio)", "Lord Huron", 208)
        ));
        for (name, artist, duration) in [
            ("The Night We Met cover", "Lord Huron", 208),
            ("The Night We Met", "Another Artist", 208),
            ("The Night We Met", "Lord Huron", 30),
            ("The Night We Met live", "Lord Huron", 208),
        ] {
            assert!(!matches(&original, &candidate(name, artist, duration)));
        }
        let original = Track {
            name: "يا الرايح".into(),
            artists: "رشيد طه".into(),
            ..Default::default()
        };
        assert!(matches(&original, &candidate("يا الرايح", "رشيد طه", 200)));
    }

    #[test]
    fn server_cover_credentials_are_never_exported_into_track_metadata() {
        let mut source = candidate("Song", "Artist", 180);
        source.cover = Some("https://server.invalid/cover?u=user&p=secret".into());
        let converted = track(Kind::Subsonic, source).unwrap();
        assert!(converted.image.is_none());
        assert!(converted.image_large.is_none());
        assert_eq!(converted.uri, "native:subsonic:recording");
        assert!(valid_uri(&converted.uri));
        assert!(!valid_uri("native:unknown:recording"));
        assert!(!valid_uri("native:youtube:bad\nrecording"));
    }

    #[tokio::test]
    #[ignore = "live guest catalogue request; uses an isolated temporary provider directory"]
    async fn live_youtube_guest_catalogue() {
        let directory = tempfile::tempdir().unwrap();
        sonora_music::credentials::configure_root(directory.path().to_path_buf());
        let (_tx, input) = mpsc::unbounded_channel();
        let connected = tokio::time::timeout(
            Duration::from_secs(45),
            connect(
                Settings {
                    kind: Kind::Youtube,
                    ..Default::default()
                },
                String::new(),
                false,
                Arc::new(|_| {}),
                input,
            ),
        )
        .await
        .expect("guest connection timed out")
        .expect("guest connection failed");
        let results = tokio::time::timeout(
            Duration::from_secs(30),
            connected.session.api.search("Lord Huron The Night We Met"),
        )
        .await
        .expect("guest search timed out")
        .expect("guest search failed");
        assert!(results
            .into_iter()
            .any(|t| track(Kind::Youtube, t).is_some()));
    }
}
