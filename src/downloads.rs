//! Permanent files matched from public audio sources, separate from Spotify cache.
use crate::{
    events::{AppEvent, ToastKind, UiTx},
    model::{Track, TrackRef},
};
use anyhow::{bail, Context as _, Result};
use lofty::file::{AudioFile as _, TaggedFileExt as _};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex, RwLock,
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    track: Track,
    file: String,
    source: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Index {
    entries: BTreeMap<String, Entry>,
}

pub struct Library {
    folder: PathBuf,
    index: RwLock<Index>,
}
impl Library {
    pub fn path(&self, uri: &str) -> Option<PathBuf> {
        let index = self.index.read().ok()?;
        let entry = index.entries.get(uri)?;
        let file = self.folder.join(&entry.file);
        file.is_file().then_some(file)
    }
    pub fn track(&self, uri: &str) -> Option<TrackRef> {
        self.index
            .read()
            .ok()?
            .entries
            .get(uri)
            .map(|e| Arc::new(e.track.clone()))
    }
    #[cfg(test)]
    pub(crate) fn fixture(folder: &Path, track: &Track, file: &str) -> Arc<Self> {
        let library = Arc::new(Self {
            folder: folder.to_path_buf(),
            index: RwLock::new(Index::default()),
        });
        library
            .insert(track, file.into(), "fixture".into())
            .expect("fixture index saves");
        library
    }
    fn tracks(&self) -> Vec<TrackRef> {
        let Ok(index) = self.index.read() else {
            return Vec::new();
        };
        index
            .entries
            .values()
            .filter_map(|e| {
                let mut track = e.track.clone();
                track.uri = crate::local::uri(&self.folder.join(&e.file))?;
                Some(Arc::new(track))
            })
            .collect()
    }
    fn read(&self) -> Result<()> {
        let manifest = self.folder.join("library.json");
        if !manifest.exists() {
            return Ok(());
        }
        anyhow::ensure!(
            manifest.metadata()?.len() <= 32 * 1024 * 1024,
            "Download library index is too large"
        );
        let mut index: Index = serde_json::from_slice(&fs::read(manifest)?)
            .context("reading download library index")?;
        // Never accept paths outside the music folder from an edited manifest.
        anyhow::ensure!(
            index.entries.values().all(|e| valid_file(&e.file)),
            "Download library contains an invalid file path"
        );
        index
            .entries
            .retain(|uri, e| track_id(uri).is_some() && self.folder.join(&e.file).is_file());
        *self
            .index
            .write()
            .map_err(|_| anyhow::anyhow!("Download library lock failed"))? = index;
        Ok(())
    }
    fn insert(&self, track: &Track, file: String, source: String) -> Result<()> {
        anyhow::ensure!(valid_file(&file), "Invalid download file name");
        let mut index = self
            .index
            .write()
            .map_err(|_| anyhow::anyhow!("Download library lock failed"))?;
        let old = index.entries.insert(
            track.uri.clone(),
            Entry {
                track: track.clone(),
                file,
                source,
            },
        );
        let save = (|| -> Result<()> {
            let bytes = serde_json::to_vec(&*index)?;
            anyhow::ensure!(
                bytes.len() <= 32 * 1024 * 1024,
                "Download library index is full"
            );
            let tmp = self.folder.join("library.json.tmp");
            let mut file = File::create(&tmp)?;
            std::io::Write::write_all(&mut file, &bytes)?;
            file.sync_all()?;
            fs::rename(tmp, self.folder.join("library.json"))?;
            Ok(())
        })();
        if save.is_err() {
            if let Some(e) = old {
                index.entries.insert(track.uri.clone(), e);
            } else {
                index.entries.remove(&track.uri);
            }
        }
        save
    }
}
fn valid_file(name: &str) -> bool {
    !name.is_empty()
        && Path::new(name)
            .components()
            .all(|p| matches!(p, Component::Normal(_)))
        && !name.contains(['/', '\\', ':'])
}
fn track_id(uri: &str) -> Option<&str> {
    uri.strip_prefix("spotify:track:")
        .filter(|id| id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric()))
}

pub fn folder(config: &crate::config::DownloadsConfig, paths: &crate::config::Paths) -> PathBuf {
    if !config.folder.as_os_str().is_empty() {
        return config.folder.clone();
    }
    directories::UserDirs::new()
        .and_then(|u| u.audio_dir().map(Path::to_path_buf))
        .unwrap_or_else(|| paths.data_dir.clone())
        .join("Lumen Downloads")
}

struct Job {
    track: TrackRef,
    explicit: Arc<AtomicBool>,
}

