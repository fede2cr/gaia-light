//! Server entry-point -- Axum + Leptos SSR.

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::Router;
    use leptos::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use tower_http::services::ServeDir;

    use gaia_light_web::app::App;

    // ── Tracing ──────────────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gaia_light_web=info,tower_http=info".into()),
        )
        .init();

    // ── Configuration ────────────────────────────────────────────────────
    let conf = get_configuration(None).await.unwrap();
    let leptos_options = conf.leptos_options.clone();
    let addr = leptos_options.site_addr;
    let routes = generate_route_list(App);

    let app = Router::new()
        .leptos_routes(&leptos_options, routes, App)
        .fallback_service(ServeDir::new(&leptos_options.site_root))
        .with_state(leptos_options);

    tracing::info!("Gaia Light Web listening on {addr}");
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
