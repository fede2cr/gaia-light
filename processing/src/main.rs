//! Gaia Light Processing Server – analyses camera-trap video clips
//! for wildlife detection and species identification.
//!
//! Pipeline:
//! 1. Poll capture server for new MP4 clips
//! 2. Extract frames from each clip (via ffmpeg)
//! 3. Run MegaDetector (animal/person/vehicle detection)
//! 4. Optionally run SpeciesNet on animal crops (species ID)
//! 5. Store detections in SQLite and save crops

mod client;
mod db;
mod frames;
mod model;
mod reporting;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{debug, error, info, warn};

use gaia_light_common::config::Config;
use gaia_light_common::discovery::{self, ServiceRole};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    // ── Load config ──────────────────────────────────────────────
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| Config::default_path().to_string());
    let config =
        gaia_light_common::config::load(&PathBuf::from(&config_path))
            .context("Config load failed")?;

    info!(
        "Gaia Light Processing starting (models={:?}, confidence={}, poll={}s)",
        config.model_slugs, config.confidence, config.poll_interval_secs,
    );

    // ── Ensure directories exist ─────────────────────────────────
    std::fs::create_dir_all(&config.recs_dir)
        .context("Cannot create recs_dir")?;
    std::fs::create_dir_all(&config.extracted_dir)
        .context("Cannot create extracted_dir")?;
    std::fs::create_dir_all(&config.model_dir)
        .context("Cannot create model_dir")?;

    // ── Database ─────────────────────────────────────────────────
    let db = db::Database::open(&config.db_path)
        .context("Cannot open SQLite database")?;
    info!("Database ready at {}", config.db_path.display());

    // ── Load models ──────────────────────────────────────────────
    let detector = match model::Detector::load(&config) {
        Ok(d) => {
            info!("Detector model loaded");
            Some(d)
        }
        Err(e) => {
            warn!(
                "Detector model not available (will retry each cycle): {e:#}"
            );
            None
        }
    };

    let classifier = match model::Classifier::load(&config) {
        Ok(c) => {
            info!("Classifier model loaded");
            Some(c)
        }
        Err(e) => {
            info!(
                "Classifier not available (detections will lack species): {e:#}"
            );
            None
        }
    };

    // ── mDNS registration ────────────────────────────────────────
    let discovery_handle =
        match discovery::register(ServiceRole::Processing, 0) {
            Ok(h) => {
                info!("mDNS: registered as {}", h.instance_name());
                Some(h)
            }
            Err(e) => {
                warn!("mDNS registration failed (non-fatal): {e:#}");
                None
            }
        };

    // ── Ctrl-C handler ───────────────────────────────────────────
    ctrlc::set_handler(move || {
        SHUTDOWN.store(true, Ordering::Relaxed);
        info!("Shutdown signal received");
    })
    .context("Cannot set Ctrl-C handler")?;

    // ── Capture client ───────────────────────────────────────────
    let capture_client = client::CaptureClient::new(
        &config.capture_server_url,
        discovery_handle.as_ref(),
    );

    // ── Main processing loop ─────────────────────────────────────
    let poll_interval = Duration::from_secs(config.poll_interval_secs);
    let mut detector = detector;
    let mut classifier = classifier;

    info!("Entering processing loop (poll every {}s)", config.poll_interval_secs);
    while !SHUTDOWN.load(Ordering::Relaxed) {
        // Retry model loading if not yet available
        if detector.is_none() {
            if let Ok(d) = model::Detector::load(&config) {
                info!("Detector model now available");
                detector = Some(d);
            }
        }
        if classifier.is_none() {
            if let Ok(c) = model::Classifier::load(&config) {
                info!("Classifier model now available");
                classifier = Some(c);
            }
        }

        match process_cycle(
            &config,
            &capture_client,
            &db,
            detector.as_ref(),
            classifier.as_ref(),
        )
        .await
        {
            Ok(n) if n > 0 => info!("Cycle complete: processed {n} clip(s)"),
            Ok(_) => {}
            Err(e) => warn!("Processing cycle error: {e:#}"),
        }

        // Sleep in small increments so we respond to shutdown quickly
        let mut remaining = poll_interval;
        while remaining > Duration::ZERO && !SHUTDOWN.load(Ordering::Relaxed) {
            let step = remaining.min(Duration::from_secs(1));
            tokio::time::sleep(step).await;
            remaining = remaining.saturating_sub(step);
        }
    }

    // ── Cleanup ──────────────────────────────────────────────────
    if let Some(dh) = discovery_handle {
        dh.shutdown();
    }
    info!("Gaia Light Processing stopped");
    Ok(())
}

