//! Root Leptos application component with routing.

use leptos::prelude::*;
use leptos::prelude::{ElementChild, IntoView};
use leptos_meta::*;
use leptos_router::{
    components::{Route, Router, Routes},
    StaticSegment,
};

use crate::components::nav::Nav;
use crate::pages::{
    detections::Detections,
    home::Home,
    people::People,
    settings::Settings,
    species::Species,
    training::Training,
};

/// Server-side application state, provided as Leptos context for server functions.
#[derive(Clone, Debug)]
#[cfg(feature = "ssr")]
pub struct AppState {
    pub db_path: std::path::PathBuf,
    pub data_dir: std::path::PathBuf,
    pub leptos_options: leptos::config::LeptosOptions,
}

/// Dummy state for the client – the type must exist so server functions can
/// reference it, but it is never constructed on WASM.
#[derive(Clone, Debug)]
#[cfg(not(feature = "ssr"))]
pub struct AppState;

/// Shell function providing the outer HTML document structure.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <meta name="description" content="Camera-trap wildlife monitoring dashboard"/>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>
                <link rel="stylesheet" id="leptos" href="/pkg/gaia-light-web.css"/>
                <Title text="Gaia Light – Wildlife Monitor"/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

/// The root `<App/>` component.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Router>
            <Nav/>
            <main class="main-content">
                <Routes fallback=|| "Not found.">
                    <Route path=StaticSegment("") view=Home/>
                    <Route path=StaticSegment("detections") view=Detections/>
                    <Route path=StaticSegment("species") view=Species/>
                    <Route path=StaticSegment("people") view=People/>
                    <Route path=StaticSegment("settings") view=Settings/>
                    <Route path=StaticSegment("training") view=Training/>
                </Routes>
            </main>
        </Router>
    }
}
