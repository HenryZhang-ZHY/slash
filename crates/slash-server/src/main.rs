mod auth;
mod catalog;
mod config;
mod correlation;
mod db;
mod deliveries;
mod flaky;
mod ingestion;
mod invocations;
mod metrics;
mod pipeline;
mod sweeper;
mod test_engine;
#[cfg(test)]
mod test_support;
mod userapi;
mod webhook;
mod worker;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::response::IntoResponse;
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
/// Where the onboarding SPA's build output lives (relative to the server's
/// working directory). Overridable via `SLASH_WEB_DIR`.
const DEFAULT_WEB_DIR: &str = "web/dist";

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub metrics: Arc<Metrics>,
    pub webhook_secret: Arc<str>,
    pub auth_secret: auth::AuthSecret,
    /// Root directory of the built SPA (`web/dist`).
    pub web_dir: Arc<str>,
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

    let web_dir = std::env::var("SLASH_WEB_DIR").unwrap_or_else(|_| DEFAULT_WEB_DIR.to_string());
    let state = AppState {
        pool: pool.clone(),
        metrics: metrics.clone(),
        webhook_secret: Arc::from(config.webhook_secret.as_str()),
        auth_secret: auth::AuthSecret(Arc::from(config.auth_secret.as_str())),
        web_dir: Arc::from(web_dir.as_str()),
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
        .route("/v1/test-engine/upload", post(ingestion::handle_upload))
        .route(
            "/v1/test-engine/quarantined",
            get(ingestion::handle_quarantined),
        )
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics_handler))
        // User onboarding API (org/user management lane, 1.0 MVP)
        .route("/api/auth/register", post(userapi::register))
        .route("/api/auth/login", post(userapi::login))
        .route("/api/auth/logout", post(userapi::logout))
        .route("/api/auth/me", get(userapi::me))
        .route("/api/teams", post(userapi::create_team))
        // Serve the React SPA: static files under /assets and the root
        // favicon/icons come from the dist dir; everything else falls back
        // to `index.html` (history-API routing) with a 200 status.
        .fallback(web_fallback)
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

/// SPA + static asset handler. Serves real files under `<dist>/<path>`
/// (e.g. `/assets/index-*.js`, `/favicon.svg`) when they exist; otherwise
/// returns `index.html` with a 200 so client-side routes (`/login`,
/// `/onboarding`) work on refresh. API routes are handled by the Router
/// above, so this never serves the SPA shell in place of an API 404.
async fn web_fallback(uri: axum::http::Uri, State(state): State<AppState>) -> axum::response::Response {
    let relative = uri.path().trim_start_matches('/');
    // Unknown `/api/*` paths are a Web-API miss, not a SPA route: return a
    // JSON 404 rather than serving the SPA shell.
    if relative.starts_with("api/") || relative == "api" {
        return (axum::http::StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error":"not found"}))).into_response();
    }
    if relative.is_empty() {
        return index_response(&state.web_dir);
    }
    // Refuse path traversal outside the dist root.
    let candidate = std::path::Path::new(state.web_dir.as_ref()).join(relative);
    if !candidate.starts_with(state.web_dir.as_ref()) {
        return index_response(&state.web_dir);
    }
    if let Ok(meta) = tokio::fs::metadata(&candidate).await
        && meta.is_file()
    {
        return match tokio::fs::read(&candidate).await {
            Ok(bytes) => {
                let content_type = content_type_for(&candidate);
                let mut resp = (axum::http::StatusCode::OK, bytes).into_response();
                if let Some(ct) = content_type
                    && let Ok(v) = axum::http::HeaderValue::from_str(ct)
                {
                    resp.headers_mut().insert(axum::http::header::CONTENT_TYPE, v);
                }
                resp
            }
            Err(_) => index_response(&state.web_dir),
        };
    }
    index_response(&state.web_dir)
}

/// Minimal content-type inference for the handful of static assets the SPA
/// emits (hashed CSS/JS, SVG favicon). Returning a qualified body as
/// `text/html` otherwise would let a browser MIME-sniff attacker-controlled
/// paths.
fn content_type_for(path: &std::path::Path) -> Option<&'static str> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("css") => Some("text/css; charset=utf-8"),
        Some("js") | Some("mjs") => Some("text/javascript; charset=utf-8"),
        Some("svg") => Some("image/svg+xml"),
        Some("json") => Some("application/json"),
        _ => None,
    }
}

fn index_response(web_dir: &str) -> axum::response::Response {
    let index = format!("{web_dir}/index.html");
    match std::fs::read(index) {
        Ok(bytes) => (axum::http::StatusCode::OK, axum::response::Html(bytes)).into_response(),
        Err(_) => axum::response::IntoResponse::into_response((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "web build not found (run `npm run build` in web/)",
        )),
    }
}

async fn metrics_handler(State(state): State<AppState>) -> String {
    state.metrics.render()
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
