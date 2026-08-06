//! Thin typed wrappers over the GitHub REST API used by `slash-core` (plan
//! M3). Each method is a per-repository, already-authenticated call — token
//! minting and caching is [`crate::GithubApp`]'s job, not this one's.
//!
//! octocrab's own builders cover most of these directly. Two calls are
//! hand-rolled instead: `workflow_dispatch` needs `return_run_details: true`
//! and a `200` JSON body octocrab's built-in dispatch (which only expects a
//! bare `204`) does not model (spec §6.3), and octocrab 0.54 has no
//! `get`/`list` for workflow runs at all.

use chrono::{DateTime, Utc};
use octocrab::Octocrab;
use octocrab::models::CheckRunId;
use octocrab::models::checks::CheckRun;
use octocrab::models::issues::Comment;
use octocrab::models::pulls::PullRequest;
use octocrab::models::reactions::{Reaction, ReactionContent};
use octocrab::models::repos::{Content, RepoPermission};
use octocrab::params::checks::{CheckRunConclusion, CheckRunOutput, CheckRunStatus};
use serde::{Deserialize, Serialize};

const API_VERSION_HEADER: &str = "x-github-api-version";
const API_VERSION: &str = "2022-11-28";

#[derive(Debug, Clone, thiserror::Error)]
pub enum ClientError {
    #[error("failed to build GitHub client: {0}")]
    ClientBuild(String),
    #[error("GitHub API error: {0}")]
    Api(String),
}

/// The `workflow_dispatch` response when `return_run_details: true` is sent
/// (spec §6.3): a `200` carrying the run id, rather than a bare `204`.
#[derive(Debug, Clone, Deserialize)]
pub struct DispatchOutcome {
    pub workflow_run_id: u64,
    pub run_url: String,
    pub html_url: String,
}

/// The subset of a workflow run's fields `slash-core` needs (correlation,
/// the sweeper's `triggering_actor` predicate, and duration). Deliberately
/// not `deny_unknown_fields`: this is a real GitHub response, which gains
/// fields over time.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowRun {
    pub id: u64,
    pub status: String,
    pub conclusion: Option<String>,
    pub head_sha: String,
    pub head_branch: String,
    pub event: String,
    pub html_url: String,
    pub created_at: DateTime<Utc>,
    pub run_started_at: Option<DateTime<Utc>>,
    pub triggering_actor: Option<Actor>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Actor {
    pub login: String,
}

#[derive(Deserialize)]
struct ListWorkflowRunsResponse {
    workflow_runs: Vec<WorkflowRun>,
}

/// Filters for `GET .../actions/runs` (spec §6.3's missing-run-id poll):
/// workflow file, triggering event, head branch, the bot's own login (never
/// a human-started run of the same workflow on the same branch), and a
/// creation-time floor.
#[derive(Debug, Clone, Default)]
pub struct ListWorkflowRunsFilter<'a> {
    pub event: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub actor: Option<&'a str>,
    pub created: Option<&'a str>,
}

pub struct RepoClient {
    octocrab: Octocrab,
    owner: String,
    repo: String,
}

impl RepoClient {
    pub fn new(
        token: &str,
        owner: impl Into<String>,
        repo: impl Into<String>,
    ) -> Result<Self, ClientError> {
        Self::with_base_uri(token, owner, repo, None)
    }

    /// `base_uri` overrides `https://api.github.com`; used in tests to point
    /// at a mock server.
    pub fn with_base_uri(
        token: &str,
        owner: impl Into<String>,
        repo: impl Into<String>,
        base_uri: Option<&str>,
    ) -> Result<Self, ClientError> {
        let Ok(header_name) = API_VERSION_HEADER.parse() else {
            return Err(ClientError::ClientBuild(
                "invalid API version header name".to_string(),
            ));
        };

        let mut builder = Octocrab::builder()
            .personal_token(token.to_string())
            .add_header(header_name, API_VERSION.to_string());
        if let Some(uri) = base_uri {
            builder = builder
                .base_uri(uri)
                .map_err(|e| ClientError::ClientBuild(e.to_string()))?;
        }
        let octocrab = builder
            .build()
            .map_err(|e| ClientError::ClientBuild(e.to_string()))?;

        Ok(Self {
            octocrab,
            owner: owner.into(),
            repo: repo.into(),
        })
    }

