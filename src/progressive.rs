//! A disk-backed audio buffer. Readers wait for arriving bytes, rather than
//! treating the current end of a partial file as the end of the song.
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
use symphonia::core::io::MediaSource;

#[derive(Default)]
struct State {
    length: u64,
    done: bool,
    failed: bool,
}

pub struct Buffer {
    path: PathBuf,
    state: Mutex<State>,
    changed: Condvar,
    cancel: Arc<AtomicBool>,
}

impl Buffer {
    #[cfg(test)]
    pub fn create(folder: &Path) -> io::Result<(Arc<Self>, Writer)> {
        Self::with_cancel(folder, Arc::new(AtomicBool::new(false)))
    }
    pub fn with_cancel(folder: &Path, cancel: Arc<AtomicBool>) -> io::Result<(Arc<Self>, Writer)> {
        std::fs::create_dir_all(folder)?;
        let path = folder.join(format!("playback-{:016x}.mp3", rand::random::<u64>()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let buffer = Arc::new(Self {
            path,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            cancel,
        });
        Ok((
            buffer.clone(),
            Writer {
                file,
                buffer,
                finished: false,
            },
        ))
    }
    pub fn complete(&self) -> bool {
        !self.cancelled() && self.state.lock().is_ok_and(|s| s.done && !s.failed)
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
    pub fn reader(self: &Arc<Self>, epoch: Arc<AtomicU64>, expected: u64) -> io::Result<Reader> {
        Ok(Reader {
            file: File::open(&self.path)?,
            buffer: self.clone(),
            position: 0,
            epoch,
            expected,
        })
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub struct Writer {
    file: File,
    buffer: Arc<Buffer>,
    finished: bool,
}
impl Writer {
    pub fn finish(mut self, success: bool) {
        self.finished = true;
        if let Ok(mut state) = self.buffer.state.lock() {
            state.done = true;
            state.failed = !success;
        }
        self.buffer.changed.notify_all();
    }
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self
            .buffer
            .state
            .lock()
            .map_err(|_| io::Error::other("Audio buffer unavailable"))?;
        if state.length + bytes.len() as u64 > 100 * 1024 * 1024 {
            return Err(io::Error::other("Audio buffer exceeds 100 MB"));
        }
        self.file.write_all(bytes)?;
        state.length += bytes.len() as u64;
        self.buffer.changed.notify_all();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        if !self.finished {
            if let Ok(mut state) = self.buffer.state.lock() {
                state.done = true;
                state.failed = true;
            }
            self.buffer.changed.notify_all();
        }
    }
}

pub struct Reader {
    file: File,
    buffer: Arc<Buffer>,
    position: u64,
    epoch: Arc<AtomicU64>,
    expected: u64,
}
impl Read for Reader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut state = self
            .buffer
            .state
            .lock()
            .map_err(|_| io::Error::other("Audio buffer unavailable"))?;
        loop {
            if self.buffer.cancelled() || self.epoch.load(Ordering::Acquire) != self.expected {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "Playback changed",
                ));
            }
            if state.failed {
                return Err(io::Error::other(
                    "Audio stream was interrupted. Replay the song to retry.",
                ));
            }
            if state.length > self.position {
                let count = bytes.len().min((state.length - self.position) as usize);
                let count = self.file.read(&mut bytes[..count])?;
                self.position += count as u64;
                return Ok(count);
            }
            if state.done {
                return Ok(0);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Audio stream stalled. Replay the song to retry.",
                ));
            }
            state = self
                .buffer
                .changed
                .wait_timeout(state, Duration::from_millis(20))
                .map_err(|_| io::Error::other("Audio buffer unavailable"))?
                .0;
        }
    }
}
impl Seek for Reader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let state = self
            .buffer
            .state
            .lock()
            .map_err(|_| io::Error::other("Audio buffer unavailable"))?;
        let position = match from {
            SeekFrom::Start(pos) => i128::from(pos),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) if state.done => i128::from(state.length) + i128::from(delta),
            SeekFrom::End(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Audio is still buffering",
                ))
            }
        };
        if position < 0 || position > i128::from(state.length) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Position is not buffered yet",
            ));
        }
        self.position = self.file.seek(SeekFrom::Start(position as u64))?;
        Ok(self.position)
    }
}
impl MediaSource for Reader {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        self.buffer
            .state
            .lock()
            .ok()
            .and_then(|s| s.done.then_some(s.length))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arriving_bytes_are_read_before_completion_and_a_gap_is_not_eof() {
        let dir = tempfile::tempdir().unwrap();
        let (buffer, mut writer) = Buffer::create(dir.path()).unwrap();
        let mut reader = buffer.reader(Arc::new(AtomicU64::new(0)), 0).unwrap();
        writer.write_all(b"first").unwrap();
        let mut first = [0; 5];
        reader.read_exact(&mut first).unwrap();
        assert_eq!(&first, b"first");
        assert!(!buffer.complete());
        let task = std::thread::spawn(move || {
            let mut tail = Vec::new();
            reader.read_to_end(&mut tail).unwrap();
            tail
        });
        std::thread::sleep(Duration::from_millis(30));
        assert!(!task.is_finished());
        writer.write_all(b"second").unwrap();
        writer.finish(true);
        assert_eq!(task.join().unwrap(), b"second");
        assert!(buffer.complete());
    }
    #[test]
    fn cancellation_wakes_a_reader_and_failed_transfers_are_not_clean_eof() {
        let dir = tempfile::tempdir().unwrap();
        let (buffer, writer) = Buffer::create(dir.path()).unwrap();
        let epoch = Arc::new(AtomicU64::new(0));
        let mut reader = buffer.reader(epoch.clone(), 0).unwrap();
        let task = std::thread::spawn(move || reader.read(&mut [0; 1]).unwrap_err().kind());
        epoch.store(1, Ordering::Release);
        assert_eq!(task.join().unwrap(), io::ErrorKind::ConnectionAborted);
        let mut reader = buffer.reader(epoch, 1).unwrap();
        writer.finish(false);
        assert_eq!(
            reader.read(&mut [0; 1]).unwrap_err().kind(),
            io::ErrorKind::Other
        );
    }
}