pub struct Downloader {
    pub library: Arc<Library>,
    tx: mpsc::SyncSender<Job>,
    pending: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    enabled: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    ui: UiTx,
}
impl Downloader {
    pub fn start(
        folder: PathBuf,
        enabled: bool,
        ui: UiTx,
        store: crate::auth::SecretStore,
        runtime: tokio::runtime::Handle,
        connection: Arc<crate::verification::Connection>,
    ) -> Result<Self> {
        let library = Arc::new(Library {
            folder,
            index: RwLock::new(Index::default()),
        });
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let enabled = Arc::new(AtomicBool::new(enabled));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel::<Job>(32);
        let (lib, waiting, on, exit) = (
            library.clone(),
            pending.clone(),
            enabled.clone(),
            stop.clone(),
        );
        let worker_ui = ui.clone();
        std::thread::Builder::new()
            .name("lumen-downloads".into())
            .spawn(move || {
                let ui = worker_ui;
                let init = (|| -> Result<()> {
                    fs::create_dir_all(&lib.folder)?;
                    lib.read()
                })();
                if let Err(e) = init {
                    ui.send(AppEvent::DownloadStatus(format!(
                        "Downloads unavailable: {e:#}"
                    )));
                    return;
                }
                ui.send(AppEvent::DownloadsChanged(lib.tracks()));
                ui.send(AppEvent::DownloadStatus(
                    "Ready — completed files are kept permanently".into(),
                ));
                let mut sources = Sources::default();
                while !exit.load(Ordering::Relaxed) {
                    let job = match rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(t) => t,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(_) => break,
                    };
                    let track = job.track;
                    let mut success = lib.path(&track.uri).is_some();
                    if (job.explicit.load(Ordering::Relaxed) || on.load(Ordering::Relaxed)) && !success {
                        ui.send(AppEvent::DownloadStatus(format!(
                            "Downloading {}…",
                            track.display_name()
                        )));
                        let result = (|| -> Result<_> {
                            let mut token = store.load(crate::spotidownloader::KEY);
                            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default().as_secs();
                            let mut verified = false;
                            if connection.enabled() && token.as_ref().is_none_or(|t| crate::spotidownloader::token_needs_refresh(t, now)) {
                                ui.send(AppEvent::DownloadStatus("Complete download verification in Lumen…".into()));
                                connection.verify(&ui, &track.uri, &exit)?;
                                ui.send(AppEvent::DownloadStatus(format!("Downloading {}…", track.display_name())));
                                token = store.load(crate::spotidownloader::KEY);
                                verified = true;
                            }
                            let Some(token) = token else {
                                anyhow::ensure!(!verified, "SpotiDownloader could not save the session.");
                                return download(&track, &lib.folder, &exit, &mut sources);
                            };
                            let result = download_service(&track, &lib.folder, &exit, &token, &runtime);
                            if !verified && connection.enabled() && result.as_ref().is_err_and(crate::spotidownloader::needs_verification) {
                                connection.verify(&ui, &track.uri, &exit)?;
                                ui.send(AppEvent::DownloadStatus(format!("Downloading {}…", track.display_name())));
                                let token = store.load(crate::spotidownloader::KEY).context("SpotiDownloader session is missing.")?;
                                download_service(&track, &lib.folder, &exit, &token, &runtime)
                            } else { result }
                        })();
                        match result {
                            Ok((file, source)) => match completed_track(&track, &lib.folder.join(&file)).and_then(|track| lib.insert(&track, file, source)) {
                                Ok(()) => {
                                    success = true;
                                    ui.send(AppEvent::DownloadsChanged(lib.tracks()));
                                    ui.send(AppEvent::DownloadStatus(format!(
                                        "Saved {}",
                                        track.display_name()
                                    )));
                                }
                                Err(e) => {
                                    ui.send(AppEvent::DownloadStatus(format!(
                                        "Could not index {}: {e:#}",
                                        track.display_name()
                                    )));
                                    ui.toast(
                                        ToastKind::Error,
                                        format!("Could not save download: {e:#}"),
                                    );
                                }
                            },
                            Err(e) if !exit.load(Ordering::Relaxed) => {
                                tracing::warn!(error=%format!("{e:#}"), "Background audio download failed");
                                let message=user_error(&format!("{e:#}"));
                                ui.send(AppEvent::DownloadStatus(message.into()));
                                ui.toast(ToastKind::Error,format!("{message} See Settings > Storage."));
                            }
                            Err(_) => {}
                        }
                    }
                    if let Ok(mut pending) = waiting.lock() {
                        pending.remove(&track.uri);
                    }
                    ui.send(AppEvent::DownloadFinished { uri: track.uri.clone(), success });
                }
            })?;
        Ok(Self {
            library,
            tx,
            pending,
            enabled,
            stop,
            ui,
        })
    }
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }
    pub fn request(&self, track: TrackRef) {
        if let Err(message) = self.enqueue(track, false) {
            self.ui.send(AppEvent::DownloadStatus(message.into()));
        }
    }
    pub fn request_playback(&self, track: TrackRef) -> Result<(), &'static str> {
        self.enqueue(track, true)
    }
    fn enqueue(&self, track: TrackRef, explicit: bool) -> Result<(), &'static str> {
        if !explicit && !self.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if track_id(&track.uri).is_none() || track.is_stub() {
            return if explicit {
                Err("Import this song's title and Spotify track ID before downloading")
            } else {
                Ok(())
            };
        }
        if self.library.path(&track.uri).is_some() {
            if explicit {
                self.ui.send(AppEvent::DownloadFinished {
                    uri: track.uri.clone(),
                    success: true,
                });
            }
            return Ok(());
        }
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| "Download queue unavailable")?;
        if let Some(requested) = pending.get(&track.uri) {
            if explicit {
                requested.store(true, Ordering::Relaxed);
            }
            return Ok(());
        }
        let uri = track.uri.clone();
        let explicit = Arc::new(AtomicBool::new(explicit));
        match self.tx.try_send(Job {
            track,
            explicit: explicit.clone(),
        }) {
            Ok(()) => {
                pending.insert(uri, explicit);
                Ok(())
            }
            Err(mpsc::TrySendError::Full(_)) => {
                Err("Download queue is full — replay the song to retry")
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                Err("Download worker is unavailable — check the music folder and restart Lumen")
            }
        }
    }
}
impl Drop for Downloader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Candidate {
    id: String,
    title: String,
    #[serde(default, alias = "uploader")]
    channel: String,
    #[serde(default)]
    webpage_url: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
}
#[derive(Deserialize)]
struct Search {
    entries: Vec<Candidate>,
}
fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn fraction(expected: &[String], actual: &[String]) -> f32 {
    if expected.is_empty() {
        return 0.;
    }
    expected.iter().filter(|s| actual.contains(s)).count() as f32 / expected.len() as f32
}
fn ranked(track: &Track, entries: Vec<Candidate>, provider: Provider) -> Vec<Candidate> {
    let title = words(&track.name);
    let artist = words(track.artists.split(", ").next().unwrap_or(&track.artists));
    let mut matches: Vec<_> = entries
        .into_iter()
        .filter_map(|entry| {
            source_url(provider, &entry)?;
            let duration = entry.duration?;
            if !duration.is_finite() || duration <= 0. || track.duration_ms == 0 {
                return None;
            }
            let difference = (duration - track.duration_ms as f64 / 1000.).abs();
            if difference > tolerance(track.duration_ms) {
                return None;
            }
            let found = words(&entry.title);
            let credit = words(&entry.channel);
            if fraction(&title, &found) < 0.85
                || fraction(&artist, &credit).max(fraction(&artist, &found)) < 0.85
            {
                return None;
            }
            for variant in [
                "cover",
                "karaoke",
                "instrumental",
                "live",
                "remix",
                "sped",
                "slowed",
            ] {
                if found.iter().any(|s| s == variant) && !title.iter().any(|s| s == variant) {
                    return None;
                }
            }
            let score = 100. - difference
                + if entry.title.to_lowercase().contains("official") {
                    3.
                } else {
                    0.
                }
                + if entry.channel.to_lowercase().contains("topic") {
                    5.
                } else {
                    0.
                };
            Some((score, entry))
        })
        .collect();
    matches.sort_by(|a, b| b.0.total_cmp(&a.0));
    matches.into_iter().map(|(_, entry)| entry).collect()
}
#[cfg(test)]
fn choose(track: &Track, entries: Vec<Candidate>) -> Option<Candidate> {
    ranked(track, entries, Provider::YouTube).into_iter().next()
}

