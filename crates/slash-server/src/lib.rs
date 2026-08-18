//! The slash-server control plane as a library crate.
//!
//! Modules are shared between the `slash-server` binary (`main.rs`) and the
//! integration tests under `tests/`, so every module is declared here as
//! `pub mod` and the entrypoint (`run`) drives the router. The binary target
//! is a thin wrapper around `slash_server::run`.

pub mod access_tokens;
pub mod admin;
pub mod auth;
pub mod catalog;
pub mod collectors;
pub mod config;
pub mod correlation;
pub mod db;
pub mod deliveries;
pub mod flaky;
pub mod github_oauth;
mod identity;
pub mod ingestion;
pub mod installations;
pub mod invocations;
pub mod junit;
pub mod metrics;
pub mod pipeline;
pub mod sweeper;
pub mod test_engine;
pub mod test_engine_api;
pub mod test_support;
pub mod userapi;
pub mod webhook;
pub mod worker;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
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
    pub admin_secret: Option<admin::AdminSecret>,
    /// App-JWT client used only by the explicit admin installation refresh.
    /// Optional in test states that never exercise the upstream operation.
    pub github_app: Option<Arc<slash_github::GithubApp>>,
    /// Root directory of the built SPA (`web/dist`).
    pub web_dir: Arc<str>,
    /// GitHub App user-auth config. `None` when it is not configured.
    pub github_oauth: Option<github_oauth::OauthState>,
}

/// Boots the server: configuration, database, metrics, GitHub App, then the
/// router. The binary entrypoint (`main.rs`) is a thin `#[tokio::main]`
/// wrapper around this.
pub async fn run() {
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
    let github_oauth = config.github_client_id.as_ref().map(|client_id| {
        github_oauth::OauthState::new(
            Arc::from(client_id.as_str()),
            Arc::from(config.github_client_secret.as_deref().unwrap_or_default()),
            Arc::from(config.github_base_url.as_deref().unwrap_or_default()),
            auth::AuthSecret(Arc::from(config.auth_secret.as_str())),
        )
    });
    let state = AppState {
        pool: pool.clone(),
        metrics: metrics.clone(),
        webhook_secret: Arc::from(config.webhook_secret.as_str()),
        auth_secret: auth::AuthSecret(Arc::from(config.auth_secret.as_str())),
        admin_secret: config
            .admin_secret
            .as_deref()
            .map(|secret| admin::AdminSecret(Arc::from(secret))),
        github_app: Some(github_app.clone()),
        web_dir: Arc::from(web_dir.as_str()),
        github_oauth,
    };

    tokio::spawn(worker::run(
        pool.clone(),
        github_app.clone(),
        metrics.clone(),
        WORKER_POLL_INTERVAL,
    ));
    tokio::spawn(sweeper::run(
        pool.clone(),
        github_app.clone(),
        sweeper::SweeperConfig::default(),
        metrics,
    ));

    let app = Router::new()
        .route("/webhook", post(webhook::handle_webhook))
        .route("/v1/test-engine/upload", post(ingestion::handle_upload))
        .route(
            "/v1/test-engine/upload/cargo",
            post(ingestion::handle_cargo_upload),
        )
        .route(
            "/v1/test-engine/upload/vitest",
            post(ingestion::handle_vitest_upload),
        )
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
        .route(
            "/api/access-tokens",
            get(access_tokens::list_tokens).post(access_tokens::issue_token),
        )
        .route(
            "/api/access-tokens/{id}",
            delete(access_tokens::revoke_token),
        )
        .route("/api/admin/auth/login", post(admin::login))
        .route("/api/admin/auth/logout", post(admin::logout))
        .route("/api/admin/auth/session", get(admin::session))
        .route("/api/admin/overview", get(admin::overview))
        .route("/api/admin/installations", get(admin::installations))
        .route(
            "/api/admin/installations/refresh",
            post(admin::refresh_installations),
        )
        .route("/api/admin/deliveries", get(admin::deliveries))
        .route("/api/admin/deliveries/{guid}", get(admin::delivery_detail))
        .route("/api/admin/invocations", get(admin::invocations))
        .route(
            "/api/auth/github/sign-in",
            get(github_oauth::start_github_sign_in),
        )
        .route(
            "/api/auth/github/callback",
            get(github_oauth::handle_github_callback),
        )
        .route(
            "/api/auth/github/connect",
            post(github_oauth::start_github_connect),
        )
        .route("/api/teams", post(userapi::create_team))
        .route(
            "/api/test-engine/suites",
            get(test_engine_api::list_suites).post(test_engine_api::create_suite),
        )
        .route(
            "/api/test-engine/suites/{id}/tests",
            get(test_engine_api::list_tests),
        )
        .route(
            "/api/test-engine/tests/{id}/executions",
            get(test_engine_api::list_test_executions),
        )
        .route(
            "/api/test-engine/tests/{id}/state",
            axum::routing::patch(test_engine_api::set_test_state),
        )
        .route(
            "/api/test-engine/suites/{id}/tokens",
            get(test_engine_api::get_token).post(test_engine_api::issue_token),
        )
        .route(
            "/api/test-engine/suites/{id}/tokens/revoke",
            post(test_engine_api::revoke_token),
        )
        .route("/admin", get(admin_page))
        .route("/admin/{*path}", get(admin_page))
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
async fn web_fallback(
    uri: axum::http::Uri,
    State(state): State<AppState>,
) -> axum::response::Response {
    let relative = uri.path().trim_start_matches('/');
    // Unknown `/api/*` paths are a Web-API miss, not a SPA route: return a
    // JSON 404 rather than serving the SPA shell.
    if relative.starts_with("api/") || relative == "api" {
        return (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error":"not found"})),
        )
            .into_response();
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
                    resp.headers_mut()
                        .insert(axum::http::header::CONTENT_TYPE, v);
                }
                resp
            }
            Err(_) => index_response(&state.web_dir),
        };
    }
    index_response(&state.web_dir)
}

async fn admin_page(State(state): State<AppState>) -> axum::response::Response {
    if !admin::enabled(&state) {
        return (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error":"not found"})),
        )
            .into_response();
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
