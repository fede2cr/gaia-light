//! Settings page — view and edit runtime detection parameters.
//!
//! Reads/writes `settings.json` on the shared data volume so the
//! processing container picks up changes on its next poll cycle.

use leptos::*;

// ── Server functions ─────────────────────────────────────────────────────

#[server(GetSettings, "/api")]
pub async fn get_settings() -> Result<SettingsPayload, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;

    let rt = gaia_light_common::settings::load(&state.data_dir);

    Ok(SettingsPayload {
        db_path: state.db_path.to_string_lossy().to_string(),
        data_dir: state.data_dir.to_string_lossy().to_string(),
        confidence: rt.confidence,
        species_confidence: rt.species_confidence,
        poll_interval_secs: rt.poll_interval_secs,
        max_frames_per_clip: rt.max_frames_per_clip,
        motion_threshold: rt.motion_threshold,
        classifiers: rt.classifiers.map(|v| {
            v.iter().map(|k| k.slug().to_string()).collect()
        }),
    })
}

#[server(SaveSettings, "/api")]
pub async fn save_settings(
    confidence: Option<f64>,
    species_confidence: Option<f64>,
    poll_interval_secs: Option<u64>,
    max_frames_per_clip: Option<u32>,
    motion_threshold: Option<f64>,
    classifiers_csv: Option<String>,
) -> Result<(), ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;

    use gaia_light_common::classifier_kind::ClassifierKind;

    let classifiers = classifiers_csv.as_ref().map(|csv| {
        csv.split(',')
            .filter_map(|s| ClassifierKind::from_slug(s.trim()))
            .collect::<Vec<_>>()
    });

    let rt = gaia_light_common::settings::RuntimeSettings {
        confidence,
        species_confidence,
        poll_interval_secs,
        max_frames_per_clip,
        motion_threshold,
        classifiers,
    };

    gaia_light_common::settings::save(&state.data_dir, &rt)
        .map_err(|e| ServerFnError::new(format!("Save failed: {e}")))?;

    Ok(())
}

// ── DTO ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SettingsPayload {
    pub db_path: String,
    pub data_dir: String,
    pub confidence: Option<f64>,
    pub species_confidence: Option<f64>,
    pub poll_interval_secs: Option<u64>,
    pub max_frames_per_clip: Option<u32>,
    pub motion_threshold: Option<f64>,
    /// Comma-separated classifier slugs (None = use config default).
    pub classifiers: Option<Vec<String>>,
}

// ── Known classifier slugs for the UI checkbox list ──────────────────────

const CLASSIFIER_OPTIONS: &[(&str, &str)] = &[
    ("ai4g-amazon-v2", "AI4G Amazon Rainforest V2"),
    ("speciesnet", "SpeciesNet v4.0.1a"),
];

// ── Component ────────────────────────────────────────────────────────────

