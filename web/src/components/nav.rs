//! Top navigation bar component.

use leptos::*;
use leptos_router::*;

/// Site-wide navigation bar.
#[component]
pub fn Nav() -> impl IntoView {
    view! {
        <nav class="nav-bar">
            <div class="nav-brand">
                <A href="/" class="nav-logo">"📷 Gaia Light"</A>
            </div>
            <div class="nav-links">
                <A href="/" class="nav-link">"Live Feed"</A>
                <A href="/detections" class="nav-link">"Detections"</A>
                <A href="/species" class="nav-link">"Species"</A>
                <A href="/settings" class="nav-link">"Settings"</A>
                <A href="/training" class="nav-link">"Training"</A>
            </div>
        </nav>
    }
}
