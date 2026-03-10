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
use std::time::{Duration, Instant};

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

    if std::env::var("RUST_LOG").map_or(false, |v| v.contains("debug")) {
        info!("🔍 Debug logging ENABLED (RUST_LOG={})", std::env::var("RUST_LOG").unwrap_or_default());
    }

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
        // Seed ONNX + labels for every configured classifier.
        for kind in &config.classifiers {
            files.push(kind.onnx_filename());
            files.push(kind.labels_filename());
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

    // Load all configured classifiers.  Those whose ONNX file is not
    // yet available are retried each processing cycle.
    let mut classifiers: Vec<model::Classifier> = Vec::new();
    let mut missing_classifiers: Vec<gaia_light_common::classifier_kind::ClassifierKind> = Vec::new();
    for &kind in &config.classifiers {
        match model::Classifier::load(&config, kind) {
            Ok(c) => {
                info!("Classifier loaded: {}", kind.display_name());
                classifiers.push(c);
            }
            Err(e) => {
                info!("Classifier {} not available (will retry): {e:#}", kind.slug());
                missing_classifiers.push(kind);
            }
        }
    }

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

    // ── Discover capture nodes ───────────────────────────────────
    let mut capture_urls = capture_client.resolve_capture_urls(
        discovery_handle.as_ref(),
    );
    info!(
        "Polling {} capture server(s): {:?}",
        capture_urls.len(),
        capture_urls
    );
    let mut last_discovery = Instant::now();

    // ── Main processing loop ─────────────────────────────────────
    let poll_interval = Duration::from_secs(config.poll_interval_secs);
    let mut detector = detector;

    info!(
        "Entering processing loop (poll every {}s, classifiers: [{}])",
        config.poll_interval_secs,
        config.classifiers.iter().map(|k| k.slug()).collect::<Vec<_>>().join(", "),
    );
    while !SHUTDOWN.load(Ordering::Relaxed) {
        // Retry model loading if not yet available
        if detector.is_none() {
            if let Ok(d) = model::Detector::load(&config) {
                info!("Detector model now available");
                detector = Some(d);
            }
        }
        // Retry any classifiers that failed to load initially
        missing_classifiers.retain(|&kind| {
            match model::Classifier::load(&config, kind) {
                Ok(c) => {
                    info!("Classifier now available: {}", kind.display_name());
                    classifiers.push(c);
                    false // remove from missing list
                }
                Err(_) => true, // keep in missing list
            }
        });

        // Read runtime settings from the shared volume (written by the
        // web dashboard settings page).  Overrides take effect each cycle.
        let rt = gaia_light_common::settings::load(&config.recs_dir);
        let effective_confidence = rt.confidence.unwrap_or(config.confidence);
        let effective_species_conf = rt.species_confidence.unwrap_or(config.species_confidence);
        let effective_poll = rt.poll_interval_secs.unwrap_or(config.poll_interval_secs);
        let effective_max_frames = rt.max_frames_per_clip.unwrap_or(config.max_frames_per_clip);
        let effective_motion_threshold = rt.motion_threshold.unwrap_or(config.motion_threshold);

        // Filter classifiers to only those selected by runtime settings.
        // If rt.classifiers is None, use all loaded classifiers.
        let active_classifiers: Vec<&model::Classifier> = match &rt.classifiers {
            Some(selected) => classifiers
                .iter()
                .filter(|c| selected.contains(&c.kind))
                .collect(),
            None => classifiers.iter().collect(),
        };

        // ── periodic mDNS re-discovery ───────────────────────────
        if last_discovery.elapsed() >= client::REDISCOVERY_INTERVAL {
            let new_urls = capture_client.resolve_capture_urls(
                discovery_handle.as_ref(),
            );
            if new_urls != capture_urls {
                info!("Capture node list updated: {:?}", new_urls);
                capture_urls = new_urls;
            }
            last_discovery = Instant::now();
        }

        match process_cycle(
            &config,
            &capture_client,
            &db,
            detector.as_ref(),
            &active_classifiers,
            &capture_urls,
            effective_confidence,
            effective_species_conf,
            effective_max_frames,
            effective_motion_threshold,
        )
        .await
        {
            Ok(n) if n > 0 => info!("Cycle complete: processed {n} clip(s)"),
            Ok(_) => {}
            Err(e) => warn!("Processing cycle error: {e:#}"),
        }

        // Sleep in small increments so we respond to shutdown quickly
        let poll_interval = Duration::from_secs(effective_poll);
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

/// Run one processing cycle: fetch clips from all capture nodes and
/// process them in round-robin (up to [`client::BATCH_PER_NODE`] clips
/// per node per cycle).
async fn process_cycle(
    config: &Config,
    capture_client: &client::CaptureClient,
    db: &db::Database,
    detector: Option<&model::Detector>,
    classifiers: &[&model::Classifier],
    capture_urls: &[String],
    effective_confidence: f64,
    effective_species_conf: f64,
    effective_max_frames: u32,
    effective_motion_threshold: f64,
) -> Result<usize> {
    let detector = match detector {
        Some(d) => d,
        None => {
            // No detector loaded yet -- nothing to do
            return Ok(0);
        }
    };

    let mut total_processed = 0;

    for base_url in capture_urls {
        if SHUTDOWN.load(Ordering::Relaxed) {
            break;
        }

        // 1. List available clips from this capture server
        let clips = match capture_client.list_clips(base_url).await {
            Ok(c) => c,
            Err(e) => {
                warn!("Cannot reach capture server {}: {e:#}", base_url);
                continue;
            }
        };
        if clips.is_empty() {
            continue;
        }

        info!(
            "[{}] Found {} clip(s) to process",
            base_url,
            clips.len()
        );

        let mut batch_count = 0;
        for clip in &clips {
            if SHUTDOWN.load(Ordering::Relaxed) {
                break;
            }

            // Round-robin: only process a limited batch from each
            // capture node per cycle so we service all nodes fairly.
            if batch_count >= client::BATCH_PER_NODE {
                debug!(
                    "[{}] Batch limit ({}) reached – rotating to next node",
                    base_url, client::BATCH_PER_NODE
                );
                break;
            }

            // Skip clips we have already processed (idempotent)
            if db.is_clip_processed(&clip.filename) {
                debug!("Skipping already-processed clip: {}", clip.filename);
                // Still delete from capture server to free space
                info!("Requesting capture to delete already-processed clip: {}", clip.filename);
                let _ = capture_client.delete_clip(base_url, &clip.filename).await;
                continue;
            }

            let clip_path = config.recs_dir.join(&clip.filename);

            // Track overall clip processing time
            let clip_start = Instant::now();

            // 2. Download clip
            if let Err(e) = capture_client
                .download_clip(base_url, &clip.filename, &clip_path)
                .await
            {
                warn!("Failed to download {}: {e:#}", clip.filename);
                continue;
            }

        // 3. Extract frames
        let frame_dir = config.recs_dir.join("_frames");
        std::fs::create_dir_all(&frame_dir)?;

        let extract_start = Instant::now();
        let frame_paths = match frames::extract_frames(
            &clip_path,
            config.capture_fps.max(1),
            &frame_dir,
        ) {
            Ok(paths) => {
                debug!(
                    "Frame extraction for {} took {:.1}s ({} frames)",
                    clip.filename,
                    extract_start.elapsed().as_secs_f64(),
                    paths.len()
                );
                paths
            }
            Err(e) => {
                warn!("Frame extraction failed for {}: {e:#}", clip.filename);

                let _ = std::fs::remove_file(&clip_path);
                cleanup_frames(&frame_dir);

                // Mark the clip as processed locally so we don't
                // re-download it every cycle, but do NOT delete it
                // from the capture server — the file may be valid
                // under a different ffmpeg version or container
                // variant (e.g. fragmented MP4 from an NVR).
                warn!(
                    "Skipping {} — will not retry until processed_clips is reset",
                    clip.filename
                );
                db.mark_clip_processed(&clip.filename, 0, 0)?;

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

        // 3a′. Always update the camera preview from the first frame
        //       so the web dashboard shows what the camera sees, even
        //       when the clip turns out to be static.
        if let Some(first_frame) = frame_paths.first() {
            if let Ok(img) = frames::load_image(first_frame) {
                reporting::save_preview_clean(
                    &img,
                    &clip.filename,
                    0,
                    &config.recs_dir,
                );
            }
        }

        // 3b. Run MegaDetector on the first frame only.
        //     • Detection found  → process ALL frames (animal present).
        //     • No detection     → fall through to motion check.
        let first_frame_has_detections = {
            let mut found = false;
            if let Some(first_frame) = frame_paths.first() {
                if let Ok(img) = frames::load_image(first_frame) {
                    let dets = detector.detect(&img, effective_confidence);
                    if !dets.is_empty() {
                        info!(
                            "[{}/frame 0] First-frame probe: {} detection(s) — will process all frames",
                            clip.filename, dets.len()
                        );
                        found = true;
                    } else {
                        info!(
                            "[{}/frame 0] First-frame probe: no detections",
                            clip.filename
                        );
                    }
                }
            }
            found
        };

        // 3c. If the first frame had no detections, check for motion.
        //     No detections + no motion → skip the clip entirely.
        if !first_frame_has_detections {
            let motion = motion::detect_motion(&frame_paths, effective_motion_threshold);
            if !motion.has_motion {
                debug!(
                    "Motion check for {}: peak_mad={:.4}, threshold={:.2} → static",
                    clip.filename, motion.peak_mad, effective_motion_threshold
                );
                info!(
                    "Skipping {} — no first-frame detections and no motion (peak MAD={:.2})",
                    clip.filename, motion.peak_mad
                );
                if let Err(e) = db.mark_clip_processed(&clip.filename, frame_paths.len(), 0) {
                    warn!("Cannot mark static clip processed: {e:#}");
                }
                cleanup_frames(&frame_dir);
                let _ = std::fs::remove_file(&clip_path);
                info!("Requesting capture to delete static clip: {}", clip.filename);
                if let Err(e) = capture_client.delete_clip(base_url, &clip.filename).await {
                    warn!("Failed to delete {} from capture server: {e:#}", clip.filename);
                }
                total_processed += 1;
                batch_count += 1;
                continue;
            }
            info!(
                "No first-frame detections but motion found (peak MAD={:.2}) — processing all frames",
                motion.peak_mad
            );
        }

        // 4. Run detection + classification on each frame
        let analysis_frames: &[std::path::PathBuf] = if effective_max_frames > 0 {
            &frame_paths[..frame_paths.len().min(effective_max_frames as usize)]
        } else {
            &frame_paths
        };
        let mut clip_det_count: usize = 0;
        for (frame_idx, frame_path) in analysis_frames.iter().enumerate() {
            if SHUTDOWN.load(Ordering::Relaxed) {
                break;
            }

            let frame_start = Instant::now();
            info!(
                "[{}/{}] Analysing frame {} of {}",
                clip.filename,
                analysis_frames.len(),
                frame_idx + 1,
                analysis_frames.len()
            );

            match frames::load_image(frame_path) {
                Ok(img) => {
                    let detections =
                        detector.detect(&img, effective_confidence);

                    debug!(
                        "[{}/frame {}] Detection took {:.1}ms",
                        clip.filename,
                        frame_idx,
                        frame_start.elapsed().as_secs_f64() * 1000.0
                    );

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
                        // Classify species from crop using ALL configured classifiers.
                        // Keep the best result (highest confidence above threshold).
                        let crop = frames::crop_detection(
                            &img,
                            det.x1,
                            det.y1,
                            det.x2,
                            det.y2,
                        );

                        let mut best_species: Option<model::Classification> = None;
                        for cls in classifiers {
                            let cls_start = Instant::now();
                            if let Some(result) = cls.classify(&crop, effective_species_conf) {
                                debug!(
                                    "  → [{}] classified in {:.0}ms: {} ({:.1}%)",
                                    cls.kind.slug(),
                                    cls_start.elapsed().as_secs_f64() * 1000.0,
                                    result.label,
                                    result.confidence * 100.0,
                                );
                                info!(
                                    "  → [{}] Species: {} ({:.1}%)",
                                    cls.kind.slug(),
                                    result.label,
                                    result.confidence * 100.0,
                                );
                                // Keep result with highest confidence
                                if best_species.as_ref().map_or(true, |b| result.confidence > b.confidence) {
                                    best_species = Some(result);
                                }
                            } else {
                                debug!("  → [{}] No species above threshold", cls.kind.slug());
                            }
                        }

                        if classifiers.is_empty() {
                            debug!("  → No classifiers configured");
                        }

                        // Track high-confidence animal detections without a
                        // species label — useful as training data for future
                        // classification models.
                        let is_training_candidate = best_species.is_none()
                            && det.class == "animal"
                            && det.confidence >= 0.8;

                        // Save crop
                        let crop_path = reporting::save_crop(
                            &img,
                            det,
                            &clip.filename,
                            frame_idx,
                            &config.extracted_dir,
                        );

                        // Build detection row
                        let row = db::DetectionRow {
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
                            species: best_species
                                .as_ref()
                                .map(|s| s.label.clone()),
                            species_confidence: best_species
                                .as_ref()
                                .map(|s| s.confidence),
                            species_model: best_species
                                .as_ref()
                                .map(|s| s.model_name.clone()),
                            crop_path: crop_path
                                .map(|p| p.to_string_lossy().to_string()),
                            latitude: config.latitude,
                            longitude: config.longitude,
                            processing_instance: config
                                .processing_instance
                                .clone(),
                            source_node: base_url.to_string(),
                        };

                        // Insert into DB
                        if let Err(e) = db.insert_detection(&row) {
                            error!(
                                "DB insert failed for {}/{}: {e}",
                                clip.filename, frame_idx
                            );
                        }

                        // Track as training candidate if high-confidence
                        // animal with no species label.
                        if is_training_candidate {
                            if let Err(e) = db.insert_training_candidate(&row) {
                                debug!(
                                    "Training candidate insert failed: {e}"
                                );
                            }
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
            "Clip {} complete: {} frame(s) analysed, {} detection(s) total (took {:.1}s)",
            clip.filename,
            frame_paths.len(),
            clip_det_count,
            clip_start.elapsed().as_secs_f64()
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
            base_url,
            &clip.created,
        );

        // 6. Mark clip as processed in DB (idempotent guard)
        let det_count = db.recent_detection_count(60).unwrap_or(0);
        if let Err(e) =
            db.mark_clip_processed(&clip.filename, frame_paths.len(), det_count)
        {
            warn!("Cannot mark clip processed: {e:#}");
        } else {
            debug!(
                "Marked clip {} as processed ({} frames, {} detections)",
                clip.filename, frame_paths.len(), det_count
            );
        }

        // 7. Clean up frames + source clip
        cleanup_frames(&frame_dir);
        let _ = std::fs::remove_file(&clip_path);

        // 8. Tell capture server we're done with this clip
        info!("Requesting capture to delete processed clip: {}", clip.filename);
        if let Err(e) = capture_client.delete_clip(base_url, &clip.filename).await {
            warn!("Failed to delete {} from capture server: {e:#}", clip.filename);
        }

        total_processed += 1;
        batch_count += 1;
    }
    } // end for base_url

    Ok(total_processed)
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
        species_confidence: 0.1,
        max_frames_per_clip: 0,
        motion_threshold: 1.5,
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
        classifiers: vec![],  // populated dynamically below
        processing_instance: "smoke-test".into(),
        db_path: PathBuf::from("/tmp/smoke-test.db"),
        disk_usage_max: 95.0,
        brightness_threshold: 20.0,
        brightness_probe_interval: 0,
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

    // --- Classifiers (optional — smoke-test every ONNX file present) -----
    use gaia_light_common::classifier_kind::ClassifierKind;
    for &kind in ClassifierKind::ALL {
        let onnx_path = model_dir.join(kind.onnx_filename());
        if onnx_path.exists() {
            info!("Loading classifier {} ({})...", kind.display_name(), kind.onnx_filename());
            let classifier = model::Classifier::load(&config, kind)
                .with_context(|| format!("{} model failed to load", kind.slug()))?;
            info!("Classifier loaded — running dummy inference...");

            let crop = DynamicImage::new_rgb8(8, 8);
            let _result = classifier.classify(&crop, 0.01);
            info!("Classifier {} smoke-test OK", kind.slug());
        } else {
            info!(
                "Classifier {} ({}) not present — skipping (optional)",
                kind.slug(),
                kind.onnx_filename(),
            );
        }
    }

    info!("=== All model checks passed ===");
    Ok(())
}
