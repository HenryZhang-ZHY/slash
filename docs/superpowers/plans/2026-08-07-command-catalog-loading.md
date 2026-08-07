# On-Demand Command Catalog Loading Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Load and validate a fresh, immutable default-branch command catalog for every strictly parsed Slash command without silently treating GitHub or configuration failures as an empty catalog.

**Architecture:** Extend `RepoClient` with typed repository/default-branch operations and status-bearing API errors. Add a focused `catalog` module that resolves the canonical default branch to a commit SHA and loads the complete `.slash` directory into a typed outcome; both issue-comment dispatch and check-run rerequest consume that module and map failures to explicit feedback, logs, and metrics.

**Tech Stack:** Rust 2024, Tokio, Octocrab 0.54, Wiremock, SQLx/PostgreSQL, Prometheus, `slash-config`

---

## File Map

- Modify `crates/slash-github/src/client.rs`: preserve GitHub HTTP status codes and expose default-branch metadata and branch-ref lookups.
- Modify `crates/slash-github/src/lib.rs`: re-export any new client types needed by `slash-server`.
- Modify `crates/slash-server/Cargo.toml`: make `base64` a runtime dependency for non-panicking content decoding.
- Create `crates/slash-server/src/catalog.rs`: resolve an immutable default-branch snapshot and load a complete, validated command catalog.
- Modify `crates/slash-server/src/main.rs`: register the new `catalog` module.
- Modify `crates/slash-core/src/messages.rs`: add safe user-facing messages for unavailable command configuration.
- Modify `crates/slash-server/src/metrics.rs`: add bounded catalog outcome/stage metrics.
- Modify `crates/slash-server/src/pipeline.rs`: replace silent command loading with the catalog service and explicit feedback.
- Modify `crates/slash-server/src/correlation.rs`: use the same catalog rules for check-run rerequests.

## Local Test Database

The catalog and GitHub client tests do not need PostgreSQL. Pipeline and
correlation integration tests do. Before those tasks, start the existing test
dependency with:

```powershell
docker run --name slash-command-catalog-pg -e POSTGRES_PASSWORD=slash -e POSTGRES_DB=slash_test -p 55432:5432 -d postgres:18
```

Every database-backed test command below sets `SLASH_TEST_DATABASE_URL` in the
same PowerShell process because shell state does not persist between tool calls.
After the final task:

```powershell
docker rm -f slash-command-catalog-pg
```

### Task 1: Preserve GitHub API Status and Resolve Repository Refs

**Files:**
- Modify: `crates/slash-github/src/client.rs:23-169`
- Modify: `crates/slash-github/src/lib.rs:11-18`
- Test: `crates/slash-github/src/client.rs` test module

- [ ] **Step 1: Write failing client tests**

Add Wiremock tests that prove status preservation, repository metadata lookup,
and branch SHA lookup:

```rust
#[tokio::test]
async fn content_error_preserves_github_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/contents/.slash"))
        .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
            "message": "Resource not accessible by integration",
            "documentation_url": "https://docs.github.com/rest"
        })))
        .mount(&server)
        .await;

    let client = RepoClient::with_base_uri("token", "acme", "widgets", Some(&server.uri()))
        .unwrap();
    let error = client.get_content(".slash", "abc123").await.unwrap_err();
    assert_eq!(error.status_code(), Some(403));
}

#[tokio::test]
async fn gets_the_repository_default_branch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "default_branch": "trunk"
        })))
        .mount(&server)
        .await;

    let client = RepoClient::with_base_uri("token", "acme", "widgets", Some(&server.uri()))
        .unwrap();
    assert_eq!(client.get_default_branch().await.unwrap(), "trunk");
}

#[tokio::test]
async fn resolves_a_branch_to_its_commit_sha() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/git/ref/heads/trunk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ref": "refs/heads/trunk",
            "node_id": "REF_1",
            "url": "https://api.github.com/repos/acme/widgets/git/refs/heads/trunk",
            "object": {
                "type": "commit",
                "sha": "deadbeef",
                "url": "https://api.github.com/repos/acme/widgets/git/commits/deadbeef"
            }
        })))
        .mount(&server)
        .await;

    let client = RepoClient::with_base_uri("token", "acme", "widgets", Some(&server.uri()))
        .unwrap();
    assert_eq!(
        client.get_branch_head_sha("trunk").await.unwrap(),
        "deadbeef"
    );
}
```

- [ ] **Step 2: Run the client tests to verify failure**

Run:

```powershell
cargo test -p slash-github client::tests -- --nocapture
```

Expected: compilation fails because `status_code`, `get_default_branch`, and
`get_branch_head_sha` do not exist.

- [ ] **Step 3: Add structured client errors and repository methods**

Replace the string-only API error with a status-bearing variant and central
conversion:

