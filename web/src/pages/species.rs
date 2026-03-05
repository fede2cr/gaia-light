//! Species ranking page — top species by detection count.

use leptos::*;

use crate::model::SpeciesSummary;

#[server(GetTopSpecies, "/api")]
pub async fn get_top_species(limit: u32) -> Result<Vec<SpeciesSummary>, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;
    crate::server::db::top_species(&state.db_path, limit)
        .map_err(|e| ServerFnError::new(format!("DB error: {e}")))
}

#[server(GetDailyCounts, "/api")]
pub async fn get_daily_counts(days: u32) -> Result<Vec<crate::model::DailyCount>, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;
    crate::server::db::daily_counts(&state.db_path, days)
        .map_err(|e| ServerFnError::new(format!("DB error: {e}")))
}

/// Species ranking and daily trend chart.
#[component]
pub fn Species() -> impl IntoView {
    let species = create_resource(|| (), |_| async { get_top_species(50).await });
    let daily = create_resource(|| (), |_| async { get_daily_counts(30).await });

    view! {
        <div class="species-page">
            <h1>"Species"</h1>

            // Species ranking table
            <section class="species-ranking">
                <h2>"Top Species"</h2>
                <Suspense fallback=move || view! { <p class="loading">"Loading species..."</p> }>
                    {move || species.get().map(|res| match res {
                        Ok(list) if list.is_empty() => view! {
                            <p class="empty-state">"No species identified yet."</p>
                        }.into_view(),
                        Ok(list) => {
                            let max_count = list.first().map(|s| s.count).unwrap_or(1);
                            view! {
                                <table class="ranking-table">
                                    <thead>
                                        <tr>
                                            <th>"#"</th>
                                            <th>"Species"</th>
                                            <th>"Detections"</th>
                                            <th>"Last Seen"</th>
                                            <th></th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {list.into_iter().enumerate().map(|(i, sp)| {
                                            let pct = (sp.count as f64 / max_count as f64 * 100.0) as u32;
                                            let last = sp.last_seen.clone().unwrap_or_default();
                                            view! {
                                                <tr>
                                                    <td class="rank">{(i + 1).to_string()}</td>
                                                    <td class="species-name">{sp.species.clone()}</td>
                                                    <td class="count">{sp.count.to_string()}</td>
                                                    <td class="last-seen">{last}</td>
                                                    <td class="bar-cell">
                                                        <div class="bar" style=format!("width:{}%", pct)></div>
                                                    </td>
                                                </tr>
                                            }
                                        }).collect_view()}
                                    </tbody>
                                </table>
                            }.into_view()
                        }
                        Err(e) => view! {
                            <p class="error">"Error: " {e.to_string()}</p>
                        }.into_view(),
                    })}
                </Suspense>
            </section>

            // Daily activity
            <section class="daily-activity">
                <h2>"Daily Activity (30 days)"</h2>
                <Suspense fallback=move || view! { <p class="loading">"Loading..."</p> }>
                    {move || daily.get().map(|res| match res {
                        Ok(counts) if counts.is_empty() =>
                            view! { <p class="empty-state">"No activity yet."</p> }.into_view(),
                        Ok(counts) => {
                            let max_c = counts.iter().map(|d| d.total).max().unwrap_or(1);
                            view! {
                                <div class="daily-chart">
                                    {counts.into_iter().map(|d| {
                                        let h = (d.total as f64 / max_c as f64 * 100.0) as u32;
                                        view! {
                                            <div class="day-bar" title=format!("{}: {}", d.date, d.total)>
                                                <div class="day-fill" style=format!("height:{}%", h)></div>
                                                <span class="day-label">{d.date.chars().skip(5).collect::<String>()}</span>
                                            </div>
                                        }
                                    }).collect_view()}
                                </div>
                            }.into_view()
                        }
                        Err(e) =>
                            view! { <p class="error">{e.to_string()}</p> }.into_view(),
                    })}
                </Suspense>
            </section>
        </div>
    }
}
