//! `POST /webhook` (spec §7.3): verify raw bytes -> `INSERT INTO deliveries`
//! -> `200`. This is the durability boundary — only after the row is
//! committed is the delivery acknowledged. JSON is never parsed here; that
//! happens once a worker claims the row.

use std::time::Instant;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};

use slash_github::{WebhookError, WebhookHeaders, verify_webhook};

use crate::AppState;
use crate::deliveries::{InsertOutcome, insert_delivery};

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

pub async fn handle_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let start = Instant::now();

    let event = header_str(&headers, "x-github-event");
    let delivery_guid = header_str(&headers, "x-github-delivery");
    let webhook_headers = WebhookHeaders {
        signature_256: header_str(&headers, "x-hub-signature-256"),
        event,
        delivery: delivery_guid,
        content_type: header_str(&headers, "content-type"),
    };

    let (status, outcome) =
        match verify_webhook(state.webhook_secret.as_bytes(), webhook_headers, &body) {
            Ok(()) => handle_verified(&state, event, delivery_guid, &body).await,
            Err(
                WebhookError::MissingSignature
                | WebhookError::InvalidSignaturePrefix
                | WebhookError::InvalidSignatureEncoding
                | WebhookError::SignatureMismatch,
            ) => (StatusCode::FORBIDDEN, "bad_signature"),
            Err(
                WebhookError::MissingEvent
                | WebhookError::MissingDelivery
                | WebhookError::InvalidContentType,
            ) => (StatusCode::BAD_REQUEST, "bad_headers"),
        };

    state
        .metrics
        .webhook_deliveries_total
        .with_label_values(&[event.unwrap_or("unknown"), outcome])
        .inc();
    state
        .metrics
        .webhook_handler_seconds
        .with_label_values(&[outcome])
        .observe(start.elapsed().as_secs_f64());

    status
}

async fn handle_verified(
    state: &AppState,
    event: Option<&str>,
    delivery_guid: Option<&str>,
    body: &[u8],
) -> (StatusCode, &'static str) {
    // `verify_webhook` already guarantees these are present; re-checking
    // here only guards against a future refactor silently dropping that
    // guarantee, never reachable in practice.
    let (Some(event), Some(guid)) = (event, delivery_guid) else {
        return (StatusCode::BAD_REQUEST, "bad_headers");
    };

    match insert_delivery(&state.pool, guid, event, body).await {
        Ok(InsertOutcome::Inserted) => (StatusCode::OK, "accepted"),
        Ok(InsertOutcome::AlreadyExists) => (StatusCode::OK, "redelivered"),
        Err(error) => {
            tracing::error!(delivery_guid = guid, %error, "failed to record delivery");
            (StatusCode::INTERNAL_SERVER_ERROR, "db_error")
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;

    use super::*;
    use crate::db;
    use crate::deliveries::state_of;
    use crate::metrics::Metrics;

    const SECRET: &str = "integration-test-secret";

    /// `None` when `SLASH_TEST_DATABASE_URL` is unset — callers skip
    /// cleanly rather than failing (plan M4).
    async fn test_state() -> Option<AppState> {
        let url = crate::test_support::test_database_url()?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        sqlx::query("TRUNCATE deliveries")
            .execute(&pool)
            .await
            .unwrap();

        Some(AppState {
            pool,
            metrics: Arc::new(Metrics::new().unwrap()),
            webhook_secret: Arc::from(SECRET),
            auth_secret: crate::auth::AuthSecret(Arc::from(SECRET)),
            web_dir: Arc::from("."),
            github_oauth: None,
        })
    }

    fn sign(body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
        mac.update(body);
        let bytes = mac.finalize().into_bytes();
        let mut hex = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            hex.push_str(&format!("{b:02x}"));
        }
        format!("sha256={hex}")
    }

    fn headers(body: &[u8], guid: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-hub-signature-256", sign(body).parse().unwrap());
        headers.insert("x-github-event", "issue_comment".parse().unwrap());
        headers.insert("x-github-delivery", guid.parse().unwrap());
        headers.insert("content-type", "application/json".parse().unwrap());
        headers
    }

    // A constructed stand-in for a real `issue_comment` payload — the plan's
    // M0.5 spike (live GitHub, not available in this environment) is what
    // should supply the real recorded fixture this eventually gets replaced
    // with.
    const ISSUE_COMMENT_JSON: &[u8] =
        br#"{"action":"created","comment":{"id":1},"issue":{"number":1}}"#;

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn a_correctly_signed_delivery_is_recorded_and_returns_200() {
        let Some(state) = test_state().await else {
            return;
        };
        let body = Bytes::from_static(ISSUE_COMMENT_JSON);
        let guid = "webhook-guid-1";

        let status = handle_webhook(State(state.clone()), headers(&body, guid), body).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            state_of(&state.pool, guid).await.unwrap().as_deref(),
            Some("pending")
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn a_bad_signature_is_rejected_and_nothing_is_recorded() {
        let Some(state) = test_state().await else {
            return;
        };
        let body = Bytes::from_static(ISSUE_COMMENT_JSON);
        let mut bad_headers = headers(&body, "webhook-guid-2");
        bad_headers.insert("x-hub-signature-256", "sha256=0000".parse().unwrap());

        let status = handle_webhook(State(state.clone()), bad_headers, body).await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(state_of(&state.pool, "webhook-guid-2").await.unwrap(), None);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn redelivering_the_same_guid_stays_a_single_row() {
        let Some(state) = test_state().await else {
            return;
        };
        let body = Bytes::from_static(ISSUE_COMMENT_JSON);
        let guid = "webhook-guid-3";

        let first = handle_webhook(State(state.clone()), headers(&body, guid), body.clone()).await;
        let second = handle_webhook(State(state.clone()), headers(&body, guid), body).await;

        assert_eq!(first, StatusCode::OK);
        assert_eq!(second, StatusCode::OK);

        let (count,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM deliveries WHERE delivery_guid = $1")
                .bind(guid)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
    }
}
