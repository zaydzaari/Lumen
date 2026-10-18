//! `tracing` setup: a daily-rotating log file in the OS data dir (5 files kept),
//! plus stderr in debug builds. `log` records from librespot are bridged in.

use std::path::Path;

use anyhow::Result;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::{fmt, EnvFilter};

/// Keep the returned guard alive for the program's lifetime so logs are flushed.
pub fn init(log_dir: &Path) -> Result<WorkerGuard> {
    std::fs::create_dir_all(log_dir)?;
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("lumen")
        .filename_suffix("log")
        .max_log_files(5)
        .build(log_dir)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_env("LUMEN_LOG").unwrap_or_else(|_| {
        EnvFilter::new("info,librespot_audio=warn,librespot_core::dealer=warn,symphonia=warn,zbus=warn,tracing=warn,eframe=warn,egui_glow=warn")
    });

    let file_layer = fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true);
    let stderr_layer = if cfg!(debug_assertions) || std::env::var_os("LUMEN_LOG_STDERR").is_some() {
        Some(fmt::layer().with_writer(std::io::stderr))
    } else {
        None
    };

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer);
    tracing::subscriber::set_global_default(subscriber)?;
    // Bridge the `log` crate (used by librespot) into tracing.
    tracing_log::LogTracer::init()?;
    Ok(guard)
}
