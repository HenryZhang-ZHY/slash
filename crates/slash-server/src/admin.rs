//! Instance-admin authentication and read-only operations console API.
//!
//! Admin access is deliberately separate from Slash user accounts. The
//! optional file-backed secret enables the surface; when it is absent every
//! `/admin` and `/api/admin` route returns 404.

use std::sync::Arc;

use axum::Json;
use axum::extract::{FromRequestParts, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;

use crate::AppState;

type HmacSha256 = Hmac<Sha256>;

const ADMIN_COOKIE: &str = "slash_admin_session";
const ADMIN_SESSION_TTL_SECS: u64 = 20 * 60;

#[derive(Clone)]
pub struct AdminSecret(pub Arc<str>);

#[derive(Serialize, Deserialize)]
struct AdminClaims {
    exp: u64,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    secret: String,
}

pub async fn login(State(state): State<AppState>, Json(request): Json<LoginRequest>) -> Response {
    let Some(secret) = state.admin_secret.as_ref() else {
        return not_found();
    };
    if !constant_time_eq(secret.0.as_bytes(), request.secret.as_bytes()) {
        return api_error(StatusCode::UNAUTHORIZED, "invalid admin secret");
    }

    let token = match sign_token(secret) {
        Ok(token) => token,
        Err(()) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "internal auth failure"),
    };
    let mut response = Json(json!({ "authenticated": true })).into_response();
    if let Ok(cookie) = HeaderValue::from_str(&set_cookie_value(&token)) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
        response
    } else {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "internal auth failure")
    }
}

pub async fn logout(admin: AdminSession) -> Response {
    let _ = admin;
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Ok(cookie) = HeaderValue::from_str(&clear_cookie_value()) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

pub async fn session(admin: AdminSession) -> Json<serde_json::Value> {
    let _ = admin;
    Json(json!({ "authenticated": true }))
}

#[derive(Clone, Copy)]
pub struct AdminSession;

impl FromRequestParts<AppState> for AdminSession {
    type Rejection = Response;

    fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let cookie = parts
            .headers
            .get(header::COOKIE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let secret = state.admin_secret.clone();
        async move {
            let Some(secret) = secret else {
                return Err(not_found());
            };
            let token = cookie_value(cookie.as_deref()).ok_or_else(|| {
                api_error(StatusCode::UNAUTHORIZED, "admin authentication required")
            })?;
            verify_token(&secret, token).map_err(|_| {
                api_error(StatusCode::UNAUTHORIZED, "admin session expired or invalid")
            })?;
            Ok(Self)
        }
    }
}

pub fn enabled(state: &AppState) -> bool {
    state.admin_secret.is_some()
}

fn sign_token(secret: &AdminSecret) -> Result<String, ()> {
    let claims = AdminClaims {
        exp: now_secs().saturating_add(ADMIN_SESSION_TTL_SECS),
    };
    let json = serde_json::to_vec(&claims).map_err(|_| ())?;
    let payload = URL_SAFE_NO_PAD.encode(json);
    let signature = hmac(secret, payload.as_bytes());
    Ok(format!("{payload}.{signature}"))
}

fn verify_token(secret: &AdminSecret, token: &str) -> Result<(), ()> {
    let (payload, signature) = token.split_once('.').ok_or(())?;
    let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| ())?;
    let expected = URL_SAFE_NO_PAD
        .decode(hmac(secret, payload.as_bytes()))
        .map_err(|_| ())?;
    if !constant_time_eq(&signature, &expected) {
        return Err(());
    }
    let claims: AdminClaims =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).map_err(|_| ())?)
            .map_err(|_| ())?;
    if claims.exp < now_secs() {
        return Err(());
    }
    Ok(())
}

fn hmac(secret: &AdminSecret, value: &[u8]) -> String {
    #[allow(clippy::expect_used)]
    let mut mac = HmacSha256::new_from_slice(secret.0.as_bytes()).expect("HMAC accepts any key");
    mac.update(value);
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn cookie_value(header_value: Option<&str>) -> Option<&str> {
    header_value?.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == ADMIN_COOKIE).then_some(value)
    })
}

fn set_cookie_value(token: &str) -> String {
    format!(
        "{ADMIN_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={ADMIN_SESSION_TTL_SECS}"
    )
}

fn clear_cookie_value() -> String {
    format!("{ADMIN_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn not_found() -> Response {
    api_error(StatusCode::NOT_FOUND, "not found")
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn secret(value: &str) -> AdminSecret {
        AdminSecret(Arc::from(value))
    }

    #[test]
    fn token_round_trip_and_secret_rotation_invalidates_it() {
        let token = sign_token(&secret("first")).unwrap();
        assert!(verify_token(&secret("first"), &token).is_ok());
        assert!(verify_token(&secret("second"), &token).is_err());
    }

    #[test]
    fn cookie_is_http_only_strict_and_short_lived() {
        let cookie = set_cookie_value("token");
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Max-Age=1200"));
        assert_eq!(
            cookie_value(Some("other=1; slash_admin_session=token")),
            Some("token")
        );
    }

    #[test]
    fn token_tampering_is_rejected() {
        let mut token = sign_token(&secret("secret")).unwrap();
        token.push('x');
        assert!(verify_token(&secret("secret"), &token).is_err());
    }
}
