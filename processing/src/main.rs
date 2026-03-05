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
mod download;
mod frames;
mod model;
mod motion;
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

    // ── Check for --check-models (build-time smoke test) ─────────
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--check-models") {
        return check_models(&args);
    }

    // ── Load config ──────────────────────────────────────────────
    let config_path = args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or_else(|| Config::default_path());
    let config =
        gaia_light_common::config::load(&PathBuf::from(config_path))
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

    // ── Ensure model files are present ────────────────────────
    // Seed from baked-in container image, or download from URLs.
    let required_files: Vec<&str> = {
        let mut files = vec![model::Detector::FILENAME];
        // Only require classifier files if speciesnet is in model_slugs
        if config.model_slugs.iter().any(|s| s.contains("speciesnet")) {
            files.push(model::Classifier::FILENAME);
            files.push(model::Classifier::LABELS_FILE);
        }
        files
    };
    let dl_errors = download::ensure_models(&config.model_dir, &required_files);
    if !dl_errors.is_empty() {
        for e in &dl_errors {
            warn!("Model download issue: {e}");
        }
        info!("Some models unavailable -- will retry loading each cycle");
    }

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
            discovery_handle.as_ref(),
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
    discovery: Option<&discovery::DiscoveryHandle>,
) -> Result<usize> {
    let detector = match detector {
        Some(d) => d,
        None => {
            // No detector loaded yet -- nothing to do
            return Ok(0);
        }
    };

    // 1. List available clips from capture server
    let clips = capture_client.list_clips(discovery).await?;
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
            let _ = capture_client.delete_clip(&clip.filename, discovery).await;
            continue;
        }

        let clip_path = config.recs_dir.join(&clip.filename);

        // 2. Download clip
        if let Err(e) = capture_client
            .download_clip(&clip.filename, &clip_path, discovery)
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

        // 3b. Motion pre-filter — skip clips with no inter-frame change
        let motion = motion::detect_motion(&frame_paths);
        if !motion.has_motion {
            info!(
                "Skipping {} — no motion detected (peak MAD={:.2})",
                clip.filename, motion.peak_mad
            );
            // Mark as processed (zero detections) so we don't re-download
            if let Err(e) = db.mark_clip_processed(&clip.filename, frame_paths.len(), 0) {
                warn!("Cannot mark static clip processed: {e:#}");
            }
            cleanup_frames(&frame_dir);
            let _ = std::fs::remove_file(&clip_path);
            if let Err(e) = capture_client.delete_clip(&clip.filename, discovery).await {
                warn!("Failed to delete {} from capture server: {e:#}", clip.filename);
            }
            processed += 1;
            continue;
        }

        // 4. Run detection + classification on each frame
        let mut clip_det_count: usize = 0;
        for (frame_idx, frame_path) in frame_paths.iter().enumerate() {
            if SHUTDOWN.load(Ordering::Relaxed) {
                break;
            }

            info!(
                "[{}/{}] Analysing frame {} of {}",
                clip.filename,
                frame_paths.len(),
                frame_idx + 1,
                frame_paths.len()
            );

            match frames::load_image(frame_path) {
                Ok(img) => {
                    let detections =
                        detector.detect(&img, config.confidence);

                    if detections.is_empty() {
                        info!(
                            "[{}/frame {}] No detections",
                            clip.filename, frame_idx
                        );
                    } else {
                        info!(
                            "[{}/frame {}] {} detection(s) found",
                            clip.filename, frame_idx, detections.len()
                        );
                        clip_det_count += detections.len();
                    }

                    // Save annotated preview (bounding boxes drawn on frame)
                    reporting::save_preview(
                        &img,
                        &detections,
                        &clip.filename,
                        frame_idx,
                        &config.recs_dir,
                    );

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
                            let result = c.classify(&crop, config.confidence);
                            match &result {
                                Some(cls) => info!(
                                    "  → Species: {} ({:.1}%)",
                                    cls.label, cls.confidence * 100.0
                                ),
                                None => debug!("  → No species classification above threshold"),
                            }
                            result
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

        info!(
            "Clip {} complete: {} frame(s) analysed, {} detection(s) total",
            clip.filename,
            frame_paths.len(),
            clip_det_count
        );

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
        if let Err(e) = capture_client.delete_clip(&clip.filename, discovery).await {
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

// ── Build-time smoke test ────────────────────────────────────────────

/// Load ONNX models from a given directory, run a dummy inference on
/// each, and exit.  Called with `--check-models <model_dir>`.
///
/// This is executed as part of the container build to fail early if the
/// exported ONNX is incompatible with tract.
fn check_models(args: &[String]) -> Result<()> {
    use image::DynamicImage;

    // Parse model dir from args: --check-models <dir>
    let model_dir = args
        .iter()
        .position(|a| a == "--check-models")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/local/share/gaia/models"));

    info!("=== Model smoke-test ===");
    info!("Model directory: {}", model_dir.display());

    // Build a minimal config pointing at the model dir
    let config = Config {
        latitude: 0.0,
        longitude: 0.0,
        confidence: 0.5,
        segment_length: 60,
        capture_fps: 0,
        capture_width: 0,
        capture_height: 0,
        video_device: None,
        rtsp_streams: vec![],
        recs_dir: PathBuf::from("/tmp"),
        extracted_dir: PathBuf::from("/tmp"),
        model_dir: model_dir.clone(),
        model_slugs: vec!["pytorch-wildlife".into()],
        processing_instance: "smoke-test".into(),
        db_path: PathBuf::from("/tmp/smoke-test.db"),
        capture_listen_addr: "0.0.0.0:8090".into(),
        capture_server_url: "http://localhost:8090".into(),
        poll_interval_secs: 5,
    };

    // --- Detector ---------------------------------------------------------
    info!("Loading detector ({})...", model::Detector::FILENAME);
    let detector = model::Detector::load(&config)
        .context("Detector model failed to load")?;
    info!("Detector loaded — running dummy inference...");

    // Tiny 8×8 black image — enough to exercise the full graph.
    let dummy = DynamicImage::new_rgb8(8, 8);
    let detections = detector.detect(&dummy, 0.99);
    info!(
        "Detector smoke-test OK ({} detections on blank image)",
        detections.len()
    );

    // --- Classifier (optional) --------------------------------------------
    let classifier_path = model_dir.join(model::Classifier::FILENAME);
    if classifier_path.exists() {
        info!("Loading classifier ({})...", model::Classifier::FILENAME);
        let classifier = model::Classifier::load(&config)
            .context("Classifier model failed to load")?;
        info!("Classifier loaded — running dummy inference...");

        let crop = DynamicImage::new_rgb8(8, 8);
        let _result = classifier.classify(&crop, 0.01);
        info!("Classifier smoke-test OK");
    } else {
        info!(
            "Classifier model ({}) not present — skipping (optional)",
            model::Classifier::FILENAME
        );
    }

    info!("=== All model checks passed ===");
    Ok(())
}
