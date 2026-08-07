mod catalog;
mod config;
mod correlation;
mod db;
mod deliveries;
mod invocations;
mod metrics;
mod pipeline;
mod sweeper;
#[cfg(test)]
mod test_support;
mod webhook;
mod worker;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::routing::{get, post};
use sqlx::PgPool;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use config::ServerConfig;
use metrics::Metrics;

/// Generously over any real GitHub webhook body (spec §7.3's explicit
/// request-body limit).
const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;
const LISTEN_ADDR: &str = "0.0.0.0:8080";
const WORKER_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub metrics: Arc<Metrics>,
    pub webhook_secret: Arc<str>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = match ServerConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "invalid configuration");
            std::process::exit(1);
        }
    };

    let pool = match db::connect(&config.database_url).await {
        Ok(pool) => pool,
        Err(error) => {
            tracing::error!(%error, "failed to connect to the database");
            std::process::exit(1);
        }
    };

    if let Err(error) = db::migrate(&pool).await {
        tracing::error!(%error, "failed to run migrations");
        std::process::exit(1);
    }

    let metrics = match Metrics::new() {
        Ok(metrics) => Arc::new(metrics),
        Err(error) => {
            tracing::error!(%error, "failed to register metrics");
            std::process::exit(1);
        }
    };

    // Fails fast at startup on a bad App ID/key, rather than on the first
    // webhook that needs to dispatch.
    let github_app = match load_github_app(&config) {
        Ok(app) => Arc::new(app),
        Err(error) => {
            tracing::error!(%error, "invalid GitHub App credentials");
            std::process::exit(1);
        }
    };

    let state = AppState {
        pool: pool.clone(),
        metrics: metrics.clone(),
        webhook_secret: Arc::from(config.webhook_secret.as_str()),
    };

    tokio::spawn(worker::run(
        pool.clone(),
        github_app.clone(),
        metrics.clone(),
        WORKER_POLL_INTERVAL,
    ));
    tokio::spawn(sweeper::run(
        pool,
        github_app,
        sweeper::SweeperConfig::default(),
        metrics,
    ));

    let app = Router::new()
        .route("/webhook", post(webhook::handle_webhook))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics_handler))
        .layer(CatchPanicLayer::new())
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = match tokio::net::TcpListener::bind(LISTEN_ADDR).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(%error, address = LISTEN_ADDR, "failed to bind listener");
            std::process::exit(1);
        }
    };

    tracing::info!(address = LISTEN_ADDR, "listening");

    if let Err(error) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!(%error, "server exited with an error");
        std::process::exit(1);
    }
}

fn load_github_app(config: &ServerConfig) -> Result<slash_github::GithubApp, String> {
    let pem = std::fs::read(&config.github_private_key_path)
        .map_err(|e| format!("reading {}: {e}", config.github_private_key_path.display()))?;
    slash_github::GithubApp::new(config.github_app_id, &pem).map_err(|e| e.to_string())
}

async fn healthz() -> &'static str {
    "ok"
}

async fn metrics_handler(State(state): State<AppState>) -> String {
    state.metrics.render()
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