```rust
#[derive(Debug, Clone, thiserror::Error)]
pub enum ClientError {
    #[error("failed to build GitHub client: {0}")]
    ClientBuild(String),
    #[error("GitHub API error: {message}")]
    Api {
        message: String,
        status: Option<u16>,
    },
    #[error("invalid GitHub API response: {0}")]
    InvalidResponse(String),
}

impl ClientError {
    fn from_octocrab(error: octocrab::Error) -> Self {
        let status = match &error {
            octocrab::Error::GitHub { source, .. } => Some(source.status_code.as_u16()),
            _ => None,
        };
        Self::Api {
            message: error.to_string(),
            status,
        }
    }

    pub fn status_code(&self) -> Option<u16> {
        match self {
            Self::Api { status, .. } => *status,
            Self::ClientBuild(_) | Self::InvalidResponse(_) => None,
        }
    }
}
```

Change every Octocrab API mapping in this file from
`map_err(|e| ClientError::Api(e.to_string()))` to
`map_err(ClientError::from_octocrab)`.

Add the minimal repository response and the two methods:

```rust
#[derive(Debug, Deserialize)]
struct RepositoryDefaults {
    default_branch: String,
}

impl RepoClient {
    pub async fn get_default_branch(&self) -> Result<String, ClientError> {
        let route = format!("/repos/{}/{}", self.owner, self.repo);
        let repository: RepositoryDefaults = self
            .octocrab
            .get(route, None::<&()>)
            .await
            .map_err(ClientError::from_octocrab)?;
        if repository.default_branch.is_empty() {
            return Err(ClientError::InvalidResponse(
                "repository default_branch is empty".to_string(),
            ));
        }
        Ok(repository.default_branch)
    }

    pub async fn get_branch_head_sha(&self, branch: &str) -> Result<String, ClientError> {
        use octocrab::models::repos::Object;
        use octocrab::params::repos::Reference;

        let reference = self
            .octocrab
            .repos(&self.owner, &self.repo)
            .get_ref(&Reference::Branch(branch.to_string()))
            .await
            .map_err(ClientError::from_octocrab)?;
        match reference.object {
            Object::Commit { sha, .. } => Ok(sha),
            Object::Tag { .. } => Err(ClientError::InvalidResponse(format!(
                "branch {branch} resolved to a tag"
            ))),
            _ => Err(ClientError::InvalidResponse(format!(
                "branch {branch} resolved to an unsupported object"
            ))),
        }
    }
}
```

Keep these methods on `RepoClient`; do not expose raw Octocrab repository or
ref models to the server crate.

- [ ] **Step 4: Run client tests**

Run:

```powershell
cargo test -p slash-github client::tests -- --nocapture
```

Expected: all `slash-github` client tests pass.

- [ ] **Step 5: Commit**

```powershell
git add crates/slash-github/src/client.rs crates/slash-github/src/lib.rs
git commit -m "feat: expose default branch snapshot APIs" -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 2: Build the Typed Command Catalog Loader

**Files:**
- Modify: `crates/slash-server/Cargo.toml`
- Create: `crates/slash-server/src/catalog.rs`
- Modify: `crates/slash-server/src/main.rs:1-12`
- Test: `crates/slash-server/src/catalog.rs`

- [ ] **Step 1: Move Base64 decoding into runtime dependencies**

Move `base64 = "0.23.1"` from `[dev-dependencies]` to `[dependencies]` in
`crates/slash-server/Cargo.toml`. The loader must decode untrusted API content
without calling Octocrab's panic-capable `Content::decoded_content()`.

- [ ] **Step 2: Register an empty catalog module and write failing resolver tests**

Add `mod catalog;` to `crates/slash-server/src/main.rs`. Create
`crates/slash-server/src/catalog.rs` with tests for the PR hint, metadata
fallback, and immutable SHA:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn resolves_the_hinted_default_branch_to_a_sha() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/git/ref/heads/trunk"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ref_json(
                "trunk", "sha-one",
            )))
            .expect(1)
            .mount(&server)
            .await;

        let client = client(&server);
        let resolved = resolve_default_branch(&client, Some("trunk")).await.unwrap();
        assert_eq!(resolved.name, "trunk");
        assert_eq!(resolved.sha, "sha-one");
    }

    #[tokio::test]
    async fn falls_back_to_repository_metadata_when_the_hint_is_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"default_branch": "stable"}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/git/ref/heads/stable"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ref_json(
                "stable", "sha-two",
            )))
            .mount(&server)
            .await;

        let resolved = resolve_default_branch(&client(&server), None).await.unwrap();
        assert_eq!(resolved.name, "stable");
        assert_eq!(resolved.sha, "sha-two");
    }
}
```

Use local test helpers that return a `RepoClient` and the exact ref JSON shown
in Task 1.

- [ ] **Step 3: Run resolver tests to verify failure**

Run:

