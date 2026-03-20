//! People page — browse and manage identified individuals
//! detected by the person re-identification pipeline.

use leptos::prelude::*;
use leptos::prelude::{
    signal, use_context, Action, ElementChild, IntoView, Resource,
    ServerFnError, Suspense,
};

use crate::model::{Individual, WebDetection};

// ── Server functions ─────────────────────────────────────────────────────

#[server(prefix = "/api")]
pub async fn get_people() -> Result<Vec<Individual>, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;

    crate::server::db::list_individuals(&state.db_path)
        .await
        .map_err(|e| ServerFnError::new(format!("DB error: {e}")))
}

#[server(prefix = "/api")]
pub async fn rename_person(
    individual_id: i64,
    new_name: String,
) -> Result<(), ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;

    crate::server::db::rename_individual(&state.db_path, individual_id, &new_name)
        .await
        .map_err(|e| ServerFnError::new(format!("DB error: {e}")))
}

#[server(prefix = "/api")]
pub async fn get_person_detections(
    individual_id: i64,
) -> Result<Vec<WebDetection>, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;

    crate::server::db::individual_detections(&state.db_path, individual_id, 50)
        .await
        .map_err(|e| ServerFnError::new(format!("DB error: {e}")))
}

// ── Component ────────────────────────────────────────────────────────────

/// People overview page showing all identified individuals.
#[component]
pub fn People() -> impl IntoView {
    let people = Resource::new(|| (), |_| async { get_people().await });

    // Selected individual for detail view
    let (selected_id, set_selected_id) = signal::<Option<i64>>(None);

    // Detections for the selected individual
    let person_detections = Resource::new(
        move || selected_id.get(),
        move |id| async move {
            match id {
                Some(id) => get_person_detections(id).await.ok(),
                None => None,
            }
        },
    );

    // Rename action
    let rename_action = Action::new(
        move |(id, name): &(i64, String)| {
            let id = *id;
            let name = name.clone();
            async move {
                let _ = rename_person(id, name).await;
                // Refetch people list after rename
                people.refetch();
            }
        },
    );

    view! {
        <div class="people-page">
            <h1>"👤 People"</h1>
            <p class="info">
                "Individuals identified by person re-identification. "
                "Click on a person to see their detections, or rename them."
            </p>

            <Suspense fallback=move || view! { <p class="loading">"Loading\u{2026}"</p> }>
                {move || people.get().map(|res| match res {
                    Ok(list) if list.is_empty() => view! {
                        <div class="empty-state">
                            <p>"No individuals detected yet."</p>
                            <p class="hint">
                                "Person re-identification will create entries here "
                                "when the processing pipeline detects people."
                            </p>
                        </div>
                    }.into_any(),
                    Ok(list) => {
                        let total = list.len();
                        view! {
                            <div class="people-stats">
                                <span class="stat-count">{total}" individual(s) identified"</span>
                            </div>

                            <div class="people-grid">
                                {list.into_iter().map(|person| {
                                    let id = person.id;
                                    let display = person.display_name();
                                    let crop_url = person.crop_url();
                                    let count = person.detection_count;
                                    let last_seen = if person.last_seen.len() >= 16 {
                                        person.last_seen[..16].replace('T', " ")
                                    } else {
                                        person.last_seen.clone()
                                    };
                                    let is_selected = move || selected_id.get() == Some(id);
                                    let is_unnamed = person.name.is_empty();

                                    view! {
                                        <div
                                            class=move || if is_selected() { "person-card selected" } else { "person-card" }
                                            on:click=move |_| {
                                                if selected_id.get() == Some(id) {
                                                    set_selected_id.set(None);
                                                } else {
                                                    set_selected_id.set(Some(id));
                                                }
                                            }
                                        >
                                            <div class="person-thumb">
                                                {match crop_url {
                                                    Some(url) => view! {
                                                        <img
                                                            class="person-crop"
                                                            src={url}
                                                            alt={display.clone()}
                                                            loading="lazy"
                                                        />
                                                    }.into_any(),
                                                    None => view! {
                                                        <div class="person-placeholder">"👤"</div>
                                                    }.into_any(),
                                                }}
                                            </div>
                                            <div class="person-info">
                                                <span class="person-name">
                                                    {display.clone()}
                                                    {is_unnamed.then(|| view! { <span class="unnamed-badge">"unnamed"</span> })}
                                                </span>
                                                <span class="person-sightings">
                                                    {count}" sighting(s)"
                                                </span>
                                                <span class="person-last-seen">
                                                    "Last: " {last_seen}
                                                </span>
                                            </div>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>

                            // Detail panel for selected individual
                            {move || selected_id.get().map(|id| {
                                let (edit_name, set_edit_name) = signal(String::new());

                                view! {
                                    <div class="person-detail">
                                        <h2>"Detections for selected individual"</h2>

                                        <div class="rename-form">
                                            <input
                                                type="text"
                                                placeholder="Enter name\u{2026}"
                                                class="rename-input"
                                                prop:value=move || edit_name.get()
                                                on:input=move |ev| {
                                                    set_edit_name.set(event_target_value(&ev));
                                                }
                                            />
                                            <button
                                                class="rename-btn"
                                                on:click=move |_| {
                                                    let name = edit_name.get();
                                                    if !name.is_empty() {
                                                        rename_action.dispatch((id, name));
                                                        set_edit_name.set(String::new());
                                                    }
                                                }
                                            >
                                                "Rename"
                                            </button>
                                        </div>

                                        <Suspense fallback=move || view! { <p class="loading">"Loading detections\u{2026}"</p> }>
                                            {move || person_detections.get().map(|dets| {
                                                match dets {
                                                    Some(detections) if !detections.is_empty() => {
                                                        view! {
                                                            <div class="person-detections-grid">
                                                                {detections.into_iter().map(|d| {
                                                                    let crop_url = d.crop_url();
                                                                    let ts = if d.timestamp.len() >= 16 {
                                                                        d.timestamp[..16].replace('T', " ")
                                                                    } else {
                                                                        d.timestamp.clone()
                                                                    };
                                                                    let conf = d.confidence_pct();

                                                                    view! {
                                                                        <div class="person-det-card">
                                                                            {match crop_url {
                                                                                Some(url) => view! {
                                                                                    <img
                                                                                        class="person-det-crop"
                                                                                        src={url}
                                                                                        loading="lazy"
                                                                                    />
                                                                                }.into_any(),
                                                                                None => view! {
                                                                                    <div class="crop-placeholder">"?"</div>
                                                                                }.into_any(),
                                                                            }}
                                                                            <div class="person-det-meta">
                                                                                <span>{ts}</span>
                                                                                <span class="confidence">{conf}</span>
                                                                            </div>
                                                                        </div>
                                                                    }
                                                                }).collect::<Vec<_>>()}
                                                            </div>
                                                        }.into_any()
                                                    },
                                                    _ => view! {
                                                        <p class="empty">"No detections found."</p>
                                                    }.into_any(),
                                                }
                                            })}
                                        </Suspense>
                                    </div>
                                }
                            })}
                        }.into_any()
                    },
                    Err(e) => view! {
                        <p class="error">"Error: " {e.to_string()}</p>
                    }.into_any(),
                })}
            </Suspense>
        </div>
    }
}
