//! Retry classification and the backoff ladder (spec §7.6).
//! `workflow_dispatch` has no idempotency key, so a blind retry of an
//! ambiguous failure can launch a second run — callers must be able to tell
//! "definitely not dispatched" (safe to retry) from "possibly dispatched"
//! (never re-POST; let the sweeper resolve it, spec §6.3) from "definitely
//! failed" (permanent).

use std::time::Duration;

/// What to do about a failure. `Ambiguous` is deliberately not `Transient`:
/// callers must branch on it explicitly rather than fall through a generic
/// "retry" path, since retrying a non-idempotent call here is the exact bug
/// spec §7.6 exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryClass {
    /// Known not to have reached GitHub (connection failure), or a rate
    /// limit that resolves by waiting: safe to retry the same request.
    Transient,
    /// The request may or may not have been applied (timeout, or a 5xx
    /// received after the body was sent). Never blindly retry; resolve by
    /// polling instead.
    Ambiguous,
    /// The request definitely did not succeed and retrying would not help.
    Permanent,
}

/// The inputs needed to classify one failed call. Read-after-write 404s
/// (e.g. fetching a PR moments after its `issue_comment` event) are *not*
/// modeled here: spec §7.6 gives them a small bounded retry as a situational
/// behavior of the caller, not a change to the general classification rule
/// that a 404 is permanent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    ConnectionError,
    Timeout,
    Status {
        code: u16,
        rate_limit_remaining: Option<u32>,
    },
}

pub fn classify(kind: FailureKind) -> RetryClass {
    match kind {
        FailureKind::ConnectionError => RetryClass::Transient,
        FailureKind::Timeout => RetryClass::Ambiguous,
        FailureKind::Status {
            rate_limit_remaining: Some(0),
            ..
        } => RetryClass::Transient,
        FailureKind::Status { code: 429, .. } => RetryClass::Transient,
        FailureKind::Status { code, .. } if (500..600).contains(&code) => RetryClass::Ambiguous,
        FailureKind::Status { .. } => RetryClass::Permanent,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BackoffConfig {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl BackoffConfig {
    /// Exponential backoff for the given zero-based attempt index, capped at
    /// `max_delay` so a worker can never hang indefinitely (spec §7.6).
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let scaled = self.base_delay.saturating_mul(1u32 << attempt.min(16));
        scaled.min(self.max_delay)
    }
}

/// Retries `f` while the classified failure is [`RetryClass::Transient`],
/// up to `config.max_attempts`. `f` returns `Err((error, FailureKind))` so
/// this function can classify the outcome without knowing anything about
/// the concrete error type. Stops immediately on `Ambiguous` or `Permanent`
/// — those are the caller's to interpret, never retried here.
pub async fn retry_transient<T, E, F, Fut>(config: &BackoffConfig, mut f: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, (E, FailureKind)>>,
{
    let mut attempt = 0;
    loop {
        match f().await {
            Ok(value) => return Ok(value),
            Err((err, kind)) => {
                if classify(kind) != RetryClass::Transient || attempt + 1 >= config.max_attempts {
                    return Err(err);
                }
                tokio::time::sleep(config.delay_for(attempt)).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn classifies_connection_errors_as_transient() {
        assert_eq!(
            classify(FailureKind::ConnectionError),
            RetryClass::Transient
        );
    }

    #[test]
    fn classifies_timeouts_as_ambiguous() {
        assert_eq!(classify(FailureKind::Timeout), RetryClass::Ambiguous);
    }

    #[test]
    fn classifies_429_as_transient() {
        assert_eq!(
            classify(FailureKind::Status {
                code: 429,
                rate_limit_remaining: None
            }),
            RetryClass::Transient
        );
    }

    #[test]
    fn classifies_rate_limit_exhaustion_as_transient_regardless_of_status() {
        assert_eq!(
            classify(FailureKind::Status {
                code: 403,
                rate_limit_remaining: Some(0)
            }),
            RetryClass::Transient
        );
    }

    #[test]
    fn classifies_5xx_as_ambiguous() {
        for code in [500, 502, 503, 599] {
            assert_eq!(
                classify(FailureKind::Status {
                    code,
                    rate_limit_remaining: None
                }),
                RetryClass::Ambiguous,
                "expected {code} to be ambiguous"
            );
        }
    }

    #[test]
    fn classifies_auth_and_semantic_errors_as_permanent() {
        for code in [401, 403, 404, 422] {
            assert_eq!(
                classify(FailureKind::Status {
                    code,
                    rate_limit_remaining: None
                }),
                RetryClass::Permanent,
                "expected {code} to be permanent"
            );
        }
    }

    #[test]
    fn backoff_delay_grows_and_caps() {
        let config = BackoffConfig {
            max_attempts: 10,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
        };
        assert_eq!(config.delay_for(0), Duration::from_millis(10));
        assert_eq!(config.delay_for(1), Duration::from_millis(20));
        assert_eq!(config.delay_for(2), Duration::from_millis(40));
        assert_eq!(config.delay_for(3), Duration::from_millis(50)); // capped
        assert_eq!(config.delay_for(10), Duration::from_millis(50)); // still capped
    }

    #[tokio::test]
    async fn retries_transient_failures_until_success() {
        let calls = AtomicU32::new(0);
        let config = BackoffConfig {
            max_attempts: 5,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        };

        let result: Result<&str, &str> = retry_transient(&config, || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err(("boom", FailureKind::ConnectionError))
                } else {
                    Ok("ok")
                }
            }
        })
        .await;

        assert_eq!(result, Ok("ok"));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn stops_immediately_on_ambiguous_failure() {
        let calls = AtomicU32::new(0);
        let config = BackoffConfig {
            max_attempts: 5,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        };

        let result: Result<&str, &str> = retry_transient(&config, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { Err(("boom", FailureKind::Timeout)) }
        })
        .await;

        assert_eq!(result, Err("boom"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stops_after_max_attempts() {
        let calls = AtomicU32::new(0);
        let config = BackoffConfig {
            max_attempts: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        };

        let result: Result<&str, &str> = retry_transient(&config, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { Err(("boom", FailureKind::ConnectionError)) }
        })
        .await;

        assert_eq!(result, Err("boom"));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }
}
