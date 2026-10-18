//! Coordinates attended download verification without blocking playback or the UI.
use crate::events::{AppEvent, UiTx};
use anyhow::{bail, Result};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

pub struct Request {
    pub track_uri: String,
    pub cancelled: Arc<AtomicBool>,
    pub complete: Option<mpsc::SyncSender<std::result::Result<(), String>>>,
}

pub struct Connection {
    enabled: AtomicBool,
}
impl Connection {
    pub fn new(enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(enabled),
        })
    }
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    pub fn set_enabled(&self, value: bool) {
        self.enabled.store(value, Ordering::Relaxed);
    }

    pub fn verify(&self, ui: &UiTx, uri: &str, stop: &AtomicBool) -> Result<()> {
        let (tx, rx) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        ui.send(AppEvent::VerifyDownload(Request {
            track_uri: uri.into(),
            cancelled: cancelled.clone(),
            complete: Some(tx),
        }));
        let started = Instant::now();
        loop {
            if stop.load(Ordering::Relaxed)
                || !self.enabled()
                || started.elapsed() > Duration::from_secs(240)
            {
                cancelled.store(true, Ordering::Relaxed);
                bail!("SpotiDownloader verification cancelled or timed out.");
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("SpotiDownloader verification cancelled or unavailable.")
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verification_resumes_or_cancels_a_waiting_job() {
        for success in [true, false] {
            let connection = Connection::new(true);
            let (tx, rx) = mpsc::channel();
            let ui = UiTx::new(tx, egui::Context::default());
            let handle = std::thread::spawn(move || {
                connection.verify(&ui, "spotify:track:fixture", &AtomicBool::new(false))
            });
            let AppEvent::VerifyDownload(request) =
                rx.recv_timeout(Duration::from_secs(2)).unwrap()
            else {
                panic!("Expected verification");
            };
            request
                .complete
                .unwrap()
                .send(if success {
                    Ok(())
                } else {
                    Err("cancelled".into())
                })
                .unwrap();
            assert_eq!(handle.join().unwrap().is_ok(), success);
        }
    }
    #[test]
    fn disabling_connection_closes_pending_verification() {
        let connection = Connection::new(true);
        let worker = connection.clone();
        let (tx, rx) = mpsc::channel();
        let ui = UiTx::new(tx, egui::Context::default());
        let handle =
            std::thread::spawn(move || worker.verify(&ui, "fixture", &AtomicBool::new(false)));
        let AppEvent::VerifyDownload(request) = rx.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("Expected verification");
        };
        connection.set_enabled(false);
        assert!(handle.join().unwrap().is_err());
        assert!(request.cancelled.load(Ordering::Relaxed));
    }
}
