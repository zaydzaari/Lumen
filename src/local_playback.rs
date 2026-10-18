//! Network-free decoding on a dedicated thread, using the same output as LazySink.
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use rodio::Source;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CodecRegistry, Decoder};
use symphonia::core::formats::{FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::probe::Hint;
use tokio::sync::mpsc as async_mpsc;

use crate::player::LazySink;

pub enum Command {
    Load {
        id: u64,
        path: PathBuf,
        playing: bool,
        position_ms: u32,
        volume: u16,
    },
    Pause,
    Resume,
    Seek(u32),
    Volume(u16),
    Stop,
}

pub enum Event {
    State {
        id: u64,
        playing: bool,
        position_ms: u32,
    },
    End(u64),
    Error(u64, String),
}

pub struct Output {
    tx: mpsc::SyncSender<Command>,
    thread: Option<std::thread::JoinHandle<()>>,
    events: async_mpsc::UnboundedSender<Event>,
    ui: crate::events::UiTx,
}

impl Output {
    pub fn start(
        events: async_mpsc::UnboundedSender<Event>,
        ui: crate::events::UiTx,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel(64);
        let worker_events = events.clone();
        let thread = std::thread::Builder::new()
            .name("lumen-local".into())
            .spawn(move || worker(rx, worker_events))?;
        Ok(Self {
            tx,
            thread: Some(thread),
            events,
            ui,
        })
    }
    pub fn send(&self, command: Command) {
        // Commands come from the engine, never the UI thread. Backpressure is bounded.
        if let Err(e) = self.tx.try_send(command) {
            self.ui.toast(
                crate::events::ToastKind::Error,
                "Local playback is busy or unavailable; retry the action",
            );
            let (mpsc::TrySendError::Full(command) | mpsc::TrySendError::Disconnected(command)) = e;
            if let Command::Load { id, .. } = command {
                let _ = self
                    .events
                    .send(Event::Error(id, "Could not schedule local playback".into()));
            }
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        // Disconnect before joining; the worker never waits for this engine.
        let (replacement, _) = mpsc::sync_channel(1);
        drop(std::mem::replace(&mut self.tx, replacement));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn worker(rx: mpsc::Receiver<Command>, events: async_mpsc::UnboundedSender<Event>) {
    let mut current: Option<(u64, PathBuf)> = None;
    let mut output: Option<(rodio::Sink, rodio::OutputStream)> = None;
    let mut position_ms = 0;
    let mut volume = 65535;
    let mut failure: Option<Arc<Mutex<Option<String>>>> = None;
    loop {
        let received = if output.is_some() {
            rx.recv_timeout(Duration::from_millis(100))
        } else {
            rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        let command = match received {
            Ok(c) => c,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if output.as_ref().is_some_and(|(sink, _)| sink.empty()) {
                    output = None;
                    if let Some((id, _)) = current.take() {
                        let error = failure
                            .as_ref()
                            .and_then(|f| f.lock().ok().and_then(|mut f| f.take()));
                        let _ = events.send(match error {
                            Some(message) => Event::Error(id, message),
                            None => Event::End(id),
                        });
                    }
                }
                continue;
            }
        };
        let result = (|| -> Result<()> {
            match command {
                Command::Load {
                    id,
                    path,
                    playing,
                    position_ms: pos,
                    volume: v,
                } => {
                    output = None;
                    current = Some((id, path.clone()));
                    position_ms = pos;
                    volume = v;
                    let mut source = LocalSource::open(&path)?;
                    failure = Some(source.failure.clone());
                    source.seek(Duration::from_millis(pos.into()))?;
                    if playing {
                        let (sink, stream) = LazySink::open().map_err(anyhow::Error::msg)?;
                        sink.set_volume(volume as f32 / 65535.0);
                        sink.append(source);
                        output = Some((sink, stream));
                    }
                    let _ = events.send(Event::State {
                        id,
                        playing,
                        position_ms,
                    });
                }
                Command::Pause => {
                    if let Some((sink, _)) = &output {
                        position_ms =
                            position_ms.saturating_add(
                                sink.get_pos().as_millis().min(u32::MAX as u128) as u32,
                            );
                    }
                    output = None;
                    if let Some((id, _)) = &current {
                        let _ = events.send(Event::State {
                            id: *id,
                            playing: false,
                            position_ms,
                        });
                    }
                }
                Command::Resume => {
                    if output.is_none() {
                        if let Some((id, path)) = &current {
                            let mut source = LocalSource::open(path)?;
                            failure = Some(source.failure.clone());
                            source.seek(Duration::from_millis(position_ms.into()))?;
                            let (sink, stream) = LazySink::open().map_err(anyhow::Error::msg)?;
                            sink.set_volume(volume as f32 / 65535.0);
                            sink.append(source);
                            output = Some((sink, stream));
                            let _ = events.send(Event::State {
                                id: *id,
                                playing: true,
                                position_ms,
                            });
                        }
                    }
                }
                Command::Seek(pos) => {
                    let was_playing = output.is_some();
                    output = None;
                    position_ms = pos;
                    if let Some((id, path)) = &current {
                        let mut source = LocalSource::open(path)?;
                        failure = Some(source.failure.clone());
                        source.seek(Duration::from_millis(pos.into()))?;
                        if was_playing {
                            let (sink, stream) = LazySink::open().map_err(anyhow::Error::msg)?;
                            sink.set_volume(volume as f32 / 65535.0);
                            sink.append(source);
                            output = Some((sink, stream));
                        }
                        let _ = events.send(Event::State {
                            id: *id,
                            playing: was_playing,
                            position_ms,
                        });
                    }
                }
                Command::Volume(v) => {
                    volume = v;
                    if let Some((sink, _)) = &output {
                        sink.set_volume(v as f32 / 65535.0);
                    }
                }
                Command::Stop => {
                    output = None;
                    current = None;
                    position_ms = 0;
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            output = None;
            if let Some((id, _)) = current.take() {
                let _ = events.send(Event::Error(id, format!("Local playback: {e:#}")));
            }
        }
    }
}

/// Streaming decoder: only a packet's PCM samples are retained, never a whole song.
pub struct LocalSource {
    failure: Arc<Mutex<Option<String>>>,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    rate: u32,
    channels: u16,
    duration: Option<Duration>,
    samples: Vec<f32>,
    offset: usize,
    discard: usize,
    eof: bool,
}

impl LocalSource {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            hint.with_extension(ext);
        }
        let probed = symphonia::default::get_probe().format(
            &hint,
            MediaSourceStream::new(Box::new(file), Default::default()),
            &symphonia::core::formats::FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &Default::default(),
        )?;
        let track = probed.format.default_track().context("no audio track")?;
        let params = track.codec_params.clone();
        let id = track.id;
        let mut codecs = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut codecs);
        codecs.register_all::<symphonia_adapter_libopus::OpusDecoder>();
        let mut decoder = codecs.make(&params, &Default::default())?;
        let mut format = probed.format;
        // AAC/M4A may only expose its channel layout after decoding a packet.
        let decoded = loop {
            let packet = format.next_packet()?;
            if packet.track_id() == id {
                break decoder.decode(&packet)?;
            }
        };
        let rate = decoded.spec().rate;
        let channels = decoded.spec().channels.count() as u16;
        anyhow::ensure!(rate > 0 && channels > 0, "invalid audio format");
        let duration = params
            .n_frames
            .map(|frames| Duration::from_secs_f64(frames as f64 / f64::from(rate)));
        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        buffer.copy_interleaved_ref(decoded);
        let samples = buffer.samples().to_vec();
        Ok(Self {
            failure: Arc::new(Mutex::new(None)),
            format,
            decoder,
            track: id,
            rate,
            channels,
            duration,
            samples,
            offset: 0,
            discard: 0,
            eof: false,
        })
    }

    fn seek(&mut self, position: Duration) -> Result<()> {
        if position.is_zero() {
            return Ok(());
        }
        let seek = self.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: position.as_secs_f64().into(),
                track_id: Some(self.track),
            },
        )?;
        self.decoder.reset();
        self.samples.clear();
        self.offset = 0;
        self.eof = false;
        // Accurate seeking lands on a packet before the requested sample.
        self.discard = seek
            .required_ts
            .saturating_sub(seek.actual_ts)
            .saturating_mul(self.channels.into())
            .min(usize::MAX as u64) as usize;
        Ok(())
    }
}

impl Iterator for LocalSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.eof {
            return None;
        }
        loop {
            if let Some(sample) = self.samples.get(self.offset).copied() {
                self.offset += 1;
                if self.discard > 0 {
                    self.discard -= 1;
                    continue;
                }
                return Some(sample);
            }
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(symphonia::core::errors::Error::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.eof = true;
                    return None;
                }
                Err(e) => {
                    if let Ok(mut error) = self.failure.lock() {
                        *error = Some(format!("Local audio: {e}"));
                    }
                    self.eof = true;
                    return None;
                }
            };
            if packet.track_id() != self.track {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                Err(e) => {
                    if let Ok(mut error) = self.failure.lock() {
                        *error = Some(format!("Local decode: {e}"));
                    }
                    self.eof = true;
                    return None;
                }
            };
            // Reject format changes rather than sending frames with the wrong layout.
            if decoded.spec().rate != self.rate
                || decoded.spec().channels.count() != usize::from(self.channels)
            {
                if let Ok(mut error) = self.failure.lock() {
                    *error = Some("Local audio format changed during playback".into());
                }
                self.eof = true;
                return None;
            }
            let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
            buffer.copy_interleaved_ref(decoded);
            self.samples.clear();
            self.samples.extend_from_slice(buffer.samples());
            self.offset = 0;
        }
    }
}

impl Source for LocalSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streams_and_seeks_all_formats_without_network_or_audio_device() {
        for extension in ["wav", "flac", "mp3", "ogg", "opus", "m4a"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/tone.{extension}"));
            let mut source =
                LocalSource::open(&path).unwrap_or_else(|e| panic!("{extension}: {e:#}"));
            assert!(
                source.by_ref().take(1000).any(|s| s.abs() > 0.001),
                "{extension}"
            );
            source
                .seek(Duration::from_millis(500))
                .unwrap_or_else(|e| panic!("{extension} seek: {e:#}"));
            let remaining = source.by_ref().count();
            assert!(remaining > 1000, "{extension}");
            assert!(remaining < 35000, "{extension}: {remaining}");
            assert!(source.failure.lock().unwrap().is_none(), "{extension}");
        }
    }
    #[test]
    fn invalid_audio_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("invalid.wav");
        std::fs::write(&file, b"not audio").unwrap();
        assert!(LocalSource::open(&file).is_err());
    }
}
