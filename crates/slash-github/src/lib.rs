//! GitHub App authentication, webhook verification, and typed REST wrappers
//! (spec §7.3, §7.5, §7.6). IO lives here at the edge — `slash-core` depends
//! on this crate's types but never talks to `octocrab` directly.

mod auth;
mod client;
mod payloads;
mod retry;
mod webhook;

pub use auth::{AppAuthError, AppInstallation, GithubApp, InstallationToken, TokenCacheKey};
pub use client::{
    Actor, CheckRunUpdate, ClientError, DispatchOutcome, ListWorkflowRunsFilter, RepoClient,
    WorkflowRun,
};
pub use payloads::{
    PayloadError, WebhookEvent, WebhookEventPayload, WebhookEventType, parse_webhook_event,
};
pub use retry::{BackoffConfig, FailureKind, RetryClass, classify, retry_transient};
pub use webhook::{WebhookError, WebhookHeaders, verify_webhook};

/// Re-exported so callers don't need a direct `octocrab` dependency for the
/// common model/param types this crate's methods accept and return.
pub mod octocrab_types {
    pub use octocrab::models::checks::CheckRun;
    pub use octocrab::models::issues::Comment;
    pub use octocrab::models::pulls::PullRequest;
    pub use octocrab::models::reactions::{Reaction, ReactionContent};
    pub use octocrab::models::repos::{Content, RepoPermission};
    pub use octocrab::params::checks::{CheckRunConclusion, CheckRunStatus};
}
