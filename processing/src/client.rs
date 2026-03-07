//! HTTP client for the Gaia Light capture server.
//!
//! Polls one or more capture servers for available MP4 clips, downloads
//! them for processing, and deletes them once analysed.  Capture server
//! URLs are discovered via mDNS, falling back to the configured URL.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use gaia_light_common::discovery::{DiscoveryHandle, ServiceRole};
use gaia_light_common::protocol::ClipInfo;

/// How long to scan mDNS when looking for capture peers.
const MDNS_BROWSE_TIMEOUT: Duration = Duration::from_secs(3);

/// Maximum number of clips to process from a single capture node
/// before moving on to the next.  This ensures fair round-robin
/// processing when multiple capture nodes are present.
pub const BATCH_PER_NODE: usize = 3;

/// How often to re-scan mDNS for new/removed capture nodes.
pub const REDISCOVERY_INTERVAL: Duration = Duration::from_secs(60);

/// Percent-encode a filename for use in a URL path segment.
/// Encodes everything except unreserved characters (RFC 3986).
fn encode_path_segment(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => { let _ = write!(out, "%{b:02X}"); }
        }
    }
    out
}

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
                .timeout(Duration::from_secs(120))
                .build()
                .expect("HTTP client"),
            fallback_url: fallback_url.trim_end_matches('/').to_string(),
        }
    }

    /// Resolve all capture server base URLs.
    ///
    /// Tries mDNS first (with a retry); falls back to the config value
    /// when mDNS is unavailable or discovers no capture nodes.
    pub fn resolve_capture_urls(
        &self,
        discovery: Option<&DiscoveryHandle>,
    ) -> Vec<String> {
        if let Some(handle) = discovery {
            for attempt in 1..=2u8 {
                let timeout = if attempt == 1 { 5 } else { 3 };
                let peers = handle.discover_peers(
                    ServiceRole::Capture,
                    Duration::from_secs(timeout),
                );
                if !peers.is_empty() {
                    let urls: Vec<String> = peers
                        .iter()
                        .filter_map(|p| p.http_url())
                        .collect();
                    info!(
                        "mDNS discovered {} capture node(s): {:?}",
                        urls.len(),
                        urls
                    );
                    return urls;
                }
                if attempt == 1 {
                    debug!("mDNS scan {attempt}: no peers yet, retrying…");
                }
            }
            info!(
                "No capture nodes found via mDNS, falling back to config URL"
            );
        }
        vec![self.fallback_url.clone()]
    }

    /// List clips available on a specific capture server.
    pub async fn list_clips(&self, base_url: &str) -> Result<Vec<ClipInfo>> {
        let url = format!("{base_url}/api/clips");
        debug!("GET {url}");

        let resp = match self.http.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
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

    /// Download a clip from a specific capture server to a local path.
    pub async fn download_clip(
        &self,
        base_url: &str,
        name: &str,
        dest: &Path,
    ) -> Result<()> {
        let url = format!("{base_url}/api/clips/{}", encode_path_segment(name));
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

    /// Tell a specific capture server to delete a clip we have finished processing.
    pub async fn delete_clip(&self, base_url: &str, name: &str) -> Result<()> {
        let url = format!("{base_url}/api/clips/{}", encode_path_segment(name));
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
