//! Frame extraction from MP4 video clips and image utilities.
//!
//! Uses `ffmpeg` to extract JPEG frames at a target FPS, then loads
//! them via the `image` crate for inference.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView, RgbImage};
use tracing::debug;

/// Quick check that an MP4 file is a valid, seekable container.
///
/// ffmpeg's segment muxer writes the moov atom last.  If the file is
/// truncated (still being written, or I/O error) the atom is missing
/// and ffmpeg will fail with "moov atom not found".
///
/// Security-camera recordings (fragmented MP4, moov-at-start, etc.)
/// may not have a moov atom in the last 64 KB, so we fall back to
/// `ffprobe` to determine whether the file is actually playable.
fn is_valid_mp4(path: &Path) -> bool {
    // Fast path: scan the last 64 KB for a standard moov atom.
    if has_moov_tail(path) {
        return true;
    }

    // Slow path: ask ffprobe — handles fragmented MP4, moov-at-start,
    // and other container variations produced by NVRs / security cameras.
    let Ok(output) = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration"])
        .arg(path.as_os_str())
        .output()
    else {
        return false;
    };
    output.status.success()
}

/// Scan the last 64 KB of a file for the ASCII bytes `moov`.
fn has_moov_tail(path: &Path) -> bool {
    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    let search_start = data.len().saturating_sub(64 * 1024);
    data[search_start..]
        .windows(4)
        .any(|w| w == b"moov")
}

/// Extract JPEG frames from a video clip using ffmpeg.
///
/// Returns the paths to the extracted frame images, sorted by name.
pub fn extract_frames(
    clip_path: &Path,
    fps: u32,
    output_dir: &Path,
) -> Result<Vec<PathBuf>> {
    if !is_valid_mp4(clip_path) {
        anyhow::bail!(
            "MP4 file has no moov atom (truncated / still being written): {}",
            clip_path.display()
        );
    }
    // Clean any previous frames in the output dir
    if output_dir.exists() {
        for entry in std::fs::read_dir(output_dir)?.flatten() {
            if entry.path().extension().and_then(|e| e.to_str()) == Some("jpg") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    let pattern = output_dir.join("frame_%06d.jpg");

    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-i",
        ])
        .arg(clip_path.as_os_str())
        .args(["-vf", &format!("fps={fps}")])
        .args(["-q:v", "2"]) // JPEG quality (2 = high)
        .arg(pattern.as_os_str())
        .status()
        .context("Failed to spawn ffmpeg for frame extraction")?;

    if !status.success() {
        anyhow::bail!(
            "ffmpeg frame extraction exited with {}",
            status
        );
    }

    // Collect the extracted frames
    let mut frames: Vec<PathBuf> = std::fs::read_dir(output_dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("jpg")
        })
        .collect();

    frames.sort();
    debug!("Extracted {} frames to {}", frames.len(), output_dir.display());
    Ok(frames)
}

/// Load an image from disk as a `DynamicImage`.
pub fn load_image(path: &Path) -> Result<DynamicImage> {
    image::open(path)
        .with_context(|| format!("Cannot open image {}", path.display()))
}

/// Crop a detection region from an image.
///
/// Coordinates are normalised (0.0 - 1.0) relative to image dimensions.
pub fn crop_detection(
    img: &DynamicImage,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
) -> DynamicImage {
    let (w, h) = img.dimensions();
    let px1 = ((x1 * w as f64).round() as u32).min(w.saturating_sub(1));
    let py1 = ((y1 * h as f64).round() as u32).min(h.saturating_sub(1));
    let px2 = ((x2 * w as f64).round() as u32).min(w);
    let py2 = ((y2 * h as f64).round() as u32).min(h);

    let crop_w = px2.saturating_sub(px1).max(1);
    let crop_h = py2.saturating_sub(py1).max(1);

    img.crop_imm(px1, py1, crop_w, crop_h)
}

/// Resize an image to the given square dimension and convert to CHW
/// float32 tensor data (RGB, normalised to 0-1).
///
/// Returns a `Vec<f32>` of length `3 * size * size` in CHW order.
pub fn image_to_chw_f32(img: &DynamicImage, size: u32) -> Vec<f32> {
    let resized = img.resize_exact(
        size,
        size,
        image::imageops::FilterType::Triangle,
    );
    let rgb: RgbImage = resized.to_rgb8();

    let npixels = (size * size) as usize;
    let mut data = vec![0.0f32; 3 * npixels];

    for (i, pixel) in rgb.pixels().enumerate() {
        data[i] = pixel[0] as f32 / 255.0;                 // R channel
        data[npixels + i] = pixel[1] as f32 / 255.0;       // G channel
        data[2 * npixels + i] = pixel[2] as f32 / 255.0;   // B channel
    }

    data
}

/// Resize an image to the given dimensions and convert to HWC
/// float32 tensor data (RGB, normalised to 0-1).
///
/// Returns a `Vec<f32>` of length `3 * height * width` in HWC order.
///
/// Not currently used by the MegaDetector/SpeciesNet pipeline (they
/// use CHW), but kept for alternative models that expect HWC layout.
#[allow(dead_code)]
pub fn image_to_hwc_f32(img: &DynamicImage, width: u32, height: u32) -> Vec<f32> {
    let resized = img.resize_exact(
        width,
        height,
        image::imageops::FilterType::Triangle,
    );
    let rgb: RgbImage = resized.to_rgb8();

    rgb.pixels()
        .flat_map(|p| {
            [
                p[0] as f32 / 255.0,
                p[1] as f32 / 255.0,
                p[2] as f32 / 255.0,
            ]
        })
        .collect()
}
