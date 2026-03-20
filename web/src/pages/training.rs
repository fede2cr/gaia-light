//! Training candidates page — browse high-confidence animal detections
//! that were not classified by any species model.
//!
//! These crops form a curated pool for future classifier training.

use leptos::prelude::*;
use leptos::prelude::{
    signal, use_context, ElementChild, IntoView, Resource, ServerFnError,
    Suspense,
};

use crate::model::TrainingCandidate;

// ── Server function ──────────────────────────────────────────────────────

#[server(prefix = "/api")]
pub async fn get_training_candidates(
    page: u32,
    per_page: u32,
) -> Result<TrainingPage, ServerFnError> {
    let state = use_context::<crate::app::AppState>()
        .ok_or_else(|| ServerFnError::new("Missing AppState"))?;

    let (rows, total) = crate::server::db::training_candidates(
        &state.db_path,
        per_page,
        page.saturating_sub(1) * per_page,
    )
    .await
    .map_err(|e| ServerFnError::new(format!("DB error: {e}")))?;

    Ok(TrainingPage { rows, total })
}

/// Server function response with paginated training candidates.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrainingPage {
    pub rows: Vec<TrainingCandidate>,
    pub total: u64,
}

// ── Component ────────────────────────────────────────────────────────────

const PER_PAGE: u32 = 48;

/// Browseable grid of unclassified animal crops for model training.
#[component]
pub fn Training() -> impl IntoView {
    let (page, set_page) = signal(1u32);

    let data = Resource::new(
        move || page.get(),
        move |p| async move { get_training_candidates(p, PER_PAGE).await },
    );

    view! {
        <div class="training-page">
            <h1>"Training Candidates"</h1>
            <p class="info">
                "High-confidence animal detections with no species classification. "
                "Use these crops to train or fine-tune future species models."
            </p>

            <Suspense fallback=move || view! { <p class="loading">"Loading\u{2026}"</p> }>
                {move || data.get().map(|res| match res {
                    Ok(tp) => {
                        let total_pages = ((tp.total as u32).max(1) + PER_PAGE - 1) / PER_PAGE;

                        view! {
                            <div class="training-stats">
                                <span class="stat-count">{tp.total}" candidate(s)"</span>
                                <span class="stat-page">"Page " {page.get()} " of " {total_pages}</span>
                            </div>

                            <div class="training-grid">
                                {tp.rows.into_iter().map(|c| {
                                    let conf = c.confidence_pct();
                                    let ts = c.timestamp.clone();
                                    let clip = c.clip_filename.clone();
                                    let crop = c.crop_url();

                                    view! {
                                        <div class="training-card">
                                            {match crop {
                                                Some(url) => view! {
                                                    <img class="training-crop" src={url} alt="crop" loading="lazy"/>
                                                }.into_any(),
                                                None => view! {
                                                    <div class="training-crop placeholder">"No crop"</div>
                                                }.into_any(),
                                            }}
                                            <div class="training-meta">
                                                <span class="confidence high">{conf}</span>
                                                <span class="training-ts">{ts}</span>
                                                <span class="training-clip">{clip}</span>
                                            </div>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>

                            <div class="pagination">
                                <button
                                    class="btn"
                                    disabled=move || page.get() <= 1
                                    on:click=move |_| set_page.update(|p| *p = p.saturating_sub(1))
                                >"\u{25c0} Prev"</button>
                                <span class="page-number">{page.get()}" / "{total_pages}</span>
                                <button
                                    class="btn"
                                    disabled=move || page.get() >= total_pages
                                    on:click=move |_| set_page.update(|p| *p += 1)
                                >"Next \u{25b6}"</button>
                            </div>
                        }.into_any()
                    }
                    Err(e) => view! {
                        <p class="error">"Error: " {e.to_string()}</p>
                    }.into_any(),
                })}
            </Suspense>
        </div>
    }
}
