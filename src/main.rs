mod auth;
mod config;
mod jobs;
mod routes;
mod subtitles;
mod ytdlp;

use std::{sync::Arc, time::Duration};

use axum::{
    middleware,
    routing::{delete, get, post},
    Router,
};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::{config::Config, jobs::Jobs, ytdlp::YtDlp};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub ytdlp: YtDlp,
    pub jobs: Jobs,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Arc::new(Config::load());
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    tokio::fs::create_dir_all(&cfg.download_dir).await?;
    let _ = tokio::fs::remove_dir_all(cfg.download_dir.join(".tmp")).await;

    let ytdlp = YtDlp::new(cfg.clone());
    if cfg.auto_update {
        ytdlp.self_update().await;
    }
    match ytdlp.version().await {
        Some(v) => tracing::info!("yt-dlp {v}"),
        None => tracing::error!("yt-dlp not found at '{}'", cfg.ytdlp_path),
    }
    if cfg.app_password.is_none() {
        tracing::warn!("APP_PASSWORD is not set: the app is open to anyone who can reach it");
    }

    let jobs = Jobs::new(cfg.clone(), ytdlp.clone());
    {
        let jobs = jobs.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(600));
            loop {
                tick.tick().await;
                jobs.sweep().await;
            }
        });
    }

    let state = AppState { cfg: cfg.clone(), ytdlp, jobs };

    let protected = Router::new()
        .route("/", get(routes::index))
        .route("/app.js", get(routes::app_js))
        .route("/api/status", get(routes::status))
        .route("/api/info", get(routes::info))
        .route("/api/transcript", get(routes::transcript))
        .route("/api/jobs", get(routes::list_jobs).post(routes::create_job))
        .route("/api/jobs/{id}", delete(routes::delete_job))
        .route("/api/events", get(routes::events))
        .route("/files/{id}", get(routes::download_file))
        .route("/logout", post(auth::logout))
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_auth));

    let app = Router::new()
        .route("/healthz", get(routes::healthz))
        .route("/style.css", get(routes::style_css))
        .route("/login", get(auth::login_page).post(auth::login))
        .merge(protected)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = format!("0.0.0.0:{}", cfg.port);
    tracing::info!("listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    // No graceful drain: open SSE streams would hold shutdown until Docker's
    // kill timeout. Running yt-dlp children die with us (kill_on_drop).
    tokio::select! {
        r = axum::serve(listener, app) => r?,
        _ = shutdown_signal() => tracing::info!("shutting down"),
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
