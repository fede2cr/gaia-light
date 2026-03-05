//! Root Leptos application component.

use leptos::*;
use leptos_meta::*;
use leptos_router::*;

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Stylesheet id="leptos" href="/pkg/gaia-light-web.css"/>
        <Title text="Gaia Light"/>
        <Router>
            <main>
                <Routes>
                    <Route path="/" view=HomePage/>
                </Routes>
            </main>
        </Router>
    }
}

#[component]
fn HomePage() -> impl IntoView {
    view! {
        <h1>"Gaia Light"</h1>
        <p>"Camera-trap wildlife monitoring dashboard."</p>
    }
}