```powershell
cargo test -p slash-server catalog::tests -- --nocapture
```

Expected: compilation fails because the catalog types and resolver do not
exist.

- [ ] **Step 4: Implement typed resolution and catalog outcomes**

Add these public crate-local types:

```rust
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use slash_config::ValidatedCommand;
use slash_github::{ClientError, RepoClient};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedDefaultBranch {
    pub name: String,
    pub sha: String,
}

#[derive(Debug)]
pub(crate) struct CommandCatalog {
    commands: Vec<ValidatedCommand>,
}

impl CommandCatalog {
    pub fn find(&self, name: &str) -> Option<&ValidatedCommand> {
        self.commands.iter().find(|command| command.command == name)
    }

    pub fn names(&self) -> Vec<String> {
        self.commands
            .iter()
            .map(|command| command.command.clone())
            .collect()
    }
}

#[derive(Debug)]
pub(crate) enum CatalogOutcome {
    Loaded(CommandCatalog),
    NotConfigured,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CatalogError {
    #[error("GitHub API unavailable during {stage}: {source}")]
    Unavailable {
        stage: &'static str,
        path: Option<String>,
        #[source]
        source: ClientError,
    },
    #[error("invalid command catalog: {details}")]
    Invalid {
        details: String,
    },
}

impl CatalogError {
    pub fn stage(&self) -> &'static str {
        match self {
            Self::Unavailable { stage, .. } => stage,
            Self::Invalid { .. } => "validation",
        }
    }

    pub fn status_code(&self) -> Option<u16> {
        match self {
            Self::Unavailable { source, .. } => source.status_code(),
            Self::Invalid { .. } => None,
        }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Unavailable { path, .. } => path.as_deref(),
            Self::Invalid { .. } => None,
        }
    }
}
```

Implement the resolver without a `main` fallback:

```rust
pub(crate) async fn resolve_default_branch(
    client: &RepoClient,
    hinted_name: Option<&str>,
) -> Result<ResolvedDefaultBranch, CatalogError> {
    let name = match hinted_name.filter(|name| !name.is_empty()) {
        Some(name) => name.to_string(),
        None => client.get_default_branch().await.map_err(|source| {
            CatalogError::Unavailable {
                stage: "default_branch",
                path: None,
                source,
            }
        })?,
    };
    let sha = client
        .get_branch_head_sha(&name)
        .await
        .map_err(|source| CatalogError::Unavailable {
            stage: "branch_ref",
            path: Some(name.clone()),
            source,
        })?;
    Ok(ResolvedDefaultBranch { name, sha })
}
```

- [ ] **Step 5: Write failing loader tests**

Add Wiremock tests covering a missing directory, a file fetch failure, invalid
YAML, duplicate commands, and successful loading at one SHA. The successful
test must assert every Contents API request carries `ref=sha-one`:

```rust
#[tokio::test]
async fn missing_directory_is_not_configured() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/contents/.slash"))
        .and(query_param("ref", "sha-one"))
        .respond_with(ResponseTemplate::new(404).set_body_json(
            serde_json::json!({"message": "Not Found"}),
        ))
        .mount(&server)
        .await;

    assert!(matches!(
        load_catalog(&client(&server), "sha-one").await.unwrap(),
        CatalogOutcome::NotConfigured
    ));
}

#[tokio::test]
async fn one_unreadable_file_fails_the_complete_catalog() {
    let server = MockServer::start().await;
    mount_directory(&server, &["deploy.yml"]).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/contents/.slash/deploy.yml"))
        .and(query_param("ref", "sha-one"))
        .respond_with(ResponseTemplate::new(403).set_body_json(
            serde_json::json!({"message": "Forbidden"}),
        ))
        .mount(&server)
        .await;

    let error = load_catalog(&client(&server), "sha-one").await.unwrap_err();
    assert_eq!(error.stage(), "file");
    assert_eq!(error.status_code(), Some(403));
    assert_eq!(error.path(), Some(".slash/deploy.yml"));
}

#[tokio::test]
async fn github_status_failures_are_unavailable_not_empty_catalogs() {
    for status in [401, 403, 429, 500, 503] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/contents/.slash"))
            .and(query_param("ref", "sha-one"))
            .respond_with(ResponseTemplate::new(status).set_body_json(
                serde_json::json!({"message": "request failed"}),
            ))
            .mount(&server)
            .await;

        let error = load_catalog(&client(&server), "sha-one")
            .await
            .unwrap_err();
        assert!(matches!(error, CatalogError::Unavailable { .. }));
        assert_eq!(error.status_code(), Some(status));
    }
}

#[tokio::test]
async fn duplicate_commands_invalidate_the_complete_catalog() {
    let server = MockServer::start().await;
    mount_directory(&server, &["one.yml", "two.yml"]).await;
    mount_file(&server, "one.yml", "command: deploy\nworkflow: one.yml\n").await;
    mount_file(&server, "two.yml", "command: deploy\nworkflow: two.yml\n").await;

    let error = load_catalog(&client(&server), "sha-one").await.unwrap_err();
    assert!(matches!(error, CatalogError::Invalid { .. }));
    assert!(error.to_string().contains("duplicate"));
}
```

