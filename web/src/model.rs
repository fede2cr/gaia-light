//! Shared data-transfer objects used by both server and client.

use serde::{Deserialize, Serialize};

// ── Detection ────────────────────────────────────────────────────────────────

/// A single camera-trap detection, serialisable for transfer via server functions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebDetection {
    pub id: i64,
    /// Detected class: "animal", "person", or "vehicle".
    pub class: String,
    /// Detection confidence (0-1).
    pub confidence: f64,
    /// Bounding box in normalised coordinates (0-1).
    pub bbox_x1: f64,
    pub bbox_y1: f64,
    pub bbox_x2: f64,
    pub bbox_y2: f64,
    /// Species label from the classifier (if available).
    #[serde(default)]
    pub species: Option<String>,
    /// Species classifier confidence.
    #[serde(default)]
    pub species_confidence: Option<f64>,
    /// Which classifier model produced the species label.
    #[serde(default)]
    pub species_model: Option<String>,
    /// Detector model that produced this detection.
    pub detector_model: String,
    /// Source clip filename.
    pub clip_filename: String,
    /// Frame index within the clip.
    pub frame_index: i64,
    /// Path to the saved crop image.
    #[serde(default)]
    pub crop_path: Option<String>,
    /// ISO-8601 timestamp from the clip.
    pub timestamp: String,
    /// When the detection was created in the DB.
    pub created_at: String,
    /// Station coordinates.
    pub latitude: f64,
    pub longitude: f64,
    /// Processing instance that produced this detection.
    #[serde(default)]
    pub processing_instance: String,
    /// URL of the capture node that recorded the source clip.
    #[serde(default)]
    pub source_node: String,
}

impl WebDetection {
    /// URL to serve the detection crop image.
    pub fn crop_url(&self) -> Option<String> {
        self.crop_path.as_ref().map(|p| {
            let filename = p
                .rsplit('/')
                .next()
                .unwrap_or(p);
            format!("/extracted/{filename}")
        })
    }

    /// Confidence formatted as a percentage string.
    pub fn confidence_pct(&self) -> String {
        format!("{:.0}%", self.confidence * 100.0)
    }

    /// CSS class for confidence badge colouring.
    pub fn confidence_class(&self) -> &'static str {
        if self.confidence >= 0.8 {
            "confidence high"
        } else if self.confidence >= 0.5 {
            "confidence medium"
        } else {
            "confidence low"
        }
    }

    /// Display label combining class + species.
    pub fn label(&self) -> String {
        match &self.species {
            Some(sp) if !sp.is_empty() => sp.clone(),
            _ => self.class.clone(),
        }
    }
}

// ── Class summary (for dashboard stats) ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassSummary {
    pub class: String,
    pub count: u32,
}

// ── Species summary ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeciesSummary {
    pub species: String,
    pub count: u32,
    pub last_seen: Option<String>,
}

// ── Daily summary ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyCount {
    pub date: String,
    pub animals: u32,
    pub persons: u32,
    pub vehicles: u32,
    pub total: u32,
}

// ── Live status (read from JSON file written by processing) ──────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveStatus {
    pub last_clip: String,
    pub frame_count: usize,
    pub detections_last_hour: usize,
    pub updated_at: String,
    /// The capture node URL this clip was fetched from.
    #[serde(default)]
    pub source_node: Option<String>,
    /// ISO-8601 capture timestamp from the clip metadata.
    #[serde(default)]
    pub captured_at: Option<String>,
}

// ── System info ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub total_detections: u64,
    pub total_animals: u64,
    pub total_species: u32,
    pub clips_processed: u64,
    pub db_size_bytes: u64,
}

// ── Preview info (for cache-busting) ─────────────────────────────────────────

/// Metadata about the latest processing preview image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewInfo {
    /// Whether the preview file exists.
    pub available: bool,
    /// Unix epoch millis of the file's last modification (used as cache-buster).
    pub modified_ms: u64,
}

// ── Training candidate ───────────────────────────────────────────────────────

/// A high-confidence animal detection with no species classification,
/// saved for future model training.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingCandidate {
    pub id: i64,
    pub timestamp: String,
    pub clip_filename: String,
    pub frame_index: i64,
    pub confidence: f64,
    pub bbox_x1: f64,
    pub bbox_y1: f64,
    pub bbox_x2: f64,
    pub bbox_y2: f64,
    #[serde(default)]
    pub crop_path: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
    pub created_at: String,
}

impl TrainingCandidate {
    /// URL to serve the crop image.
    pub fn crop_url(&self) -> Option<String> {
        self.crop_path.as_ref().map(|p| {
            let filename = p.rsplit('/').next().unwrap_or(p);
            format!("/extracted/{filename}")
        })
    }

    /// Confidence formatted as a percentage string.
    pub fn confidence_pct(&self) -> String {
        format!("{:.0}%", self.confidence * 100.0)
    }
}
