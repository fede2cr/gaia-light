//! Shared HTTP types for communication between capture and processing.

use serde::{Deserialize, Serialize};

/// Information about a single video clip available on the capture server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipInfo {
    pub filename: String,
    pub size: u64,
    /// ISO-8601 creation timestamp.
    pub created: String,
}

/// Health-check response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub uptime_secs: u64,
    /// Current disk usage of the recording volume (0–100).
    #[serde(default)]
    pub disk_usage_pct: f64,
    /// `true` when capture is paused because disk usage exceeds the
    /// configured threshold.
    #[serde(default)]
    pub capture_paused: bool,
}

/// SSE payload for new-clip notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewClipEvent {
    pub filename: String,
    pub size: u64,
}

/// Camera brightness / low-light status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraStatus {
    /// Detected camera model name.
    pub camera_name: String,
    /// V4L2 device path.
    pub device: String,
    /// Last measured mean luminance (0–255 scale).
    pub mean_luma: f64,
    /// Darkness threshold in use.
    pub threshold: f64,
    /// `true` when the last probe was below the threshold.
    pub is_dark: bool,
    /// `true` when low-light compensation is currently active.
    pub low_light_active: bool,
}

/// Request body for `POST /api/camera/low-light`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraControlRequest {
    /// `true` to enable low-light compensation, `false` to disable.
    pub enable: bool,
}
