//! Short-lived encrypted GitHub user credential used only for repository
//! discovery in the command activity console. The credential stays in an
//! HttpOnly browser cookie; PostgreSQL, JavaScript, and API bodies never see
//! the GitHub token. See `docs/design/command-invocation-history.md`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use crate::auth::{AuthError, AuthSecret};

type HmacSha256 = Hmac<Sha256>;

const COOKIE_NAME: &str = "slash_github_access";
const COOKIE_PATH: &str = "/api/github";
const NONCE_LEN: usize = 12;
pub const MAX_TTL_SECS: u64 = 8 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub user_id: Uuid,
    pub github_subject: String,
    pub access_token: String,
    pub exp: u64,
}

pub fn seal(secret: &AuthSecret, credential: &Credential) -> Result<String, AuthError> {
    let key = encryption_key(secret)?;
    let mut plaintext = serde_json::to_vec(credential).map_err(|_| AuthError::Encode)?;
    let mut nonce_bytes = [0_u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| AuthError::Internal)?;
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce_bytes),
        Aad::empty(),
        &mut plaintext,
    )
    .map_err(|_| AuthError::Internal)?;
    let mut encoded = Vec::with_capacity(NONCE_LEN + plaintext.len());
    encoded.extend_from_slice(&nonce_bytes);
    encoded.extend_from_slice(&plaintext);
    Ok(URL_SAFE_NO_PAD.encode(encoded))
}

pub fn open(secret: &AuthSecret, token: &str) -> Result<Credential, AuthError> {
    let encoded = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| AuthError::InvalidToken)?;
    if encoded.len() <= NONCE_LEN {
        return Err(AuthError::InvalidToken);
    }
    let (nonce, ciphertext) = encoded.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| AuthError::InvalidToken)?;
    let mut ciphertext = ciphertext.to_vec();
    let plaintext = encryption_key(secret)?
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut ciphertext,
        )
        .map_err(|_| AuthError::InvalidToken)?;
    let credential: Credential =
        serde_json::from_slice(plaintext).map_err(|_| AuthError::InvalidToken)?;
    if credential.exp < now_secs() {
        return Err(AuthError::ExpiredToken);
    }
    Ok(credential)
}

pub fn cookie_value(token: &str, ttl_secs: u64) -> String {
    let ttl_secs = ttl_secs.min(MAX_TTL_SECS);
    format!(
        "{COOKIE_NAME}={token}; Path={COOKIE_PATH}; HttpOnly; Secure; SameSite=Lax; Max-Age={ttl_secs}"
    )
}

pub fn clear_cookie_value() -> String {
    format!("{COOKIE_NAME}=; Path={COOKIE_PATH}; HttpOnly; Secure; SameSite=Lax; Max-Age=0")
}

pub fn token_from_header(cookie_header: Option<&str>) -> Option<String> {
    cookie_header?.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == COOKIE_NAME).then(|| value.to_string())
    })
}

pub fn expires_at(ttl_secs: Option<u64>) -> u64 {
    now_secs() + ttl_secs.unwrap_or(MAX_TTL_SECS).min(MAX_TTL_SECS)
}

fn encryption_key(secret: &AuthSecret) -> Result<LessSafeKey, AuthError> {
    let mut mac =
        HmacSha256::new_from_slice(secret.0.as_bytes()).map_err(|_| AuthError::Internal)?;
    mac.update(b"slash/github-user-access-cookie/v1");
    let derived = mac.finalize().into_bytes();
    let key = UnboundKey::new(&AES_256_GCM, &derived).map_err(|_| AuthError::Internal)?;
    Ok(LessSafeKey::new(key))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn secret(value: &'static str) -> AuthSecret {
        AuthSecret(Arc::from(value))
    }

    fn credential(exp: u64) -> Credential {
        Credential {
            user_id: Uuid::new_v4(),
            github_subject: "42".into(),
            access_token: "ghu_secret".into(),
            exp,
        }
    }

    #[test]
    fn encrypted_credential_round_trips_without_exposing_plaintext() {
        let credential = credential(now_secs() + 60);
        let sealed = seal(&secret("root-secret"), &credential).unwrap();

        assert!(!sealed.contains("ghu_secret"));
        assert_eq!(open(&secret("root-secret"), &sealed).unwrap(), credential);
    }

    #[test]
    fn wrong_key_tampering_and_expiry_fail_closed() {
        let sealed = seal(&secret("root-secret"), &credential(now_secs() + 60)).unwrap();
        assert!(open(&secret("different-secret"), &sealed).is_err());

        let mut tampered = sealed.into_bytes();
        let last = tampered.last_mut().unwrap();
        *last = if *last == b'A' { b'B' } else { b'A' };
        assert!(
            open(
                &secret("root-secret"),
                std::str::from_utf8(&tampered).unwrap()
            )
            .is_err()
        );

        let expired = seal(&secret("root-secret"), &credential(now_secs() - 1)).unwrap();
        assert!(matches!(
            open(&secret("root-secret"), &expired),
            Err(AuthError::ExpiredToken)
        ));
    }

    #[test]
    fn cookie_is_bounded_secure_and_narrowly_scoped() {
        let value = cookie_value("encrypted", MAX_TTL_SECS + 1);
        assert!(value.starts_with("slash_github_access=encrypted;"));
        assert!(value.contains("Path=/api/github"));
        assert!(value.contains("HttpOnly"));
        assert!(value.contains("Secure"));
        assert!(value.contains("SameSite=Lax"));
        assert!(value.contains(&format!("Max-Age={MAX_TTL_SECS}")));
        assert_eq!(
            token_from_header(Some("other=1; slash_github_access=encrypted")),
            Some("encrypted".into())
        );
        assert!(clear_cookie_value().contains("Max-Age=0"));
    }
}
