//! Webhook signature verification (spec §7.3). Operates on raw bytes only —
//! JSON is parsed by callers *after* this succeeds, never before.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WebhookError {
    #[error("missing X-Hub-Signature-256 header")]
    MissingSignature,
    #[error("signature header must start with 'sha256='")]
    InvalidSignaturePrefix,
    #[error("signature header is not valid hex")]
    InvalidSignatureEncoding,
    #[error("signature does not match")]
    SignatureMismatch,
    #[error("missing X-GitHub-Event header")]
    MissingEvent,
    #[error("missing X-GitHub-Delivery header")]
    MissingDelivery,
    #[error("content-type must be application/json")]
    InvalidContentType,
}

/// The headers spec §7.3 requires on every webhook delivery. The legacy
/// SHA-1 `X-Hub-Signature` is never read — only `X-Hub-Signature-256`.
#[derive(Debug, Clone, Copy)]
pub struct WebhookHeaders<'a> {
    pub signature_256: Option<&'a str>,
    pub event: Option<&'a str>,
    pub delivery: Option<&'a str>,
    pub content_type: Option<&'a str>,
}

/// Verifies a webhook delivery's signature and required headers over the
/// exact received `body` bytes. Signature is checked first — it is the
/// actual trust boundary; the other header checks are well-formedness.
/// The caller (`slash-server`) maps [`WebhookError::MissingSignature`] and
/// signature failures to `403`, per spec §7.3.
pub fn verify_webhook(
    secret: &[u8],
    headers: WebhookHeaders<'_>,
    body: &[u8],
) -> Result<(), WebhookError> {
    verify_signature(secret, headers.signature_256, body)?;

    if headers.event.is_none() {
        return Err(WebhookError::MissingEvent);
    }
    if headers.delivery.is_none() {
        return Err(WebhookError::MissingDelivery);
    }
    match headers.content_type {
        Some(ct) if ct.to_ascii_lowercase().starts_with("application/json") => {}
        _ => return Err(WebhookError::InvalidContentType),
    }

    Ok(())
}

fn verify_signature(
    secret: &[u8],
    signature_header: Option<&str>,
    body: &[u8],
) -> Result<(), WebhookError> {
    let header = signature_header.ok_or(WebhookError::MissingSignature)?;
    let hex_sig = header
        .strip_prefix("sha256=")
        .ok_or(WebhookError::InvalidSignaturePrefix)?;
    let sig_bytes = decode_hex(hex_sig).ok_or(WebhookError::InvalidSignatureEncoding)?;

    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        // HMAC accepts any key length; this is unreachable in practice. Fail
        // closed rather than reach for `.expect()`.
        return Err(WebhookError::SignatureMismatch);
    };
    mac.update(body);
    mac.verify_slice(&sig_bytes)
        .map_err(|_| WebhookError::SignatureMismatch)
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(s.len() / 2);
    let mut chars = s.chars();
    while let (Some(hi), Some(lo)) = (chars.next(), chars.next()) {
        let hi = hi.to_digit(16)?;
        let lo = lo.to_digit(16)?;
        bytes.push(((hi << 4) | lo) as u8);
    }
    Some(bytes)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"topsecret";
    const BODY: &[u8] = br#"{"action":"created"}"#;

    fn valid_headers(sig: &str) -> WebhookHeaders<'_> {
        WebhookHeaders {
            signature_256: Some(sig),
            event: Some("issue_comment"),
            delivery: Some("11111111-1111-1111-1111-111111111111"),
            content_type: Some("application/json"),
        }
    }

    fn sign(secret: &[u8], body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(body);
        let bytes = mac.finalize().into_bytes();
        let mut hex = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            hex.push_str(&format!("{b:02x}"));
        }
        format!("sha256={hex}")
    }

    #[test]
    fn accepts_a_correctly_signed_request() {
        let sig = sign(SECRET, BODY);
        assert!(verify_webhook(SECRET, valid_headers(&sig), BODY).is_ok());
    }

    #[test]
    fn rejects_a_wrong_signature() {
        let sig = sign(b"wrong-secret", BODY);
        assert_eq!(
            verify_webhook(SECRET, valid_headers(&sig), BODY),
            Err(WebhookError::SignatureMismatch)
        );
    }

    #[test]
    fn rejects_a_signature_for_different_bytes() {
        let sig = sign(SECRET, br#"{"action":"deleted"}"#);
        assert_eq!(
            verify_webhook(SECRET, valid_headers(&sig), BODY),
            Err(WebhookError::SignatureMismatch)
        );
    }

    #[test]
    fn rejects_missing_signature() {
        let mut headers = valid_headers("sha256=00");
        headers.signature_256 = None;
        assert_eq!(
            verify_webhook(SECRET, headers, BODY),
            Err(WebhookError::MissingSignature)
        );
    }

    #[test]
    fn never_honors_legacy_sha1_signature() {
        // Even a header carrying only the SHA-1 form (no `sha256=` prefix)
        // must be rejected, not silently accepted via a fallback path.
        let sig = "sha1=deadbeef";
        assert_eq!(
            verify_webhook(SECRET, valid_headers(sig), BODY),
            Err(WebhookError::InvalidSignaturePrefix)
        );
    }

    #[test]
    fn rejects_non_hex_signature() {
        let sig = "sha256=not-hex-zzzz";
        assert_eq!(
            verify_webhook(SECRET, valid_headers(sig), BODY),
            Err(WebhookError::InvalidSignatureEncoding)
        );
    }

    #[test]
    fn rejects_missing_required_headers() {
        let sig = sign(SECRET, BODY);
        let mut headers = valid_headers(&sig);
        headers.event = None;
        assert_eq!(
            verify_webhook(SECRET, headers, BODY),
            Err(WebhookError::MissingEvent)
        );

        let mut headers = valid_headers(&sig);
        headers.delivery = None;
        assert_eq!(
            verify_webhook(SECRET, headers, BODY),
            Err(WebhookError::MissingDelivery)
        );

        let mut headers = valid_headers(&sig);
        headers.content_type = Some("text/plain");
        assert_eq!(
            verify_webhook(SECRET, headers, BODY),
            Err(WebhookError::InvalidContentType)
        );

        let mut headers = valid_headers(&sig);
        headers.content_type = None;
        assert_eq!(
            verify_webhook(SECRET, headers, BODY),
            Err(WebhookError::InvalidContentType)
        );
    }

    #[test]
    fn accepts_content_type_with_charset_parameter() {
        let sig = sign(SECRET, BODY);
        let mut headers = valid_headers(&sig);
        headers.content_type = Some("application/json; charset=utf-8");
        assert!(verify_webhook(SECRET, headers, BODY).is_ok());
    }
}
