//! HTTP server bootstrap.
//!
//! Binds to `host:port`, registers the read-only routes from `handlers`,
//! and serves forever until the process is signalled. Logs the bound
//! address so the user knows where to point their browser / Vite dev proxy.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{routing::get, Router};
use tower_http::cors::CorsLayer;

use crate::core::skill_store::SkillStore;
use crate::web::handlers;
use crate::web::write_handlers;

/// Bind and serve. Returns only on failure; Ctrl+C / SIGTERM kills the
/// tokio runtime the caller is expected to be running under (the
/// `skills-manager-web` bin uses `#[tokio::main]`).
pub async fn run_server(host: &str, port: u16, store: Arc<SkillStore>) -> Result<()> {
    let app = Router::new()
        .route("/api/health", get(handlers::health))
        .route("/api/tools", get(handlers::list_tools))
        .route("/api/skills", get(handlers::list_skills))
        .route("/api/skills/{id}", get(handlers::get_skill))
        .route("/api/presets", get(handlers::list_presets))
        .route("/api/presets/active", get(handlers::get_active_preset))
        // Write surface (MVP batch 2): install / deploy / undeploy / remove / tag.
        // Mounted under the same Router so the same axum state + CORS layer
        // apply uniformly. Auth-free by design — see write_handlers.rs header.
        .merge(write_handlers::write_routes())
        .with_state(store)
        // CORS for the Vite dev server on :1420 (the project's Tauri dev
        // port). Tighten this if the server is ever exposed beyond localhost.
        .layer(CorsLayer::permissive());

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("invalid bind address {host}:{port}"))?;

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    log::info!(
        "skills-manager-web listening on http://{addr} (routes: /api/health /api/tools /api/skills /api/skills/:id /api/presets /api/presets/active)"
    );

    axum::serve(listener, app)
        .await
        .with_context(|| "web server crashed")?;
    Ok(())
}