/// Run one processing cycle: fetch clips, analyse, report.
async fn process_cycle(
    config: &Config,
    capture_client: &client::CaptureClient,
    db: &db::Database,
    detector: Option<&model::Detector>,
    classifier: Option<&model::Classifier>,
) -> Result<usize> {
    let detector = match detector {
        Some(d) => d,
        None => {
            // No detector loaded yet -- nothing to do
            return Ok(0);
        }
    };

    // 1. List available clips from capture server
    let clips = capture_client.list_clips().await?;
    if clips.is_empty() {
        return Ok(0);
    }

    info!("Found {} clip(s) to process", clips.len());
    let mut processed = 0;

    for clip in &clips {
        if SHUTDOWN.load(Ordering::Relaxed) {
            break;
        }

        // Skip clips we have already processed (idempotent)
        if db.is_clip_processed(&clip.filename) {
            debug!("Skipping already-processed clip: {}", clip.filename);
            // Still delete from capture server to free space
            let _ = capture_client.delete_clip(&clip.filename).await;
            continue;
        }

        let clip_path = config.recs_dir.join(&clip.filename);

        // 2. Download clip
        if let Err(e) = capture_client
            .download_clip(&clip.filename, &clip_path)
            .await
        {
            warn!("Failed to download {}: {e:#}", clip.filename);
            continue;
        }

        // 3. Extract frames
        let frame_dir = config.recs_dir.join("_frames");
        std::fs::create_dir_all(&frame_dir)?;

        let frame_paths = match frames::extract_frames(
            &clip_path,
            config.capture_fps.max(1),
            &frame_dir,
        ) {
            Ok(paths) => paths,
            Err(e) => {
                warn!("Frame extraction failed for {}: {e:#}", clip.filename);
                cleanup_frames(&frame_dir);
                continue;
            }
        };

        if frame_paths.is_empty() {
            info!("No frames extracted from {}", clip.filename);
            cleanup_frames(&frame_dir);
            continue;
        }

        info!(
            "Extracted {} frame(s) from {}",
            frame_paths.len(),
            clip.filename
        );

        // 4. Run detection + classification on each frame
        for (frame_idx, frame_path) in frame_paths.iter().enumerate() {
            if SHUTDOWN.load(Ordering::Relaxed) {
                break;
            }

            match frames::load_image(frame_path) {
                Ok(img) => {
                    let detections =
                        detector.detect(&img, config.confidence);

                    for det in &detections {
                        // Optionally classify species from crop
                        let species = classifier.and_then(|c| {
                            let crop = frames::crop_detection(
                                &img,
                                det.x1,
                                det.y1,
                                det.x2,
                                det.y2,
                            );
                            c.classify(&crop, config.confidence)
                        });

                        // Save crop
                        let crop_path = reporting::save_crop(
                            &img,
                            det,
                            &clip.filename,
                            frame_idx,
                            &config.extracted_dir,
                        );

                        // Insert into DB
                        if let Err(e) = db.insert_detection(
                            &db::DetectionRow {
                                timestamp: clip.created.clone(),
                                clip_filename: clip.filename.clone(),
                                frame_index: frame_idx as i64,
                                detector_model: det.model_name.clone(),
                                class: det.class.clone(),
                                confidence: det.confidence,
                                bbox_x1: det.x1,
                                bbox_y1: det.y1,
                                bbox_x2: det.x2,
                                bbox_y2: det.y2,
                                species: species
                                    .as_ref()
                                    .map(|s| s.label.clone()),
                                species_confidence: species
                                    .as_ref()
                                    .map(|s| s.confidence),
                                crop_path: crop_path
                                    .map(|p| p.to_string_lossy().to_string()),
                                latitude: config.latitude,
                                longitude: config.longitude,
                                processing_instance: config
                                    .processing_instance
                                    .clone(),
                            },
                        ) {
                            error!(
                                "DB insert failed for {}/{}: {e}",
                                clip.filename, frame_idx
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Cannot load frame {} from {}: {e:#}",
                        frame_idx, clip.filename
                    );
                }
            }
        }

        // 5. Update live status with class breakdown, top species, and recent labels
        let class_counts = db.class_counts().unwrap_or_default();
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let top_species = db.species_summary(&today, &today).unwrap_or_default();
        let recent_labels: Vec<String> = db
            .recent_detections(5)
            .unwrap_or_default()
            .iter()
            .map(|d| {
                d.species
                    .as_deref()
                    .unwrap_or(&d.class)
                    .to_string()
            })
            .collect();

        reporting::write_live_status(
            &config.recs_dir,
            &clip.filename,
            frame_paths.len(),
            db.recent_detection_count(3600).unwrap_or(0),
            &class_counts,
            &top_species,
            &recent_labels,
        );

        // 6. Mark clip as processed in DB (idempotent guard)
        let det_count = db.recent_detection_count(60).unwrap_or(0);
        if let Err(e) =
            db.mark_clip_processed(&clip.filename, frame_paths.len(), det_count)
        {
            warn!("Cannot mark clip processed: {e:#}");
        }

        // 7. Clean up frames + source clip
        cleanup_frames(&frame_dir);
        let _ = std::fs::remove_file(&clip_path);

        // 8. Tell capture server we're done with this clip
        if let Err(e) = capture_client.delete_clip(&clip.filename).await {
            warn!("Failed to delete {} from capture server: {e:#}", clip.filename);
        }

        processed += 1;
    }

    Ok(processed)
}

/// Remove all files from the temporary frame directory.
fn cleanup_frames(dir: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
