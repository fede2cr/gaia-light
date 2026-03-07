//! Home page -- live detection feed with stats overview.

use leptos::*;

use crate::components::detection_card::DetectionCard;
use crate::components::stats_bar::StatsBar;
use crate::model::{LiveStatus, PreviewInfo, SystemInfo, WebDetection};

/// Extract a short node label from a capture URL.
/// "http://192.168.1.50:8090" → "192.168.1.50"
/// "http://gaia-lt-cap-01.local:8090" → "gaia-lt-cap-01"
fn node_label(url: &str) -> String {
    let stripped = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    // Remove port
    stripped
        .split(':')
        .next()
        .unwrap_or(stripped)
        .trim_end_matches('.')
        .to_string()
}

/// Parse an ISO-8601 / RFC-3339 timestamp, apply a UTC offset (hours)
/// and optional DST (+1h), and return a human-readable local time string.
///
/// Does simple arithmetic without pulling in chrono for the WASM bundle.
fn format_local_time(iso: &str, offset_hours: i32, dst: bool) -> String {
    // Expect "YYYY-MM-DDTHH:MM:SS…" (at least 19 chars)
    if iso.len() < 19 {
        return iso.to_string();
    }
    let parse = || -> Option<String> {
        let year: i32  = iso[0..4].parse().ok()?;
        let mon: u32   = iso[5..7].parse().ok()?;
        let day: u32   = iso[8..10].parse().ok()?;
        let hour: i32  = iso[11..13].parse().ok()?;
        let min: u32   = iso[14..16].parse().ok()?;
        let sec: u32   = iso[17..19].parse().ok()?;

        let total_offset = offset_hours + if dst { 1 } else { 0 };
        let mut h = hour + total_offset;
        let mut d = day as i32;
        let mut m = mon;
        let mut y = year;

        // Normalise hour overflow / underflow into day shift
        if h >= 24 { h -= 24; d += 1; }
        if h < 0   { h += 24; d -= 1; }

        let days_in_month = |mm: u32, yy: i32| -> u32 {
            match mm {
                1|3|5|7|8|10|12 => 31,
                4|6|9|11 => 30,
                2 => if yy % 4 == 0 && (yy % 100 != 0 || yy % 400 == 0) { 29 } else { 28 },
                _ => 30,
            }
        };
        if d < 1 {
            m = if m == 1 { 12 } else { m - 1 };
            if m == 12 && mon == 1 { y -= 1; }
            d = days_in_month(m, y) as i32;
        } else if d > days_in_month(m, y) as i32 {
            d = 1;
            m += 1;
            if m > 12 { m = 1; y += 1; }
        }

        Some(format!(
            "{y:04}-{m:02}-{:02} {:02}:{min:02}:{sec:02}",
            d, h
        ))
    };
    parse().unwrap_or_else(|| iso.to_string())
}

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

#[server(GetPreviewInfo, "/api")]
pub async fn get_preview_info() -> Result<PreviewInfo, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;
    Ok(crate::server::db::preview_info(&state.data_dir))
}

/// Return the current UTC offset and DST flag from settings.
#[server(GetTzSettings, "/api")]
pub async fn get_tz_settings() -> Result<(i32, bool), ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;
    let rt = gaia_light_common::settings::load(&state.data_dir);
    Ok((rt.utc_offset_hours.unwrap_or(0), rt.dst.unwrap_or(false)))
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
    let tz =
        create_resource(|| (), |_| async { get_tz_settings().await });

    // Preview: poll server for file modification time to cache-bust the <img>
    let (preview_ts, set_preview_ts) = create_signal::<u64>(0);
    let (preview_avail, set_preview_avail) = create_signal(false);

    // Initial preview check
    let preview_res =
        create_resource(|| (), |_| async { get_preview_info().await });
    create_effect(move |_| {
        if let Some(Ok(info)) = preview_res.get() {
            set_preview_avail.set(info.available);
            set_preview_ts.set(info.modified_ms);
        }
    });

    // Auto-refresh preview every 3 seconds
    #[cfg(feature = "hydrate")]
    {
        set_interval_with_handle(
            move || {
                spawn_local(async move {
                    if let Ok(info) = get_preview_info().await {
                        set_preview_avail.set(info.available);
                        if info.modified_ms != preview_ts.get_untracked() {
                            set_preview_ts.set(info.modified_ms);
                        }
                    }
                });
            },
            std::time::Duration::from_secs(3),
        )
        .ok();
    }

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
                {move || {
                    let tz_data = tz.get().and_then(|r| r.ok()).unwrap_or((0, false));
                    live.get().map(|res| match res {
                        Ok(Some(status)) => {
                            let node = status.source_node
                                .as_deref()
                                .map(|u| node_label(u))
                                .unwrap_or_else(|| "unknown".into());
                            let local_time = format_local_time(
                                &status.updated_at, tz_data.0, tz_data.1
                            );
                            view! {
                                <div class="live-indicator">
                                    <span class="live-dot"></span>
                                    " Capture: "
                                    <strong>{node}</strong>
                                    " | "
                                    {status.last_clip}
                                    " | "
                                    {status.detections_last_hour.to_string()} " det/h"
                                    " | " {local_time}
                                </div>
                            }.into_view()
                        }
                        _ => view! {
                            <div class="live-indicator offline">
                                <span class="live-dot offline"></span>
                                " Waiting for first clip\u{2026}"
                            </div>
                        }.into_view(),
                    })
                }}
            </Suspense>

            // Camera preview panel
            <section class="preview-panel">
                <h2>"Camera Preview"</h2>
                <div class="preview-container">
                    {move || {
                        if preview_avail.get() {
                            let ts = preview_ts.get();
                            let src = format!("/preview/preview_latest.jpg?t={ts}");
                            view! {
                                <img
                                    class="preview-image"
                                    src={src}
                                    alt="Latest processed frame"
                                />
                            }.into_view()
                        } else {
                            view! {
                                <div class="preview-placeholder">
                                    <svg viewBox="0 0 24 24" width="48" height="48"
                                         fill="none" stroke="currentColor" stroke-width="1">
                                        <rect x="2" y="3" width="20" height="14" rx="2"/>
                                        <circle cx="12" cy="10" r="3"/>
                                        <path d="M2 17l20 0"/>
                                        <circle cx="12" cy="21" r="1"/>
                                    </svg>
                                    <p>"Waiting for first frame..."</p>
                                </div>
                            }.into_view()
                        }
                    }}
                </div>
            </section>

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
