//! Auto-download ONNX model files on first start.
//!
//! When the processing server starts and a required ONNX file is missing
//! from the `model_dir`, this module downloads it from a known URL.
//!
//! ## Supported models
//!
//! | File                      | Source                          | Size   |
//! |---------------------------|---------------------------------|--------|
//! | `megadetector_v6.onnx`    | Baked into container image      | ~280 MB|
//! | `speciesnet.onnx`         | Baked into container image      | ~220 MB|
//! | `speciesnet_labels.txt`   | Baked / Addax HuggingFace       | ~256 KB|
//!
//! If the baked-in files exist (see [`BAKED_MODELS_DIR`]), they are simply
//! copied into the models volume.  Otherwise, the files are downloaded
//! from HuggingFace or GitHub.
//!
//! Note: `speciesnet.onnx` is converted from PyTorch at build time and
//! has no public ONNX download URL — it can only come from the baked-in
//! container layer.  The labels file can be downloaded from Addax.
//!
//! This mirrors the pattern used by gaia-audio-processing.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

/// Directory where pre-converted ONNX models are baked into the container
/// image at build time (see `processing/Containerfile`, converter stage).
const BAKED_MODELS_DIR: &str = "/usr/local/share/gaia/models";

/// Known model files and their fallback download URLs.
///
/// Format: `(filename, download_url)`
///
/// The download URLs are only used when the baked-in copy is not available
/// (e.g. running outside the container, or an older container image).
const MODEL_FILES: &[(&str, &str)] = &[
    (
        "megadetector_v6.onnx",
        "https://huggingface.co/ai-for-good-lab/megadetector-onnx/resolve/main/megadetector_v6.onnx",
    ),
    // speciesnet.onnx is converted from PyTorch at build time — no public
    // ONNX URL exists.  It can only come from the baked-in container layer.
    // If missing, the system works without species classification.
    (
        "speciesnet.onnx",
        "",
    ),
    (
        "speciesnet_labels.txt",
        "https://huggingface.co/Addax-Data-Science/SPECIESNET-v4-0-1-A-v1/resolve/main/always_crop_99710272_22x8_v12_epoch_00148.labels.txt",
    ),
];

/// Ensure all required model files are present in `model_dir`.
///
/// For each missing file, tries (in order):
/// 1. Copy from the baked-in container directory
/// 2. Download from the fallback URL
///
/// Returns `Ok(())` if all files are present after the function returns,
/// or a warning-level error for each file that could not be obtained.
/// This is non-fatal: the processing loop will retry model loading each
/// cycle.
pub fn ensure_models(model_dir: &Path, required_files: &[&str]) -> Vec<String> {
    let mut errors = Vec::new();

    for &(filename, url) in MODEL_FILES {
        // Only process files that are actually required
        if !required_files.contains(&filename) {
            continue;
        }

        let dest = model_dir.join(filename);
        if dest.exists() {
            debug!("Model file already present: {}", dest.display());
            continue;
        }

        // Try baked-in copy first
        let baked = PathBuf::from(BAKED_MODELS_DIR).join(filename);
        if baked.exists() {
            info!(
                "Seeding model from baked-in image: {} -> {}",
                baked.display(),
                dest.display()
            );
            match std::fs::copy(&baked, &dest) {
                Ok(bytes) => {
                    info!("Copied {} ({} bytes)", filename, bytes);
                    continue;
                }
                Err(e) => {
                    warn!("Cannot copy baked-in {}: {e}", filename);
                    // Fall through to download
                }
            }
        }

        // Try downloading
        if url.is_empty() {
            let msg = format!(
                "{filename} not present and no download URL — \
                 must be baked into the container image"
            );
            warn!("{msg}");
            errors.push(msg);
            continue;
        }
        info!("Downloading {} from {}", filename, url);
        match download_file(url, &dest) {
            Ok(()) => info!("Downloaded {} successfully", filename),
            Err(e) => {
                let msg = format!("Cannot obtain {filename}: {e:#}");
                warn!("{msg}");
                errors.push(msg);
            }
        }
    }

    errors
}

/// Download a file from `url` to `dest`, using a `.part` temp file.
fn download_file(url: &str, dest: &Path) -> Result<()> {
    let part = dest.with_extension("onnx.part");

    // Use blocking reqwest (we're called from sync context before tokio runtime
    // is fully used for the processing loop)
    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .context("Cannot create HTTP client")?
        .get(url)
        .send()
        .with_context(|| format!("GET {url}"))?;

    if !response.status().is_success() {
        anyhow::bail!(
            "HTTP {} from {}",
            response.status(),
            url
        );
    }

    let bytes = response
        .bytes()
        .context("Failed to read response body")?;

    info!(
        "Downloaded {} bytes for {}",
        bytes.len(),
        dest.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    );

    // Write atomically via temp file
    std::fs::write(&part, &bytes)
        .with_context(|| format!("Cannot write {}", part.display()))?;

    std::fs::rename(&part, dest)
        .with_context(|| format!("Cannot rename {} -> {}", part.display(), dest.display()))?;

    Ok(())
}