`mount_directory` and `mount_file` must return the same GitHub Contents JSON
shape already used by `pipeline.rs` tests and must require `ref=sha-one`.

- [ ] **Step 6: Run loader tests to verify failure**

Run:

```powershell
cargo test -p slash-server catalog::tests -- --nocapture
```

Expected: resolver tests pass; loader tests fail because `load_catalog` is not
implemented.

- [ ] **Step 7: Implement complete catalog loading**

Implement safe decoding, all-file validation, and cross-file duplicate
validation:

```rust
pub(crate) async fn load_catalog(
    client: &RepoClient,
    sha: &str,
) -> Result<CatalogOutcome, CatalogError> {
    let files = match client.get_content(".slash", sha).await {
        Ok(files) => files,
        Err(source) if source.status_code() == Some(404) => {
            return Ok(CatalogOutcome::NotConfigured);
        }
        Err(source) => {
            return Err(CatalogError::Unavailable {
                stage: "directory",
                path: Some(".slash".to_string()),
                source,
            });
        }
    };

    let yaml_files: Vec<_> = files
        .iter()
        .filter(|file| {
            file.r#type == "file"
                && (file.name.ends_with(".yml") || file.name.ends_with(".yaml"))
        })
        .collect();
    if yaml_files.is_empty() {
        return Ok(CatalogOutcome::NotConfigured);
    }

    let mut commands = Vec::with_capacity(yaml_files.len());
    let mut validation_errors = Vec::new();
    let mut command_sources = Vec::new();

    for file in yaml_files {
        let content_items = client
            .get_content(&file.path, sha)
            .await
            .map_err(|source| CatalogError::Unavailable {
                stage: "file",
                path: Some(file.path.clone()),
                source,
            })?;
        let content = content_items.first().ok_or_else(|| CatalogError::Invalid {
            details: format!("{} returned no content", file.name),
        })?;
        let encoded = content.content.as_deref().ok_or_else(|| CatalogError::Invalid {
            details: format!("{} has no base64 content", file.name),
        })?;
        let compact: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
        let bytes = BASE64.decode(compact.as_bytes()).map_err(|error| {
            CatalogError::Invalid {
                details: format!("{} has invalid base64 content: {error}", file.name),
            }
        })?;

        match slash_config::load_command_file(&file.name, &bytes) {
            Ok(command) => {
                command_sources.push((file.name.clone(), command.command.clone()));
                commands.push(command);
            }
            Err(errors) => validation_errors.extend(errors.into_iter().map(|error| error.to_string())),
        }
    }

    validation_errors.extend(
        slash_config::find_duplicate_commands(&command_sources)
            .into_iter()
            .map(|error| error.to_string()),
    );
    if !validation_errors.is_empty() {
        return Err(CatalogError::Invalid {
            details: validation_errors.join("; "),
        });
    }

    Ok(CatalogOutcome::Loaded(CommandCatalog { commands }))
}
```

- [ ] **Step 8: Run catalog tests**

Run:

```powershell
cargo test -p slash-server catalog::tests -- --nocapture
```

Expected: all catalog tests pass, including status, duplicate, and immutable
SHA assertions.

- [ ] **Step 9: Commit**

```powershell
git add crates/slash-server/Cargo.toml crates/slash-server/src/catalog.rs crates/slash-server/src/main.rs Cargo.lock
git commit -m "feat: load command catalogs from immutable refs" -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3: Add Catalog Feedback and Observability

**Files:**
- Modify: `crates/slash-core/src/messages.rs:24-57`
- Modify: `crates/slash-server/src/metrics.rs:10-105`
- Test: existing test modules in both files

- [ ] **Step 1: Write failing message and metric tests**

Add:

```rust
#[test]
fn command_catalog_unavailable_does_not_expose_internal_errors() {
    assert_eq!(
        command_catalog_unavailable(),
        "Slash could not read this repository's `.slash/` configuration. Please try again later."
    );
}
```

Extend `renders_registered_metrics_in_text_format`:

```rust
metrics
    .command_catalog_loads_total
    .with_label_values(&["unavailable", "directory"])
    .inc();
