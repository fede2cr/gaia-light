//! Cheap frame-differencing motion detector.
//!
//! Runs **before** the expensive ONNX model inference to skip clips
//! that contain only static background (e.g. wind-free vegetation).
//!
//! ## Algorithm
//!
//! 1. Convert each frame to greyscale and down-sample to a small
//!    thumbnail (`THUMB_W × THUMB_H`) so the comparison is fast.
//! 2. For each consecutive pair of thumbnails, compute the
//!    **Mean Absolute Difference** (MAD) of pixel intensities.
//! 3. A pair is classified as "has motion" when the MAD exceeds
//!    [`MOTION_THRESHOLD`].
//!
//! If *any* pair in the clip shows motion, the clip is worth running
//! through the detector.  Single-frame clips are always considered
//! active (no basis for comparison).
//!
//! The entire pass is pure CPU pixel arithmetic — no ONNX, no GPU.

use std::path::Path;

use image::imageops::FilterType;
use image::DynamicImage;
use tracing::{debug, info};

// ── Tuning knobs ─────────────────────────────────────────────────────

/// Thumbnail width used for differencing (pixels).
const THUMB_W: u32 = 320;
/// Thumbnail height used for differencing (pixels).
const THUMB_H: u32 = 240;

/// Mean Absolute Difference threshold (0–255 scale).
///
/// Typical values for camera-trap footage:
///   2–5   → very sensitive (leaf rustle, cloud shadow)
///   8–15  → moderate (walking animal fills a portion of the frame)
///   20+   → only large/fast movement
///
/// 5.0 is a conservative default — most real animal activity exceeds
/// this easily, while electrical noise and subtle lighting drift stay
/// below it.
const MOTION_THRESHOLD: f64 = 5.0;

/// Fraction of pixels that must individually exceed a per-pixel
/// threshold before the frame pair is flagged as motion.  This is a
/// secondary guard against sensor noise which can raise the global MAD
/// without any real movement.
///
/// Set to 0.0 to disable the per-pixel check and rely solely on MAD.
const _PIXEL_FRAC_THRESHOLD: f64 = 0.0; // reserved for future use

// ── Public API ───────────────────────────────────────────────────────

/// Result of a motion scan on a clip's extracted frames.
#[derive(Debug, Clone)]
pub struct MotionResult {
    /// Whether any consecutive frame pair exceeded the motion threshold.
    pub has_motion: bool,
    /// The highest MAD value observed across all frame pairs.
    pub peak_mad: f64,
    /// Number of frame pairs that exceeded the threshold.
    pub active_pairs: usize,
    /// Total number of frame pairs compared.
    pub total_pairs: usize,
}

/// Scan a sequence of frame images for inter-frame motion.
///
/// `frame_paths` must be sorted in temporal order (as returned by
/// [`crate::frames::extract_frames`]).
///
/// Returns a [`MotionResult`] describing the motion content.
/// This function is intentionally **infallible** — if a frame cannot
/// be loaded it is silently skipped (better to err on the side of
/// running the detector).
pub fn detect_motion(frame_paths: &[impl AsRef<Path>]) -> MotionResult {
    let n = frame_paths.len();

    // Trivial cases: 0 or 1 frame → always consider active
    if n <= 1 {
        return MotionResult {
            has_motion: true,
            peak_mad: f64::NAN,
            active_pairs: 0,
            total_pairs: 0,
        };
    }

    let mut prev_thumb: Option<Vec<u8>> = None;
    let mut peak_mad: f64 = 0.0;
    let mut active_pairs: usize = 0;
    let mut total_pairs: usize = 0;

    for (i, path) in frame_paths.iter().enumerate() {
        let thumb = match load_grey_thumb(path.as_ref()) {
            Some(t) => t,
            None => {
                // Cannot load → reset prev so we don't compare stale data.
                prev_thumb = None;
                continue;
            }
        };

        if let Some(ref prev) = prev_thumb {
            let mad = mean_abs_diff(prev, &thumb);
            total_pairs += 1;

            if mad > peak_mad {
                peak_mad = mad;
            }

            if mad > MOTION_THRESHOLD {
                debug!(
                    "Motion detected between frames {} and {} (MAD={:.2})",
                    i - 1,
                    i,
                    mad
                );
                active_pairs += 1;
            }
        }

        prev_thumb = Some(thumb);
    }

    let has_motion = active_pairs > 0;

    info!(
        "Motion scan: {} pair(s) checked, {} active (peak MAD={:.2}, threshold={}) → {}",
        total_pairs,
        active_pairs,
        peak_mad,
        MOTION_THRESHOLD,
        if has_motion { "MOTION" } else { "STATIC" }
    );

    MotionResult {
        has_motion,
        peak_mad,
        active_pairs,
        total_pairs,
    }
}

// ── Internals ────────────────────────────────────────────────────────

/// Load an image, convert to greyscale, resize to thumbnail, return raw
/// luma bytes.  Returns `None` on any I/O or decode error.
fn load_grey_thumb(path: &Path) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?;
    let grey = img.grayscale();
    let resized = grey.resize_exact(THUMB_W, THUMB_H, FilterType::Triangle);
    Some(resized.to_luma8().into_raw())
}

/// Mean Absolute Difference of two equal-length byte slices.
fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    debug_assert_eq!(a.len(), b.len());
    if a.is_empty() {
        return 0.0;
    }
    let sum: u64 = a
        .iter()
        .zip(b.iter())
        .map(|(&x, &y)| (x as i16 - y as i16).unsigned_abs() as u64)
        .sum();
    sum as f64 / a.len() as f64
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mean_abs_diff_identical() {
        let a = vec![100u8; 64];
        assert!((mean_abs_diff(&a, &a) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_mean_abs_diff_opposite() {
        let a = vec![0u8; 4];
        let b = vec![255u8; 4];
        assert!((mean_abs_diff(&a, &b) - 255.0).abs() < 1e-9);
    }

    #[test]
    fn test_mean_abs_diff_known() {
        let a = vec![10, 20, 30, 40];
        let b = vec![15, 25, 35, 45];
        // Each diff is 5, mean = 5.0
        assert!((mean_abs_diff(&a, &b) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn test_single_frame_is_active() {
        let paths: Vec<&Path> = vec![Path::new("/nonexistent/frame.jpg")];
        let result = detect_motion(&paths);
        assert!(result.has_motion);
        assert_eq!(result.total_pairs, 0);
    }

    #[test]
    fn test_empty_frames_is_active() {
        let paths: Vec<&Path> = vec![];
        let result = detect_motion(&paths);
        assert!(result.has_motion);
        assert_eq!(result.total_pairs, 0);
    }
}
