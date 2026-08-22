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
