//! Test-engine collection-token crypto — the *pure* part (docs/design/
//! 1.0-test-engine.md §4). Lives in `slash-core` because the token
//! encrypt/decrypt/hash logic is pure computation over a secret, with zero
//! network or database IO. The server layer stores the ciphertext/hash and
//! maps the opaque `AuthSecret` onto the raw secret bytes here.
//!
//! Fail-closed by construction: a token whose nonce is malformed or whose
//! ciphertext fails to authenticate maps to a `TokenCryptoError` rather than
//! a partial result.

/// An AES-GCM-encrypted collection token, kept as opaque bytes for storage
/// (design §4: ciphertext + nonce stored separately in `collection_tokens`).
#[derive(Debug, Clone)]
pub struct EncryptedCollectionToken {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum TokenCryptoError {
    #[error("collection token encryption failed")]
    Encryption,
    #[error("collection token decryption failed")]
    Decryption,
}

/// Derives the AES-256-GCM key from the raw secret bytes via SHA-256 with a
/// domain-separation prefix, so this key can never collide with a key derived
/// for another purpose from the same secret.
fn collection_token_cipher(secret: &[u8]) -> Result<aes_gcm::Aes256Gcm, TokenCryptoError> {
    use aes_gcm::KeyInit;
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(b"slash:collection-token:v1\0");
    hasher.update(secret);
    let key: [u8; 32] = hasher.finalize().into();
    aes_gcm::Aes256Gcm::new_from_slice(&key).map_err(|_| TokenCryptoError::Encryption)
}

/// Encrypts a raw token with a random nonce, returning the ciphertext +
/// nonce pair for storage.
pub fn encrypt_collection_token(
    raw_token: &str,
    secret: &[u8],
) -> Result<EncryptedCollectionToken, TokenCryptoError> {
    use aes_gcm::aead::{Aead, AeadCore, OsRng};

    let cipher = collection_token_cipher(secret)?;
    let nonce = aes_gcm::Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, raw_token.as_bytes())
        .map_err(|_| TokenCryptoError::Encryption)?;
    Ok(EncryptedCollectionToken {
        ciphertext,
        nonce: nonce.to_vec(),
    })
}

/// Decrypts a stored ciphertext back to the raw token. Fails closed on any
/// malformed or unauthenticated input.
pub fn decrypt_collection_token(
    encrypted: &EncryptedCollectionToken,
    secret: &[u8],
) -> Result<String, TokenCryptoError> {
    use aes_gcm::aead::Aead;

    if encrypted.nonce.len() != 12 {
        return Err(TokenCryptoError::Decryption);
    }
    let cipher = collection_token_cipher(secret).map_err(|_| TokenCryptoError::Decryption)?;
    let nonce_bytes: [u8; 12] = encrypted
        .nonce
        .as_slice()
        .try_into()
        .map_err(|_| TokenCryptoError::Decryption)?;
    let nonce = aes_gcm::Nonce::from(nonce_bytes);
    let plaintext = cipher
        .decrypt(&nonce, encrypted.ciphertext.as_ref())
        .map_err(|_| TokenCryptoError::Decryption)?;
    String::from_utf8(plaintext).map_err(|_| TokenCryptoError::Decryption)
}

/// Hashes a raw token with sha256 for storage / lookup. Returns the raw byte
/// hash.
pub fn hash_token(token: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hasher.finalize().into()
}

/// Generates a cryptographically random, URL-safe token. Backs
/// `issue_collection_token` (M2-4 token management).
pub fn crypto_random_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn collection_token_encryption_round_trips() {
        let secret = b"correct-auth-secret";
        let encrypted = encrypt_collection_token("collector-token", secret).unwrap();

        assert_ne!(encrypted.ciphertext, b"collector-token");
        assert_eq!(
            decrypt_collection_token(&encrypted, secret).unwrap(),
            "collector-token"
        );
    }

    #[test]
    fn collection_token_decryption_rejects_wrong_secret() {
        let secret = b"correct-auth-secret";
        let wrong_secret = b"wrong-auth-secret";
        let encrypted = encrypt_collection_token("collector-token", secret).unwrap();

        assert!(decrypt_collection_token(&encrypted, wrong_secret).is_err());
    }

    #[test]
    fn hash_token_is_deterministic_and_32_bytes() {
        let a = hash_token("suite-token");
        let b = hash_token("suite-token");
        let c = hash_token("different");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn crypto_random_token_is_unique_and_parseable_uuid() {
        let a = crypto_random_token();
        let b = crypto_random_token();
        assert_ne!(a, b);
        assert!(uuid::Uuid::parse_str(&a).is_ok());
    }
}