#[derive(Clone, Copy)]
enum Provider {
    YouTube,
    SoundCloud,
}
#[derive(Default)]
struct Sources {
    youtube_blocked_until: Option<Instant>,
}
fn source_url(provider: Provider, entry: &Candidate) -> Option<String> {
    match provider {
        Provider::YouTube
            if entry.id.len() == 11
                && entry
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') =>
        {
            Some(format!("https://www.youtube.com/watch?v={}", entry.id))
        }
        Provider::SoundCloud => {
            let raw = entry.webpage_url.as_ref()?;
            let url = url::Url::parse(raw).ok()?;
            (url.scheme() == "https"
                && url.host_str() == Some("soundcloud.com")
                && url.username().is_empty()
                && url.password().is_none()
                && url.port().is_none()
                && url.query().is_none()
                && url.path_segments()?.filter(|s| !s.is_empty()).count() == 2)
                .then(|| url.to_string())
        }
        _ => None,
    }
}
fn auth_or_rate_limit(error: &str) -> bool {
    let message = error.to_ascii_lowercase();
    [
        "not a bot",
        "sign in",
        "login required",
        "429",
        "too many requests",
    ]
    .iter()
    .any(|p| message.contains(p))
}
fn user_error(error: &str) -> &'static str {
    let message = error.to_ascii_lowercase();
    if message.contains("spotidownloader verification") {
        "Download verification was cancelled or unavailable. Reconnect in Settings > Storage."
    } else if message.contains("spotidownloader token") {
        "SpotiDownloader token expired or was rejected. Reconnect in Settings > Storage."
    } else if message.contains("spotidownloader is rate limited") {
        "SpotiDownloader is rate limited. Try again later."
    } else if message.contains("spotidownloader") {
        "SpotiDownloader could not download this song. Check the token or try again later."
    } else if message.contains("starting download helper") {
        "Download helper is missing or cannot start."
    } else if message.contains("duration does not match") {
        "Downloaded recording has the wrong duration."
    } else if message.contains("no full-length") || message.contains("no confident") {
        "No full-length matching recording is available."
    } else if auth_or_rate_limit(&message) {
        "Public sources are unavailable or require sign-in."
    } else if message.contains("access is denied") || message.contains("permission denied") {
        "The download folder cannot be written."
    } else {
        "Could not save this song from public sources."
    }
}
fn tolerance(ms: u32) -> f64 {
    (ms as f64 / 1000. * 0.03).clamp(6., 12.)
}
fn stem(track: &Track) -> Result<String> {
    let id = track_id(&track.uri).context("Not a Spotify song")?;
    let name = format!("{} - {}", track.artists, track.name)
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(90)
        .collect::<String>();
    Ok(format!("{} [{id}]", name.trim_end_matches([' ', '.'])))
}
fn command(program: &str) -> Command {
    // Prefer the complete official helper installed for Lumen, including its EJS scripts.
    let bundled = crate::config::Paths::resolve().ok().map(|paths| {
        paths.data_dir.join("tools").join(if cfg!(windows) {
            "yt-dlp.exe"
        } else {
            "yt-dlp"
        })
    });
    let executable = if program == "yt-dlp" {
        bundled
            .filter(|path| path.is_file())
            .unwrap_or_else(|| program.into())
    } else {
        program.into()
    };
    let mut cmd = Command::new(executable);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(0x08000000);
    }
    cmd.stdin(Stdio::null());
    cmd
}
fn run(
    mut cmd: Command,
    stage: &Path,
    name: &str,
    stop: &AtomicBool,
    seconds: u64,
) -> Result<Vec<u8>> {
    let stdout = stage.join(format!("{name}.out"));
    let stderr = stage.join(format!("{name}.err"));
    cmd.stdout(Stdio::from(File::create(&stdout)?))
        .stderr(Stdio::from(File::create(&stderr)?));
    let mut child = cmd
        .spawn()
        .context("Starting download helper (yt-dlp and FFmpeg must be installed)")?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                let error_bytes = fs::read(&stderr).unwrap_or_default();
                let error = String::from_utf8_lossy(&error_bytes);
                let tail = error
                    .lines()
                    .rev()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(600)
                    .collect::<String>();
                bail!(
                    "{name} ({status}): {}",
                    if tail.is_empty() {
                        "Download helper returned no diagnostic output"
                    } else {
                        &tail
                    }
                );
            }
            break;
        }
        if stop.load(Ordering::Relaxed)
            || start.elapsed() > Duration::from_secs(seconds)
            || [stdout.as_path(), stderr.as_path()]
                .iter()
                .any(|p| p.metadata().is_ok_and(|m| m.len() > 2 * 1024 * 1024))
        {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Download cancelled or timed out");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let result = fs::read(&stdout)?;
    anyhow::ensure!(
        result.len() <= 2 * 1024 * 1024,
        "Download helper output is too large"
    );
    Ok(result)
}
fn search(
    track: &Track,
    stage: &Path,
    stop: &AtomicBool,
    provider: Provider,
) -> Result<Vec<Candidate>> {
    let (prefix, name) = match provider {
        Provider::YouTube => ("ytsearch5", "search"),
        Provider::SoundCloud => ("scsearch5", "search-soundcloud"),
    };
    let mut search = command("yt-dlp");
    search
        .args([
            "--ignore-config",
            "--no-cache-dir",
            "--flat-playlist",
            "--dump-single-json",
            "--no-warnings",
            "--socket-timeout",
            "12",
            "--retries",
            "0",
            "--extractor-retries",
            "0",
        ])
        .arg(format!("{prefix}:{} {}", track.artists, track.name));
    let result: Search = serde_json::from_slice(&run(search, stage, name, stop, 45)?)?;
    Ok(ranked(track, result.entries, provider))
}
fn fetch(track: &Track, stage: &Path, stop: &AtomicBool, source: &str) -> Result<()> {
    // A previous provider may have left a partial file. Never reuse one source's
    // bytes as the next source: force each attempt to start fresh.
    for file in [
        "audio.mp3",
        "audio.mp3.part",
        "audio.webm",
        "audio.webm.part",
        "audio.m4a",
        "audio.m4a.part",
        "audio.mp4",
        "audio.mp4.part",
        "audio.mp3.ytdl",
    ] {
        let _ = fs::remove_file(stage.join(file));
    }
    let mut fetch = command("yt-dlp");
    fetch
        .args([
            "--ignore-config",
            "--no-cache-dir",
            "--no-playlist",
            "--no-continue",
            "--force-overwrites",
            "--no-progress",
            "--no-warnings",
            "--no-color",
            "--socket-timeout",
            "12",
            "--retries",
            "0",
            "--extractor-retries",
            "0",
            "--js-runtimes",
            "node",
            "--max-filesize",
            "100M",
            "-f",
            "bestaudio",
            "-x",
            "--audio-format",
            "mp3",
            "--audio-quality",
            "0",
            "-o",
        ])
        .arg(stage.join("audio.%(ext)s"))
        .arg("--")
        .arg(source);
    run(fetch, stage, "download", stop, 180)?;
    let tagged = lofty::read_from_path(stage.join("audio.mp3"))
        .context("Downloaded file is not readable audio")?;
    anyhow::ensure!(
        (tagged.properties().duration().as_secs_f64() - track.duration_ms as f64 / 1000.).abs()
            <= tolerance(track.duration_ms),
        "Downloaded recording duration does not match"
    );
    Ok(())
}
fn download(
    track: &Track,
    folder: &Path,
    stop: &AtomicBool,
    sources: &mut Sources,
) -> Result<(String, String)> {
    let id = track_id(&track.uri).context("Not a Spotify song")?;
    let stage = folder.join(".incomplete").join(id);
    fs::create_dir_all(&stage)?;
    let mut selected = None;
    let mut failures = Vec::new();
    for provider in [Provider::YouTube, Provider::SoundCloud] {
        if stop.load(Ordering::Relaxed) {
            bail!("Download cancelled");
        }
        if matches!(provider, Provider::YouTube)
            && sources
                .youtube_blocked_until
                .is_some_and(|until| until > Instant::now())
        {
            continue;
        }
        let candidates = match search(track, &stage, stop, provider) {
            Ok(items) => items,
            Err(e) => {
                let message = format!("{e:#}");
                if matches!(provider, Provider::YouTube) && auth_or_rate_limit(&message) {
                    sources.youtube_blocked_until =
                        Some(Instant::now() + Duration::from_secs(1800));
                }
                failures.push(message);
                continue;
            }
        };
        for candidate in candidates.into_iter().take(2) {
            let Some(source) = source_url(provider, &candidate) else {
                continue;
            };
            match fetch(track, &stage, stop, &source) {
                Ok(()) => {
                    selected = Some(source);
                    break;
                }
                Err(e) => {
                    let message = format!("{e:#}");
                    if matches!(provider, Provider::YouTube) && auth_or_rate_limit(&message) {
                        // Do not hammer an auth wall or request browser credentials.
                        sources.youtube_blocked_until =
                            Some(Instant::now() + Duration::from_secs(1800));
                        failures.push(message);
                        break;
                    }
                    failures.push(message);
                }
            }
        }
        if selected.is_some() {
            break;
        }
    }
    let source = selected.ok_or_else(|| {
        anyhow::anyhow!(
            "No full-length match could be downloaded from public sources. {}",
            failures.join(" | ")
        )
    })?;
    finish(track, folder, &stage, stop, source)
}

