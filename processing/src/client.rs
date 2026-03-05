//! HTTP client for the Gaia Light capture server.
//!
//! Polls the capture server for available MP4 clips, downloads them
//! for processing, and deletes them once analysed.  The capture server
//! URL is discovered via mDNS, falling back to the configured URL.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use gaia_light_common::discovery::{DiscoveryHandle, ServiceRole};
use gaia_light_common::protocol::ClipInfo;

/// How long to scan mDNS when looking for a capture peer.
const MDNS_BROWSE_TIMEOUT: Duration = Duration::from_secs(3);

/// Client that talks to one or more capture servers.
pub struct CaptureClient {
    http: reqwest::Client,
    /// Configured fallback URL (from config file).
    fallback_url: String,
    /// Cached mDNS-discovered URL (refreshed on failure).
    discovered_url: Mutex<Option<String>>,
}

impl CaptureClient {
    pub fn new(fallback_url: &str, _discovery: Option<&DiscoveryHandle>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("HTTP client"),
            fallback_url: fallback_url.trim_end_matches('/').to_string(),
            discovered_url: Mutex::new(None),
        }
    }

    /// Resolve the capture server base URL.
    ///
    /// Tries (in order):
    /// 1. Cached mDNS-discovered URL
    /// 2. Fresh mDNS browse for `_gaia-lt-cap._tcp`
    /// 3. Configured `CAPTURE_SERVER_URL` (fallback)
    fn base_url(&self, discovery: Option<&DiscoveryHandle>) -> String {
        // 1. Return cached mDNS URL if available
        if let Some(url) = self.discovered_url.lock().unwrap().as_ref() {
            return url.clone();
        }

        // 2. Try mDNS discovery
        if let Some(handle) = discovery {
            let peers = handle.discover_peers(ServiceRole::Capture, MDNS_BROWSE_TIMEOUT);
            if let Some(peer) = peers.first() {
                if let Some(url) = peer.http_url() {
                    info!(
                        "Discovered capture server via mDNS: {} at {}",
                        peer.instance_name, url
                    );
                    *self.discovered_url.lock().unwrap() = Some(url.clone());
                    return url;
                }
            }
            debug!("No capture peers found via mDNS, using fallback URL");
        }

        // 3. Fallback
        self.fallback_url.clone()
    }

    /// Clear the cached mDNS URL (called on connection failure so we
    /// re-discover next time).
    fn invalidate_discovered_url(&self) {
        *self.discovered_url.lock().unwrap() = None;
    }

    /// List clips available on the capture server.
    pub async fn list_clips(&self, discovery: Option<&DiscoveryHandle>) -> Result<Vec<ClipInfo>> {
        let url = format!("{}/api/clips", self.base_url(discovery));
        debug!("GET {url}");

        let resp = match self.http.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                self.invalidate_discovered_url();
                return Err(e).context("Capture server unreachable");
            }
        };

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
        discovery: Option<&DiscoveryHandle>,
    ) -> Result<()> {
        let url = format!("{}/api/clips/{}", self.base_url(discovery), name);
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
    pub async fn delete_clip(&self, name: &str, discovery: Option<&DiscoveryHandle>) -> Result<()> {
        let url = format!("{}/api/clips/{}", self.base_url(discovery), name);
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
