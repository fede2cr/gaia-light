//! HTTP server – exposes the same API as the regular capture node.
//!
//! Routes:
//!   GET    /api/health          → health check
//!   GET    /api/clips           → list available MP4 symlinks
//!   GET    /api/clips/:name     → download (follows symlinks)
//!   DELETE /api/clips/:name     → move symlink to processed/

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::{delete, get};
use axum::Router;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tracing::info;

use gaia_light_common::protocol::{ClipInfo, HealthResponse};

/// Shared state for route handlers.
#[derive(Clone)]
struct AppState {
    stream_dir: PathBuf,
    processed_dir: PathBuf,
    start_time: Instant,
    #[allow(dead_code)]
    shutdown: Arc<AtomicBool>,
}

/// Start the HTTP server. Blocks until shutdown.
pub async fn run(
    stream_dir: PathBuf,
    processed_dir: PathBuf,
    listen_addr: &str,
    shutdown: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let state = AppState {
        stream_dir,
        processed_dir,
        start_time: Instant::now(),
        shutdown: shutdown.clone(),
    };

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/clips", get(list_clips))
        .route("/api/clips/:name", get(download_clip))
        .route("/api/clips/:name", delete(delete_clip))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = TcpListener::bind(listen_addr).await?;
    info!("Video-import HTTP server listening on {listen_addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
            }
        })
        .await?;

    Ok(())
}

// ── Route handlers ───────────────────────────────────────────────────────

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        uptime_secs: state.start_time.elapsed().as_secs(),
        disk_usage_pct: 0.0,
        capture_paused: false,
    })
}

async fn list_clips(
    State(state): State<AppState>,
) -> Result<Json<Vec<ClipInfo>>, StatusCode> {
    let dir = &state.stream_dir;
    if !dir.exists() {
        return Ok(Json(vec![]));
    }

    let mut clips = Vec::new();

    let entries =
        std::fs::read_dir(dir).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Skip files whose target was modified in the last 5 seconds —
    // the NVR may still be writing.  `metadata()` follows symlinks
    // so we get the underlying file's mtime.
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(5);

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("mp4") {
            continue;
        }
        if let Ok(meta) = path.metadata() {
            let modified =
                meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            if modified > cutoff {
                // Target still being written by the NVR
                continue;
            }
            if meta.len() == 0 {
                continue;
            }
            let created = modified
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| {
                    chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                })
                .unwrap_or_default();

            clips.push(ClipInfo {
                filename: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                size: meta.len(),
                created,
            });
        }
    }

    clips.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(Json(clips))
}

async fn download_clip(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    // Sanitise: prevent directory traversal
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let file_path = state.stream_dir.join(&name);
    if !file_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    // `tokio::fs::read` follows symlinks — serves the underlying file.
    let bytes = tokio::fs::read(&file_path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((
        [(axum::http::header::CONTENT_TYPE, "video/mp4")],
        Body::from(bytes),
    ))
}

/// Instead of deleting the original recording, move the symlink to
/// `processed/` so the scanner knows not to re-import it.
async fn delete_clip(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> StatusCode {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return StatusCode::BAD_REQUEST;
    }

    let src = state.stream_dir.join(&name);
    if !src.exists() {
        return StatusCode::NOT_FOUND;
    }

    let dst = state.processed_dir.join(&name);
    match tokio::fs::rename(&src, &dst).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