fn download_service(
    track: &Track,
    folder: &Path,
    stop: &AtomicBool,
    token: &str,
    runtime: &tokio::runtime::Handle,
) -> Result<(String, String)> {
    let id = track_id(&track.uri).context("Not a Spotify song")?;
    let stage = folder.join(".incomplete").join(id);
    fs::create_dir_all(&stage)?;
    let audio = stage.join("audio.mp3");
    // Never let a failed service request leave an older provider's audio behind.
    let _ = fs::remove_file(&audio);
    let hls = match runtime.block_on(crate::spotidownloader::download(id, token, &audio, stop)) {
        Ok(hls) => hls,
        Err(error) => {
            let _ = fs::remove_file(&audio);
            return Err(error);
        }
    };
    if hls {
        normalize_segmented_audio(&stage, stop)?;
    }
    finish(
        track,
        folder,
        &stage,
        stop,
        crate::spotidownloader::SITE.into(),
    )
}

fn normalize_segmented_audio(stage: &Path, stop: &AtomicBool) -> Result<()> {
    // Rebuild the duration/index of concatenated fragmented MP4 or transport
    // stream audio. Limit protocols to local files, even for malformed input.
    let normalized = stage.join("normalized.m4a");
    let mut remux = command("ffmpeg");
    remux
        .args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-protocol_whitelist",
            "file",
            "-i",
        ])
        .arg(stage.join("audio.mp3"))
        .args(["-map", "0:a:0", "-c:a", "copy"])
        .arg(&normalized);
    run(remux, stage, "segments", stop, 180)?;
    fs::remove_file(stage.join("audio.mp3"))?;
    fs::rename(normalized, stage.join("audio.mp3"))?;
    Ok(())
}

