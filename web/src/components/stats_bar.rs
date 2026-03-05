//! Stats bar component showing key metrics.

use leptos::*;

use crate::model::SystemInfo;

/// Horizontal bar of key statistics.
#[component]
pub fn StatsBar(info: SystemInfo) -> impl IntoView {
    let db_mb = format!("{:.1} MB", info.db_size_bytes as f64 / (1024.0 * 1024.0));

    view! {
        <div class="stats-bar">
            <div class="stat-item">
                <span class="stat-value">{info.total_detections.to_string()}</span>
                <span class="stat-label">"Detections"</span>
            </div>
            <div class="stat-item">
                <span class="stat-value">{info.total_animals.to_string()}</span>
                <span class="stat-label">"Animals"</span>
            </div>
            <div class="stat-item">
                <span class="stat-value">{info.total_species.to_string()}</span>
                <span class="stat-label">"Species"</span>
            </div>
            <div class="stat-item">
                <span class="stat-value">{info.clips_processed.to_string()}</span>
                <span class="stat-label">"Clips"</span>
            </div>
            <div class="stat-item">
                <span class="stat-value">{db_mb}</span>
                <span class="stat-label">"DB Size"</span>
            </div>
        </div>
    }
}
