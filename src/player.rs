//! Playback engine.
//!
//! Two librespot [`Player`]s share one session and one soft mixer:
//!
//! * the **local player**, driven by Lumen's own queue (play from any list,
//!   add/remove/reorder queue, shuffle, repeat off/all/one, gapless preloading);
//! * the **Connect player**, owned by librespot's [`Spirc`], so Lumen shows up as a
//!   Spotify Connect device and can be controlled from the phone app.
//!
//! When a Connect client starts playback on Lumen the engine switches to
//! *Connect mode* (local player paused, UI controls are routed to Spirc). Playing
//! something from Lumen's UI switches back to *local mode*.
//!
//! Both players use [`LazySink`], which opens the audio device only while audio
//! is actually playing, keeping idle memory and CPU low.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use librespot_connect::{ConnectConfig, Spirc};
use librespot_core::authentication::Credentials;
use librespot_core::config::DeviceType;
use librespot_core::{Session, SpotifyUri};
use librespot_metadata::audio::{AudioItem, UniqueFields};
use librespot_playback::audio_backend::{Sink, SinkError, SinkResult};
use librespot_playback::config::{Bitrate, PlayerConfig};
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;
use librespot_playback::mixer::softmixer::SoftMixer;
use librespot_playback::mixer::{Mixer, MixerConfig};
use librespot_playback::player::{Player, PlayerEvent, PlayerEventChannel};
use rand::seq::SliceRandom as _;
use tokio::sync::mpsc;

use crate::events::{ToastKind, UiTx};
use crate::local_playback::{Command as FileCmd, Event as FileEvent, Output as FileOutput};
use crate::model::{Track, TrackRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Local,
    Connect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioSettings {
    pub bitrate: u16,
    pub normalisation: bool,
    /// Spotify's "Volume level" with normalisation: +3 dB loud, 0 normal, -6 dB quiet.
    pub volume_level_db: i8,
    pub gapless: bool,
}

/// Immutable view of the playback state sent to the UI on every change.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub mode: Mode,
    pub track: Option<TrackRef>,
    pub playing: bool,
    pub loading: bool,
    pub position_ms: u32,
    pub at: Option<Instant>,
    pub shuffle: bool,
    pub repeat: Repeat,
    /// 0..=65535
    pub volume: u16,
    /// Explicitly queued tracks (played first).
    pub queue: Vec<TrackRef>,
    /// Next tracks from the context (first [`UPCOMING_PREVIEW`] only).
    pub upcoming: Vec<TrackRef>,
    pub upcoming_total: usize,
    pub context_name: String,
    pub context_key: Option<String>,
    /// Name of the Connect client that controls us (Connect mode).
    pub remote_client: Option<String>,
}

impl Snapshot {
    /// Interpolated playback position.
    pub fn position_now(&self) -> u32 {
        let base = self.position_ms;
        let pos = match (self.playing && !self.loading, self.at) {
            (true, Some(at)) => base.saturating_add(at.elapsed().as_millis() as u32),
            _ => base,
        };
        match &self.track {
            Some(t) if t.duration_ms > 0 => pos.min(t.duration_ms),
            _ => pos,
        }
    }
}

pub const UPCOMING_PREVIEW: usize = 100;
const HISTORY_CAP: usize = 200;

pub enum PlayerCmd {
    Independent(bool),
    PlaybackBuffer {
        uri: String,
        buffer: Arc<crate::progressive::Buffer>,
    },
    DownloadFinished {
        uri: String,
        success: bool,
    },
    DownloadLibrary(Arc<crate::downloads::Library>),
    AttachConnection(Box<Connection>),
    DetachConnection,
    /// Play `tracks[start]` and continue with the rest of the list.
    PlayContext {
        name: String,
        key: Option<String>,
        tracks: Vec<TrackRef>,
        start: usize,
    },
    /// Load the last session's song paused at `position_ms` (resume on startup).
    /// Ignored if something is already playing.
    Restore {
        name: String,
        key: Option<String>,
        tracks: Vec<TrackRef>,
        start: usize,
        position_ms: u32,
    },
    /// More tracks for the context that is currently playing (pagination).
    ExtendContext {
        key: String,
        tracks: Vec<TrackRef>,
    },
    /// Metadata for stub tracks.
    UpdateTracks(Vec<TrackRef>),
    TogglePlay,
    Play,
    Pause,
    Next,
    Previous,
    Seek(u32),
    SetVolume(u16),
    SetShuffle(bool),
    SetRepeat(Repeat),
    AddToQueue(Vec<TrackRef>),
    QueueRemove(usize),
    QueueMove {
        from: usize,
        to: usize,
    },
    UpcomingRemove(usize),
    UpcomingMove {
        from: usize,
        to: usize,
    },
    /// Jump to item `i` of the user queue.
    PlayQueued(usize),
    /// Jump to item `i` of the upcoming list.
    PlayUpcoming(usize),
    ClearQueue,
    ApplyAudio(AudioSettings),
    Shutdown,
}

#[derive(Clone)]
pub struct PlayerHandle {
    tx: mpsc::UnboundedSender<PlayerCmd>,
}

impl PlayerHandle {
    pub fn send(&self, cmd: PlayerCmd) {
        if self.tx.send(cmd).is_err() {
            tracing::warn!("player engine is not running");
        }
    }
}

// ---------------------------------------------------------------- audio sink

/// A rodio/cpal sink that opens the output device on `start()` and closes it on
/// `stop()` (librespot stops the sink when paused or stopped). Errors never
/// panic: if no device is available the audio is dropped and the UI is told.
pub struct LazySink {
    stream: Option<(rodio::Sink, rodio::OutputStream)>,
    on_error: Arc<dyn Fn(String) + Send + Sync>,
    failed: bool,
}

impl LazySink {
    pub fn new(on_error: Arc<dyn Fn(String) + Send + Sync>) -> Self {
        Self {
            stream: None,
            on_error,
            failed: false,
        }
    }

    pub(crate) fn open() -> Result<(rodio::Sink, rodio::OutputStream), String> {
        let mut stream = rodio::OutputStreamBuilder::open_default_stream()
            .map_err(|e| format!("could not open the audio device: {e}"))?;
        stream.log_on_drop(false);
        let sink = rodio::Sink::connect_new(stream.mixer());
        Ok((sink, stream))
    }
}

impl Sink for LazySink {
    fn start(&mut self) -> SinkResult<()> {
        if self.stream.is_none() {
            match Self::open() {
                Ok(s) => {
                    self.failed = false;
                    self.stream = Some(s);
                }
                Err(e) => {
                    if !self.failed {
                        (self.on_error)(e.clone());
                    }
                    self.failed = true;
                    return Err(SinkError::ConnectionRefused(e));
                }
            }
        }
        if let Some((sink, _)) = &self.stream {
            sink.play();
        }
        Ok(())
    }

