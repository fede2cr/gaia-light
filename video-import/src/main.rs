//! Gaia Light Video Import – serves pre-recorded security-camera
//! videos to processing nodes through the standard capture HTTP API.
//!
//! Instead of recording from a live camera, this binary watches a
//! directory of existing recordings (e.g. from an NVR), creates
//! symlinks with standardised names, and exposes them through the
//! same HTTP endpoints that processing nodes already consume.
//!
//! # Configuration (environment variables)
//!
//! | Variable             | Required | Default         | Description                          |
//! |----------------------|----------|-----------------|--------------------------------------|
//! | `IMPORT_DIR`         | yes      | —               | Source directory with date sub-dirs   |
//! | `CAMERA_NAME`        | yes      | —               | Human-readable camera identifier     |
//! | `LISTEN_ADDR`        | no       | `0.0.0.0:8090`  | HTTP listen address                  |
//! | `DATA_DIR`           | no       | `/data`         | Working directory for symlinks       |
//! | `SCAN_INTERVAL_SECS` | no       | `30`            | Seconds between re-scans             |
//!
//! # Container usage
//!
//! ```sh
//! podman run --rm --network=host \
//!   -v /path/to/nvr/recordings:/import:ro \
//!   -v video-import-data:/data \
//!   -e IMPORT_DIR=/import \
//!   -e CAMERA_NAME=front-yard \
//!   gaia-light-video-import
//! ```

mod import;
mod server;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tracing::info;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_required(key: &str) -> Result<String> {
    std::env::var(key)
        .with_context(|| format!("{key} environment variable is required"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    if std::env::var("RUST_LOG").map_or(false, |v| v.contains("debug")) {
        info!("🔍 Debug logging ENABLED (RUST_LOG={})", std::env::var("RUST_LOG").unwrap_or_default());
    }

    // ── load configuration ───────────────────────────────────────────
    let import_dir = PathBuf::from(env_required("IMPORT_DIR")?);
    let camera_name = env_required("CAMERA_NAME")?;
    let listen_addr = env_or("LISTEN_ADDR", "0.0.0.0:8090");
    let data_dir = PathBuf::from(env_or("DATA_DIR", "/data"));
    let scan_interval = Duration::from_secs(
        env_or("SCAN_INTERVAL_SECS", "30").parse().unwrap_or(30),
    );

    if !import_dir.exists() {
        bail!("IMPORT_DIR does not exist: {}", import_dir.display());
    }

    info!(
        "Gaia Light Video Import starting (camera={camera_name}, \
         import={}, listen={listen_addr})",
        import_dir.display(),
    );

    let stream_dir = data_dir.join("StreamData");
    let processed_dir = data_dir.join("processed");
    std::fs::create_dir_all(&stream_dir)
        .context("Cannot create StreamData directory")?;
    std::fs::create_dir_all(&processed_dir)
        .context("Cannot create processed directory")?;

    // ── ctrl-c ───────────────────────────────────────────────────────
    ctrlc::set_handler(move || {
        SHUTDOWN.store(true, Ordering::Relaxed);
        info!("Shutdown signal received");
        std::process::exit(0);
    })
    .context("Cannot set Ctrl-C handler")?;

    // ── initial scan ─────────────────────────────────────────────────
    let count = import::scan_and_link(
        &import_dir,
        &camera_name,
        &stream_dir,
        &processed_dir,
    )?;
    info!("Initial scan: {count} new clip(s) linked");

    // ── mDNS registration ────────────────────────────────────────────
    let port: u16 = listen_addr
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8090);

    let discovery = match gaia_light_common::discovery::register(
        gaia_light_common::discovery::ServiceRole::Capture,
        port,
    ) {
        Ok(h) => {
            info!("mDNS: registered as {}", h.instance_name());
            Some(h)
        }
        Err(e) => {
            tracing::warn!("mDNS registration failed (non-fatal): {e:#}");
            None
        }
    };

    // ── periodic re-scan ─────────────────────────────────────────────
    let scan_import = import_dir.clone();
    let scan_camera = camera_name.clone();
    let scan_stream = stream_dir.clone();
    let scan_processed = processed_dir.clone();

    let _scanner = tokio::task::spawn_blocking(move || {
        while !SHUTDOWN.load(Ordering::Relaxed) {
            std::thread::sleep(scan_interval);
            if SHUTDOWN.load(Ordering::Relaxed) {
                break;
            }
            match import::scan_and_link(
                &scan_import,
                &scan_camera,
                &scan_stream,
                &scan_processed,
            ) {
                Ok(n) if n > 0 => info!("Re-scan: {n} new clip(s) linked"),
                Ok(_) => {}
                Err(e) => tracing::warn!("Re-scan error: {e:#}"),
            }
        }
    });

    // ── start HTTP server (blocks until shutdown) ────────────────────
    let shutdown = Arc::new(AtomicBool::new(false));
    server::run(stream_dir, processed_dir, &listen_addr, shutdown).await?;

    // ── clean up ─────────────────────────────────────────────────────
    SHUTDOWN.store(true, Ordering::Relaxed);
    if let Some(dh) = discovery {
        dh.shutdown();
    }
    info!("Gaia Light Video Import stopped");

    Ok(())
}
