//! Detection reporting: crop extraction, annotated preview, and live-status JSON.
//!
//! After each clip is processed the module:
//! - Saves a JPEG crop for every detection (animal bounding box)
//! - Writes an annotated preview frame (`preview_latest.jpg`)
//! - Writes a `live_status.json` file for the web dashboard

use std::path::{Path, PathBuf};

use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use tracing::{debug, info, warn};

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

// ── Preview frame with bounding boxes ────────────────────────────────────────

/// Colour palette for detection classes.
fn box_colour(class: &str) -> Rgb<u8> {
    match class {
        "animal"  => Rgb([76, 175, 80]),   // green
        "person"  => Rgb([66, 165, 245]),   // blue
        "vehicle" => Rgb([255, 152, 0]),    // orange
        _         => Rgb([158, 158, 158]),  // grey
    }
}

/// Draw a rectangle outline on an RGB image (2 px thick).
fn draw_rect(img: &mut RgbImage, x1: u32, y1: u32, x2: u32, y2: u32, colour: Rgb<u8>) {
    let (w, h) = img.dimensions();
    let x2 = x2.min(w.saturating_sub(1));
    let y2 = y2.min(h.saturating_sub(1));

    for thickness in 0..2u32 {
        let t = thickness;
        // Horizontal edges
        for x in x1.saturating_sub(t)..=x2.saturating_add(t).min(w - 1) {
            if y1 + t < h { img.put_pixel(x, y1 + t, colour); }
            if y1 >= t     { img.put_pixel(x, y1 - t, colour); }
            if y2 + t < h { img.put_pixel(x, y2 + t, colour); }
            if y2 >= t     { img.put_pixel(x, y2 - t, colour); }
        }
        // Vertical edges
        for y in y1.saturating_sub(t)..=y2.saturating_add(t).min(h - 1) {
            if x1 + t < w { img.put_pixel(x1 + t, y, colour); }
            if x1 >= t     { img.put_pixel(x1 - t, y, colour); }
            if x2 + t < w { img.put_pixel(x2 + t, y, colour); }
            if x2 >= t     { img.put_pixel(x2 - t, y, colour); }
        }
    }
}

/// Draw a small filled label background above a bounding box.
fn draw_label_bg(img: &mut RgbImage, x1: u32, y1: u32, label_width: u32, colour: Rgb<u8>) {
    let (w, h) = img.dimensions();
    let label_h = 16u32;
    let ly = y1.saturating_sub(label_h);
    for y in ly..y1.min(h) {
        for x in x1..(x1 + label_width).min(w) {
            img.put_pixel(x, y, colour);
        }
    }
}

/// Save an annotated preview frame with detection bounding boxes drawn.
///
/// Writes to `{data_dir}/preview_latest.jpg` atomically (via tmp + rename).
/// The web dashboard can poll `/preview/preview_latest.jpg` for a live view.
pub fn save_preview(
    img: &DynamicImage,
    detections: &[Detection],
    clip_name: &str,
    frame_idx: usize,
    data_dir: &Path,
) {
    let (iw, ih) = img.dimensions();
    let mut canvas = img.to_rgb8();

    for det in detections {
        let colour = box_colour(&det.class);
        let px1 = (det.x1 * iw as f64).round() as u32;
        let py1 = (det.y1 * ih as f64).round() as u32;
        let px2 = (det.x2 * iw as f64).round() as u32;
        let py2 = (det.y2 * ih as f64).round() as u32;

        draw_rect(&mut canvas, px1, py1, px2, py2, colour);

        // Label background (approximate width from text length)
        let label = format!("{} {:.0}%", det.class, det.confidence * 100.0);
        let label_w = label.len() as u32 * 7 + 8; // ~7px per char
        draw_label_bg(&mut canvas, px1, py1, label_w, colour);
    }

    // Write atomically — use a .jpg tmp name so `image` recognises the format
    let dest = data_dir.join("preview_latest.jpg");
    let tmp = data_dir.join(".preview_latest.tmp.jpg");

    let annotated = DynamicImage::ImageRgb8(canvas);
    match annotated.save(&tmp) {
        Ok(()) => {
            if let Err(e) = std::fs::rename(&tmp, &dest) {
                warn!("Cannot rename preview: {e}");
            } else {
                debug!(
                    "Saved preview: {} ({iw}x{ih}, {} detections, {clip_name} frame {frame_idx})",
                    dest.display(),
                    detections.len()
                );
            }
        }
        Err(e) => warn!("Cannot save preview: {e}"),
    }
}

/// Save a preview for a frame with no detections (clean image, no boxes).
pub fn save_preview_clean(
    img: &DynamicImage,
    clip_name: &str,
    frame_idx: usize,
    data_dir: &Path,
) {
    save_preview(img, &[], clip_name, frame_idx, data_dir);
}
