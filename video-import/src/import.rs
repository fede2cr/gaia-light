//! Filesystem scanner – finds security-camera recordings in date
//! sub-directories and creates compatibly-named symlinks in the
//! `StreamData` directory.
//!
//! Filename transformation (example):
//!
//! ```text
//! Source:  2025-08-11/RecM05_20250811_070122_070131_0_7D1E82100_2B3591.mp4
//! Link:   StreamData/2025-08-11-camera-front-yard-070122_070131.mp4
//! ```
//!
//! The scanner is idempotent: existing symlinks (both pending in
//! `StreamData` and consumed in `processed/`) are skipped.

use std::os::unix::fs::symlink;
use std::path::Path;

use anyhow::{Context, Result};
use tracing::{debug, warn};

/// Scan `import_dir` for `.mp4` files (in date sub-directories) and
/// create symlinks in `stream_dir` with standardised names.
///
/// Files whose symlinks already exist in either `stream_dir` (pending)
/// or `processed_dir` (already consumed) are skipped.
///
/// Returns the number of **new** symlinks created.
pub fn scan_and_link(
    import_dir: &Path,
    camera_name: &str,
    stream_dir: &Path,
    processed_dir: &Path,
) -> Result<usize> {
    let mut count = 0;

    let entries = std::fs::read_dir(import_dir)
        .with_context(|| format!("Cannot read import dir: {}", import_dir.display()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Recurse one level into date sub-directories (e.g. 2025-08-11/)
            count += scan_subdir(&path, camera_name, stream_dir, processed_dir)?;
        } else if is_mp4(&path) {
            // Top-level .mp4 files (no date sub-dir)
            count += maybe_link(&path, camera_name, stream_dir, processed_dir)?;
        }
    }

    Ok(count)
}

// ── Internal helpers ─────────────────────────────────────────────────────

/// Scan a single sub-directory for `.mp4` files.
fn scan_subdir(
    dir: &Path,
    camera_name: &str,
    stream_dir: &Path,
    processed_dir: &Path,
) -> Result<usize> {
    let mut count = 0;

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!("Cannot read sub-dir {}: {e}", dir.display());
            return Ok(0);
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if is_mp4(&path) {
            count += maybe_link(&path, camera_name, stream_dir, processed_dir)?;
        }
    }

    Ok(count)
}

/// Create a symlink for `source` in `stream_dir` if one doesn't
/// already exist (in either stream or processed).  Returns 1 on
/// success, 0 if skipped.
fn maybe_link(
    source: &Path,
    camera_name: &str,
    stream_dir: &Path,
    processed_dir: &Path,
) -> Result<usize> {
    let original_name = match source.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return Ok(0),
    };

    let renamed = rename_file(original_name, camera_name);

    // Already pending or already consumed → skip
    if stream_dir.join(&renamed).exists() || processed_dir.join(&renamed).exists() {
        return Ok(0);
    }

    let abs_source = std::fs::canonicalize(source)
        .with_context(|| format!("Cannot resolve: {}", source.display()))?;

    let link_path = stream_dir.join(&renamed);
    symlink(&abs_source, &link_path).with_context(|| {
        format!(
            "Cannot create symlink {} → {}",
            link_path.display(),
            abs_source.display(),
        )
    })?;

    debug!("{} → {}", renamed, abs_source.display());
    Ok(1)
}

// ── Filename helpers ─────────────────────────────────────────────────────

/// Rename a security-camera recording to the standard capture format.
///
/// ```text
/// Input:  RecM05_20250811_070122_070131_0_7D1E82100_2B3591.mp4
/// Output: 2025-08-11-camera-front-yard-070122_070131.mp4
/// ```
///
/// Falls back to the original filename if the pattern can't be parsed.
pub fn rename_file(original: &str, camera_name: &str) -> String {
    if let Some((date, start, end)) = parse_nvr_filename(original) {
        format!("{date}-camera-{camera_name}-{start}_{end}.mp4")
    } else {
        original.to_string()
    }
}

/// Try to extract `(YYYY-MM-DD, HHMMSS_start, HHMMSS_end)` from a
/// security-camera filename.
///
/// Looks for three consecutive underscore-separated tokens where the
/// first is 8 digits (YYYYMMDD) and the following two are 6 digits
/// each (HHMMSS).
fn parse_nvr_filename(filename: &str) -> Option<(String, String, String)> {
    let stem = filename.strip_suffix(".mp4")?;
    let parts: Vec<&str> = stem.split('_').collect();

    for i in 0..parts.len().saturating_sub(2) {
        let date_raw = parts[i];
        let time_start = parts[i + 1];
        let time_end = parts[i + 2];

        if date_raw.len() == 8
            && date_raw.bytes().all(|b| b.is_ascii_digit())
            && time_start.len() == 6
            && time_start.bytes().all(|b| b.is_ascii_digit())
            && time_end.len() == 6
            && time_end.bytes().all(|b| b.is_ascii_digit())
        {
            let date = format!(
                "{}-{}-{}",
                &date_raw[..4],
                &date_raw[4..6],
                &date_raw[6..8],
            );
            return Some((date, time_start.to_string(), time_end.to_string()));
        }
    }

    None
}