    fn stop(&mut self) -> SinkResult<()> {
        if let Some((sink, stream)) = self.stream.take() {
            sink.sleep_until_end();
            sink.pause();
            drop(sink);
            drop(stream);
        }
        Ok(())
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        let Some((sink, _)) = &self.stream else {
            // Device unavailable: drop audio rather than failing (librespot
            // terminates the process on write errors).
            return Ok(());
        };
        let Ok(samples) = packet.samples() else {
            return Ok(());
        };
        let samples_f32: &[f32] = &converter.f64_to_f32(samples);
        let source = rodio::buffer::SamplesBuffer::new(
            librespot_playback::NUM_CHANNELS as u16,
            librespot_playback::SAMPLE_RATE,
            samples_f32,
        );
        sink.append(source);
        // Keep roughly half a second buffered.
        while sink.len() > 26 {
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}

fn make_player(
    session: &Session,
    mixer: &Arc<SoftMixer>,
    settings: &AudioSettings,
    ui: &UiTx,
) -> Arc<Player> {
    let config = PlayerConfig {
        bitrate: match settings.bitrate {
            96 => Bitrate::Bitrate96,
            160 => Bitrate::Bitrate160,
            _ => Bitrate::Bitrate320,
        },
        gapless: settings.gapless,
        normalisation: settings.normalisation,
        normalisation_pregain_db: settings.volume_level_db as f64,
        ..PlayerConfig::default()
    };
    let ui = ui.clone();
    let on_error: Arc<dyn Fn(String) + Send + Sync> =
        Arc::new(move |e: String| ui.toast(ToastKind::Error, e));
    Player::new(
        config,
        session.clone(),
        mixer.get_soft_volume(),
        move || Box::new(LazySink::new(on_error)),
    )
}

#[cfg(test)]
pub(crate) async fn smoke_stream(session: &Session, track: &TrackRef) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ui = UiTx::new(tx, egui::Context::default());
    let mixer = Arc::new(SoftMixer::open(MixerConfig::default()).map_err(|e| anyhow!("{e}"))?);
    mixer.set_volume(0);
    let player = make_player(
        session,
        &mixer,
        &AudioSettings {
            bitrate: 160,
            normalisation: false,
            volume_level_db: 0,
            gapless: true,
        },
        &ui,
    );
    let mut events = player.get_player_event_channel();
    player.load(SpotifyUri::from_uri(&track.uri)?, true, 0);
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(event) = events.recv().await {
            match event {
                PlayerEvent::Playing { .. } => {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    return Ok(());
                }
                PlayerEvent::Unavailable { .. } => {
                    return Err(anyhow!("Spotify marked the test track unavailable"))
                }
                _ => {}
            }
        }
        Err(anyhow!("Spotify playback stopped before starting"))
    })
    .await
    .map_err(|_| anyhow!("Spotify playback timed out"));
    player.stop();
    tokio::task::spawn_blocking(move || drop(player)).await?;
    for event in rx.try_iter() {
        if let crate::events::AppEvent::Toast(ToastKind::Error, message) = event {
            return Err(anyhow!(message));
        }
    }
    result?
}

fn volume_from_percent(p: u8) -> u16 {
    (p.min(100) as u32 * 65535 / 100) as u16
}

// ---------------------------------------------------------------- startup

pub struct Started {
    pub handle: PlayerHandle,
    pub session: Session,
    pub connect_enabled: bool,
}

pub struct StartOptions {
    pub device_name: String,
    pub audio: AudioSettings,
    pub volume_percent: u8,
}

/// Connect the session (through Spirc when possible) and start the engine task.
/// `make_session` must return a fresh, *unconnected* session.
pub async fn start(
    make_session: impl Fn() -> Session,
    credentials: Credentials,
    opts: StartOptions,
    ui: UiTx,
    existing: PlayerHandle,
) -> Result<Started> {
    let mixer =
        Arc::new(SoftMixer::open(MixerConfig::default()).map_err(|e| anyhow!("audio mixer: {e}"))?);
    mixer.set_volume(volume_from_percent(opts.volume_percent));

    let session = make_session();
    let connect_player = make_player(&session, &mixer, &opts.audio, &ui);
    let connect_events = connect_player.get_player_event_channel();
    let connect_config = ConnectConfig {
        name: opts.device_name.clone(),
        device_type: DeviceType::Computer,
        initial_volume: volume_from_percent(opts.volume_percent),
        ..ConnectConfig::default()
    };
    let spirc = Spirc::new(
        connect_config,
        session.clone(),
        credentials.clone(),
        connect_player.clone(),
        mixer.clone(),
    )
    .await;

    let (session, spirc, connect_events) = match spirc {
        Ok((spirc, task)) => {
            tokio::spawn(task);
            (session, Some(spirc), Some(connect_events))
        }
        Err(e) => {
            // Connect is a nice-to-have: fall back to a plain session.
            if e.kind == librespot_core::error::ErrorKind::PermissionDenied
                || e.kind == librespot_core::error::ErrorKind::Unauthenticated
            {
                return Err(anyhow!("login failed: {e}"));
            }
            tracing::warn!("Spotify Connect unavailable: {e}");
            ui.toast(ToastKind::Info, format!("Spotify Connect unavailable: {e}"));
            drop(connect_player);
            if !session.is_invalid() {
                session.shutdown();
            }
            let session = make_session();
            session
                .connect(credentials, false)
                .await
                .map_err(|e| anyhow!("login failed: {e}"))?;
            (session, None, None)
        }
    };

    let player = make_player(&session, &mixer, &opts.audio, &ui);
    let events = player.get_player_event_channel();
    let connect_enabled = spirc.is_some();
    existing.send(PlayerCmd::AttachConnection(Box::new(Connection {
        session: session.clone(),
        mixer,
        audio: opts.audio,
        player,
        events: Some(events),
        spirc,
        connect_events,
    })));
    Ok(Started {
        handle: existing,
        session,
        connect_enabled,
    })
}

/// Session resources attached to the permanent queue engine.
pub struct Connection {
    session: Session,
    mixer: Arc<SoftMixer>,
    audio: AudioSettings,
    player: Arc<Player>,
    events: Option<PlayerEventChannel>,
    spirc: Option<Spirc>,
    connect_events: Option<PlayerEventChannel>,
}

/// Requires an entered Tokio runtime; performs no network requests or login.
pub fn start_offline(audio: AudioSettings, volume: u8, ui: UiTx) -> Result<PlayerHandle> {
    let engine = offline_engine(audio, volume, ui)?;
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(engine.run(rx));
    Ok(PlayerHandle { tx })
}

fn offline_engine(audio: AudioSettings, volume: u8, ui: UiTx) -> Result<Engine> {
    let session = Session::new(Default::default(), None);
    let mixer =
        Arc::new(SoftMixer::open(MixerConfig::default()).map_err(|e| anyhow!("audio mixer: {e}"))?);
    mixer.set_volume(volume_from_percent(volume));
    let player = make_player(&session, &mixer, &audio, &ui);
    let events = player.get_player_event_channel();
    let (file_tx, file_events) = mpsc::unbounded_channel();
    let file_output = FileOutput::start(file_tx, ui.clone())?;
    let engine = Engine {
        session,
        mixer,
        audio,
        player,
        events: Some(events),
        spirc: None,
        connect_events: None,
        online: false,
        file_output,
        file_events,
        file_generation: 0,
        file_loaded: false,
        current_file: None,
        current_stream: None,
        downloads: None,
        independent: false,
        waiting_download: None,
        mode: Mode::Local,
        ui,
        ctx_name: String::new(),
        ctx_key: None,
        context: Vec::new(),
        history: Vec::new(),
        current: None,
        queue: VecDeque::new(),
        upcoming: VecDeque::new(),
        shuffle: false,
        repeat: Repeat::Off,
        play_request_id: None,
        playing: false,
        loading: false,
        position_ms: 0,
        position_at: Instant::now(),
        skipped_in_a_row: 0,
        preloaded: None,
        connect: ConnectState::default(),
    };
    Ok(engine)
}

// ---------------------------------------------------------------- engine

#[derive(Default)]
struct ConnectState {
    track: Option<TrackRef>,
    playing: bool,
    position_ms: u32,
    at: Option<Instant>,
    shuffle: bool,
    repeat: Repeat,
    client: Option<String>,
    play_request_id: Option<u64>,
}

struct PendingDownload {
    uri: String,
    playing: bool,
    position_ms: u32,
}

