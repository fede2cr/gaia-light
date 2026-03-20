//! Settings page — view and edit runtime detection parameters.
//!
//! Reads/writes `settings.json` on the shared data volume so the
//! processing container picks up changes on its next poll cycle.

use leptos::prelude::*;use leptos::prelude::{
    signal, use_context, Action, Effect, ElementChild, IntoView, Resource,
    ServerFnError, Suspense,
};
// ── Server functions ─────────────────────────────────────────────────────

#[server(prefix = "/api")]
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
        utc_offset_hours: rt.utc_offset_hours,
        dst: rt.dst,
    })
}

#[server(prefix = "/api")]
pub async fn save_settings(
    confidence: Option<f64>,
    species_confidence: Option<f64>,
    poll_interval_secs: Option<u64>,
    max_frames_per_clip: Option<u32>,
    motion_threshold: Option<f64>,
    classifiers_csv: Option<String>,
    utc_offset_hours: Option<i32>,
    dst: Option<bool>,
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
        utc_offset_hours,
        dst,
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
    /// UTC offset in whole hours.
    pub utc_offset_hours: Option<i32>,
    /// Whether DST is active (+1h).
    pub dst: Option<bool>,
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
    let settings = Resource::new(|| (), |_| async { get_settings().await });
    let save_action = Action::new(
        move |(conf, sp_conf, poll, max_fr, mt, cls_csv, tz_off, tz_dst): &(
            Option<f64>,
            Option<f64>,
            Option<u64>,
            Option<u32>,
            Option<f64>,
            Option<String>,
            Option<i32>,
            Option<bool>,
        )| {
            let (c, sc, p, mf, m, csv, off, dst) = (conf.clone(), sp_conf.clone(), poll.clone(), max_fr.clone(), mt.clone(), cls_csv.clone(), tz_off.clone(), tz_dst.clone());
            async move { save_settings(c, sc, p, mf, m, csv, off, dst).await }
        },
    );

    // Signals for each field
    let (confidence, set_confidence) = signal(String::new());
    let (species_conf, set_species_conf) = signal(String::new());
    let (poll_secs, set_poll_secs) = signal(String::new());
    let (max_frames, set_max_frames) = signal(String::new());
    let (motion_thresh, set_motion_thresh) = signal(String::new());
    let (selected_cls, set_selected_cls) = signal(Vec::<String>::new());
    let (utc_offset, set_utc_offset) = signal(String::from("0"));
    let (dst_on, set_dst_on) = signal(false);

    // Populate signals from loaded settings
    Effect::new(move || {
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
            set_utc_offset.set(s.utc_offset_hours.map(|v| format!("{v}")).unwrap_or_else(|| "0".into()));
            set_dst_on.set(s.dst.unwrap_or(false));
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
        let tz: Option<i32> = utc_offset.get().parse().ok();
        let dst = Some(dst_on.get());
        save_action.dispatch((conf, sp, poll, mf, mt, cls, tz, dst));
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
                                    <tr><td class="key">"Database"</td><td class="value">{cfg.db_path.clone()}</td></tr>
                                    <tr><td class="key">"Data Directory"</td><td class="value">{cfg.data_dir.clone()}</td></tr>
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
                                }).collect::<Vec<_>>()}
                            </div>
                        </section>

                        <section class="config-section">
                            <h2>"Timezone"</h2>
                            <p class="info">"Set the UTC offset for displaying timestamps in local time."</p>

                            <div class="settings-form">
                                <div class="form-row">
                                    <label for="utc_offset">"UTC offset (hours)"</label>
                                    <select
                                        id="utc_offset"
                                        prop:value={move || utc_offset.get()}
                                        on:change=move |ev| set_utc_offset.set(event_target_value(&ev))
                                    >
                                        <option value="-12">"UTC\u{2212}12"</option>
                                        <option value="-11">"UTC\u{2212}11"</option>
                                        <option value="-10">"UTC\u{2212}10"</option>
                                        <option value="-9">"UTC\u{2212}9"</option>
                                        <option value="-8">"UTC\u{2212}8"</option>
                                        <option value="-7">"UTC\u{2212}7"</option>
                                        <option value="-6">"UTC\u{2212}6"</option>
                                        <option value="-5">"UTC\u{2212}5"</option>
                                        <option value="-4">"UTC\u{2212}4"</option>
                                        <option value="-3">"UTC\u{2212}3"</option>
                                        <option value="-2">"UTC\u{2212}2"</option>
                                        <option value="-1">"UTC\u{2212}1"</option>
                                        <option value="0">"UTC\u{00b1}0"</option>
                                        <option value="1">"UTC+1"</option>
                                        <option value="2">"UTC+2"</option>
                                        <option value="3">"UTC+3"</option>
                                        <option value="4">"UTC+4"</option>
                                        <option value="5">"UTC+5"</option>
                                        <option value="6">"UTC+6"</option>
                                        <option value="7">"UTC+7"</option>
                                        <option value="8">"UTC+8"</option>
                                        <option value="9">"UTC+9"</option>
                                        <option value="10">"UTC+10"</option>
                                        <option value="11">"UTC+11"</option>
                                        <option value="12">"UTC+12"</option>
                                        <option value="13">"UTC+13"</option>
                                        <option value="14">"UTC+14"</option>
                                    </select>
                                </div>
                                <div class="form-row">
                                    <label for="dst">"Daylight Saving Time (+1h)"</label>
                                    <label class="toggle-switch">
                                        <input
                                            type="checkbox"
                                            id="dst"
                                            prop:checked=move || dst_on.get()
                                            on:change=move |_| {
                                                set_dst_on.update(|v| *v = !*v);
                                            }
                                        />
                                        <span class="toggle-slider"></span>
                                    </label>
                                </div>
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
                                Ok(()) => view! { <span class="save-ok">"Saved \u{2713}"</span> }.into_any(),
                                Err(e) => view! { <span class="save-err">"Error: " {e.to_string()}</span> }.into_any(),
                            })}
                        </div>
                    }.into_any(),
                    Err(e) => view! {
                        <p class="error">"Error: " {e.to_string()}</p>
                    }.into_any(),
                })}
            </Suspense>
        </div>
    }
}
