//! ONNX model inference for camera-trap image analysis.
//!
//! ## Supported models
//!
//! | Slug               | Purpose                       | Architecture |
//! |--------------------|-------------------------------|-------------|
//! | `pytorch-wildlife` | MegaDetector v6 – detection   | YOLOv5      |
//! | `speciesnet`       | Google SpeciesNet – species ID | Classifier  |
//!
//! ## MegaDetector v6
//!
//! Input : `[1, 3, 640, 640]`  (NCHW, RGB, normalised 0-1)
//! Output: `[1, N, 8]`         (N anchors; 8 = cx,cy,w,h,obj,cls0,cls1,cls2)
//!
//! Classes: 0 = animal, 1 = person, 2 = vehicle
//!
//! ## SpeciesNet
//!
//! Input : `[1, 3, 224, 224]`  (NCHW, RGB, normalised 0-1)
//! Output: `[1, C]`            (C = number of species classes)

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::DynamicImage;
use tract_onnx::prelude::*;
use tracing::{debug, info};

use gaia_light_common::config::Config;

use crate::frames;

// ── Detection types ──────────────────────────────────────────────────────

/// A single detection from the object-detection model.
#[derive(Debug, Clone)]
pub struct Detection {
    /// Human-readable class label.
    pub class: String,
    /// Detection confidence (0-1).
    pub confidence: f64,
    /// Bounding box in normalised coordinates (0-1).
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    /// Name of the model that produced this detection.
    pub model_name: String,
}

/// MegaDetector class labels.
const MD_CLASSES: [&str; 3] = ["animal", "person", "vehicle"];

// ── Classification types ─────────────────────────────────────────────────

/// A species classification result.
#[derive(Debug, Clone)]
pub struct Classification {
    pub label: String,
    pub confidence: f64,
}

// ── Detector (MegaDetector) ──────────────────────────────────────────────

/// MegaDetector v6 object-detection model.
pub struct Detector {
    model: TypedRunnableModel<TypedModel>,
    input_size: u32,
}

type TypedRunnableModel<M> = RunnableModel<TypedFact, Box<dyn TypedOp>, M>;

impl Detector {
    /// Expected ONNX filename inside the model directory.
    const FILENAME: &'static str = "megadetector_v6.onnx";
    /// Input image dimension (square).
    const INPUT_SIZE: u32 = 640;
    /// IoU threshold for non-maximum suppression.
    const NMS_IOU: f64 = 0.45;

    /// Load the detector ONNX model from disk.
    pub fn load(config: &Config) -> Result<Self> {
        let model_path = find_onnx(&config.model_dir, Self::FILENAME)?;
        info!("Loading detector from {}", model_path.display());

        let model = tract_onnx::onnx()
            .model_for_path(&model_path)
            .context("Cannot parse ONNX model")?
            .with_input_fact(
                0,
                InferenceFact::dt_shape(
                    f32::datum_type(),
                    tvec![1, 3, Self::INPUT_SIZE as i64, Self::INPUT_SIZE as i64],
                ),
            )?
            .into_optimized()
            .context("Cannot optimise model")?
            .into_runnable()
            .context("Cannot make model runnable")?;

        Ok(Self {
            model,
            input_size: Self::INPUT_SIZE,
        })
    }

    /// Run detection on a single image.
    ///
    /// Returns detections with confidence >= `threshold`.
    pub fn detect(
        &self,
        img: &DynamicImage,
        threshold: f64,
    ) -> Vec<Detection> {
        // 1. Preprocess: resize to input_size, convert to CHW float tensor
        let data =
            frames::image_to_chw_f32(img, self.input_size);

        let tensor: Tensor = tract_ndarray::Array4::from_shape_vec(
            (1, 3, self.input_size as usize, self.input_size as usize),
            data,
        )
        .expect("tensor shape")
        .into();

        // 2. Run inference
        let outputs = match self.model.run(tvec!(tensor.into())) {
            Ok(o) => o,
            Err(e) => {
                tracing::error!("Detector inference failed: {e:#}");
                return vec![];
            }
        };

        // 3. Parse output tensor
        //    Expected shape: [1, N, 8]
        //    Columns: cx, cy, w, h, objectness, cls0, cls1, cls2
        let output = match outputs[0].to_array_view::<f32>() {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("Cannot read detector output tensor: {e}");
                return vec![];
            }
        };

        let shape = output.shape();
        debug!("Detector output shape: {:?}", shape);

        if shape.len() < 2 {
            tracing::error!(
                "Unexpected detector output rank: {} (expected >= 2)",
                shape.len()
            );
            return vec![];
        }