assert!(output.contains(
    "slash_command_catalog_loads_total{outcome=\"unavailable\",stage=\"directory\"} 1"
));
```

Move `let output = metrics.render();` after all metric increments.

- [ ] **Step 2: Run focused tests to verify failure**

Run:

```powershell
cargo test -p slash-core messages::tests -- --nocapture
cargo test -p slash-server metrics::tests -- --nocapture
```

Expected: compilation fails because the message and metric do not exist.

- [ ] **Step 3: Add the safe message and bounded metric**

Add to `messages.rs`:

```rust
pub fn command_catalog_unavailable() -> String {
    "Slash could not read this repository's `.slash/` configuration. Please try again later."
        .to_string()
}
```

Add to `Metrics`:

```rust
pub command_catalog_loads_total: IntCounterVec,
```

Register it in `Metrics::new`:

```rust
let command_catalog_loads_total = register_int_counter_vec_with_registry!(
    "slash_command_catalog_loads_total",
    "Command catalog loads by terminal outcome and bounded processing stage.",
    &["outcome", "stage"],
    registry
)?;
```

Add `command_catalog_loads_total` to the returned struct. Only fixed values
such as `loaded`, `not_configured`, `invalid`, `unavailable`, `complete`,
`default_branch`, `branch_ref`, `directory`, `file`, and `validation` may be
used as labels. Repository names, paths, commands, and status codes belong in
logs, not metric labels.

- [ ] **Step 4: Run focused tests**

Run:

```powershell
cargo test -p slash-core messages::tests -- --nocapture
cargo test -p slash-server metrics::tests -- --nocapture
```

Expected: both test groups pass.

- [ ] **Step 5: Commit**

```powershell
git add crates/slash-core/src/messages.rs crates/slash-server/src/metrics.rs
git commit -m "feat: report command catalog failures" -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 4: Integrate Catalog Loading into Issue Comments

**Files:**
- Modify: `crates/slash-server/src/pipeline.rs:1-205, 386-413`
- Test: `crates/slash-server/src/pipeline.rs` test module

- [ ] **Step 1: Update the common happy-path mock to use a branch SHA**

In `mount_common`, add a default-branch ref response:

```rust
Mock::given(method("GET"))
    .and(path("/repos/acme/widgets/git/ref/heads/main"))
    .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "ref": "refs/heads/main",
        "node_id": "REF_1",
        "url": "https://api.github.com/repos/acme/widgets/git/refs/heads/main",
        "object": {
            "type": "commit",
            "sha": "config-sha",
            "url": "https://api.github.com/repos/acme/widgets/git/commits/config-sha"
        }
    })))
    .mount(server)
    .await;
```

Require `query_param("ref", "config-sha")` on the `.slash` directory and file
mocks. Import `query_param` alongside the existing Wiremock matchers.

- [ ] **Step 2: Write failing pipeline behavior tests**

Add tests proving operational failures are not unknown commands and invalid
configuration prevents invocation creation:

```rust
#[tokio::test]
async fn multiline_command_makes_no_github_requests() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://localhost/unused")
        .unwrap();
    let server = MockServer::start().await;
    let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
    let metrics = Metrics::new().unwrap();

    let result = handle_issue_comment(
        &ctx(&pool, &app, &server, &metrics),
        &issue_comment_payload("/echo hello\nsecond line"),
    )
    .await;

    assert!(result.is_ok());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[serial_test::serial(db)]
#[tokio::test]
async fn unavailable_catalog_gets_feedback_and_creates_no_invocation() {
    let Some(pool) = test_pool().await else { return };
    let server = MockServer::start().await;
    mount_common(&server, "deadbeef").await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/contents/.slash"))
        .and(query_param("ref", "config-sha"))
        .respond_with(ResponseTemplate::new(403).set_body_json(
            serde_json::json!({"message": "Forbidden"}),
        ))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/widgets/issues/7/comments"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
    let metrics = Metrics::new().unwrap();
    handle_issue_comment(&ctx(&pool, &app, &server, &metrics), &issue_comment_payload(
        "/echo hello",
    ))
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invocations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        metrics
            .command_catalog_loads_total
            .with_label_values(&["unavailable", "directory"])
            .get(),
        1
    );
}

#[serial_test::serial(db)]
#[tokio::test]
async fn invalid_catalog_is_reported_instead_of_partially_loaded() {
    let Some(pool) = test_pool().await else { return };
    let server = MockServer::start().await;
    mount_common(&server, "deadbeef").await;
    let invalid_yaml = "not: valid: yaml";
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/contents/.slash/echo.yml"))
        .and(query_param("ref", "config-sha"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "name": "echo.yml", "path": ".slash/echo.yml", "sha": "abc",
            "size": invalid_yaml.len(),
            "url": "https://api.github.com/repos/acme/widgets/contents/.slash/echo.yml",
            "type": "file", "encoding": "base64",
            "content": BASE64.encode(invalid_yaml),
            "_links": {
                "self": "https://api.github.com/repos/acme/widgets/contents/.slash/echo.yml",
                "git": null, "html": null
            }
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/widgets/issues/7/comments"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
    let metrics = Metrics::new().unwrap();
    handle_issue_comment(&ctx(&pool, &app, &server, &metrics), &issue_comment_payload(
        "/echo hello",
    ))
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invocations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
```

