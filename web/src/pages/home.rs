//! Home page -- live detection feed with stats overview.

use leptos::prelude::*;
use leptos::prelude::{
    signal, use_context, Effect, ElementChild, For, IntoView, Resource,
    ServerFnError, Suspense,
};
#[cfg(feature = "hydrate")]
use leptos::task::spawn_local;

use crate::components::detection_card::DetectionCard;
use crate::components::stats_bar::StatsBar;
use crate::model::{LiveStatus, PreviewInfo, SystemInfo, WebDetection};

/// Extract a short node label from a capture URL.
///
/// Uses `NODE_NAME` env var for local sources, otherwise strips the URL
/// to just the hostname.
fn node_label(url: &str) -> String {
    let stripped = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let host = stripped
        .split(':')
        .next()
        .unwrap_or(stripped)
        .trim_end_matches('.');
    if host.is_empty() || host == "localhost" || host.starts_with("127.") {
        return node_name_or_local();
    }
    host.to_string()
}

/// Return the `NODE_NAME` env var if set, otherwise `"local"`.
fn node_name_or_local() -> String {
    std::env::var("NODE_NAME")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "local".into())
}

/// Format a capture timestamp into a compact 12-hour string with TZ label.
///
/// `"2026-03-07T16:21:37"` with offset -6 and dst false → `"10:21am (UTC-6)"`
///
/// Pure arithmetic — no chrono dependency (WASM-safe).
fn format_capture_time(iso: &str, offset_hours: i32, dst: bool) -> String {
    if iso.len() < 16 {
        return String::new();
    }
    let parse = || -> Option<String> {
        let year: i32 = iso[0..4].parse().ok()?;
        let month: u32 = iso[5..7].parse().ok()?;
        let day: u32 = iso[8..10].parse().ok()?;
        let hour: i32 = iso[11..13].parse().ok()?;
        let min: u32 = iso[14..16].parse().ok()?;

        let total_offset = offset_hours + if dst { 1 } else { 0 };
        let mut h = hour + total_offset;
        let mut d = day as i32;
        let mut m = month as i32;
        let mut y = year;
        if h >= 24 {
            h -= 24;
            d += 1;
            let days_in_month = match m {
                1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
                4 | 6 | 9 | 11 => 30,
                2 => if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 29 } else { 28 },
                _ => 31,
            };
            if d > days_in_month {
                d = 1;
                m += 1;
                if m > 12 { m = 1; y += 1; }
            }
        } else if h < 0 {
            h += 24;
            d -= 1;
            if d < 1 {
                m -= 1;
                if m < 1 { m = 12; y -= 1; }
                d = match m {
                    1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
                    4 | 6 | 9 | 11 => 30,
                    2 => if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 29 } else { 28 },
                    _ => 31,
                };
            }
        }

        let (h12, ampm) = match h {
            0 => (12, "am"),
            1..=11 => (h, "am"),
            12 => (12, "pm"),
            _ => (h - 12, "pm"),
        };

        let tz_label = if total_offset >= 0 {
            format!("UTC+{total_offset}")
        } else {
            format!("UTC{total_offset}")
        };

        Some(format!("{y}-{m:02}-{d:02} {h12}:{min:02}{ampm} ({tz_label})"))
    };
    parse().unwrap_or_default()
}

// ── Server functions ─────────────────────────────────────────────────────────

#[server(prefix = "/api")]
pub async fn get_recent_detections(
    limit: u32,
    after_id: Option<i64>,
) -> Result<Vec<WebDetection>, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;
    crate::server::db::recent_detections(&state.db_path, limit, after_id)
        .await
        .map_err(|e| ServerFnError::new(format!("DB error: {e}")))
}

#[server(prefix = "/api")]
pub async fn get_system_info() -> Result<SystemInfo, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;
    crate::server::db::system_info(&state.db_path)
        .await
        .map_err(|e| ServerFnError::new(format!("DB error: {e}")))
}

#[server(prefix = "/api")]
pub async fn get_live_status() -> Result<Option<LiveStatus>, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;
    Ok(crate::server::db::read_live_status(&state.data_dir))
}

#[server(prefix = "/api")]
pub async fn get_preview_info() -> Result<PreviewInfo, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;
    Ok(crate::server::db::preview_info(&state.data_dir))
}

/// Return the current UTC offset and DST flag from settings.
#[server(prefix = "/api")]
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
        Resource::new(|| (), |_| async { get_recent_detections(50, None).await });
    let sys_info =
        Resource::new(|| (), |_| async { get_system_info().await });
    // Live status: signal + initial load + auto-refresh so the
    // capture timestamp updates as new clips are processed.
    let (live_status, set_live_status) = signal::<Option<LiveStatus>>(None);
    {
        let live_init =
            Resource::new(|| (), |_| async { get_live_status().await });
        Effect::new(move || {
            if let Some(Ok(status)) = live_init.get() {
                set_live_status.set(status);
            }
        });
    }

    // Auto-refresh live status every 5 seconds
    #[cfg(feature = "hydrate")]
    {
        set_interval_with_handle(
            move || {
                spawn_local(async move {
                    if let Ok(status) = get_live_status().await {
                        set_live_status.set(status);
                    }
                });
            },
            std::time::Duration::from_secs(5),
        )
        .ok();
    }

    let tz =
        Resource::new(|| (), |_| async { get_tz_settings().await });

    // Preview: poll server for file modification time to cache-bust the <img>
    let (preview_ts, set_preview_ts) = signal::<u64>(0);
    let (preview_avail, set_preview_avail) = signal(false);

    // Initial preview check
    let preview_res =
        Resource::new(|| (), |_| async { get_preview_info().await });
    Effect::new(move || {
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
    let (feed, set_feed) = signal::<Vec<WebDetection>>(vec![]);
    #[allow(unused_variables)]
    let (max_id, set_max_id) = signal::<Option<i64>>(None);

    // Populate feed when initial data arrives
    Effect::new(move || {
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
                    Ok(info) => view! { <StatsBar info=info/> }.into_any(),
                    Err(_) => view! {}.into_any(),
                })}
            </Suspense>

            // Live status indicator (auto-refreshes every 5s)
            {move || {
                let tz_data = tz.get().and_then(|r| r.ok()).unwrap_or((0, false));
                match live_status.get() {
                    Some(status) => {
                        let node = status.source_node
                            .as_deref()
                            .map(|u| node_label(u))
                            .unwrap_or_else(node_name_or_local);
                        let cap_time = status.captured_at
                            .as_deref()
                            .map(|ts| format_capture_time(ts, tz_data.0, tz_data.1))
                            .unwrap_or_default();

                        view! {
                            <div class="live-indicator">
                                <span class="live-dot"></span>
                                " Now processing from "
                                <strong>{node}</strong>
                                {if !cap_time.is_empty() {
                                    view! { <span>", captured at " {cap_time}</span> }.into_any()
                                } else {
                                    view! {}.into_any()
                                }}
                                " \u{2014} "
                                {status.detections_last_hour.to_string()} " det/h"
                            </div>
                        }.into_any()
                    }
                    None => view! {
                        <div class="live-indicator offline">
                            <span class="live-dot offline"></span>
                            " Waiting for first clip\u{2026}"
                        </div>
                    }.into_any(),
                }
            }}

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
                            }.into_any()
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
                            }.into_any()
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