struct Engine {
    independent: bool,
    waiting_download: Option<PendingDownload>,
    online: bool,
    file_output: FileOutput,
    file_events: mpsc::UnboundedReceiver<FileEvent>,
    file_generation: u64,
    file_loaded: bool,
    current_file: Option<std::path::PathBuf>,
    current_stream: Option<Arc<crate::progressive::Buffer>>,
    downloads: Option<Arc<crate::downloads::Library>>,
    session: Session,
    mixer: Arc<SoftMixer>,
    audio: AudioSettings,
    player: Arc<Player>,
    events: Option<PlayerEventChannel>,
    spirc: Option<Spirc>,
    connect_events: Option<PlayerEventChannel>,
    mode: Mode,
    ui: UiTx,

    ctx_name: String,
    ctx_key: Option<String>,
    /// Full context in original order (for repeat-all refills and un-shuffle).
    context: Vec<TrackRef>,
    history: Vec<TrackRef>,
    current: Option<TrackRef>,
    queue: VecDeque<TrackRef>,
    upcoming: VecDeque<TrackRef>,
    shuffle: bool,
    repeat: Repeat,

    play_request_id: Option<u64>,
    playing: bool,
    loading: bool,
    position_ms: u32,
    position_at: Instant,
    skipped_in_a_row: u32,
    preloaded: Option<String>,

    connect: ConnectState,
}