    pub async fn get_pull_request(&self, number: u64) -> Result<PullRequest, ClientError> {
        self.octocrab
            .pulls(&self.owner, &self.repo)
            .get(number)
            .await
            .map_err(|e| ClientError::Api(e.to_string()))
    }

    /// Reads `role_name`, never the legacy top-level `permission` field
    /// (spec §5.2).
    pub async fn get_collaborator_permission(
        &self,
        username: &str,
    ) -> Result<RepoPermission, ClientError> {
        self.octocrab
            .repos(&self.owner, &self.repo)
            .get_contributor_permission(username)
            .send()
            .await
            .map_err(|e| ClientError::Api(e.to_string()))
    }

    /// Lists a directory (e.g. `.slash`) or fetches one file, at `git_ref`.
    /// Always the default branch for `.slash/*`, the PR head ref for the
    /// workflow file (spec §4, §4.2).
    pub async fn get_content(
        &self,
        path: &str,
        git_ref: &str,
    ) -> Result<Vec<Content>, ClientError> {
        let mut items = self
            .octocrab
            .repos(&self.owner, &self.repo)
            .get_content()
            .path(path)
            .r#ref(git_ref)
            .send()
            .await
            .map_err(|e| ClientError::Api(e.to_string()))?;
        Ok(items.take_items())
    }

    /// Creates `slash/<command>` on `head_sha`, `status: queued` (spec §6.1).
    pub async fn create_check_run(
        &self,
        name: &str,
        head_sha: &str,
        external_id: &str,
    ) -> Result<CheckRun, ClientError> {
        self.octocrab
            .checks(&self.owner, &self.repo)
            .create_check_run(name, head_sha)
            .status(CheckRunStatus::Queued)
            .external_id(external_id)
            .send()
            .await
            .map_err(|e| ClientError::Api(e.to_string()))
    }

    pub async fn update_check_run(
        &self,
        check_run_id: u64,
        update: CheckRunUpdate<'_>,
    ) -> Result<CheckRun, ClientError> {
        let handler = self.octocrab.checks(&self.owner, &self.repo);
        let mut builder = handler.update_check_run(CheckRunId(check_run_id));
        if let Some(status) = update.status {
            builder = builder.status(status);
        }
        if let Some(conclusion) = update.conclusion {
            builder = builder.conclusion(conclusion);
        }
        if let Some(details_url) = update.details_url {
            builder = builder.details_url(details_url);
        }
        if let Some((title, summary)) = update.output {
            builder = builder.output(CheckRunOutput {
                title: title.to_string(),
                summary: summary.to_string(),
                text: None,
                annotations: Vec::new(),
                images: Vec::new(),
            });
        }
        builder
            .send()
            .await
            .map_err(|e| ClientError::Api(e.to_string()))
    }

    pub async fn create_comment(
        &self,
        issue_number: u64,
        body: &str,
    ) -> Result<Comment, ClientError> {
        self.octocrab
            .issues(&self.owner, &self.repo)
            .create_comment(issue_number, body)
            .await
            .map_err(|e| ClientError::Api(e.to_string()))
    }

    /// Reacts on the *comment* (not the issue) — the triggering artifact
    /// (spec §6.4).
    pub async fn create_comment_reaction(
        &self,
        comment_id: u64,
        content: ReactionContent,
    ) -> Result<Reaction, ClientError> {
        self.octocrab
            .issues(&self.owner, &self.repo)
            .create_comment_reaction(comment_id, content)
            .await
            .map_err(|e| ClientError::Api(e.to_string()))
    }

    /// Dispatches `workflow_file` on `git_ref` with `return_run_details:
    /// true` (spec §6.3) — octocrab's built-in dispatch only models a bare
    /// `204`, so this is a raw call.
    pub async fn dispatch_workflow(
        &self,
        workflow_file: &str,
        git_ref: &str,
        inputs: serde_json::Value,
    ) -> Result<DispatchOutcome, ClientError> {
        #[derive(Serialize)]
        struct Body {
            r#ref: String,
            inputs: serde_json::Value,
            return_run_details: bool,
        }

        let route = format!(
            "/repos/{owner}/{repo}/actions/workflows/{workflow_file}/dispatches",
            owner = self.owner,
            repo = self.repo,
        );
        let body = Body {
            r#ref: git_ref.to_string(),
            inputs,
            return_run_details: true,
        };

        self.octocrab
            .post(route, Some(&body))
            .await
            .map_err(|e| ClientError::Api(e.to_string()))
    }