/// Settings page showing runtime configuration with editable controls.
#[component]
pub fn Settings() -> impl IntoView {
    let settings = create_resource(|| (), |_| async { get_settings().await });
    let save_action = create_action(
        move |(conf, sp_conf, poll, max_fr, mt, cls_csv): &(
            Option<f64>,
            Option<f64>,
            Option<u64>,
            Option<u32>,
            Option<f64>,
            Option<String>,
        )| {
            let (c, sc, p, mf, m, csv) = (conf.clone(), sp_conf.clone(), poll.clone(), max_fr.clone(), mt.clone(), cls_csv.clone());
            async move { save_settings(c, sc, p, mf, m, csv).await }
        },
    );

    // Signals for each field
    let (confidence, set_confidence) = create_signal(String::new());
    let (species_conf, set_species_conf) = create_signal(String::new());
    let (poll_secs, set_poll_secs) = create_signal(String::new());
    let (max_frames, set_max_frames) = create_signal(String::new());
    let (motion_thresh, set_motion_thresh) = create_signal(String::new());
    let (selected_cls, set_selected_cls) = create_signal(Vec::<String>::new());

    // Populate signals from loaded settings
    create_effect(move |_| {
        if let Some(Ok(s)) = settings.get() {
            set_confidence.set(s.confidence.map(|v| format!("{v}")).unwrap_or_default());
            set_species_conf.set(s.species_confidence.map(|v| format!("{v}")).unwrap_or_default());
            set_poll_secs.set(s.poll_interval_secs.map(|v| format!("{v}")).unwrap_or_default());
            set_max_frames.set(s.max_frames_per_clip.map(|v| format!("{v}")).unwrap_or_default());
            set_motion_thresh.set(s.motion_threshold.map(|v| format!("{v}")).unwrap_or_default());
            set_selected_cls.set(
                s.classifiers
                    .unwrap_or_else(|| vec!["ai4g-amazon-v2".into()]),
            );
        }
    });

    let on_save = move |_| {
        let conf: Option<f64> = confidence.get().parse().ok();
        let sp: Option<f64> = species_conf.get().parse().ok();
        let poll: Option<u64> = poll_secs.get().parse().ok();
        let mf: Option<u32> = max_frames.get().parse().ok();
        let mt: Option<f64> = motion_thresh.get().parse().ok();
        let cls = {
            let v = selected_cls.get();
            if v.is_empty() { None } else { Some(v.join(",")) }
        };
        save_action.dispatch((conf, sp, poll, mf, mt, cls));
    };

    view! {
        <div class="settings-page">
            <h1>"Settings"</h1>

            <Suspense fallback=move || view! { <p class="loading">"Loading..."</p> }>
                {move || settings.get().map(|res| match res {
                    Ok(cfg) => view! {
                        <section class="config-section">
                            <h2>"Paths"</h2>
                            <table class="config-table">
                                <tbody>
                                    <tr><td class="key">"Database"</td><td class="value">{&cfg.db_path}</td></tr>
                                    <tr><td class="key">"Data Directory"</td><td class="value">{&cfg.data_dir}</td></tr>
                                </tbody>
                            </table>
                        </section>

                        <section class="config-section">
                            <h2>"Detection Thresholds"</h2>
                            <p class="info">"Changes are picked up by the processing container on its next poll cycle."</p>

                            <div class="settings-form">
                                <div class="form-row">
                                    <label for="confidence">"Detector confidence (0\u{2013}1)"</label>
                                    <input
                                        id="confidence"
                                        type="number"
                                        step="any"
                                        min="0" max="1"
                                        placeholder="0.5 (default)"
                                        prop:value={move || confidence.get()}
                                        on:input=move |ev| set_confidence.set(event_target_value(&ev))
                                    />
                                </div>
                                <div class="form-row">
                                    <label for="species_conf">"Species confidence (0\u{2013}1)"</label>
                                    <input
                                        id="species_conf"
                                        type="number"
                                        step="any"
                                        min="0" max="1"
                                        placeholder="0.1 (default)"
                                        prop:value={move || species_conf.get()}
                                        on:input=move |ev| set_species_conf.set(event_target_value(&ev))
                                    />
                                </div>
                                <div class="form-row">
                                    <label for="poll_secs">"Poll interval (seconds)"</label>
                                    <input
                                        id="poll_secs"
                                        type="number"
                                        step="1"
                                        min="1"
                                        placeholder="10 (default)"
                                        prop:value={move || poll_secs.get()}
                                        on:input=move |ev| set_poll_secs.set(event_target_value(&ev))
                                    />
                                </div>
                                <div class="form-row">
                                    <label for="max_frames">"Max frames per clip (0 = all)"</label>
                                    <input
                                        id="max_frames"
                                        type="number"
                                        step="1"
                                        min="0"
                                        placeholder="0 (all)"
                                        prop:value={move || max_frames.get()}
                                        on:input=move |ev| set_max_frames.set(event_target_value(&ev))
                                    />
                                </div>
                                <div class="form-row">
                                    <label for="motion_thresh">"Motion threshold (MAD, 0\u{2013}255)"</label>
                                    <input
                                        id="motion_thresh"
                                        type="number"
                                        step="any"
                                        min="0" max="255"
                                        placeholder="1.5 (default)"
                                        prop:value={move || motion_thresh.get()}
                                        on:input=move |ev| set_motion_thresh.set(event_target_value(&ev))
                                    />
                                </div>
                            </div>
                        </section>

                        <section class="config-section">
                            <h2>"Species Classifiers"</h2>
                            <p class="info">"Select which classifiers to run on detection crops.  Multiple classifiers can be active simultaneously\u{2014}the best result is stored."</p>

                            <div class="classifier-list">
                                {CLASSIFIER_OPTIONS.iter().map(|&(slug, name)| {
                                    let slug_str = slug.to_string();
                                    let slug_for_change = slug_str.clone();
                                    let is_checked = {
                                        let s = slug_str.clone();
                                        move || selected_cls.get().iter().any(|c| c == &s)
                                    };
                                    view! {
                                        <label class="classifier-option">
                                            <input
                                                type="checkbox"
                                                prop:checked=is_checked
                                                on:change=move |_| {
                                                    let mut current = selected_cls.get();
                                                    if current.contains(&slug_for_change) {
                                                        current.retain(|c| c != &slug_for_change);
                                                    } else {
                                                        current.push(slug_for_change.clone());
                                                    }
                                                    set_selected_cls.set(current);
                                                }
                                            />
                                            <span class="classifier-name">{name}</span>
                                            <span class="classifier-slug">{slug}</span>
                                        </label>
                                    }
                                }).collect_view()}
                            </div>
                        </section>

                        <section class="config-section">
                            <h2>"Models"</h2>
                            <table class="config-table">
                                <tbody>
                                    <tr><td class="key">"Detector"</td><td class="value">"MegaDetector v5a (YOLOv5, 640\u{00d7}640)"</td></tr>
                                </tbody>
                            </table>
                        </section>

                        <div class="settings-actions">
                            <button class="btn-save" on:click=on_save>"Save Settings"</button>
                            {move || save_action.value().get().map(|res| match res {
                                Ok(()) => view! { <span class="save-ok">"Saved \u{2713}"</span> }.into_view(),
                                Err(e) => view! { <span class="save-err">"Error: " {e.to_string()}</span> }.into_view(),
                            })}
                        </div>
                    }.into_view(),
                    Err(e) => view! {
                        <p class="error">"Error: " {e.to_string()}</p>
                    }.into_view(),
                })}
            </Suspense>
        </div>
    }
}
