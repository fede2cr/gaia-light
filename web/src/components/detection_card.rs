//! Card component for a single camera-trap detection.

use leptos::*;

use crate::model::WebDetection;

/// Renders a detection card with crop image, class/species info, and metadata.
#[component]
pub fn DetectionCard(detection: WebDetection) -> impl IntoView {
    let confidence_pct = detection.confidence_pct();
    let confidence_class = detection.confidence_class();
    let label = detection.label();
    let crop_url = detection.crop_url();
    let class_badge = detection.class.clone();
    let source_label = detection.source_label();

    // Individual identity for person detections
    let individual_label = if detection.class == "person" {
        detection.individual_name.as_ref()
            .filter(|n| !n.is_empty())
            .cloned()
            .or_else(|| detection.individual_id.map(|id| format!("Person #{id}")))
    } else {
        None
    };

    // Pick the best available timestamp
    let timestamp = if !detection.timestamp.is_empty() {
        // Clip capture time: "2026-03-05T19:08:27" → "2026-03-05 19:08"
        let ts = detection.timestamp.replace('T', " ");
        if ts.len() >= 16 { ts[..16].to_string() } else { ts }
    } else if !detection.created_at.is_empty() {
        let ts = &detection.created_at;
        if ts.len() >= 16 { ts[..16].to_string() } else { ts.clone() }
    } else {
        String::new()
    };

    // Species sub-label with confidence and classifier model
    let species_line = detection.species.as_ref().and_then(|sp| {
        if sp.is_empty() { return None; }
        let conf = detection
            .species_confidence
            .map(|c| format!(" ({:.0}%)", c * 100.0))
            .unwrap_or_default();
        Some(format!("{sp}{conf}"))
    });
    let species_model = detection.species_model.clone();

    // Short clip name: drop the date prefix for compactness
    let clip_short = {
        let name = &detection.clip_filename;
        // "2026-03-05-camera-v4l2-185521.mp4" → "camera-v4l2-185521"
        let stem = name.strip_suffix(".mp4").unwrap_or(name);
        // Skip the date prefix "YYYY-MM-DD-"
        if stem.len() > 11 && stem.as_bytes()[4] == b'-' && stem.as_bytes()[7] == b'-' {
            stem[11..].to_string()
        } else {
            stem.to_string()
        }
    };

    view! {
        <div class="detection-card">
            // Crop image
            <div class="detection-thumb">
                {match crop_url {
                    Some(url) => view! {
                        <img
                            class="crop-thumb"
                            src={url}
                            alt={label.clone()}
                            loading="lazy"
                        />
                    }.into_view(),
                    None => view! {
                        <div class="crop-placeholder">
                            <svg viewBox="0 0 24 24" width="32" height="32"
                                 fill="none" stroke="currentColor" stroke-width="1.5">
                                <rect x="3" y="3" width="18" height="18" rx="2"/>
                                <circle cx="8.5" cy="8.5" r="1.5"/>
                                <polyline points="21 15 16 10 5 21"/>
                            </svg>
                        </div>
                    }.into_view(),
                }}
            </div>

            // Detection details
            <div class="detection-info">
                <div class="detection-label">
                    <span class={format!("class-badge class-{class_badge}")}>{&class_badge}</span>
                    <span class={confidence_class}>{confidence_pct}</span>
                </div>

                {species_line.map(|sp| view! {
                    <div class="species-row">
                        <span class="species-name">{sp}</span>
                        {species_model.map(|m| view! {
                            <span class="species-model" title="Classifier model">{m}</span>
                        })}
                    </div>
                })}

                {individual_label.map(|name| view! {
                    <div class="individual-row">
                        <span class="individual-badge">"👤"</span>
                        <span class="individual-name">{name}</span>
                    </div>
                })}

                <div class="detection-meta">
                    <time>{timestamp}</time>
                    <span class="source-badge" title="Capture node">{source_label}</span>
                    <span class="clip-ref">{clip_short} ":" {detection.frame_index.to_string()}</span>
                </div>
            </div>
        </div>
    }
}