    /// Re-fetches a run directly (spec §6.3: never trust the webhook body for
    /// a terminal conclusion; also used by the sweeper's polling fallbacks).
    pub async fn get_workflow_run(&self, run_id: u64) -> Result<WorkflowRun, ClientError> {
        let route = format!(
            "/repos/{owner}/{repo}/actions/runs/{run_id}",
            owner = self.owner,
            repo = self.repo,
        );
        self.octocrab
            .get(route, None::<&()>)
            .await
            .map_err(|e| ClientError::Api(e.to_string()))
    }

    /// The spec §6.3 missing-run-id poll: filtered by workflow file, event,
    /// branch, and the App's own bot login (`triggering_actor`) — the
    /// predicate that keeps this from ever claiming a human-started run.
    pub async fn list_workflow_runs(
        &self,
        workflow_file: &str,
        filter: ListWorkflowRunsFilter<'_>,
    ) -> Result<Vec<WorkflowRun>, ClientError> {
        #[derive(Serialize)]
        struct Query<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            event: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            branch: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            actor: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            created: Option<&'a str>,
        }

        let route = format!(
            "/repos/{owner}/{repo}/actions/workflows/{workflow_file}/runs",
            owner = self.owner,
            repo = self.repo,
        );
        let query = Query {
            event: filter.event,
            branch: filter.branch,
            actor: filter.actor,
            created: filter.created,
        };

        let response: ListWorkflowRunsResponse = self
            .octocrab
            .get(route, Some(&query))
            .await
            .map_err(|e| ClientError::Api(e.to_string()))?;
        Ok(response.workflow_runs)
    }
}

