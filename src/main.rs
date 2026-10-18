//! Lumen — a lightweight, open-source Spotify desktop client.
//!
//! Unofficial and not affiliated with Spotify.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod api;
mod auth;
mod backend;
mod cache;
mod collection_pb;
mod config;
mod discord;
mod downloads;
mod events;
mod free_library;
mod history;
mod local;
mod local_playback;
mod logging;
mod lyrics;
mod media_controls;
mod memory;
mod model;
mod player;
mod plugins;
mod progressive;
mod provider_catalog;
mod spotidownloader;
mod spotify_library;
mod themes;
mod tray;
mod ui;
mod updates;
mod verification;

use std::process::ExitCode;

use anyhow::Context as _;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("fatal: {e:#}");
            eprintln!("Lumen failed to start: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    if std::env::args().nth(1).as_deref() == Some("--download-verification") {
        return ui::verification::run_linux_helper();
    }
    let paths = config::Paths::resolve()?;
    paths.create_all()?;
    let _log_guard = logging::init(&paths.logs_dir()).context("initialising logging")?;
    tracing::info!("Lumen {} starting", env!("CARGO_PKG_VERSION"));

    let loaded = config::Config::load(&paths);

    // A small runtime: networking and decoding are mostly I/O bound.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(8)
        .thread_name("lumen-rt")
        .enable_all()
        .build()
        .context("creating async runtime")?;
    let shared = backend::Shared::new(rt.handle().clone(), paths)?;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lumen")
            .with_app_id("lumen")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([800.0, 560.0])
            // Spotify-style window: Lumen draws its own title bar (Windows/Linux).
            .with_decorations(!loaded.config.ui.custom_title_bar || cfg!(target_os = "macos"))
            .with_icon(egui::IconData {
                rgba: tray::icon_rgba(
                    64,
                    config::parse_hex_color(&loaded.config.ui.accent).unwrap_or([30, 215, 96]),
                ),
                width: 64,
                height: 64,
            }),
        renderer: eframe::Renderer::Glow,
        multisampling: 0,
        depth_buffer: 0,
        stencil_buffer: 0,
        ..Default::default()
    };

    let (tx, rx) = std::sync::mpsc::channel();
    let result = eframe::run_native(
        "Lumen",
        options,
        Box::new(move |cc| {
            Ok(Box::new(ui::App::new(
                cc,
                shared,
                loaded.config,
                loaded.warning,
                rx,
                tx,
            )))
        }),
    );
    // Give background tasks (session shutdown) a moment, then stop.
    rt.shutdown_timeout(std::time::Duration::from_millis(500));
    result.map_err(|e| anyhow::anyhow!("window error: {e}"))
}
