//! User onboarding authN (org/user management lane, 1.0 MVP).
//!
//! account/password authN owned by slash (not GitHub-coupled) for the
//! onboarding Web App. Password hashing via Argon2 (PHC string, stored in
//! `users.password_hash`); sessions are stateless HMAC-SHA256-signed tokens
//! carried in an HttpOnly cookie (`slash_session`). Stateless tokens are a
//! deliberate MVP choice: zero session-table overhead
//! and fail-closed discipline; revocation can be added additively later.

use std::sync::Arc;

use argon2::password_hash::{SaltString, rand_core::OsRng};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// How long a session token is valid. Tokens carry an expiry inside the
/// signed payload, so a stolen cookie cannot outlive this window.
const TOKEN_TTL_SECS: u64 = 7 * 24 * 60 * 60; // 7 days
const SESSION_COOKIE: &str = "slash_session";

#[derive(Debug, Clone)]
pub struct AuthSecret(pub Arc<str>);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionClaims {
    pub sub: uuid::Uuid, // user id
    pub exp: u64,        // unix seconds
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    // Reserved for the richer auth surface (currently the onboarding MVP
    // responds with generic 401 messages instead of surfacing these).
    #[allow(dead_code)]
    #[error("invalid credential")]
    InvalidCredential,
    #[error("invalid session token")]
    InvalidToken,
    #[error("expired session token")]
    ExpiredToken,
    #[allow(dead_code)]
    #[error("session cookie missing")]
    MissingToken,
    #[error("failed to serialize token")]
    Encode,
    #[error("internal auth failure")]
    Internal,
}

/// Hash a password with a freshly generated salt (Argon2id, default params).
/// Output is the PHC string stored verbatim in `users.password_hash`.
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let params = argon2::Params::default();
    // argon2 0.5: `Argon2::new` returns `Self` directly (Params is already
    // validated). Argon2id with default params.
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| AuthError::Internal)
}

/// Verify a plaintext password against a stored PHC string. Constant-time
/// via Argon2 verification on the stored hash.
pub fn verify_password(password: &str, phc_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Sign a session token for `user_id`. Format:
/// `b64url(payload_json).b64url(hmac)` — the payload is not secret but is
/// integrity-protected; a wrong key makes verification fail closed.
pub fn sign_token(secret: &AuthSecret, user_id: uuid::Uuid) -> Result<String, AuthError> {
    let claims = SessionClaims {
        sub: user_id,
        exp: now_secs() + TOKEN_TTL_SECS,
    };
    let payload_json = serde_json::to_string(&claims).map_err(|_| AuthError::Encode)?;
    let payload = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
    let mac = hmac(secret, payload.as_bytes());
    Ok(format!("{payload}.{mac}"))
}

/// Verify a session token and return its `user_id`. Fails closed on any
/// tampering, expiry, or signature mismatch.
pub fn verify_token(secret: &AuthSecret, token: &str) -> Result<uuid::Uuid, AuthError> {
    let (payload, mac_str) = token.split_once('.').ok_or(AuthError::InvalidToken)?;
    // Constant-time compare on the MAC.
    let expected = hmac(secret, payload.as_bytes());
    let actual_bytes = URL_SAFE_NO_PAD
        .decode(mac_str)
        .map_err(|_| AuthError::InvalidToken)?;
    let expected_bytes = URL_SAFE_NO_PAD
        .decode(&expected)
        .map_err(|_| AuthError::Internal)?;
    if !constant_time_eq(&expected_bytes, &actual_bytes) {
        return Err(AuthError::InvalidToken);
    }
    let payload_json = match URL_SAFE_NO_PAD.decode(payload) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return Err(AuthError::InvalidToken),
        },
        Err(_) => return Err(AuthError::InvalidToken),
    };
    let claims: SessionClaims =
        serde_json::from_str(&payload_json).map_err(|_| AuthError::InvalidToken)?;
    if claims.exp < now_secs() {
        return Err(AuthError::ExpiredToken);
    }
    Ok(claims.sub)
}

/// Build a `Set-Cookie` value for the session cookie.
pub fn set_cookie_value(token: &str) -> String {
    format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={TOKEN_TTL_SECS}")
}

/// Build a `Set-Cookie` value that clears the session cookie (logout).
pub fn clear_cookie_value() -> String {
    format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

/// Extract the session token from a raw `Cookie` header value.
pub fn session_token_from_header(cookie_header: Option<&str>) -> Option<String> {
    for pair in cookie_header?.split(';') {
        let (k, v) = pair.trim().split_once('=')?;
        if k.trim() == SESSION_COOKIE {
            return Some(v.trim().to_string());
        }
    }
    None
}

fn hmac(secret: &AuthSecret, data: &[u8]) -> String {
    // HMAC accepts keys of any length, so this is infallible (clippy
    // expect-used is satisfied by the unvalidated, constant-length key auth
    // already performed above).
    #[allow(clippy::expect_used)]
    let mut mac = HmacSha256::new_from_slice(secret.0.as_bytes()).expect("HMAC accepts any key");
    mac.update(data);
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Constant-time byte comparison for MAC checks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn secret() -> AuthSecret {
        AuthSecret(Arc::from("test-secret"))
    }

    #[test]
    fn hash_and_verify_roundtrip() {
        let hash = hash_password("correct horse").unwrap();
        assert!(verify_password("correct horse", &hash));
        assert!(!verify_password("wrong", &hash));
    }

    #[test]
    fn two_hashes_of_same_password_differ() {
        let a = hash_password("pw").unwrap();
        let b = hash_password("pw").unwrap();
        assert_ne!(a, b); // unique salt each time
        assert!(verify_password("pw", &a));
        assert!(verify_password("pw", &b));
    }

    #[test]
    fn verify_rejects_malformed_hash() {
        assert!(!verify_password("pw", "not-a-phc-hash"));
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let id = uuid::Uuid::new_v4();
        let token = sign_token(&secret(), id).unwrap();
        assert_eq!(verify_token(&secret(), &token).unwrap(), id);
    }

    #[test]
    fn tampered_token_fails_closed() {
        let id = uuid::Uuid::new_v4();
        let token = sign_token(&secret(), id).unwrap();
        let tampered = format!("{}X", &token[..token.len() - 2]);
        assert!(matches!(
            verify_token(&secret(), &tampered),
            Err(AuthError::InvalidToken)
        ));
    }

    #[test]
    fn wrong_key_fails_closed() {
        let token = sign_token(&secret(), uuid::Uuid::new_v4()).unwrap();
        let other = AuthSecret(Arc::from("other-secret"));
        assert!(matches!(
            verify_token(&other, &token),
            Err(AuthError::InvalidToken)
        ));
    }

    #[test]
    fn cookie_header_extraction() {
        let v = set_cookie_value("abc");
        assert!(v.starts_with("slash_session=abc;"));
        assert!(v.contains("HttpOnly"));
        let header = "something=1; slash_session=thetoken; other=2";
        assert_eq!(
            session_token_from_header(Some(header)).as_deref(),
            Some("thetoken")
        );
        assert_eq!(session_token_from_header(None), None);
    }
}