fn completed_track(track: &Track, path: &Path) -> Result<Track> {
    let mut track = track.clone();
    if track.duration_ms == 0 {
        let tagged = lofty::probe::Probe::open(path)?.guess_file_type()?.read()?;
        track.duration_ms = tagged
            .properties()
            .duration()
            .as_millis()
            .try_into()
            .context("Audio duration is too large")?;
        anyhow::ensure!(track.duration_ms > 0, "Downloaded recording is empty");
    }
    Ok(track)
}

fn finish(
    track: &Track,
    folder: &Path,
    stage: &Path,
    stop: &AtomicBool,
    source: String,
) -> Result<(String, String)> {
    anyhow::ensure!(!stop.load(Ordering::Relaxed), "Download cancelled");
    let audio = stage.join("audio.mp3");
    // A service link can contain AAC/MP4 even when staged under an MP3 name.
    // Detect the actual bytes rather than trusting the URL or file extension.
    let tagged = lofty::probe::Probe::open(&audio)?
        .guess_file_type()?
        .read()
        .context("Downloaded file is not readable audio")?;
    let duration = tagged.properties().duration().as_secs_f64();
    anyhow::ensure!(
        duration.is_finite()
            && duration > 0.
            && ((track.duration_ms == 0 && source == crate::spotidownloader::SITE)
                || (duration - track.duration_ms as f64 / 1000.).abs()
                    <= tolerance(track.duration_ms)),
        "Downloaded recording duration does not match"
    );
    let file = format!("{}.mp3", stem(track)?);
    let staging = stage.join("tagged.mp3");
    let mut tag = command("ffmpeg");
    tag.args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(&audio)
        .args(["-map", "0:a:0", "-c:a"]);
    if tagged.file_type() == lofty::file::FileType::Mpeg {
        tag.arg("copy");
    } else {
        tag.args(["libmp3lame", "-q:a", "0"]);
    }
    tag.arg("-metadata")
        .arg(format!("title={}", track.name))
        .arg("-metadata")
        .arg(format!("artist={}", track.artists))
        .arg("-metadata")
        .arg(format!("album={}", track.album))
        .arg(&staging);
    run(tag, stage, "tags", stop, 180)?;
    anyhow::ensure!(staging.metadata()?.len() > 0, "Download is empty");
    fs::rename(&staging, folder.join(&file))?;
    // Remove only this job's known intermediate files; never touch completed music.
    for name in [
        "audio.mp3",
        "search.out",
        "search.err",
        "search-soundcloud.out",
        "search-soundcloud.err",
        "download.out",
        "download.err",
        "tags.out",
        "tags.err",
        "segments.out",
        "segments.err",
    ] {
        let _ = fs::remove_file(stage.join(name));
    }
    let _ = fs::remove_dir(stage);
    Ok((file, source))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "uses FFmpeg to generate and remux a synthetic fragmented HLS response"]
    fn service_hls_response_is_converted_tagged_and_decodable() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join(".incomplete").join("fixture");
        fs::create_dir_all(&stage).unwrap();
        let stop = AtomicBool::new(false);
        let mut generate = command("ffmpeg");
        generate.current_dir(&stage);
        generate
            .args(["-nostdin", "-v", "error", "-y", "-i"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone.m4a"))
            .args([
                "-c:a",
                "copy",
                "-hls_time",
                "0.25",
                "-hls_segment_type",
                "fmp4",
                "-hls_list_size",
                "0",
            ])
            .arg("list.m3u8");
        run(generate, &stage, "fixture", &stop, 30).unwrap();
        let parts = crate::spotidownloader::hls_parts(
            &fs::read(stage.join("list.m3u8")).unwrap(),
            &url::Url::parse("https://cdn.example.com/list.m3u8").unwrap(),
        )
        .unwrap();
        assert!(parts.len() > 2);
        let mut output = File::create(stage.join("audio.mp3")).unwrap();
        for part in parts {
            output
                .write_all(&fs::read(stage.join(part.path().trim_start_matches('/'))).unwrap())
                .unwrap();
        }
        drop(output);
        normalize_segmented_audio(&stage, &stop).unwrap();
        let mut track = track();
        track.duration_ms = 1000;
        let (file, _) = finish(
            &track,
            dir.path(),
            &stage,
            &stop,
            crate::spotidownloader::SITE.into(),
        )
        .unwrap();
        let path = dir.path().join(file);
        assert_eq!(
            lofty::read_from_path(&path).unwrap().file_type(),
            lofty::file::FileType::Mpeg
        );
        assert!(
            crate::local_playback::LocalSource::open(&path)
                .unwrap()
                .take(48000)
                .count()
                > 0
        );
    }

    #[test]
    #[ignore = "uses FFmpeg to convert a synthetic AAC/MP4 service response"]
    fn service_aac_response_is_converted_tagged_and_decodable() {
        use lofty::tag::Accessor as _;
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join(".incomplete").join("fixture");
        fs::create_dir_all(&stage).unwrap();
        fs::copy("tests/fixtures/tone.m4a", stage.join("audio.mp3")).unwrap();
        let mut track = track();
        track.duration_ms = 1000;
        let (file, source) = finish(
            &track,
            dir.path(),
            &stage,
            &AtomicBool::new(false),
            crate::spotidownloader::SITE.into(),
        )
        .unwrap();
        let path = dir.path().join(file);
        let audio = lofty::read_from_path(&path).unwrap();
        assert_eq!(audio.file_type(), lofty::file::FileType::Mpeg);
        assert_eq!(
            audio.primary_tag().unwrap().title().as_deref(),
            Some(track.name.as_str())
        );
        assert_eq!(source, crate::spotidownloader::SITE);
        assert!(
            crate::local_playback::LocalSource::open(&path)
                .unwrap()
                .take(48000)
                .count()
                > 0
        );
    }
    fn track() -> Track {
        Track {
            uri: "spotify:track:0123456789abcdefghijkl".into(),
            name: "Test Song".into(),
            artists: "Test Artist".into(),
            duration_ms: 200_000,
            ..Default::default()
        }
    }
    fn candidate(title: &str, channel: &str, duration: f64) -> Candidate {
        Candidate {
            id: "abcdefghijk".into(),
            title: title.into(),
            channel: channel.into(),
            duration: Some(duration),
            webpage_url: None,
        }
    }
    #[test]
    #[ignore = "uses live YouTube search, yt-dlp and FFmpeg; saves a temporary matched recording"]
    fn live_matched_download_smoke() {
        let dir = tempfile::tempdir().unwrap();
        let track = Track {
            uri: "spotify:track:4uLU6hMCjMI75M1A2tKUQC".into(),
            name: "Never Gonna Give You Up".into(),
            artists: "Rick Astley".into(),
            album: "Whenever You Need Somebody".into(),
            duration_ms: 213_000,
            ..Default::default()
        };
        let (file, source) = download(
            &track,
            dir.path(),
            &AtomicBool::new(false),
            &mut Sources::default(),
        )
        .unwrap();
        let path = dir.path().join(&file);
        assert!(path.metadata().unwrap().len() > 100_000);
        let library = Library {
            folder: dir.path().into(),
            index: RwLock::new(Index::default()),
        };
        library.insert(&track, file, source).unwrap();
        library.read().unwrap();
        assert!(library.path(&track.uri).is_some());
        let _decoder = crate::local_playback::LocalSource::open(&path).unwrap();
        println!("Live matched download saved, indexed, reopened and decoded successfully");
    }
    #[test]
    fn match_requires_artist_title_duration_and_correct_version() {
        let t = track();
        assert!(choose(
            &t,
            vec![candidate("Test Song", "Test Artist - Topic", 200.)]
        )
        .is_some());
        for c in [
            candidate("Test Song", "Other Artist", 200.),
            candidate("Other Song", "Test Artist", 200.),
            candidate("Test Song Live", "Test Artist", 200.),
            candidate("Test Song", "Test Artist", 240.),
        ] {
            assert!(choose(&t, vec![c]).is_none());
        }
    }

    #[test]
    fn public_soundcloud_matching_rejects_previews_and_external_urls() {
        let t = track();
        let make = |seconds: f64, url: &str| Candidate {
            id: "1234".into(),
            title: t.name.clone(),
            channel: t.artists.clone(),
            duration: Some(seconds),
            webpage_url: Some(url.into()),
        };
        assert!(ranked(
            &t,
            vec![make(30., "https://soundcloud.com/artist/song")],
            Provider::SoundCloud
        )
        .is_empty());
        assert_eq!(
            ranked(
                &t,
                vec![make(200., "https://soundcloud.com/artist/song")],
                Provider::SoundCloud
            )
            .len(),
            1
        );
        for url in [
            "https://example.com/artist/song",
            "http://soundcloud.com/artist/song",
            "https://soundcloud.com@other.example/artist/song",
        ] {
            assert!(ranked(&t, vec![make(200., url)], Provider::SoundCloud).is_empty());
        }
    }

    #[test]
    fn source_errors_are_short_and_do_not_expose_helper_output() {
        let error="ERROR: Sign in to confirm you're not a bot. Use --cookies-from-browser https://example.com/very-long-help-link";
        let message = user_error(error);
        assert!(auth_or_rate_limit(error));
        assert!(message.len() < 100);
        assert!(!message.contains("http"));
        assert!(!message.contains("cookies"));
    }

    #[test]
    #[ignore = "live public SoundCloud download of the song reported by the user"]
    fn live_reported_song_public_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let track = Track {
            uri: "spotify:track:2llGNw4clBNOxpcORvUtYF".into(),
            name: "When I Close My Eyes".into(),
            artists: "Tom Odell".into(),
            duration_ms: 193_000,
            ..Default::default()
        };
        let mut sources = Sources {
            youtube_blocked_until: Some(Instant::now() + Duration::from_secs(60)),
        };
        let (file, source) =
            download(&track, dir.path(), &AtomicBool::new(false), &mut sources).unwrap();
        assert!(source.starts_with("https://soundcloud.com/"));
        let audio = dir.path().join(file);
        let source = crate::local_playback::LocalSource::open(&audio).unwrap();
        assert!(source.take(48000).count() > 0);
        println!("Reported song downloaded and decoded from a public full-length source, without cookies");
    }
    #[test]
    fn file_names_cannot_escape_folder() {
        assert!(!valid_file("../song.mp3"));
        assert!(!valid_file("C:\\song.mp3"));
        assert!(!valid_file(""));
        let mut t = track();
        t.name = "../../CON:*? / music".into();
        let file = format!("{}.mp3", stem(&t).unwrap());
        assert!(valid_file(&file));
        t.uri = "spotify:track:../../no".into();
        assert!(stem(&t).is_err());
    }

    #[test]
    fn export_track_duration_is_filled_from_completed_audio() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone.mp3");
        let mut t = track();
        t.duration_ms = 0;
        let completed = completed_track(&t, &file).unwrap();
        assert!(completed.duration_ms >= 900 && completed.duration_ms <= 1200);
        assert_eq!(completed.uri, t.uri);
    }

    #[test]
    fn scheduling_deduplicates_tracks_and_bounds_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        let library = Arc::new(Library {
            folder: dir.path().into(),
            index: RwLock::new(Index::default()),
        });
        let (tx, rx) = mpsc::sync_channel(32);
        let (events, notifications) = mpsc::channel();
        let downloader = Downloader {
            library,
            tx,
            pending: Arc::new(Mutex::new(HashMap::new())),
            enabled: Arc::new(AtomicBool::new(true)),
            stop: Arc::new(AtomicBool::new(false)),
            ui: UiTx::new(events, egui::Context::default()),
        };
        for i in 0..33 {
            let mut t = track();
            t.uri = format!("spotify:track:{i:022}");
            downloader.request(Arc::new(t.clone()));
            downloader.request(Arc::new(t));
        }
        assert_eq!(rx.try_iter().count(), 32);
        assert_eq!(downloader.pending.lock().unwrap().len(), 32);
        assert!(notifications
            .try_iter()
            .any(|e| matches!(e,AppEvent::DownloadStatus(s) if s.contains("queue is full"))));
        downloader.set_enabled(false);
        downloader.request(Arc::new(track()));
        assert!(rx.try_recv().is_err());
        downloader.pending.lock().unwrap().clear();
        downloader.request_playback(Arc::new(track())).unwrap();
        assert!(rx.try_recv().unwrap().explicit.load(Ordering::Relaxed));
    }
    #[test]
    fn saved_library_survives_restart_and_ignores_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let t = track();
        let file = "song.mp3";
        let lib = Library {
            folder: dir.path().into(),
            index: RwLock::new(Index::default()),
        };
        fs::write(dir.path().join(file), b"fixture").unwrap();
        lib.insert(
            &t,
            file.into(),
            "https://www.youtube.com/watch?v=abcdefghijk".into(),
        )
        .unwrap();
        let loaded = Library {
            folder: dir.path().into(),
            index: RwLock::new(Index::default()),
        };
        loaded.read().unwrap();
        assert!(loaded.path(&t.uri).is_some());
        assert_eq!(loaded.tracks()[0].name, t.name);
        fs::remove_file(dir.path().join(file)).unwrap();
        assert!(loaded.path(&t.uri).is_none());
        loaded.read().unwrap();
        assert!(loaded.tracks().is_empty());
    }
}
