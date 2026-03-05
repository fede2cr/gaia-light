//! Detections listing page — full table of recent detections with pagination.

use leptos::*;

use crate::components::detection_card::DetectionCard;
use crate::model::WebDetection;

#[server(GetDetectionPage, "/api")]
pub async fn get_detection_page(
    limit: u32,
    offset: u32,
) -> Result<Vec<WebDetection>, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;
    // Re-use recent_detections with no after_id filter; offset handled client-side
    // For a real pagination we'd add offset support, but for now we fetch a large batch.
    crate::server::db::recent_detections(
        &state.db_path,
        limit + offset,
        None,
    )
    .map(|all| all.into_iter().skip(offset as usize).collect())
    .map_err(|e| ServerFnError::new(format!("DB error: {e}")))
}

/// Full detections page with class filter and pagination.
#[component]
pub fn Detections() -> impl IntoView {
    let (page, set_page) = create_signal(0_u32);
    let page_size = 30_u32;

    let detections = create_resource(
        move || page.get(),
        move |p| async move { get_detection_page(page_size, p * page_size).await },
    );

    let (class_filter, set_class_filter) = create_signal(String::new());

    view! {
        <div class="detections-page">
            <header class="page-header">
                <h1>"Detections"</h1>
                <div class="filters">
                    <select on:change=move |ev| {
                        set_class_filter.set(event_target_value(&ev));
                        set_page.set(0);
                    }>
                        <option value="">"All classes"</option>
                        <option value="animal">"Animal"</option>
                        <option value="person">"Person"</option>
                        <option value="vehicle">"Vehicle"</option>
                    </select>
                </div>
            </header>

            <div class="detection-grid">
                <Suspense fallback=move || view! { <p class="loading">"Loading..."</p> }>
                    {move || {
                        let cf = class_filter.get();
                        detections.get().map(|res| match res {
                            Ok(dets) => {
                                let filtered: Vec<WebDetection> = if cf.is_empty() {
                                    dets
                                } else {
                                    dets.into_iter().filter(|d| d.class == cf).collect()
                                };
                                if filtered.is_empty() {
                                    view! { <p class="empty-state">"No detections found."</p> }.into_view()
                                } else {
                                    filtered.into_iter().map(|det| {
                                        view! { <DetectionCard detection=det/> }
                                    }).collect_view()
                                }
                            }
                            Err(e) => view! {
                                <p class="error">"Error: " {e.to_string()}</p>
                            }.into_view(),
                        })
                    }}
                </Suspense>
            </div>

            // Pagination controls
            <nav class="pagination">
                <button
                    class="btn"
                    disabled=move || page.get() == 0
                    on:click=move |_| set_page.update(|p| *p = p.saturating_sub(1))
                >
                    "← Previous"
                </button>
                <span class="page-number">"Page " {move || (page.get() + 1).to_string()}</span>
                <button
                    class="btn"
                    on:click=move |_| set_page.update(|p| *p += 1)
                >
                    "Next →"
                </button>
            </nav>
        </div>
    }
}
