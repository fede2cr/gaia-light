//! Home page -- live detection feed with stats overview.

use leptos::*;

use crate::components::detection_card::DetectionCard;
use crate::components::stats_bar::StatsBar;
use crate::model::{LiveStatus, SystemInfo, WebDetection};

// ── Server functions ─────────────────────────────────────────────────────────

#[server(GetRecentDetections, "/api")]
pub async fn get_recent_detections(
    limit: u32,
    after_id: Option<i64>,
) -> Result<Vec<WebDetection>, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;
    crate::server::db::recent_detections(&state.db_path, limit, after_id)
        .map_err(|e| ServerFnError::new(format!("DB error: {e}")))
}

#[server(GetSystemInfo, "/api")]
pub async fn get_system_info() -> Result<SystemInfo, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;
    crate::server::db::system_info(&state.db_path)
        .map_err(|e| ServerFnError::new(format!("DB error: {e}")))
}

#[server(GetLiveStatus, "/api")]
pub async fn get_live_status() -> Result<Option<LiveStatus>, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;
    Ok(crate::server::db::read_live_status(&state.data_dir))
}

// ── Page component ───────────────────────────────────────────────────────────

/// Live detection feed with auto-polling and system stats.
#[component]
pub fn Home() -> impl IntoView {
    // Initial data loads
    let detections =
        create_resource(|| (), |_| async { get_recent_detections(50, None).await });
    let sys_info =
        create_resource(|| (), |_| async { get_system_info().await });
    let live =
        create_resource(|| (), |_| async { get_live_status().await });

    // Live feed signal for auto-refresh
    let (feed, set_feed) = create_signal::<Vec<WebDetection>>(vec![]);
    #[allow(unused_variables)]
    let (max_id, set_max_id) = create_signal::<Option<i64>>(None);

    // Populate feed when initial data arrives
    create_effect(move |_| {
        if let Some(Ok(initial)) = detections.get() {
            if let Some(first) = initial.first() {
                set_max_id.set(Some(first.id));
            }
            set_feed.set(initial);
        }
    });

    // Auto-refresh every 5 seconds
    #[cfg(feature = "hydrate")]
    {
        set_interval_with_handle(
            move || {
                let rid = max_id.get();
                spawn_local(async move {
                    if let Ok(new) = get_recent_detections(20, rid).await {
                        if !new.is_empty() {
                            if let Some(first) = new.first() {
                                set_max_id.set(Some(first.id));
                            }
                            set_feed.update(|f| {
                                let mut combined = new;
                                combined.extend(f.drain(..));
                                combined.truncate(100);
                                *f = combined;
                            });
                        }
                    }
                });
            },
            std::time::Duration::from_secs(5),
        )
        .ok();
    }

    view! {
        <div class="home-page">
            // System stats bar
            <Suspense fallback=move || view! { <div class="stats-bar loading">"Loading stats..."</div> }>
                {move || sys_info.get().map(|res| match res {
                    Ok(info) => view! { <StatsBar info=info/> }.into_view(),
                    Err(_) => view! {}.into_view(),
                })}
            </Suspense>

            // Live status indicator
            <Suspense fallback=move || ()>
                {move || live.get().map(|res| match res {
                    Ok(Some(status)) => view! {
                        <div class="live-indicator">
                            <span class="live-dot"></span>
                            " Processing: "
                            <strong>{status.last_clip}</strong>
                            " | "
                            {status.detections_last_hour.to_string()} " detections/hour"
                            " | Updated: " {status.updated_at}
                        </div>
                    }.into_view(),
                    _ => view! {
                        <div class="live-indicator offline">
                            <span class="live-dot offline"></span>
                            " Processing offline"
                        </div>
                    }.into_view(),
                })}
            </Suspense>

            // Detection feed
            <section class="live-feed">
                <h2>"Recent Detections"</h2>
                <div class="feed-list">
                    <Suspense fallback=move || view! { <p class="loading">"Loading detections..."</p> }>
                        <For
                            each=move || feed.get()
                            key=|d| d.id
                            children=move |det: WebDetection| {
                                view! { <DetectionCard detection=det/> }
                            }
                        />
                    </Suspense>
                </div>
            </section>
        </div>
    }
}