async fn recv_opt(ch: &mut Option<PlayerEventChannel>) -> Option<PlayerEvent> {
    match ch {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

impl Engine {
    fn is_file(&self) -> bool {
        self.current_stream.is_some()
            || self.current_file.is_some()
            || self
                .current
                .as_ref()
                .is_some_and(|t| crate::local::path(&t.uri).is_some())
    }

    fn pause(&mut self) {
        if let Some(waiting) = &mut self.waiting_download {
            waiting.playing = false;
            self.playing = false;
            self.emit();
            return;
        }
        if self.is_file() {
            self.file_output.send(FileCmd::Pause);
        } else {
            self.player.pause();
        }
    }

    fn seek(&mut self, position: u32) {
        if let Some(buffer) = &self.current_stream {
            if !buffer.complete()
                || self
                    .downloads
                    .as_ref()
                    .and_then(|l| self.current.as_ref().and_then(|t| l.path(&t.uri)))
                    .is_none()
            {
                self.ui.toast(
                    ToastKind::Info,
                    "This song is still arriving. Seeking becomes available once it is saved.",
                );
                return;
            }
            if let Some(track) = self.current.clone() {
                self.load(&track, self.playing, position);
            }
            return;
        }
        if let Some(waiting) = &mut self.waiting_download {
            waiting.position_ms = position;
            self.set_position(position);
            self.emit();
            return;
        }
        let position = self.current.as_ref().map_or(position, |t| {
            if t.duration_ms > 0 {
                position.min(t.duration_ms.saturating_sub(1))
            } else {
                position
            }
        });
        if self.is_file() {
            if self.file_loaded {
                self.file_output.send(FileCmd::Seek(position));
            } else if let Some(track) = self.current.clone() {
                self.load(&track, false, position);
                return;
            }
        } else {
            self.player.seek(position);
        }
        self.set_position(position);
        self.emit();
    }

    fn stop_file(&mut self) {
        self.current_stream = None;
        self.file_loaded = false;
        self.file_generation = self.file_generation.wrapping_add(1);
        self.file_output.send(FileCmd::Stop);
    }

    fn handle_file_event(&mut self, event: FileEvent) {
        if !self.is_file() || self.mode != Mode::Local {
            return;
        }
        match event {
            FileEvent::State {
                id,
                playing,
                position_ms,
            } if id == self.file_generation => {
                self.playing = playing;
                self.loading = false;
                self.skipped_in_a_row = 0;
                self.set_position(position_ms);
                self.emit();
            }
            FileEvent::End(id) if id == self.file_generation => {
                self.file_loaded = false;
                self.advance(true);
            }
            FileEvent::Error(id, message) if id == self.file_generation => {
                self.ui.toast(ToastKind::Error, message);
                if self.current_stream.is_some() {
                    self.advance_stop();
                } else {
                    self.skip_unplayable();
                }
            }
            _ => {}
        }
    }

    fn attach(&mut self, connection: Connection) {
        let Connection {
            session,
            mixer,
            audio,
            player,
            events,
            spirc,
            connect_events,
        } = connection;
        mixer.set_volume(self.mixer.volume());
        self.player.stop();
        let old = std::mem::replace(&mut self.player, player);
        tokio::task::spawn_blocking(move || drop(old));
        self.session.shutdown();
        self.session = session;
        self.mixer = mixer;
        self.audio = audio;
        self.events = events;
        self.spirc = spirc;
        self.connect_events = connect_events;
        self.online = true;
        self.play_request_id = None;
        self.emit();
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<PlayerCmd>) {
        self.emit();
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(PlayerCmd::Shutdown) | None => break,
                    Some(cmd) => self.handle_cmd(cmd),
                },
                Some(ev) = self.file_events.recv() => self.handle_file_event(ev),
                ev = recv_opt(&mut self.events) => match ev {
                    Some(ev) => self.handle_local_event(ev),
                    None => self.events = None,
                },
                ev = recv_opt(&mut self.connect_events) => match ev {
                    Some(ev) => self.handle_connect_event(ev),
                    None => self.connect_events = None,
                },
            }
        }
        if let Some(spirc) = &self.spirc {
            let _ = spirc.shutdown();
        }
        self.player.stop();
        self.file_output.send(FileCmd::Stop);
        tracing::info!("player engine stopped");
    }

    // ------------------------------------------------------------ snapshot

    fn emit(&self) {
        let snap = match self.mode {
            Mode::Local => Snapshot {
                mode: Mode::Local,
                track: self.current.clone(),
                playing: self.playing,
                loading: self.loading,
                position_ms: self.position_ms,
                at: Some(self.position_at),
                shuffle: self.shuffle,
                repeat: self.repeat,
                volume: self.mixer.volume(),
                queue: self.queue.iter().cloned().collect(),
                upcoming: self
                    .upcoming
                    .iter()
                    .take(UPCOMING_PREVIEW)
                    .cloned()
                    .collect(),
                upcoming_total: self.upcoming.len(),
                context_name: self.ctx_name.clone(),
                context_key: self.ctx_key.clone(),
                remote_client: None,
            },
            Mode::Connect => Snapshot {
                mode: Mode::Connect,
                track: self.connect.track.clone(),
                playing: self.connect.playing,
                loading: false,
                position_ms: self.connect.position_ms,
                at: self.connect.at,
                shuffle: self.connect.shuffle,
                repeat: self.connect.repeat,
                volume: self.mixer.volume(),
                queue: Vec::new(),
                upcoming: Vec::new(),
                upcoming_total: 0,
                context_name: "Spotify Connect".into(),
                context_key: None,
                remote_client: self.connect.client.clone(),
            },
        };
        self.ui
            .send(crate::events::AppEvent::Playback(Arc::new(snap)));
    }

    fn position_now(&self) -> u32 {
        if self.playing && !self.loading {
            self.position_ms
                .saturating_add(self.position_at.elapsed().as_millis() as u32)
        } else {
            self.position_ms
        }
    }

    fn set_position(&mut self, ms: u32) {
        self.position_ms = ms;
        self.position_at = Instant::now();
    }

    // ------------------------------------------------------------ commands

    fn handle_cmd(&mut self, cmd: PlayerCmd) {
        // In Connect mode, transport commands go to Spirc.
        if self.mode == Mode::Connect {
            if let Some(spirc) = &self.spirc {
                let res = match &cmd {
                    PlayerCmd::TogglePlay => Some(spirc.play_pause()),
                    PlayerCmd::Play => Some(spirc.play()),
                    PlayerCmd::Pause => Some(spirc.pause()),
                    PlayerCmd::Next => Some(spirc.next()),
                    PlayerCmd::Previous => Some(spirc.prev()),
                    PlayerCmd::Seek(ms) => Some(spirc.set_position_ms(*ms)),
                    PlayerCmd::SetShuffle(s) => Some(spirc.shuffle(*s)),
                    PlayerCmd::SetRepeat(r) => Some(
                        spirc
                            .repeat(*r != Repeat::Off)
                            .and_then(|_| spirc.repeat_track(*r == Repeat::One)),
                    ),
                    PlayerCmd::SetVolume(v) => Some(spirc.set_volume(*v)),
                    PlayerCmd::AddToQueue(_)
                    | PlayerCmd::QueueRemove(_)
                    | PlayerCmd::QueueMove { .. }
                    | PlayerCmd::UpcomingRemove(_)
                    | PlayerCmd::UpcomingMove { .. }
                    | PlayerCmd::PlayQueued(_)
                    | PlayerCmd::PlayUpcoming(_)
                    | PlayerCmd::ClearQueue => {
                        self.ui.toast(
                            ToastKind::Info,
                            "Queue editing is available when playing from Lumen (not while controlled via Spotify Connect)",
                        );
                        return;
                    }
                    _ => None,
                };
                if let Some(res) = res {
                    if let Err(e) = res {
                        self.ui
                            .toast(ToastKind::Error, format!("Spotify Connect: {e}"));
                    }
                    return;
                }
            }
        }

        match cmd {
            PlayerCmd::Independent(enabled) => self.independent = enabled,
            PlayerCmd::PlaybackBuffer { uri, buffer } => self.stream_ready(&uri, buffer),
            PlayerCmd::DownloadFinished { uri, success } => self.download_finished(&uri, success),
            PlayerCmd::DownloadLibrary(library) => self.downloads = Some(library),
            PlayerCmd::AttachConnection(connection) => self.attach(*connection),
            PlayerCmd::DetachConnection => {
                self.online = false;
                if let Some(spirc) = self.spirc.take() {
                    let _ = spirc.shutdown();
                }
                self.connect_events = None;
                self.session.shutdown();
                self.leave_connect_mode();
                if !self.is_file() {
                    self.advance_stop();
                } else {
                    self.emit();
                }
            }
            PlayerCmd::PlayContext {
                name,
                key,
                tracks,
                start,
            } => self.play_context(name, key, tracks, start),
            PlayerCmd::Restore {
                name,
                key,
                tracks,
                start,
                position_ms,
            } => self.restore(name, key, tracks, start, position_ms),
            PlayerCmd::ExtendContext { key, tracks } => {
                if self.ctx_key.as_deref() == Some(key.as_str()) {
                    self.context.extend(tracks.iter().cloned());
                    if self.shuffle {
                        let mut rng = rand::rng();
                        let mut v: Vec<_> = self.upcoming.drain(..).chain(tracks).collect();
                        v.shuffle(&mut rng);
                        self.upcoming = v.into();
                    } else {
                        self.upcoming.extend(tracks);
                    }
                    self.emit();
                }
            }
            PlayerCmd::UpdateTracks(tracks) => self.update_tracks(tracks),
            PlayerCmd::TogglePlay => {
                if self.playing || self.waiting_download.as_ref().is_some_and(|w| w.playing) {
                    self.pause();
                } else {
                    self.resume();
                }
            }
            PlayerCmd::Play => self.resume(),
            PlayerCmd::Pause => self.pause(),
            PlayerCmd::Next => self.advance(false),
            PlayerCmd::Previous => self.previous(),
            PlayerCmd::Seek(ms) => {
                if self.current.is_some() {
                    self.seek(ms);
                }
            }
            PlayerCmd::SetVolume(v) => {
                self.mixer.set_volume(v);
                self.file_output.send(FileCmd::Volume(v));
                if let Some(spirc) = &self.spirc {
                    // Keep the Connect device's reported volume in sync.
                    let _ = spirc.set_volume(v);
                }
                self.emit();
            }
            PlayerCmd::SetShuffle(s) => {
                self.set_shuffle(s);
                self.emit();
            }
            PlayerCmd::SetRepeat(r) => {
                self.repeat = r;
                self.preloaded = None;
                self.emit();
            }
            PlayerCmd::AddToQueue(tracks) => {
                let n = tracks.len();
                self.queue.extend(tracks);
                self.ui.toast(
                    ToastKind::Info,
                    if n == 1 {
                        "Added to queue".to_string()
                    } else {
                        format!("Added {n} tracks to queue")
                    },
                );
                if self.current.is_none() {
                    self.advance(false);
                } else {
                    self.emit();
                }
            }
            PlayerCmd::QueueRemove(i) => {
                if i < self.queue.len() {
                    self.queue.remove(i);
                    self.emit();
                }
            }
            PlayerCmd::QueueMove { from, to } => {
                move_item(&mut self.queue, from, to);
                self.emit();
            }
            PlayerCmd::UpcomingRemove(i) => {
                if i < self.upcoming.len() {
                    self.upcoming.remove(i);
                    self.emit();
                }
            }
            PlayerCmd::UpcomingMove { from, to } => {
                move_item(&mut self.upcoming, from, to);
                self.emit();
            }
            PlayerCmd::PlayQueued(i) => {
                if i < self.queue.len() {
                    self.queue.drain(..i);
                    self.advance(false);
                }
            }
            PlayerCmd::PlayUpcoming(i) => {
                if i < self.upcoming.len() {
                    let skipped: Vec<_> = self.upcoming.drain(..i).collect();
                    self.history.extend(skipped);
                    self.trim_history();
                    if let Some(t) = self.upcoming.pop_front() {
                        self.push_current_to_history();
                        self.play(t);
                    }
                }
            }
            PlayerCmd::ClearQueue => {
                self.queue.clear();
                self.emit();
            }
            PlayerCmd::ApplyAudio(settings) => self.apply_audio(settings),
            PlayerCmd::Shutdown => {}
        }
    }

    fn play_context(
        &mut self,
        name: String,
        key: Option<String>,
        tracks: Vec<TrackRef>,
        start: usize,
    ) {
        if tracks.is_empty() {
            return;
        }
        self.leave_connect_mode();
        let start = start.min(tracks.len() - 1);
        self.ctx_name = name;
        self.ctx_key = key;
        self.context = tracks;
        let first = self.context[start].clone();
        if self.shuffle {
            let mut rest: Vec<_> = self
                .context
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != start)
                .map(|(_, t)| t.clone())
                .collect();
            rest.shuffle(&mut rand::rng());
            self.upcoming = rest.into();
        } else {
            self.upcoming = self.context[start + 1..].iter().cloned().collect();
        }
        self.push_current_to_history();
        self.skipped_in_a_row = 0;
        self.play(first);
    }

    fn restore(
        &mut self,
        name: String,
        key: Option<String>,
        tracks: Vec<TrackRef>,
        start: usize,
        position_ms: u32,
    ) {
        if self.current.is_some() || tracks.is_empty() || self.mode == Mode::Connect {
            return;
        }
        let start = start.min(tracks.len() - 1);
        let first = tracks[start].clone();
        // A Spotify song needs the session; without it, don't touch anything
        // (loading would skip ahead to other songs).
        if crate::local::path(&first.uri).is_none()
            && !self.online
            && self
                .downloads
                .as_ref()
                .and_then(|l| l.path(&first.uri))
                .is_none()
        {
            return;
        }
        self.ctx_name = name;
        self.ctx_key = key;
        self.context = tracks;
        self.upcoming = if self.shuffle {
            let mut rest: Vec<_> = self
                .context
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != start)
                .map(|(_, t)| t.clone())
                .collect();
            rest.shuffle(&mut rand::rng());
            rest.into()
        } else {
            self.context[start + 1..].iter().cloned().collect()
        };
        let dur = first.duration_ms;
        let pos = if dur > 0 && position_ms + 2000 >= dur {
            0
        } else {
            position_ms
        };
        self.load(&first, false, pos);
    }

    fn leave_connect_mode(&mut self) {
        if self.mode == Mode::Connect {
            if let Some(spirc) = &self.spirc {
                let _ = spirc.disconnect(true);
            }
            self.mode = Mode::Local;
            self.connect.playing = false;
        }
    }

    fn resume(&mut self) {
        self.leave_connect_mode();
        if let Some(waiting) = &mut self.waiting_download {
            waiting.playing = true;
            self.emit();
            return;
        }
        if self.is_file() {
            if self.file_loaded {
                self.file_output.send(FileCmd::Resume);
            } else if let Some(track) = self.current.clone() {
                self.load(&track, true, self.position_ms);
            }
            return;
        }
        match &self.current {
            Some(cur)
                if self.play_request_id.is_some() && !self.loading && self.position_ms > 0 =>
            {
                let _ = cur;
                self.player.play();
            }
            Some(cur) => {
                // Stopped (e.g. end of context) → reload the current track.
                let cur = cur.clone();
                let pos = self.position_ms;
                self.load(&cur, true, pos);
            }
            None => {
                if !self.queue.is_empty() || !self.upcoming.is_empty() {
                    self.advance(false);
                }
            }
        }
    }

    fn set_shuffle(&mut self, s: bool) {
        if self.shuffle == s {
            return;
        }
        self.shuffle = s;
        self.preloaded = None;
        if s {
            let mut v: Vec<_> = self.upcoming.drain(..).collect();
            v.shuffle(&mut rand::rng());
            self.upcoming = v.into();
        } else if let Some(cur) = &self.current {
            // Restore context order after the current track.
            if let Some(i) = self.context.iter().position(|t| t.uri == cur.uri) {
                self.upcoming = self.context[i + 1..].iter().cloned().collect();
            }
        }
    }

    fn update_tracks(&mut self, tracks: Vec<TrackRef>) {
        let map: HashMap<&str, &TrackRef> = tracks.iter().map(|t| (t.uri.as_str(), t)).collect();
        let fix = |t: &mut TrackRef| {
            if t.is_stub() {
                if let Some(n) = map.get(t.uri.as_str()) {
                    *t = (*n).clone();
                }
            }
        };
        self.context.iter_mut().for_each(fix);
        self.upcoming.iter_mut().for_each(fix);
        self.queue.iter_mut().for_each(fix);
        self.history.iter_mut().for_each(fix);
        if let Some(c) = self.current.as_mut() {
            fix(c);
        }
        self.emit();
    }

    fn apply_audio(&mut self, settings: AudioSettings) {
        if settings == self.audio {
            return;
        }
        self.audio = settings;
        let resume = self
            .current
            .clone()
            .map(|t| (t, self.playing, self.position_now()));
        let old = std::mem::replace(
            &mut self.player,
            make_player(&self.session, &self.mixer, &self.audio, &self.ui),
        );
        self.events = Some(self.player.get_player_event_channel());
        old.stop();
        // Dropping a Player joins its thread; do it off the async workers.
        tokio::task::spawn_blocking(move || drop(old));
        self.play_request_id = None;
        self.preloaded = None;
        if let Some((t, playing, pos)) = resume {
            self.load(&t, playing, pos);
        }
        self.ui.toast(ToastKind::Info, "Audio settings applied");
    }

    // ------------------------------------------------------------ queue logic

    fn push_current_to_history(&mut self) {
        if let Some(c) = self.current.take() {
            self.history.push(c);
            self.trim_history();
        }
    }

    fn trim_history(&mut self) {
        if self.history.len() > HISTORY_CAP {
            let excess = self.history.len() - HISTORY_CAP;
            self.history.drain(..excess);
        }
    }

    fn refill_from_context(&mut self) {
        let mut v = self.context.clone();
        if self.shuffle {
            v.shuffle(&mut rand::rng());
        }
        self.upcoming = v.into();
    }

    /// The track that would play after the current one ends.
    fn peek_next(&self) -> Option<TrackRef> {
        if self.repeat == Repeat::One {
            return self.current.clone();
        }
        self.queue
            .front()
            .or(self.upcoming.front())
            .cloned()
            .or_else(|| {
                (self.repeat == Repeat::All && !self.shuffle)
                    .then(|| self.context.first().cloned())
                    .flatten()
            })
    }

    fn next_track(&mut self, auto: bool) -> Option<TrackRef> {
        if auto && self.repeat == Repeat::One {
            return self.current.clone();
        }
        if let Some(t) = self.queue.pop_front() {
            return Some(t);
        }
        if let Some(t) = self.upcoming.pop_front() {
            return Some(t);
        }
        if self.repeat == Repeat::All && !self.context.is_empty() {
            self.refill_from_context();
            return self.upcoming.pop_front();
        }
        None
    }

    fn advance(&mut self, auto: bool) {
        let repeat_one = auto && self.repeat == Repeat::One;
        match self.next_track(auto) {
            Some(t) => {
                if !repeat_one {
                    self.push_current_to_history();
                }
                self.play(t);
            }
            None => {
                self.player.stop();
                self.stop_file();
                self.playing = false;
                self.loading = false;
                self.play_request_id = None;
                self.set_position(0);
                self.emit();
            }
        }
    }

    fn previous(&mut self) {
        if self.position_now() > 3000 || self.history.is_empty() {
            if self.current.is_some() {
                self.seek(0);
            }
            return;
        }
        if let Some(prev) = self.history.pop() {
            if let Some(cur) = self.current.take() {
                self.upcoming.push_front(cur);
            }
            self.play(prev);
        }
    }

    fn play(&mut self, t: TrackRef) {
        self.load(&t, true, 0);
    }

    fn load(&mut self, t: &TrackRef, start_playing: bool, position_ms: u32) {
        self.waiting_download = None;
        let saved = self
            .downloads
            .as_ref()
            .and_then(|library| library.track(&t.uri));
        let t = if t.is_stub() || t.duration_ms == 0 {
            saved.as_ref().unwrap_or(t)
        } else {
            t
        };
        self.stop_file();
        let file = crate::local::path(&t.uri)
            .map(std::path::Path::to_path_buf)
            .or_else(|| {
                self.downloads
                    .as_ref()
                    .and_then(|library| library.path(&t.uri))
            });
        self.current_file = file.clone();
        if let Some(path) = file {
            self.player.stop();
            self.play_request_id = None;
            self.current = Some(t.clone());
            self.file_loaded = true;
            self.loading = true;
            self.playing = false;
            self.preloaded = None;
            self.set_position(position_ms);
            self.file_output.send(FileCmd::Load {
                id: self.file_generation,
                path: path.to_path_buf(),
                playing: start_playing,
                position_ms,
                volume: self.mixer.volume(),
            });
            self.emit();
            return;
        }
        if (self.independent || !self.online) && t.uri.starts_with("spotify:track:") && !t.is_stub()
        {
            self.player.stop();
            self.play_request_id = None;
            self.preloaded = None;
            self.current = Some(t.clone());
            self.loading = true;
            self.playing = false;
            self.set_position(position_ms);
            self.waiting_download = Some(PendingDownload {
                uri: t.uri.clone(),
                playing: start_playing,
                position_ms,
            });
            self.emit();
            self.ui
                .send(crate::events::AppEvent::PlaybackDownload(t.clone()));
            return;
        }
        if !self.online {
            self.current = Some(t.clone());
            self.ui
                .toast(ToastKind::Info, "Offline — Spotify features unavailable");
            self.skip_unplayable();
            return;
        }
        match SpotifyUri::from_uri(&t.uri) {
            Ok(id) => {
                self.player.load(id, start_playing, position_ms);
                self.current = Some(t.clone());
                self.loading = true;
                self.playing = start_playing;
                self.preloaded = None;
                self.set_position(position_ms);
                self.emit();
            }
            Err(e) => {
                tracing::warn!("cannot play {}: {e}", t.uri);
                self.current = Some(t.clone());
                self.skip_unplayable();
            }
        }
    }

    fn download_finished(&mut self, uri: &str, success: bool) {
        if self.current_stream.is_some() && self.current.as_ref().is_some_and(|t| t.uri == uri) {
            if !success {
                self.advance_stop();
            } else if let Some(track) = self
                .downloads
                .as_ref()
                .and_then(|library| library.track(uri))
            {
                self.current = Some(track);
                self.emit();
            }
            return;
        }
        if self.mode != Mode::Local
            || self.current.as_ref().is_none_or(|t| t.uri != uri)
            || self.waiting_download.as_ref().is_none_or(|w| w.uri != uri)
        {
            return;
        }
        let waiting = self.waiting_download.take().unwrap();
        if success && self.downloads.as_ref().and_then(|l| l.path(uri)).is_some() {
            let track = self.current.clone().unwrap();
            self.load(&track, waiting.playing, waiting.position_ms);
        } else {
            // Keep the failed song selected for an explicit retry; never skip/download an entire list.
            self.loading = false;
            self.playing = false;
            self.emit();
        }
    }

    fn stream_ready(&mut self, uri: &str, buffer: Arc<crate::progressive::Buffer>) {
        if buffer.cancelled()
            || self.mode != Mode::Local
            || self.current.as_ref().is_none_or(|t| t.uri != uri)
            || self
                .waiting_download
                .as_ref()
                .is_none_or(|w| w.uri != uri || w.position_ms != 0)
        {
            return;
        }
        let waiting = self.waiting_download.take().unwrap();
        self.stop_file();
        self.current_file = None;
        self.current_stream = Some(buffer.clone());
        self.file_loaded = true;
        self.loading = true;
        self.playing = false;
        self.file_output.send(FileCmd::Stream {
            id: self.file_generation,
            buffer,
            playing: waiting.playing,
            volume: self.mixer.volume(),
        });
        self.emit();
    }

    fn skip_unplayable(&mut self) {
        self.skipped_in_a_row += 1;
        if self.skipped_in_a_row > 15 {
            self.ui.toast(
                ToastKind::Error,
                "Too many unavailable tracks in a row; stopped",
            );
            self.skipped_in_a_row = 0;
            self.advance_stop();
            return;
        }
        self.advance(false);
    }

    fn advance_stop(&mut self) {
        self.waiting_download = None;
        self.player.stop();
        self.stop_file();
        self.playing = false;
        self.loading = false;
        self.emit();
    }

    // ------------------------------------------------------------ player events

    fn is_current(&self, id: Option<u64>) -> bool {
        matches!((id, self.play_request_id), (Some(a), Some(b)) if a == b)
    }

    fn handle_local_event(&mut self, ev: PlayerEvent) {
        if self.is_file() || self.waiting_download.is_some() {
            return;
        }
        if let PlayerEvent::PlayRequestIdChanged { play_request_id } = ev {
            self.play_request_id = Some(play_request_id);
            return;
        }
        if let PlayerEvent::TrackChanged { audio_item } = &ev {
            if let Some(cur) = &self.current {
                if cur.is_stub() && cur.uri == audio_item.uri {
                    self.current = Some(track_from_audio_item(audio_item));
                    self.emit();
                }
            }
            return;
        }
        if let PlayerEvent::VolumeChanged { .. } = ev {
            self.emit();
            return;
        }
        if !self.is_current(ev.get_play_request_id()) {
            return;
        }
        match ev {
            PlayerEvent::Loading { position_ms, .. } => {
                self.loading = true;
                self.set_position(position_ms);
            }
            PlayerEvent::Playing { position_ms, .. } => {
                self.loading = false;
                self.playing = true;
                self.skipped_in_a_row = 0;
                self.set_position(position_ms);
                self.emit();
            }
            PlayerEvent::Paused { position_ms, .. } => {
                self.loading = false;
                self.playing = false;
                self.set_position(position_ms);
                self.emit();
            }
            PlayerEvent::Stopped { .. } => {
                self.playing = false;
                self.loading = false;
                self.emit();
            }
            PlayerEvent::Seeked { position_ms, .. }
            | PlayerEvent::PositionCorrection { position_ms, .. }
            | PlayerEvent::PositionChanged { position_ms, .. } => {
                self.set_position(position_ms);
                self.emit();
            }
            PlayerEvent::TimeToPreloadNextTrack { .. } => {
                if let Some(next) = self.peek_next() {
                    if self.preloaded.as_deref() != Some(next.uri.as_str())
                        && !self
                            .downloads
                            .as_ref()
                            .is_some_and(|library| library.path(&next.uri).is_some())
                    {
                        if let Ok(id) = SpotifyUri::from_uri(&next.uri) {
                            self.player.preload(id);
                            self.preloaded = Some(next.uri.clone());
                        }
                    }
                }
            }
            PlayerEvent::EndOfTrack { .. } => self.advance(true),
            PlayerEvent::Unavailable { track_id, .. } => {
                tracing::info!("track unavailable: {track_id:?}");
                self.ui.toast(
                    ToastKind::Info,
                    "Track unavailable in your region — skipped",
                );
                self.skip_unplayable();
            }
            _ => {}
        }
    }

    fn handle_connect_event(&mut self, ev: PlayerEvent) {
        match &ev {
            PlayerEvent::PlayRequestIdChanged { play_request_id } => {
                self.connect.play_request_id = Some(*play_request_id);
                return;
            }
            PlayerEvent::TrackChanged { audio_item } => {
                self.connect.track = Some(track_from_audio_item(audio_item));
            }
            PlayerEvent::Playing { position_ms, .. } => {
                if self.mode == Mode::Local {
                    if self.playing {
                        self.pause();
                    }
                    self.mode = Mode::Connect;
                    self.ui
                        .toast(ToastKind::Info, "Now playing via Spotify Connect");
                }
                self.connect.playing = true;
                self.connect.position_ms = *position_ms;
                self.connect.at = Some(Instant::now());
            }
            PlayerEvent::Paused { position_ms, .. } => {
                self.connect.playing = false;
                self.connect.position_ms = *position_ms;
                self.connect.at = Some(Instant::now());
            }
            PlayerEvent::Stopped { .. } => {
                self.connect.playing = false;
            }
            PlayerEvent::Seeked { position_ms, .. }
            | PlayerEvent::PositionCorrection { position_ms, .. }
            | PlayerEvent::PositionChanged { position_ms, .. } => {
                self.connect.position_ms = *position_ms;
                self.connect.at = Some(Instant::now());
            }
            PlayerEvent::ShuffleChanged { shuffle } => self.connect.shuffle = *shuffle,
            PlayerEvent::RepeatChanged { context, track } => {
                self.connect.repeat = if *track {
                    Repeat::One
                } else if *context {
                    Repeat::All
                } else {
                    Repeat::Off
                };
            }
            PlayerEvent::SessionClientChanged { client_name, .. } => {
                self.connect.client = Some(client_name.clone()).filter(|s| !s.is_empty());
            }
            PlayerEvent::VolumeChanged { .. } => {}
            _ => return,
        }
        if self.mode == Mode::Connect || matches!(ev, PlayerEvent::VolumeChanged { .. }) {
            self.emit();
        }
    }
}