Wiremock priority `1` makes each failure response override the successful
catalog response installed by `mount_common` (default priority `5`). Returning
500 from the feedback mock is intentional: the test asserts that feedback was
attempted without needing to reproduce Octocrab's full `Comment` response
model.

- [ ] **Step 3: Run pipeline tests to verify failure**

Run:

```powershell
$env:SLASH_TEST_DATABASE_URL='postgres://postgres:slash@localhost:55432/slash_test'; cargo test -p slash-server pipeline::tests -- --nocapture
```

Expected: the new tests fail because the pipeline still converts catalog
errors to an empty list and still reads by branch name.

- [ ] **Step 4: Add catalog result feedback helpers**

Import the catalog module:

```rust
use crate::catalog::{
    CatalogError, CatalogOutcome, CommandCatalog, load_catalog, resolve_default_branch,
};
```

Add helpers that keep feedback and observability out of the main guard flow:

```rust
async fn report_catalog_error(
    ctx: &PipelineContext<'_>,
    client: &RepoClient,
    issue_number: u64,
    comment_id: u64,
    can_comment: bool,
    error: &CatalogError,
) {
    let outcome = match error {
        CatalogError::Invalid { .. } => "invalid",
        CatalogError::Unavailable { .. } => "unavailable",
    };
    ctx.metrics
        .command_catalog_loads_total
        .with_label_values(&[outcome, error.stage()])
        .inc();
    tracing::warn!(
        owner = %ctx.owner,
        repo = %ctx.repo,
        stage = error.stage(),
        path = error.path(),
        status = error.status_code(),
        error = %error,
        "command catalog load failed"
    );

    if can_comment {
        let body = match error {
            CatalogError::Invalid { details } => messages::config_error(details),
            CatalogError::Unavailable { .. } => messages::command_catalog_unavailable(),
        };
        if let Err(feedback_error) = client.create_comment(issue_number, &body).await {
            tracing::warn!(error = %feedback_error, "failed to post command catalog feedback");
        }
    }
    if let Err(feedback_error) = client
        .create_comment_reaction(comment_id, ReactionContent::Confused)
        .await
    {
        tracing::warn!(error = %feedback_error, "failed to react to command catalog failure");
    }
}
```

Use `Option<&str>` logging forms accepted by `tracing`; if the compiler rejects
the direct field form, log `path = ?error.path()` and
`status = ?error.status_code()` without changing the data.

- [ ] **Step 5: Replace silent loading with the immutable catalog flow**

Make the single-line rule explicit before parsing; `parse_comment` currently
parses only the first line and therefore cannot enforce this pipeline guard:

```rust
let body = payload.comment.body.clone().unwrap_or_default();
if body.contains('\n') || body.contains('\r') {
    return Ok(());
}
let Ok(Some(parsed)) = slash_command::parse_comment(&body) else {
    return Ok(());
};
```

Replace the `default_branch`, `config_files`, and `commands` block with:

```rust
let hinted_default_branch = pr
    .base
    .repo
    .as_ref()
    .and_then(|repo| repo.default_branch.as_deref());
let resolved = match resolve_default_branch(&client, hinted_default_branch).await {
    Ok(resolved) => resolved,
    Err(error) => {
        report_catalog_error(
            ctx,
            &client,
            payload.issue.number,
            payload.comment.id.0,
            can_comment,
            &error,
        )
        .await;
        return Ok(());
    }
};
let catalog = match load_catalog(&client, &resolved.sha).await {
    Ok(CatalogOutcome::Loaded(catalog)) => {
        ctx.metrics
            .command_catalog_loads_total
            .with_label_values(&["loaded", "complete"])
            .inc();
        catalog
    }
    Ok(CatalogOutcome::NotConfigured) => {
        ctx.metrics
            .command_catalog_loads_total
            .with_label_values(&["not_configured", "complete"])
            .inc();
        if can_comment {
            let _ = client
                .create_comment(
                    payload.issue.number,
                    &messages::installed_but_not_configured(),
                )
                .await;
        }
        let _ = client
            .create_comment_reaction(payload.comment.id.0, ReactionContent::Confused)
            .await;
        return Ok(());
    }
    Err(error) => {
        report_catalog_error(
            ctx,
            &client,
            payload.issue.number,
            payload.comment.id.0,
            can_comment,
            &error,
        )
        .await;
        return Ok(());
    }
};

let Some(validated) = catalog.find(&parsed.name) else {
    let names = catalog.names();
    if can_comment && slash_core::should_suggest_commands(&parsed.name, &names) {
        let _ = client
            .create_comment(
                payload.issue.number,
                &messages::unknown_command_suggestion(&parsed.name, &names),
            )
            .await;
        let _ = client
            .create_comment_reaction(payload.comment.id.0, ReactionContent::Confused)
            .await;
    }
    return Ok(());
};
```

