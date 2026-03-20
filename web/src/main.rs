//! Server entry-point -- Axum + Leptos SSR.

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::Router;
    use leptos::prelude::*;
    use leptos::prelude::ElementChild;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use tower_http::services::ServeDir;

    use gaia_light_web::app::{shell, App, AppState};

    // ── Tracing ──────────────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gaia_light_web=info,tower_http=info".into()),
        )
        .init();

    if std::env::var("RUST_LOG").map_or(false, |v| v.contains("debug")) {
        tracing::info!("🔍 Debug logging ENABLED (RUST_LOG={})", std::env::var("RUST_LOG").unwrap_or_default());
    }

    // ── Configuration ────────────────────────────────────────────────────
    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options.clone();
    let addr = leptos_options.site_addr;
    let site_root = leptos_options.site_root.clone();

    let db_path = std::path::PathBuf::from(
        std::env::var("GAIA_DB_PATH").unwrap_or_else(|_| "/data/light.db".into()),
    );
    let data_dir = std::path::PathBuf::from(
        std::env::var("GAIA_DATA_DIR").unwrap_or_else(|_| "/data".into()),
    );

    tracing::info!("Database: {}", db_path.display());
    tracing::info!("Data dir: {}", data_dir.display());

    let data_dir_str = data_dir.to_string_lossy().to_string();

    let state = AppState {
        db_path,
        data_dir: data_dir.clone(),
        leptos_options: leptos_options.clone(),
    };

    // ── Routes ───────────────────────────────────────────────────────────
    let routes = generate_route_list(App);

    let app = Router::new()
        .leptos_routes_with_context(
            &leptos_options,
            routes,
            {
                let state = state.clone();
                move || {
                    provide_context(state.clone());
                }
            },
            {
                let options = leptos_options.clone();
                move || shell(options.clone())
            },
        )
        // Serve static assets (WASM bundle, CSS, images, etc.)
        .nest_service(
            "/pkg",
            ServeDir::new(format!("{}/pkg", site_root)),
        )
        // Serve extracted crop images
        .nest_service(
            "/extracted",
            ServeDir::new(format!("{}/Extracted", &data_dir_str)),
        )
        // Serve processing preview frames
        .nest_service(
            "/preview",
            ServeDir::new(&data_dir_str),
        )
        // Serve live status files
        .nest_service(
            "/live",
            ServeDir::new(&data_dir_str),
        )
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options);

    tracing::info!("Gaia Light Web listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}

#[cfg(not(feature = "ssr"))]
pub fn main() {
    // Hydrate entry-point is in lib.rs; this stub prevents
    // `cargo build` without features from failing.
}