fn move_item<T>(v: &mut VecDeque<T>, from: usize, to: usize) {
    if from >= v.len() || from == to {
        return;
    }
    if let Some(item) = v.remove(from) {
        let to = to.min(v.len());
        v.insert(to, item);
    }
}

/// Build a compact [`Track`] from librespot's audio item (used for Connect playback
/// and for stub tracks once they start playing).
pub fn track_from_audio_item(a: &AudioItem) -> TrackRef {
    let mut covers: Vec<_> = a.covers.iter().collect();
    covers.sort_by_key(|c| c.width);
    let pick = |min: i32| {
        covers
            .iter()
            .find(|c| c.width >= min)
            .or(covers.last())
            .map(|c| c.url.clone())
    };
    let (artists, artist_uri, album) = match &a.unique_fields {
        UniqueFields::Track { artists, album, .. } => (
            artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            artists.first().and_then(|a| a.id.to_uri().ok()),
            album.clone(),
        ),
        UniqueFields::Episode { show_name, .. } => (show_name.clone(), None, String::new()),
        UniqueFields::Local { artists, album, .. } => (
            artists.clone().unwrap_or_default(),
            None,
            album.clone().unwrap_or_default(),
        ),
    };
    Arc::new(Track {
        uri: a.uri.clone(),
        name: a.name.clone(),
        artists,
        artist_uri,
        album,
        album_uri: None,
        duration_ms: a.duration_ms,
        image: pick(64),
        image_large: pick(300),
        explicit: a.is_explicit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn progressive_ready_respects_pause_and_rejects_a_stale_song() {
        use std::io::Write as _;
        let folder = tempfile::tempdir().unwrap();
        let (buffer, mut writer) = crate::progressive::Buffer::create(folder.path()).unwrap();
        writer
            .write_all(
                &std::fs::read(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/tone.mp3"),
                )
                .unwrap(),
            )
            .unwrap();
        writer.finish(true);
        let (tx, _) = std::sync::mpsc::channel();
        let ui = UiTx::new(tx, egui::Context::default());
        let mut engine = offline_engine(
            AudioSettings {
                bitrate: 320,
                normalisation: false,
                volume_level_db: 0,
                gapless: true,
            },
            0,
            ui,
        )
        .unwrap();
        let track = Arc::new(Track {
            uri: "spotify:track:0123456789abcdefghijkl".into(),
            name: "Streamed song".into(),
            duration_ms: 1000,
            ..Default::default()
        });
        engine.play(track.clone());
        engine.handle_cmd(PlayerCmd::TogglePlay);
        engine.stream_ready("different-song", buffer.clone());
        assert!(engine.waiting_download.is_some());
        assert!(engine.current_stream.is_none());
        engine.stream_ready(&track.uri, buffer);
        assert!(engine.waiting_download.is_none());
        assert!(engine.current_stream.is_some());
        let event = tokio::time::timeout(Duration::from_secs(2), engine.file_events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(!matches!(event, FileEvent::Error(..)));
        engine.handle_file_event(event);
        assert!(!engine.playing);
        assert!(!engine.loading);
        engine.seek(500); // no completed file: do not stop the stream
        assert!(engine.current_stream.is_some());
        engine.download_finished(&track.uri, false);
        assert!(!engine.playing);
        assert!(engine.current_stream.is_none());
    }

    #[tokio::test]
    async fn independent_download_wait_respects_pause_seek_and_stale_completion() {
        let folder = tempfile::tempdir().unwrap();
        let track = Track {
            uri: "spotify:track:0123456789abcdefghijkl".into(),
            name: "Imported song".into(),
            duration_ms: 1000,
            ..Default::default()
        };
        let library = crate::downloads::Library::fixture(folder.path(), &track, "song.mp3");
        let (tx, rx) = std::sync::mpsc::channel();
        let ui = UiTx::new(tx, egui::Context::default());
        let mut engine = offline_engine(
            AudioSettings {
                bitrate: 320,
                normalisation: false,
                volume_level_db: 0,
                gapless: true,
            },
            0,
            ui,
        )
        .unwrap();
        engine.handle_cmd(PlayerCmd::DownloadLibrary(library));
        engine.handle_cmd(PlayerCmd::Independent(true));
        engine.play(Arc::new(track.clone()));
        assert!(!engine.online);
        assert!(engine.loading);
        assert!(engine.waiting_download.as_ref().unwrap().playing);
        assert!(engine.play_request_id.is_none());
        assert_eq!(
            rx.try_iter()
                .filter(|e| matches!(e, crate::events::AppEvent::PlaybackDownload(_)))
                .count(),
            1
        );
        engine.handle_cmd(PlayerCmd::TogglePlay);
        engine.handle_cmd(PlayerCmd::Seek(500));
        assert!(!engine.waiting_download.as_ref().unwrap().playing);
        assert_eq!(engine.waiting_download.as_ref().unwrap().position_ms, 500);
        engine.handle_cmd(PlayerCmd::DownloadFinished {
            uri: "different-song".into(),
            success: true,
        });
        assert!(engine.waiting_download.is_some());
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone.mp3"),
            folder.path().join("song.mp3"),
        )
        .unwrap();
        engine.handle_cmd(PlayerCmd::DownloadFinished {
            uri: track.uri,
            success: true,
        });
        assert!(engine.waiting_download.is_none());
        assert!(engine.is_file());
        let event = tokio::time::timeout(Duration::from_secs(2), engine.file_events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(!matches!(event, FileEvent::Error(..)));
        engine.handle_file_event(event);
        assert!(!engine.playing);
        assert!(!engine.loading);
        assert_eq!(engine.position_ms, 500);
    }

    #[tokio::test]
    async fn independent_failure_stops_for_retry_instead_of_downloading_the_whole_queue() {
        let (tx, rx) = std::sync::mpsc::channel();
        let ui = UiTx::new(tx, egui::Context::default());
        let mut engine = offline_engine(
            AudioSettings {
                bitrate: 320,
                normalisation: false,
                volume_level_db: 0,
                gapless: true,
            },
            0,
            ui,
        )
        .unwrap();
        let first = Arc::new(Track {
            uri: "spotify:track:0123456789abcdefghijkl".into(),
            name: "First".into(),
            ..Default::default()
        });
        let next = Arc::new(Track {
            uri: "spotify:track:abcdefghijkl0123456789".into(),
            name: "Next".into(),
            ..Default::default()
        });
        engine.play_context(
            "Imported".into(),
            None,
            vec![first.clone(), next.clone()],
            0,
        );
        engine.handle_cmd(PlayerCmd::DownloadFinished {
            uri: first.uri.clone(),
            success: false,
        });
        assert!(!engine.loading);
        assert_eq!(engine.current.as_ref().unwrap().uri, first.uri);
        assert_eq!(engine.upcoming.len(), 1);
        engine.handle_cmd(PlayerCmd::Play);
        assert!(engine.waiting_download.is_some());
        engine.handle_cmd(PlayerCmd::Next);
        engine.handle_cmd(PlayerCmd::DownloadFinished {
            uri: first.uri.clone(),
            success: true,
        });
        assert_eq!(engine.current.as_ref().unwrap().uri, next.uri);
        assert_eq!(engine.waiting_download.as_ref().unwrap().uri, next.uri);
        assert_eq!(
            rx.try_iter()
                .filter(|e| matches!(e, crate::events::AppEvent::PlaybackDownload(_)))
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn saved_spotify_uri_plays_locally_without_session_or_network() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone.mp3"),
            folder.path().join("song.mp3"),
        )
        .unwrap();
        let track = Track {
            uri: "spotify:track:0123456789abcdefghijkl".into(),
            name: "Saved song".into(),
            duration_ms: 1000,
            ..Default::default()
        };
        let library = crate::downloads::Library::fixture(folder.path(), &track, "song.mp3");
        let (tx, _rx) = std::sync::mpsc::channel();
        let ui = UiTx::new(tx, egui::Context::default());
        let mut engine = offline_engine(
            AudioSettings {
                bitrate: 320,
                normalisation: false,
                volume_level_db: 0,
                gapless: true,
            },
            0,
            ui,
        )
        .unwrap();
        engine.handle_cmd(PlayerCmd::DownloadLibrary(library));
        let stub = Track::stub(&track.uri);
        engine.load(&stub, false, 0);
        assert!(!engine.online);
        assert!(engine.is_file());
        assert_eq!(engine.current.as_ref().unwrap().name, "Saved song");
        assert_eq!(engine.current.as_ref().unwrap().uri, track.uri);
        let event = tokio::time::timeout(Duration::from_secs(2), engine.file_events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(!matches!(event, FileEvent::Error(..)));
        engine.handle_file_event(event);
        assert!(!engine.loading);
        engine.handle_cmd(PlayerCmd::Seek(500));
        assert_eq!(engine.position_ms, 500);
    }

    #[tokio::test]
    #[ignore = "requires a working audio output device; plays synthetic fixtures at zero volume"]
    async fn local_audio_device_play_pause_seek_and_next() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let ui = UiTx::new(tx, egui::Context::default());
        let mut engine = offline_engine(
            AudioSettings {
                bitrate: 320,
                normalisation: false,
                volume_level_db: 0,
                gapless: true,
            },
            0,
            ui,
        )
        .unwrap();
        let track = |extension: &str| {
            Arc::new(Track {
                uri: crate::local::uri(
                    &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join(format!("tests/fixtures/tone.{extension}")),
                )
                .unwrap(),
                name: extension.into(),
                duration_ms: 1000,
                ..Track::default()
            })
        };
        let first = track("wav");
        let second = track("opus");
        engine.handle_cmd(PlayerCmd::PlayContext {
            name: "offline".into(),
            key: None,
            tracks: vec![first, second.clone()],
            start: 0,
        });
        async fn next_state(engine: &mut Engine) {
            let event = tokio::time::timeout(Duration::from_secs(5), engine.file_events.recv())
                .await
                .unwrap()
                .unwrap();
            if let FileEvent::Error(_, message) = &event {
                panic!("{message}");
            }
            engine.handle_file_event(event);
        }
        next_state(&mut engine).await;
        assert!(engine.playing);
        assert!(!engine.loading);
        engine.handle_cmd(PlayerCmd::Pause);
        next_state(&mut engine).await;
        assert!(!engine.playing);
        engine.handle_cmd(PlayerCmd::Seek(500));
        next_state(&mut engine).await;
        assert_eq!(engine.position_ms, 500);
        engine.handle_cmd(PlayerCmd::Play);
        next_state(&mut engine).await;
        assert!(engine.playing);
        engine.handle_cmd(PlayerCmd::Next);
        next_state(&mut engine).await;
        assert_eq!(engine.current.as_ref().unwrap().uri, second.uri);
        assert!(engine.playing);
        next_state(&mut engine).await;
        assert!(!engine.playing);
        assert!(!engine.loading);
        engine.handle_cmd(PlayerCmd::Play);
        next_state(&mut engine).await;
        assert!(
            engine.playing,
            "Play should restart the last local track after the queue ends"
        );
    }

    #[tokio::test]
    async fn offline_engine_keeps_queue_controls_and_local_seek_without_login() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let ui = UiTx::new(tx, egui::Context::default());
        let audio = AudioSettings {
            bitrate: 320,
            normalisation: false,
            volume_level_db: 0,
            gapless: true,
        };
        let mut engine = offline_engine(audio.clone(), 0, ui.clone()).unwrap();
        assert!(!engine.online);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone.wav");
        let track = Arc::new(Track {
            uri: crate::local::uri(&path).unwrap(),
            name: "Owned audio".into(),
            duration_ms: 1000,
            ..Track::default()
        });
        engine.load(&track, false, 0);
        let event = tokio::time::timeout(Duration::from_secs(2), engine.file_events.recv())
            .await
            .unwrap()
            .unwrap();
        engine.handle_file_event(event);
        assert!(engine.is_file());
        assert!(!engine.loading);
        assert!(!engine.playing);
        engine.handle_cmd(PlayerCmd::SetShuffle(true));
        engine.handle_cmd(PlayerCmd::SetRepeat(Repeat::One));
        engine.handle_cmd(PlayerCmd::SetVolume(12000));
        engine.handle_cmd(PlayerCmd::AddToQueue(vec![
            track.clone(),
            Track::stub("spotify:track:queued"),
        ]));
        assert_eq!(engine.queue.len(), 2);
        assert!(engine.shuffle);
        assert_eq!(engine.repeat, Repeat::One);
        assert_eq!(engine.next_track(true).unwrap().uri, track.uri);
        engine.handle_cmd(PlayerCmd::Seek(500));
        let event = tokio::time::timeout(Duration::from_secs(2), engine.file_events.recv())
            .await
            .unwrap()
            .unwrap();
        engine.handle_file_event(event);
        assert_eq!(engine.position_ms, 500);
        engine.handle_cmd(PlayerCmd::QueueMove { from: 0, to: 1 });
        assert_eq!(engine.queue.front().unwrap().uri, "spotify:track:queued");
        // Simulate attaching freshly authorized resources: queue and local state survive.
        let online = offline_engine(audio, 0, ui).unwrap();
        engine.attach(Connection {
            session: online.session.clone(),
            mixer: online.mixer.clone(),
            audio: online.audio.clone(),
            player: online.player.clone(),
            events: Some(online.player.get_player_event_channel()),
            spirc: None,
            connect_events: None,
        });
        assert!(engine.online);
        assert_eq!(engine.queue.len(), 2);
        assert_eq!(engine.current.as_ref().unwrap().uri, track.uri);
        assert_eq!(engine.position_ms, 500);
        assert_eq!(engine.mixer.volume(), 12000);
        engine.handle_cmd(PlayerCmd::DetachConnection);
        assert!(!engine.online);
        assert_eq!(engine.current.as_ref().unwrap().uri, track.uri);
    }

    #[test]
    fn repeat_cycles() {
        assert_eq!(Repeat::Off.cycle(), Repeat::All);
        assert_eq!(Repeat::All.cycle(), Repeat::One);
        assert_eq!(Repeat::One.cycle(), Repeat::Off);
    }

    #[test]
    fn move_items() {
        let mut v: VecDeque<i32> = (0..5).collect();
        move_item(&mut v, 0, 4);
        assert_eq!(v, [1, 2, 3, 4, 0]);
        move_item(&mut v, 4, 0);
        assert_eq!(v, [0, 1, 2, 3, 4]);
        move_item(&mut v, 9, 0);
        assert_eq!(v, [0, 1, 2, 3, 4]);
        move_item(&mut v, 1, 99);
        assert_eq!(v, [0, 2, 3, 4, 1]);
    }

    #[test]
    fn snapshot_position_interpolates_and_clamps() {
        let t = Arc::new(Track {
            duration_ms: 1000,
            name: "x".into(),
            ..Default::default()
        });
        let s = Snapshot {
            track: Some(t),
            playing: true,
            position_ms: 900,
            at: Some(Instant::now() - Duration::from_millis(500)),
            ..Default::default()
        };
        assert_eq!(s.position_now(), 1000);
        let paused = Snapshot {
            playing: false,
            ..s
        };
        assert_eq!(paused.position_now(), 900);
    }

    #[test]
    fn volume_mapping() {
        assert_eq!(volume_from_percent(0), 0);
        assert_eq!(volume_from_percent(100), 65535);
        assert_eq!(volume_from_percent(200), 65535);
    }
}
