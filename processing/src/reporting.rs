//! Detection reporting: crop extraction and live-status JSON.
//!
//! After each clip is processed the module:
//! - Saves a JPEG crop for every detection (animal bounding box)
//! - Writes a `live_status.json` file for the web dashboard

use std::path::{Path, PathBuf};

use image::DynamicImage;
use tracing::{debug, warn};

use crate::model::Detection;

/// Save a cropped image for a detection.
///
/// Returns the path to the saved crop, or `None` on failure.
pub fn save_crop(
    img: &DynamicImage,
    det: &Detection,
    clip_name: &str,
    frame_idx: usize,
    extracted_dir: &Path,
) -> Option<PathBuf> {
    use crate::frames;

    let crop = frames::crop_detection(img, det.x1, det.y1, det.x2, det.y2);

    // Build filename: {clip_stem}_{frame}_{class}_{confidence}.jpg
    let stem = clip_name
        .strip_suffix(".mp4")
        .unwrap_or(clip_name);
    let conf_pct = (det.confidence * 100.0) as u32;
    let filename = format!(
        "{stem}_f{frame_idx:04}_{class}_{conf_pct}.jpg",
        class = det.class
    );

    let dest = extracted_dir.join(&filename);

    match crop.save(&dest) {
        Ok(()) => {
            debug!("Saved crop: {}", dest.display());
            Some(dest)
        }
        Err(e) => {
            warn!("Cannot save crop {}: {e}", dest.display());
            None
        }
    }
}

/// Write a live-status JSON file for the web dashboard.
///
/// The file is written atomically (write to .tmp, then rename) so
/// readers never see a partial file.
pub fn write_live_status(
    data_dir: &Path,
    last_clip: &str,
    frame_count: usize,
    detections_last_hour: usize,
    class_counts: &[(String, i64)],
    top_species: &[(String, i64)],
    recent_labels: &[String],
) {
    let status = serde_json::json!({
        "last_clip": last_clip,
        "frame_count": frame_count,
        "detections_last_hour": detections_last_hour,
        "class_counts": class_counts.iter()
            .map(|(c, n)| serde_json::json!({"class": c, "count": n}))
            .collect::<Vec<_>>(),
        "top_species": top_species.iter()
            .map(|(s, n)| serde_json::json!({"species": s, "count": n}))
            .collect::<Vec<_>>(),
        "recent_labels": recent_labels,
        "updated_at": chrono::Utc::now().to_rfc3339(),
    });

    let path = data_dir.join("live_status.json");
    let tmp = data_dir.join("live_status.json.tmp");

    let json =
        serde_json::to_string_pretty(&status).unwrap_or_default();

    if let Err(e) = std::fs::write(&tmp, json.as_bytes()) {
        warn!("Cannot write live status tmp: {e}");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        warn!("Cannot rename live status: {e}");
    }
}
