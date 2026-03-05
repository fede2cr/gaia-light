//! HTTP client for the Gaia Light capture server.
//!
//! Polls the capture server for available MP4 clips, downloads them
//! for processing, and deletes them once analysed.  The capture server
//! URL can come from configuration or be discovered via mDNS.

use std::path::Path;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use gaia_light_common::discovery::DiscoveryHandle;
use gaia_light_common::protocol::ClipInfo;

/// Client that talks to one or more capture servers.
pub struct CaptureClient {
    http: reqwest::Client,
    /// Configured fallback URL (from config file).
    fallback_url: String,
}

impl CaptureClient {
    pub fn new(fallback_url: &str, _discovery: Option<&DiscoveryHandle>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("HTTP client"),
            fallback_url: fallback_url.trim_end_matches('/').to_string(),
        }
    }

    /// Resolve the capture server base URL.
    ///
    /// Prefers mDNS-discovered capture peers; falls back to the
    /// configured `CAPTURE_SERVER_URL`.
    fn base_url(&self) -> String {
        // TODO: integrate mDNS peer discovery here.  For now we
        // always use the configured URL which works for the standard
        // single-host container deployment.
        self.fallback_url.clone()
    }

    /// List clips available on the capture server.
    pub async fn list_clips(&self) -> Result<Vec<ClipInfo>> {
        let url = format!("{}/api/clips", self.base_url());
        debug!("GET {url}");

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("Capture server unreachable")?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "Capture server returned {} for GET /api/clips",
                resp.status()
            );
        }

        let clips: Vec<ClipInfo> = resp
            .json()
            .await
            .context("Invalid JSON from capture server")?;

        debug!("Capture server has {} clip(s)", clips.len());
        Ok(clips)
    }

    /// Download a clip from the capture server to a local path.
    pub async fn download_clip(
        &self,
        name: &str,
        dest: &Path,
    ) -> Result<()> {
        let url = format!("{}/api/clips/{}", self.base_url(), name);
        info!("Downloading clip: {name}");

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("Download request failed")?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "Capture server returned {} for GET /api/clips/{name}",
                resp.status()
            );
        }

        let bytes = resp.bytes().await.context("Failed to read clip body")?;

        tokio::fs::write(dest, &bytes)
            .await
            .with_context(|| {
                format!("Cannot write clip to {}", dest.display())
            })?;

        info!(
            "Downloaded {} ({} bytes)",
            name,
            bytes.len()
        );
        Ok(())
    }

    /// Tell the capture server to delete a clip we have finished processing.
    pub async fn delete_clip(&self, name: &str) -> Result<()> {
        let url = format!("{}/api/clips/{}", self.base_url(), name);
        debug!("DELETE {url}");

        let resp = self
            .http
            .delete(&url)
            .send()
            .await
            .context("Delete request failed")?;

        if !resp.status().is_success() {
            warn!(
                "Capture server returned {} for DELETE /api/clips/{name}",
                resp.status()
            );
        }
        Ok(())
    }
}