Delete the old `load_commands` function. Keep command parsing before token
minting so rejected comments still make zero GitHub API calls.

- [ ] **Step 6: Run pipeline tests**

Run:

```powershell
$env:SLASH_TEST_DATABASE_URL='postgres://postgres:slash@localhost:55432/slash_test'; cargo test -p slash-server pipeline::tests -- --nocapture
```

Expected: all pipeline tests pass; unavailable and invalid catalogs create no
invocations and increment the expected metrics.

- [ ] **Step 7: Commit**

```powershell
git add crates/slash-server/src/pipeline.rs
git commit -m "fix: surface command catalog loading failures" -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 5: Apply the Same Catalog Rules to Check-Run Rerequests

**Files:**
- Modify: `crates/slash-server/src/correlation.rs:210-320`
- Test: `crates/slash-server/src/correlation.rs` test module

- [ ] **Step 1: Update rerequest mocks to resolve and read one SHA**

Add the branch-ref mock from Task 4 to rerequest fixtures. Require
`ref=config-sha` on `.slash` directory and file requests.

- [ ] **Step 2: Write a failing rerequest test**

Add a test proving unavailable configuration creates no new invocation and
completes the rerequested check run as `action_required`:

```rust
#[serial_test::serial(db)]
#[tokio::test]
async fn rerequest_with_unavailable_catalog_is_denied_without_a_new_invocation() {
    let Some(pool) = test_pool().await else { return };
    let id = Uuid::new_v4();
    invocations::claim(&pool, &sample(id)).await.unwrap();
    invocations::set_check_run_id(&pool, id, 55).await.unwrap();

    let server = MockServer::start().await;
    mount_rerequest_common(&server).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/collaborators/bob/permission"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "permission": "push", "role_name": "write",
            "user": {
                "login": "bob", "id": 9, "node_id": "n",
                "avatar_url": "https://avatars.githubusercontent.com/u/9",
                "gravatar_id": "", "url": "https://api.github.com/users/bob",
                "html_url": "https://github.com/bob",
                "followers_url": "https://api.github.com/users/bob/followers",
                "following_url": "https://api.github.com/users/bob/following{/other_user}",
                "gists_url": "https://api.github.com/users/bob/gists{/gist_id}",
                "starred_url": "https://api.github.com/users/bob/starred{/owner}{/repo}",
                "subscriptions_url": "https://api.github.com/users/bob/subscriptions",
                "organizations_url": "https://api.github.com/users/bob/orgs",
                "repos_url": "https://api.github.com/users/bob/repos",
                "events_url": "https://api.github.com/users/bob/events{/privacy}",
                "received_events_url": "https://api.github.com/users/bob/received_events",
                "type": "User", "site_admin": false,
                "permissions": {"admin": false, "push": true, "pull": true}
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/contents/.slash"))
        .and(query_param("ref", "config-sha"))
        .respond_with(ResponseTemplate::new(500).set_body_json(
            serde_json::json!({"message": "GitHub unavailable"}),
        ))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/widgets/check-runs/55"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
    let metrics = Metrics::new().unwrap();
    handle_check_run_rerequested(
        &ctx(&pool, &app, &server, &metrics),
        &check_run_rerequested_event(),
    )
        .await
        .unwrap();

    let attempts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM invocations WHERE comment_id = 100",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 1);
}
```

- [ ] **Step 3: Run rerequest tests to verify failure**

Run:

```powershell
$env:SLASH_TEST_DATABASE_URL='postgres://postgres:slash@localhost:55432/slash_test'; cargo test -p slash-server correlation::tests -- --nocapture
```

Expected: new tests fail because rerequests still use the deleted
`pipeline::load_commands` and silently return.

- [ ] **Step 4: Replace rerequest configuration loading**

Import and call the shared catalog functions:

```rust
let hinted_default_branch = pr
    .base
    .repo
    .as_ref()
    .and_then(|repo| repo.default_branch.as_deref());
let resolved = match crate::catalog::resolve_default_branch(
    &client,
    hinted_default_branch,
)
.await
{
    Ok(resolved) => resolved,
    Err(error) => {
        tracing::warn!(
            owner = %ctx.owner,
            repo = %ctx.repo,
            stage = error.stage(),
            path = ?error.path(),
            status = ?error.status_code(),
            error = %error,
            "rerequest command catalog resolution failed"
        );
        let _ = client
            .update_check_run(
                check_run.id,
                CheckRunUpdate {
                    status: Some(CheckRunStatus::Completed),
                    conclusion: Some(CheckRunConclusion::ActionRequired),
                    details_url: None,
                    output: Some((
                        "Re-run unavailable",
                        &messages::command_catalog_unavailable(),
                    )),
                },
            )
            .await;
        return Ok(());
    }
};
let catalog = match crate::catalog::load_catalog(&client, &resolved.sha).await {
    Ok(crate::catalog::CatalogOutcome::Loaded(catalog)) => catalog,
    Ok(crate::catalog::CatalogOutcome::NotConfigured) => {
        let _ = client
            .update_check_run(
                check_run.id,
                CheckRunUpdate {
                    status: Some(CheckRunStatus::Completed),
                    conclusion: Some(CheckRunConclusion::ActionRequired),
                    details_url: None,
                    output: Some((
                        "Re-run denied",
                        &messages::installed_but_not_configured(),
                    )),
                },
            )
            .await;
        return Ok(());
    }
    Err(error) => {
        let (title, body) = match &error {
            crate::catalog::CatalogError::Invalid { details } => {
                ("Re-run denied", messages::config_error(details))
            }
            crate::catalog::CatalogError::Unavailable { .. } => (
                "Re-run unavailable",
                messages::command_catalog_unavailable(),
            ),
        };
        tracing::warn!(
            owner = %ctx.owner,
            repo = %ctx.repo,
            stage = error.stage(),
            path = ?error.path(),
            status = ?error.status_code(),
            error = %error,
            "rerequest command catalog load failed"
        );
        let _ = client
            .update_check_run(
                check_run.id,
                CheckRunUpdate {
                    status: Some(CheckRunStatus::Completed),
                    conclusion: Some(CheckRunConclusion::ActionRequired),
                    details_url: None,
                    output: Some((title, &body)),
                },
            )
            .await;
        return Ok(());
    }
};
let Some(validated) = catalog.find(&original.command) else {
    return Ok(());
};
```

Increment `command_catalog_loads_total` with the same outcome/stage values as
the issue-comment pipeline before each return. Keep rerequests comment-free;
the check-run conclusion is their user-visible surface.

- [ ] **Step 5: Run correlation tests**

Run:

```powershell
$env:SLASH_TEST_DATABASE_URL='postgres://postgres:slash@localhost:55432/slash_test'; cargo test -p slash-server correlation::tests -- --nocapture
```

Expected: all correlation tests pass and no rerequest creates an invocation
when catalog resolution or loading fails.

- [ ] **Step 6: Commit**

```powershell
git add crates/slash-server/src/correlation.rs
git commit -m "fix: validate fresh config for check reruns" -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 6: Verify the Complete Behavior

**Files:**
- Verify all modified Rust and manifest files
- Compare against: `docs/superpowers/specs/2026-08-07-command-catalog-loading-design.md`

- [ ] **Step 1: Format and inspect the diff**

Run:

```powershell
cargo fmt --all
git --no-pager diff --check
git status --short
```

Expected: formatting succeeds, `diff --check` emits no output, and status lists
only intended files if formatting changed anything.

- [ ] **Step 2: Run focused crate tests**

Run:

```powershell
cargo test -p slash-github
cargo test -p slash-core
$env:SLASH_TEST_DATABASE_URL='postgres://postgres:slash@localhost:55432/slash_test'; cargo test -p slash-server
```

Expected: all tests pass with `SLASH_TEST_DATABASE_URL` set.

- [ ] **Step 3: Run workspace lint and tests**

Run:

```powershell
cargo clippy --workspace --all-targets -- -D warnings
$env:SLASH_TEST_DATABASE_URL='postgres://postgres:slash@localhost:55432/slash_test'; cargo test --workspace
```

Expected: Clippy reports no warnings and every workspace test passes.

- [ ] **Step 4: Confirm the original silent path is gone**

Run:

```powershell
rg 'Err\\(_\\) => Vec::new\\(\\)|let Ok\\(content_files\\).*else|load_commands' crates/slash-server/src
```

Expected: no matches. Inspect all `.slash` reads:

```powershell
rg 'get_content\\(\"\\.slash\"|load_catalog|resolve_default_branch' crates/slash-server/src
```

Expected: issue comments and rerequests route through `resolve_default_branch`
and `load_catalog`; no direct pipeline fallback to an empty vector remains.

- [ ] **Step 5: Commit formatting changes if necessary**

If `cargo fmt` changed tracked files:

```powershell
git add crates/slash-github/src/client.rs crates/slash-github/src/lib.rs crates/slash-server/Cargo.toml crates/slash-server/src/catalog.rs crates/slash-server/src/main.rs crates/slash-core/src/messages.rs crates/slash-server/src/metrics.rs crates/slash-server/src/pipeline.rs crates/slash-server/src/correlation.rs Cargo.lock
git commit -m "style: format command catalog loading" -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

If the worktree is clean, do not create an empty commit.

- [ ] **Step 6: Stop the test database**

Run:

```powershell
docker rm -f slash-command-catalog-pg
```

Expected: Docker removes only `slash-command-catalog-pg`; the environment
variable is removed from the current PowerShell process.
