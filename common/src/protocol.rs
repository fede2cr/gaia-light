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
}

/// SSE payload for new-clip notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewClipEvent {
    pub filename: String,
    pub size: u64,
}
