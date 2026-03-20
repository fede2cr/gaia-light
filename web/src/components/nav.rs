//! Top navigation bar component.

use leptos::prelude::*;
use leptos::prelude::{ElementChild, IntoView};

/// Site-wide navigation bar.
#[component]
pub fn Nav() -> impl IntoView {
    view! {
        <nav class="nav-bar">
            <div class="nav-brand">
                <a href="/" class="nav-logo">"📷 Gaia Light"</a>
            </div>
            <div class="nav-links">
                <a href="/" class="nav-link">"Live Feed"</a>
                <a href="/detections" class="nav-link">"Detections"</a>
                <a href="/species" class="nav-link">"Species"</a>
                <a href="/people" class="nav-link">"People"</a>
                <a href="/settings" class="nav-link">"Settings"</a>
                <a href="/training" class="nav-link">"Training"</a>
            </div>
        </nav>
    }
}