fn is_mp4(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("mp4")
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nvr_filename() {
        let (date, start, end) = parse_nvr_filename(
            "RecM05_20250811_070122_070131_0_7D1E82100_2B3591.mp4",
        )
        .expect("should parse");
        assert_eq!(date, "2025-08-11");
        assert_eq!(start, "070122");
        assert_eq!(end, "070131");
    }

    #[test]
    fn test_parse_nvr_different_prefix() {
        let (date, start, end) =
            parse_nvr_filename("CamA_20241225_235959_000005_1_ABCDEF.mp4")
                .expect("should parse different prefix");
        assert_eq!(date, "2024-12-25");
        assert_eq!(start, "235959");
        assert_eq!(end, "000005");
    }

    #[test]
    fn test_parse_nvr_no_match() {
        assert!(parse_nvr_filename("random_file.mp4").is_none());
        assert!(parse_nvr_filename("not_a_video.txt").is_none());
        assert!(parse_nvr_filename("short_12_34.mp4").is_none());
    }

    #[test]
    fn test_rename_file() {
        assert_eq!(
            rename_file(
                "RecM05_20250811_070122_070131_0_7D1E82100_2B3591.mp4",
                "front-yard",
            ),
            "2025-08-11-camera-front-yard-070122_070131.mp4",
        );
    }

    #[test]
    fn test_rename_fallback() {
        assert_eq!(
            rename_file("unknown_format.mp4", "cam"),
            "unknown_format.mp4",
        );
    }

    #[test]
    fn test_scan_and_link() {
        let tmp = std::env::temp_dir().join("gaia-test-video-import");
        let _ = std::fs::remove_dir_all(&tmp);

        let import = tmp.join("import/2025-08-11");
        let stream = tmp.join("StreamData");
        let processed = tmp.join("processed");
        std::fs::create_dir_all(&import).unwrap();
        std::fs::create_dir_all(&stream).unwrap();
        std::fs::create_dir_all(&processed).unwrap();

        // Create a fake source file
        let src = import.join("RecM05_20250811_070122_070131_0_7D1E82100_2B3591.mp4");
        std::fs::write(&src, b"fake mp4").unwrap();

        let count = scan_and_link(
            &tmp.join("import"),
            "test-cam",
            &stream,
            &processed,
        )
        .unwrap();
        assert_eq!(count, 1);

        let link = stream.join("2025-08-11-camera-test-cam-070122_070131.mp4");
        assert!(link.exists(), "symlink should exist");
        assert!(link.is_symlink(), "should be a symlink");

        // Re-scan should find nothing new
        let count2 = scan_and_link(
            &tmp.join("import"),
            "test-cam",
            &stream,
            &processed,
        )
        .unwrap();
        assert_eq!(count2, 0);

        // Simulate processing: move symlink to processed/
        std::fs::rename(
            &link,
            processed.join(link.file_name().unwrap()),
        )
        .unwrap();

        // Re-scan should still skip (file is in processed/)
        let count3 = scan_and_link(
            &tmp.join("import"),
            "test-cam",
            &stream,
            &processed,
        )
        .unwrap();
        assert_eq!(count3, 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_scan_top_level_mp4() {
        let tmp = std::env::temp_dir().join("gaia-test-video-import-toplevel");
        let _ = std::fs::remove_dir_all(&tmp);

        let import = tmp.join("import");
        let stream = tmp.join("StreamData");
        let processed = tmp.join("processed");
        std::fs::create_dir_all(&import).unwrap();
        std::fs::create_dir_all(&stream).unwrap();
        std::fs::create_dir_all(&processed).unwrap();

        // File directly in import dir (no date sub-dir)
        let src = import.join("RecM05_20250811_070122_070131_0_7D1E82100_2B3591.mp4");
        std::fs::write(&src, b"fake mp4").unwrap();

        let count = scan_and_link(&import, "cam", &stream, &processed).unwrap();
        assert_eq!(count, 1);
        assert!(stream.join("2025-08-11-camera-cam-070122_070131.mp4").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
