//! Shared runtime settings file — adjustable from the web dashboard.
//!
//! The web dashboard writes a `settings.json` file to the shared
//! data volume.  The processing container reads it at the start of
//! each poll cycle, so changes take effect within one interval.
//!
//! All fields are optional: a missing field means "use the default
//! from the environment / config file".

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::classifier_kind::ClassifierKind;

/// Runtime-adjustable settings (persisted as JSON on the shared volume).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeSettings {
    /// Minimum detector confidence (0.0 – 1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,

    /// Minimum species-classifier confidence (0.0 – 1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub species_confidence: Option<f64>,

    /// Capture → processing poll interval in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_interval_secs: Option<u64>,

    /// Maximum frames to analyse per clip (0 = all frames).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_frames_per_clip: Option<u32>,

    /// Which species classifiers to run on detection crops.
    /// `None` means "use the default from CLASSIFIERS env var / config".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifiers: Option<Vec<ClassifierKind>>,

    /// Motion-detection threshold (MAD on 0–255 scale).
    /// Higher values require more inter-frame change to count as motion.
    /// `None` means "use the default (1.5)".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion_threshold: Option<f64>,
}

/// Canonical filename inside the shared data directory.
const SETTINGS_FILE: &str = "settings.json";

/// Full path: `{data_dir}/settings.json`.
pub fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SETTINGS_FILE)
}

/// Load settings from disk.  Returns defaults if the file does not exist.
pub fn load(data_dir: &Path) -> RuntimeSettings {
    let path = settings_path(data_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => RuntimeSettings::default(),
    }
}

/// Save settings to disk (atomic write via tmp + rename).
pub fn save(data_dir: &Path, settings: &RuntimeSettings) -> anyhow::Result<()> {
    let path = settings_path(data_dir);
    let tmp = data_dir.join(".settings.json.tmp");
    let json = serde_json::to_string_pretty(settings)?;
    std::fs::write(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join("gaia_light_settings_test");
        std::fs::create_dir_all(&dir).unwrap();

        let s = RuntimeSettings {
            confidence: Some(0.7),
            species_confidence: Some(0.3),
            poll_interval_secs: Some(15),
            max_frames_per_clip: Some(20),
            classifiers: Some(vec![
                ClassifierKind::AI4GAmazonV2,
                ClassifierKind::SpeciesNet,
            ]),
            motion_threshold: Some(2.5),
        };
        save(&dir, &s).unwrap();
        let loaded = load(&dir);
        assert_eq!(loaded.confidence, Some(0.7));
        assert_eq!(loaded.species_confidence, Some(0.3));
        assert_eq!(loaded.poll_interval_secs, Some(15));
        assert_eq!(loaded.max_frames_per_clip, Some(20));
        assert_eq!(
            loaded.classifiers.as_deref(),
            Some(&[ClassifierKind::AI4GAmazonV2, ClassifierKind::SpeciesNet][..]),
        );
        assert_eq!(loaded.motion_threshold, Some(2.5));

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_returns_defaults() {
        let s = load(Path::new("/nonexistent"));
        assert!(s.confidence.is_none());
    }
}
