//! Typed webhook event payloads (spec §7.3: `issue_comment`, `workflow_run`,
//! `check_run`, `installation`, `installation_repositories`, `pull_request`).
//!
//! octocrab already ships a comprehensive, actively-maintained set of these
//! models (`octocrab::models::webhook_events`) driven off the
//! `X-GitHub-Event` header, so this module is a thin wrapper rather than a
//! reimplementation — re-deriving GitHub's webhook schema by hand would just
//! be a second, unmaintained copy of what octocrab already tracks.
//!
//! The M0.5 spike (plan) calls for parsing *real, recorded* payloads from a
//! live installation into these structs; that requires live GitHub access
//! this environment does not have. The tests below use payloads constructed
//! from GitHub's documented webhook schema as a stand-in, and should be
//! replaced with the real M0.5 fixtures once captured.

pub use octocrab::models::webhook_events::{WebhookEvent, WebhookEventPayload, WebhookEventType};

#[derive(Debug, Clone, thiserror::Error)]
#[error("failed to parse {event} webhook payload: {message}")]
pub struct PayloadError {
    pub event: String,
    pub message: String,
}

/// Parses a webhook delivery's body according to its `X-GitHub-Event`
/// header value. Call only after [`crate::verify_webhook`] has succeeded —
/// this never runs on unauthenticated bytes.
pub fn parse_webhook_event(event_name: &str, body: &[u8]) -> Result<WebhookEvent, PayloadError> {
    WebhookEvent::try_from_header_and_body(event_name, body).map_err(|e| PayloadError {
        event: event_name.to_string(),
        message: e.to_string(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    // From octocrab's own `webhook_events` module documentation — a minimal,
    // known-valid payload, used here to prove `parse_webhook_event` routes
    // correctly rather than to re-verify octocrab's own deserialization.
    const PING_JSON: &str = r#"{
        "zen": "Design for failure.",
        "hook_id": 423885699,
        "hook": {
            "type": "App",
            "id": 423885699,
            "name": "web",
            "active": true,
            "events": ["issue_comment", "pull_request", "workflow_run", "check_run"],
            "config": {
                "content_type": "json",
                "insecure_ssl": "0",
                "url": "https://example.com/webhook"
            },
            "updated_at": "2023-07-13T09:30:45Z",
            "created_at": "2023-07-13T09:30:45Z",
            "app_id": 360617,
            "deliveries_url": "https://api.github.com/app/hook/deliveries"
        }
    }"#;

    #[test]
    fn routes_to_the_payload_matching_the_event_header() {
        let event = parse_webhook_event("ping", PING_JSON.as_bytes()).unwrap();
        assert_eq!(event.kind, WebhookEventType::Ping);
        assert!(matches!(event.specific, WebhookEventPayload::Ping(_)));
    }

    #[test]
    fn an_unrecognized_event_name_parses_as_unknown_rather_than_erroring() {
        // Forward-compatible with GitHub adding new event types: callers
        // match on `event.kind` and ignore anything they don't subscribe to
        // (spec §7.3 lists exactly which events Slash handles).
        let event = parse_webhook_event("not_a_real_event", PING_JSON.as_bytes()).unwrap();
        assert_eq!(
            event.kind,
            WebhookEventType::Unknown("not_a_real_event".to_string())
        );
    }

    #[test]
    fn rejects_a_body_that_does_not_match_the_declared_event() {
        // "workflow_run" needs a `workflow_run` key the ping body doesn't have.
        let err = parse_webhook_event("workflow_run", PING_JSON.as_bytes()).unwrap_err();
        assert_eq!(err.event, "workflow_run");
    }
}
