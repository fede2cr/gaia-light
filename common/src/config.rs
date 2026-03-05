//! Configuration parsing – reads a KEY=VALUE file.
//!
//! Follows the same format as gaia-audio's `gaia.conf`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;

use crate::classifier_kind::ClassifierKind;

/// Application configuration, shared between capture, processing, and web.
#[derive(Debug, Clone)]
pub struct Config {
    // ── location ─────────────────────────────────────────────────────
    pub latitude: f64,
    pub longitude: f64,

    // ── detection thresholds (processing) ────────────────────────────
    pub confidence: f64,
    /// Minimum species-classifier confidence to accept a label.
    pub species_confidence: f64,
    /// Maximum frames to analyse per clip (0 = all frames).
    pub max_frames_per_clip: u32,

    // ── recording (capture) ──────────────────────────────────────────
    /// Length of each video segment in seconds.
    pub segment_length: u32,
    /// Frames per second to extract for inference (0 = use native fps).
    pub capture_fps: u32,
    /// Resolution width. 0 = use native.
    pub capture_width: u32,
    /// Resolution height. 0 = use native.
    pub capture_height: u32,
    /// Local V4L2 device path (e.g. `/dev/video0`).  Preferred over
    /// RTSP when set.
    pub video_device: Option<String>,
    /// RTSP camera URLs (fallback when no local V4L2 device).
    pub rtsp_streams: Vec<String>,
    /// Base data directory.
    pub recs_dir: PathBuf,
    /// Directory for extracted detection crops.
    pub extracted_dir: PathBuf,

    // ── model (processing) ───────────────────────────────────────────
    pub model_dir: PathBuf,
    pub model_slugs: Vec<String>,
    /// Which species classifiers to run on detection crops.
    /// Parsed from the `CLASSIFIERS` key (comma-separated slugs).
    /// Default: `[AI4GAmazonV2]`.
    pub classifiers: Vec<ClassifierKind>,
    pub processing_instance: String,

    // ── database (processing / web) ──────────────────────────────────
    pub db_path: PathBuf,

    // ── network (capture ↔ processing) ───────────────────────────────
    pub capture_listen_addr: String,
    pub capture_server_url: String,
    pub poll_interval_secs: u64,
}

impl Config {
    pub fn default_path() -> &'static str {
        "/etc/gaia/gaia-light.conf"
    }

    /// Convenience: the StreamData subdirectory under `recs_dir`.
    pub fn stream_data_dir(&self) -> PathBuf {
        self.recs_dir.join("StreamData")
    }
}

/// Parse a `KEY=VALUE` configuration file.
pub fn load(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read config: {}", path.display()))?;

    let map = parse_conf(&text);
    info!("Loaded config from {}", path.display());

    let get = |key: &str| -> Option<String> {
        std::env::var(key)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| map.get(key).cloned())
    };
    let get_f64 = |key: &str, default: f64| -> f64 {
        get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    };
    let get_u32 = |key: &str, default: u32| -> u32 {
        get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    };

    let recs_dir = PathBuf::from(get("RECS_DIR").unwrap_or_else(|| "/data".into()));
    let extracted_dir = get("EXTRACTED")
        .map(PathBuf::from)
        .unwrap_or_else(|| recs_dir.join("Extracted"));

    let rtsp_streams: Vec<String> = get("RTSP_STREAMS")
        .map(|s| {
            s.split(',')
                .map(|u| u.trim().to_string())
                .filter(|u| !u.is_empty())
                .collect()
        })
        .unwrap_or_default();

    Ok(Config {
        latitude: get_f64("LATITUDE", -1.0),
        longitude: get_f64("LONGITUDE", -1.0),
        confidence: get_f64("CONFIDENCE", 0.5),
        species_confidence: get_f64("SPECIES_CONFIDENCE", 0.1),
        max_frames_per_clip: get_u32("MAX_FRAMES_PER_CLIP", 0),

        segment_length: get_u32("SEGMENT_LENGTH", 60),
        capture_fps: get_u32("CAPTURE_FPS", 1),
        capture_width: get_u32("CAPTURE_WIDTH", 0),
        capture_height: get_u32("CAPTURE_HEIGHT", 0),
        video_device: get("VIDEO_DEVICE"),
        rtsp_streams,
        recs_dir,
        extracted_dir,

        model_dir: PathBuf::from(get("MODEL_DIR").unwrap_or_else(|| "/models".into())),
        model_slugs: get("MODEL_SLUGS")
            .map(|s| {
                s.split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        classifiers: parse_classifiers(
            &get("CLASSIFIERS").unwrap_or_else(|| "ai4g-amazon-v2".into()),
        ),
        processing_instance: get("PROCESSING_INSTANCE").unwrap_or_default(),

        db_path: PathBuf::from(
            get("DB_PATH").unwrap_or_else(|| "/data/detections.db".into()),
        ),

        capture_listen_addr: get("CAPTURE_LISTEN_ADDR")
            .unwrap_or_else(|| "0.0.0.0:8090".into()),
        capture_server_url: get("CAPTURE_SERVER_URL")
            .unwrap_or_else(|| "http://localhost:8090".into()),
        poll_interval_secs: get("POLL_INTERVAL_SECS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(10),
    })
}

/// Parse a comma-separated list of classifier slugs into `ClassifierKind`s.
///
/// Unknown slugs are logged and skipped.  If nothing valid remains, falls
/// back to the default (`AI4GAmazonV2`).
fn parse_classifiers(raw: &str) -> Vec<ClassifierKind> {
    let mut kinds: Vec<ClassifierKind> = raw
        .split(',')
        .filter_map(|s| {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            match ClassifierKind::from_slug(s) {
                Some(k) => Some(k),
                None => {
                    tracing::warn!("Unknown classifier slug: {s:?} (ignored)");
                    None
                }
            }
        })
        .collect();

    // Deduplicate while preserving order
    let mut seen = std::collections::HashSet::new();
    kinds.retain(|k| seen.insert(*k));

    if kinds.is_empty() {
        kinds.push(ClassifierKind::AI4GAmazonV2);
    }
    kinds
}

fn parse_conf(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, val)) = line.split_once('=') {
            let key = key.trim();
            let val = val.trim().trim_matches('"');
            map.insert(key.to_string(), val.to_string());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_conf() {
        let text = r#"
# comment
LATITUDE=42.36
LONGITUDE="-72.52"
RTSP_STREAMS="rtsp://cam1,rtsp://cam2"
CAPTURE_LISTEN_ADDR=0.0.0.0:9090
"#;
        let map = parse_conf(text);
        assert_eq!(map["LATITUDE"], "42.36");
        assert_eq!(map["LONGITUDE"], "-72.52");
        assert_eq!(map["CAPTURE_LISTEN_ADDR"], "0.0.0.0:9090");
    }
}