        let n_predictions = shape[shape.len() - 2];
        let n_values = shape[shape.len() - 1];

        // We expect at least 8 values per prediction (cx,cy,w,h,obj,c0,c1,c2)
        if n_values < 8 {
            tracing::error!(
                "Unexpected prediction width: {} (expected >= 8)",
                n_values
            );
            return vec![];
        }

        let flat = output.as_slice().unwrap_or(&[]);

        let mut raw_dets: Vec<Detection> = Vec::new();

        for i in 0..n_predictions {
            let base = i * n_values;
            let cx = flat[base] as f64;
            let cy = flat[base + 1] as f64;
            let w = flat[base + 2] as f64;
            let h = flat[base + 3] as f64;
            let objectness = flat[base + 4] as f64;

            // Find best class
            let mut best_cls = 0usize;
            let mut best_cls_conf = flat[base + 5] as f64;
            for c in 1..MD_CLASSES.len() {
                let conf = flat[base + 5 + c] as f64;
                if conf > best_cls_conf {
                    best_cls_conf = conf;
                    best_cls = c;
                }
            }

            let confidence = objectness * best_cls_conf;
            if confidence < threshold {
                continue;
            }

            // Convert cx,cy,w,h to normalised x1,y1,x2,y2
            let size = self.input_size as f64;
            let x1 = ((cx - w / 2.0) / size).clamp(0.0, 1.0);
            let y1 = ((cy - h / 2.0) / size).clamp(0.0, 1.0);
            let x2 = ((cx + w / 2.0) / size).clamp(0.0, 1.0);
            let y2 = ((cy + h / 2.0) / size).clamp(0.0, 1.0);

            raw_dets.push(Detection {
                class: MD_CLASSES[best_cls].to_string(),
                confidence,
                x1,
                y1,
                x2,
                y2,
                model_name: "megadetector-v6".into(),
            });
        }

        // 4. Non-maximum suppression
        let filtered = nms(&mut raw_dets, Self::NMS_IOU);

        debug!(
            "Detector: {} raw predictions -> {} after NMS (threshold={threshold})",
            n_predictions,
            filtered.len()
        );

        filtered
    }
}

// ── Classifier (SpeciesNet) ──────────────────────────────────────────────

/// SpeciesNet species classification model.
pub struct Classifier {
    model: TypedRunnableModel<TypedModel>,
    input_size: u32,
    labels: Vec<String>,
}

impl Classifier {
    const FILENAME: &'static str = "speciesnet.onnx";
    const LABELS_FILE: &'static str = "speciesnet_labels.txt";
    const INPUT_SIZE: u32 = 224;

    /// Load the classifier ONNX model and labels from disk.
    pub fn load(config: &Config) -> Result<Self> {
        let model_path = find_onnx(&config.model_dir, Self::FILENAME)?;
        let labels_path = config.model_dir.join(Self::LABELS_FILE);

        info!("Loading classifier from {}", model_path.display());

        let labels = load_labels(&labels_path)?;
        info!("Loaded {} species labels", labels.len());

        let model = tract_onnx::onnx()
            .model_for_path(&model_path)
            .context("Cannot parse classifier ONNX model")?
            .with_input_fact(
                0,
                InferenceFact::dt_shape(
                    f32::datum_type(),
                    tvec![1, 3, Self::INPUT_SIZE as i64, Self::INPUT_SIZE as i64],
                ),
            )?
            .into_optimized()
            .context("Cannot optimise classifier model")?
            .into_runnable()
            .context("Cannot make classifier model runnable")?;

        Ok(Self {
            model,
            input_size: Self::INPUT_SIZE,
            labels,
        })
    }

    /// Classify a cropped detection image.
    ///
    /// Returns `Some(Classification)` if the top prediction is above
    /// `threshold`, or `None` otherwise.
    pub fn classify(
        &self,
        crop: &DynamicImage,
        threshold: f64,
    ) -> Option<Classification> {
        let data = frames::image_to_chw_f32(crop, self.input_size);

        let tensor: Tensor = tract_ndarray::Array4::from_shape_vec(
            (1, 3, self.input_size as usize, self.input_size as usize),
            data,
        )
        .expect("tensor shape")
        .into();

        let outputs = match self.model.run(tvec!(tensor.into())) {
            Ok(o) => o,
            Err(e) => {
                tracing::error!("Classifier inference failed: {e:#}");
                return None;
            }
        };

        let logits = match outputs[0].to_array_view::<f32>() {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("Cannot read classifier output: {e}");
                return None;
            }
        };

        let flat = logits.as_slice().unwrap_or(&[]);
        if flat.is_empty() {
            return None;
        }

