//! Settings page — display current configuration and detection thresholds.

use leptos::*;

#[server(GetConfig, "/api")]
pub async fn get_config() -> Result<SettingsData, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;

    Ok(SettingsData {
        db_path: state.db_path.to_string_lossy().to_string(),
        data_dir: state.data_dir.to_string_lossy().to_string(),
    })
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SettingsData {
    pub db_path: String,
    pub data_dir: String,
}

/// Settings page showing runtime configuration.
#[component]
pub fn Settings() -> impl IntoView {
    let config = create_resource(|| (), |_| async { get_config().await });

    view! {
        <div class="settings-page">
            <h1>"Settings"</h1>

            <Suspense fallback=move || view! { <p class="loading">"Loading..."</p> }>
                {move || config.get().map(|res| match res {
                    Ok(cfg) => view! {
                        <section class="config-section">
                            <h2>"Runtime Configuration"</h2>
                            <table class="config-table">
                                <tbody>
                                    <tr>
                                        <td class="key">"Database"</td>
                                        <td class="value">{&cfg.db_path}</td>
                                    </tr>
                                    <tr>
                                        <td class="key">"Data Directory"</td>
                                        <td class="value">{&cfg.data_dir}</td>
                                    </tr>
                                </tbody>
                            </table>
                        </section>

                        <section class="config-section">
                            <h2>"Detection Thresholds"</h2>
                            <p class="info">
                                "Detection thresholds are configured via environment variables on the "
                                <strong>"gaia-light-processing"</strong>
                                " container."
                            </p>
                            <table class="config-table">
                                <tbody>
                                    <tr>
                                        <td class="key">"GAIA_CONFIDENCE_THRESHOLD"</td>
                                        <td class="value">"Minimum detector confidence (default: 0.3)"</td>
                                    </tr>
                                    <tr>
                                        <td class="key">"GAIA_SPECIES_THRESHOLD"</td>
                                        <td class="value">"Minimum species confidence (default: 0.1)"</td>
                                    </tr>
                                    <tr>
                                        <td class="key">"GAIA_POLL_INTERVAL"</td>
                                        <td class="value">"Poll interval in seconds (default: 30)"</td>
                                    </tr>
                                    <tr>
                                        <td class="key">"GAIA_MAX_FRAMES"</td>
                                        <td class="value">"Max frames per clip (default: 10)"</td>
                                    </tr>
                                </tbody>
                            </table>
                        </section>

                        <section class="config-section">
                            <h2>"Models"</h2>
                            <table class="config-table">
                                <tbody>
                                    <tr>
                                        <td class="key">"Detector"</td>
                                        <td class="value">"MegaDetector v6 (YOLOv5, 640×640)"</td>
                                    </tr>
                                    <tr>
                                        <td class="key">"Classifier"</td>
                                        <td class="value">"SpeciesNet (224×224)"</td>
                                    </tr>
                                </tbody>
                            </table>
                        </section>
                    }.into_view(),
                    Err(e) => view! {
                        <p class="error">"Error: " {e.to_string()}</p>
                    }.into_view(),
                })}
            </Suspense>
        </div>
    }
}
