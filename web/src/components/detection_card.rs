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
    let timestamp = if detection.created_at.is_empty() {
        detection.timestamp.clone()
    } else {
        detection.created_at.clone()
    };
    let class_badge = detection.class.clone();
    let model = detection.detector_model.clone();

    let species_info = detection.species.clone().map(|sp| {
        let conf = detection
            .species_confidence
            .map(|c| format!("{:.0}%", c * 100.0))
            .unwrap_or_default();
        (sp, conf)
    });

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
                    <span class="label-text">{label}</span>
                    <span class={format!("class-badge class-{class_badge}")}>{&class_badge}</span>
                </div>

                {species_info.map(|(sp, conf)| view! {
                    <div class="species-row">
                        <span class="species-name">{sp}</span>
                        <span class="species-conf">{conf}</span>
                    </div>
                })}

                <div class="detection-meta">
                    <span class={confidence_class}>{confidence_pct}</span>
                    <span class="model-badge" title="Detection model">{model}</span>
                </div>

                <div class="detection-timestamp">
                    <svg class="icon-clock" viewBox="0 0 16 16" width="14" height="14">
                        <circle cx="8" cy="8" r="7" fill="none"
                                stroke="currentColor" stroke-width="1.5"/>
                        <polyline points="8,4 8,8 11,10" fill="none"
                                  stroke="currentColor" stroke-width="1.5"
                                  stroke-linecap="round"/>
                    </svg>
                    <time>{timestamp}</time>
                </div>

                <div class="detection-source">
                    <span class="clip-name" title="Source clip">
                        {detection.clip_filename}
                    </span>
                    <span class="frame-idx">"frame " {detection.frame_index.to_string()}</span>
                </div>
            </div>
        </div>
    }
}