        // Softmax to get probabilities
        let probs = softmax(flat);

        // Find argmax
        let (best_idx, &best_conf) = probs
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();

        let confidence = best_conf as f64;
        if confidence < threshold {
            return None;
        }

        let label = self
            .labels
            .get(best_idx)
            .cloned()
            .unwrap_or_else(|| format!("class_{best_idx}"));

        debug!("Classified as {label} ({confidence:.3})");
        Some(Classification { label, confidence })
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Find an ONNX file in the model directory.
///
/// First tries the exact filename, then looks for any `.onnx` file
/// whose name contains the stem (e.g. "megadetector" matches
/// "megadetector_v6_yolov5.onnx").
fn find_onnx(model_dir: &Path, filename: &str) -> Result<PathBuf> {
    let exact = model_dir.join(filename);
    if exact.exists() {
        return Ok(exact);
    }

    // Fuzzy search: match stem substring
    let stem = filename
        .strip_suffix(".onnx")
        .unwrap_or(filename)
        .to_lowercase();

    if let Ok(entries) = std::fs::read_dir(model_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_lowercase();
            if name_str.ends_with(".onnx") && name_str.contains(&stem) {
                return Ok(entry.path());
            }
        }
    }

    anyhow::bail!(
        "Model file not found: {} (searched in {})",
        filename,
        model_dir.display()
    )
}

/// Load newline-separated labels from a text file.
fn load_labels(path: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read labels from {}", path.display()))?;

    Ok(text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect())
}

/// Softmax over a float slice.
fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|&x| (x - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.into_iter().map(|e| e / sum).collect()
}

/// Non-maximum suppression: keep only the highest-confidence detection
/// among overlapping boxes exceeding `iou_threshold`.
fn nms(dets: &mut Vec<Detection>, iou_threshold: f64) -> Vec<Detection> {
    dets.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut keep: Vec<Detection> = Vec::new();

    for det in dets.iter() {
        let dominated = keep.iter().any(|kept| iou(kept, det) > iou_threshold);
        if !dominated {
            keep.push(det.clone());
        }
    }

    keep
}

/// Compute intersection-over-union between two detections.
fn iou(a: &Detection, b: &Detection) -> f64 {
    let ix1 = a.x1.max(b.x1);
    let iy1 = a.y1.max(b.y1);
    let ix2 = a.x2.min(b.x2);
    let iy2 = a.y2.min(b.y2);

    let inter = (ix2 - ix1).max(0.0) * (iy2 - iy1).max(0.0);
    let area_a = (a.x2 - a.x1) * (a.y2 - a.y1);
    let area_b = (b.x2 - b.x1) * (b.y2 - b.y1);
    let union = area_a + area_b - inter;

    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_softmax() {
        let logits = vec![1.0, 2.0, 3.0];
        let probs = softmax(&logits);
        let sum: f32 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
        assert!(probs[2] > probs[1]);
        assert!(probs[1] > probs[0]);
    }

    #[test]
    fn test_iou_identical() {
        let d = Detection {
            class: "animal".into(),
            confidence: 0.9,
            x1: 0.1,
            y1: 0.1,
            x2: 0.5,
            y2: 0.5,
            model_name: "test".into(),
        };
        assert!((iou(&d, &d) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_iou_no_overlap() {
        let a = Detection {
            class: "animal".into(),
            confidence: 0.9,
            x1: 0.0,
            y1: 0.0,
            x2: 0.1,
            y2: 0.1,
            model_name: "test".into(),
        };
        let b = Detection {
            class: "animal".into(),
            confidence: 0.9,
            x1: 0.5,
            y1: 0.5,
            x2: 0.6,
            y2: 0.6,
            model_name: "test".into(),
        };
        assert!(iou(&a, &b) < 1e-6);
    }

    #[test]
    fn test_nms() {
        let mut dets = vec![
            Detection {
                class: "animal".into(),
                confidence: 0.9,
                x1: 0.1,
                y1: 0.1,
                x2: 0.5,
                y2: 0.5,
                model_name: "test".into(),
            },
            Detection {
                class: "animal".into(),
                confidence: 0.7,
                x1: 0.12,
                y1: 0.12,
                x2: 0.52,
                y2: 0.52,
                model_name: "test".into(),
            },
            Detection {
                class: "person".into(),
                confidence: 0.8,
                x1: 0.7,
                y1: 0.7,
                x2: 0.9,
                y2: 0.9,
                model_name: "test".into(),
            },
        ];
        let kept = nms(&mut dets, 0.45);
        // The two overlapping animal boxes should merge to 1; person stays
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].class, "animal");
        assert_eq!(kept[1].class, "person");
    }
}
