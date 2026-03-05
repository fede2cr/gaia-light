//! Root Leptos application component with routing.

use leptos::*;
use leptos_meta::*;
use leptos_router::*;

use crate::components::nav::Nav;
use crate::pages::{
    detections::Detections,
    home::Home,
    settings::Settings,
    species::Species,
};

/// Server-side application state, provided as Leptos context for server functions.
#[derive(Clone, Debug)]
#[cfg(feature = "ssr")]
pub struct AppState {
    pub db_path: std::path::PathBuf,
    pub data_dir: std::path::PathBuf,
    pub leptos_options: leptos::LeptosOptions,
}

/// Dummy state for the client – the type must exist so server functions can
/// reference it, but it is never constructed on WASM.
#[derive(Clone, Debug)]
#[cfg(not(feature = "ssr"))]
pub struct AppState;

/// The root `<App/>` component.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Stylesheet id="leptos" href="/pkg/gaia-light-web.css"/>
        <Title text="Gaia Light – Wildlife Monitor"/>
        <Meta name="viewport" content="width=device-width, initial-scale=1"/>
        <Meta name="description" content="Camera-trap wildlife monitoring dashboard"/>

        <Router>
            <Nav/>
            <main class="main-content">
                <Routes>
                    <Route path="/" view=Home/>
                    <Route path="/detections" view=Detections/>
                    <Route path="/species" view=Species/>
                    <Route path="/settings" view=Settings/>
                </Routes>
            </main>
        </Router>
    }
}
