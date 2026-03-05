//! Supported species-classification models.
//!
//! Each variant knows its own ONNX filename, labels filename,
//! input size, and human-readable display name, so the rest of
//! the codebase can work generically with any classifier.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A species-classification model that can be loaded and run on
/// detection crops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClassifierKind {
    /// Google SpeciesNet v4.0.1a (EfficientNet V2 M, 480×480, ~2500 classes).
    #[serde(rename = "speciesnet")]
    SpeciesNet,

    /// Microsoft AI for Good Lab — Amazon Rainforest V2 (pytorch-wildlife).
    #[serde(rename = "ai4g-amazon-v2")]
    AI4GAmazonV2,
}

impl ClassifierKind {
    /// All known classifier variants, in recommended display order.
    pub const ALL: &'static [ClassifierKind] = &[
        ClassifierKind::AI4GAmazonV2,
        ClassifierKind::SpeciesNet,
    ];

    /// Short slug used in config files, CLI args, and JSON.
    pub fn slug(self) -> &'static str {
        match self {
            Self::SpeciesNet => "speciesnet",
            Self::AI4GAmazonV2 => "ai4g-amazon-v2",
        }
    }

    /// Human-readable name for UI display.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::SpeciesNet => "SpeciesNet v4.0.1a",
            Self::AI4GAmazonV2 => "AI4G Amazon Rainforest V2",
        }
    }

    /// ONNX model filename expected inside the models directory.
    pub fn onnx_filename(self) -> &'static str {
        match self {
            Self::SpeciesNet => "speciesnet.onnx",
            Self::AI4GAmazonV2 => "ai4g_amazon_v2.onnx",
        }
    }

    /// Labels text file (newline-separated) for this classifier.
    pub fn labels_filename(self) -> &'static str {
        match self {
            Self::SpeciesNet => "speciesnet_labels.txt",
            Self::AI4GAmazonV2 => "ai4g_amazon_v2_labels.txt",
        }
    }

    /// Square input dimension the model expects (width = height).
    pub fn input_size(self) -> u32 {
        match self {
            Self::SpeciesNet => 480,
            Self::AI4GAmazonV2 => 224,
        }
    }

    /// Parse a slug string (case-insensitive) into a `ClassifierKind`.
    pub fn from_slug(s: &str) -> Option<Self> {
        match s.to_lowercase().trim() {
            "speciesnet" => Some(Self::SpeciesNet),
            "ai4g-amazon-v2" | "ai4gamazonv2" | "ai4g_amazon_v2" => {
                Some(Self::AI4GAmazonV2)
            }
            _ => None,
        }
    }
}

impl fmt::Display for ClassifierKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_slug() {
        for kind in ClassifierKind::ALL {
            let slug = kind.slug();
            let parsed = ClassifierKind::from_slug(slug)
                .unwrap_or_else(|| panic!("Cannot parse slug: {slug}"));
            assert_eq!(*kind, parsed);
        }
    }

    #[test]
    fn serde_round_trip() {
        for kind in ClassifierKind::ALL {
            let json = serde_json::to_string(kind).unwrap();
            let parsed: ClassifierKind = serde_json::from_str(&json).unwrap();
            assert_eq!(*kind, parsed);
        }
    }

    #[test]
    fn fuzzy_slugs() {
        assert_eq!(
            ClassifierKind::from_slug("AI4G-Amazon-V2"),
            Some(ClassifierKind::AI4GAmazonV2)
        );
        assert_eq!(
            ClassifierKind::from_slug("ai4g_amazon_v2"),
            Some(ClassifierKind::AI4GAmazonV2)
        );
    }
}