#[derive(Debug, Clone, Default)]
pub struct CheckRunUpdate<'a> {
    pub status: Option<CheckRunStatus>,
    pub conclusion: Option<CheckRunConclusion>,
    pub details_url: Option<&'a str>,
    /// `(title, summary)`.
    pub output: Option<(&'a str, &'a str)>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn client_against(server: &MockServer) -> RepoClient {
        RepoClient::with_base_uri("tok_abc", "acme", "widgets", Some(&server.uri())).unwrap()
    }

    #[tokio::test]
    async fn get_pull_request_hits_the_expected_route() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/pulls/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1, "number": 7, "state": "open",
                "url": "https://api.github.com/repos/acme/widgets/pulls/7",
                "head": {"ref": "feature", "sha": "deadbeef"},
                "base": {"ref": "main", "sha": "cafef00d"}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_against(&server).await;
        let pr = client.get_pull_request(7).await.unwrap();
        assert_eq!(pr.number, 7);
    }

    #[tokio::test]
    async fn get_collaborator_permission_reads_role_name() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/collaborators/alice/permission"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "permission": "push",
                "role_name": "maintain",
                "user": {
                    "login": "alice", "id": 1, "node_id": "n",
                    "avatar_url": "https://avatars.githubusercontent.com/u/1",
                    "gravatar_id": "", "url": "https://api.github.com/users/alice",
                    "html_url": "https://github.com/alice",
                    "followers_url": "https://api.github.com/users/alice/followers",
                    "following_url": "https://api.github.com/users/alice/following{/other_user}",
                    "gists_url": "https://api.github.com/users/alice/gists{/gist_id}",
                    "starred_url": "https://api.github.com/users/alice/starred{/owner}{/repo}",
                    "subscriptions_url": "https://api.github.com/users/alice/subscriptions",
                    "organizations_url": "https://api.github.com/users/alice/orgs",
                    "repos_url": "https://api.github.com/users/alice/repos",
                    "events_url": "https://api.github.com/users/alice/events{/privacy}",
                    "received_events_url": "https://api.github.com/users/alice/received_events",
                    "type": "User", "site_admin": false,
                    "permissions": {"admin": false, "push": false, "pull": true}
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_against(&server).await;
        let permission = client.get_collaborator_permission("alice").await.unwrap();
        // The legacy `permission` field collapses maintain into push/write;
        // `role_name` is the one that keeps "maintain" faithful (spec §5.2).
        assert_eq!(permission.role_name, "maintain");
    }

    #[tokio::test]
    async fn get_content_lists_a_directory() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/contents/.slash"))
            .and(query_param("ref", "main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "name": "deploy.yml", "path": ".slash/deploy.yml", "sha": "abc",
                    "size": 10, "url": "https://api.github.com/repos/acme/widgets/contents/.slash/deploy.yml",
                    "type": "file",
                    "_links": {
                        "self": "https://api.github.com/repos/acme/widgets/contents/.slash/deploy.yml",
                        "git": null,
                        "html": null
                    }
                }
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_against(&server).await;
        let items = client.get_content(".slash", "main").await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "deploy.yml");
    }

    #[tokio::test]
    async fn create_check_run_sends_queued_status_and_external_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/widgets/check-runs"))
            .and(body_json(serde_json::json!({
                "name": "slash/deploy",
                "head_sha": "deadbeef",
                "external_id": "run-uuid",
                "status": "queued"
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": 55, "node_id": "n1", "head_sha": "deadbeef",
                "url": "https://api.github.com/repos/acme/widgets/check-runs/55",
                "html_url": null, "details_url": null, "conclusion": null,
                "output": {
                    "title": null, "summary": null, "text": null,
                    "annotations_count": 0,
                    "annotations_url": "https://api.github.com/repos/acme/widgets/check-runs/55/annotations"
                },
                "started_at": null, "completed_at": null,
                "name": "slash/deploy", "pull_requests": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_against(&server).await;
        let check_run = client
            .create_check_run("slash/deploy", "deadbeef", "run-uuid")
            .await
            .unwrap();
        assert_eq!(check_run.id.0, 55);
    }

    #[tokio::test]
    async fn dispatch_workflow_sends_return_run_details_and_parses_the_200_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/repos/acme/widgets/actions/workflows/deploy.yml/dispatches",
            ))
            .and(body_json(serde_json::json!({
                "ref": "refs/heads/feature",
                "inputs": {"env": "staging"},
                "return_run_details": true
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "workflow_run_id": 999,
                "run_url": "https://api.github.com/repos/acme/widgets/actions/runs/999",
                "html_url": "https://github.com/acme/widgets/actions/runs/999"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_against(&server).await;
        let outcome = client
            .dispatch_workflow(
                "deploy.yml",
                "refs/heads/feature",
                serde_json::json!({"env": "staging"}),
            )
            .await
            .unwrap();
        assert_eq!(outcome.workflow_run_id, 999);
        assert_eq!(
            outcome.html_url,
            "https://github.com/acme/widgets/actions/runs/999"
        );
    }

    #[tokio::test]
    async fn get_workflow_run_parses_triggering_actor_and_timestamps() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/actions/runs/999"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 999, "status": "completed", "conclusion": "success",
                "head_sha": "deadbeef", "head_branch": "feature", "event": "workflow_dispatch",
                "html_url": "https://github.com/acme/widgets/actions/runs/999",
                "created_at": "2024-01-01T00:00:00Z",
                "run_started_at": "2024-01-01T00:00:05Z",
                "triggering_actor": {"login": "slash[bot]"}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_against(&server).await;
        let run = client.get_workflow_run(999).await.unwrap();
        assert_eq!(run.status, "completed");
        assert_eq!(run.conclusion.as_deref(), Some("success"));
        assert_eq!(run.triggering_actor.unwrap().login, "slash[bot]");
    }

    #[tokio::test]
    async fn list_workflow_runs_sends_the_filter_query_params() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/repos/acme/widgets/actions/workflows/deploy.yml/runs",
            ))
            .and(query_param("event", "workflow_dispatch"))
            .and(query_param("branch", "feature"))
            .and(query_param("actor", "slash[bot]"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total_count": 1,
                "workflow_runs": [{
                    "id": 999, "status": "in_progress", "conclusion": null,
                    "head_sha": "deadbeef", "head_branch": "feature", "event": "workflow_dispatch",
                    "html_url": "https://github.com/acme/widgets/actions/runs/999",
                    "created_at": "2024-01-01T00:00:00Z",
                    "run_started_at": null,
                    "triggering_actor": {"login": "slash[bot]"}
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_against(&server).await;
        let runs = client
            .list_workflow_runs(
                "deploy.yml",
                ListWorkflowRunsFilter {
                    event: Some("workflow_dispatch"),
                    branch: Some("feature"),
                    actor: Some("slash[bot]"),
                    created: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, 999);
    }
}
