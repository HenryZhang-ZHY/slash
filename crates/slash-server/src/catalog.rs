use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use slash_config::ValidatedCommand;
use slash_github::{ClientError, RepoClient};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedDefaultBranch {
    pub name: String,
    pub sha: String,
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
    Invalid { details: String },
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

pub(crate) async fn resolve_default_branch(
    client: &RepoClient,
    hinted_name: Option<&str>,
) -> Result<ResolvedDefaultBranch, CatalogError> {
    let name = match hinted_name.filter(|name| !name.is_empty()) {
        Some(name) => name.to_string(),
        None => client
            .get_default_branch()
            .await
            .map_err(|source| CatalogError::Unavailable {
                stage: "default_branch",
                path: None,
                source,
            })?,
    };
    let sha =
        client
            .get_branch_head_sha(&name)
            .await
            .map_err(|source| CatalogError::Unavailable {
                stage: "branch_ref",
                path: Some(name.clone()),
                source,
            })?;
    Ok(ResolvedDefaultBranch { name, sha })
}

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
            file.r#type == "file" && (file.name.ends_with(".yml") || file.name.ends_with(".yaml"))
        })
        .collect();
    if yaml_files.is_empty() {
        return Ok(CatalogOutcome::NotConfigured);
    }

    let mut total_bytes = 0u64;
    for file in &yaml_files {
        let Ok(size) = u64::try_from(file.size) else {
            return Err(CatalogError::Invalid {
                details: format!("{} has a negative size", file.name),
            });
        };
        let Some(next_total) = total_bytes.checked_add(size) else {
            return Err(CatalogError::Invalid {
                details: "configuration directory size overflowed".to_string(),
            });
        };
        total_bytes = next_total;
    }
    slash_config::check_directory_limits(yaml_files.len(), total_bytes).map_err(|error| {
        CatalogError::Invalid {
            details: error.to_string(),
        }
    })?;

    // Fetch each file, base64-decode, then hand the whole directory to the
    // pure `slash_config::assemble_directory` (parse + validate + duplicate
    // detection). Only the fetch/decode stays in the server.
    let mut files = Vec::with_capacity(yaml_files.len());
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
        let encoded = content
            .content
            .as_deref()
            .ok_or_else(|| CatalogError::Invalid {
                details: format!("{} has no base64 content", file.name),
            })?;
        let compact: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
        let bytes = BASE64
            .decode(compact.as_bytes())
            .map_err(|error| CatalogError::Invalid {
                details: format!("{} has invalid base64 content: {error}", file.name),
            })?;
        files.push((file.name.clone(), bytes));
    }

    match slash_config::assemble_directory(&files) {
        Ok(commands) => Ok(CatalogOutcome::Loaded(CommandCatalog { commands })),
        Err(validation_errors) => Err(CatalogError::Invalid {
            details: validation_errors.join("; "),
        }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use slash_github::RepoClient;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> RepoClient {
        RepoClient::with_base_uri("token", "acme", "widgets", Some(&server.uri())).unwrap()
    }

    fn ref_json(branch: &str, sha: &str) -> serde_json::Value {
        serde_json::json!({
            "ref": format!("refs/heads/{branch}"),
            "node_id": "REF_1",
            "url": format!(
                "https://api.github.com/repos/acme/widgets/git/refs/heads/{branch}"
            ),
            "object": {
                "type": "commit",
                "sha": sha,
                "url": format!(
                    "https://api.github.com/repos/acme/widgets/git/commits/{sha}"
                )
            }
        })
    }

    async fn mount_directory(server: &MockServer, files: &[&str]) {
        let items: Vec<_> = files
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "path": format!(".slash/{name}"),
                    "sha": format!("sha-{name}"),
                    "size": 10,
                    "url": format!(
                        "https://api.github.com/repos/acme/widgets/contents/.slash/{name}"
                    ),
                    "type": "file",
                    "_links": {
                        "self": format!(
                            "https://api.github.com/repos/acme/widgets/contents/.slash/{name}"
                        ),
                        "git": null,
                        "html": null
                    }
                })
            })
            .collect();
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/contents/.slash"))
            .and(query_param("ref", "sha-one"))
            .respond_with(ResponseTemplate::new(200).set_body_json(items))
            .mount(server)
            .await;
    }

    async fn mount_file(server: &MockServer, name: &str, yaml: &str) {
        Mock::given(method("GET"))
            .and(path(format!("/repos/acme/widgets/contents/.slash/{name}")))
            .and(query_param("ref", "sha-one"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": name,
                "path": format!(".slash/{name}"),
                "sha": format!("sha-{name}"),
                "size": yaml.len(),
                "url": format!(
                    "https://api.github.com/repos/acme/widgets/contents/.slash/{name}"
                ),
                "type": "file",
                "encoding": "base64",
                "content": BASE64.encode(yaml),
                "_links": {
                    "self": format!(
                        "https://api.github.com/repos/acme/widgets/contents/.slash/{name}"
                    ),
                    "git": null,
                    "html": null
                }
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn resolves_the_hinted_default_branch_to_a_sha() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/git/ref/heads/trunk"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ref_json("trunk", "sha-one")))
            .expect(1)
            .mount(&server)
            .await;

        let resolved = resolve_default_branch(&client(&server), Some("trunk"))
            .await
            .unwrap();
        assert_eq!(resolved.name, "trunk");
        assert_eq!(resolved.sha, "sha-one");
    }

    #[tokio::test]
    async fn falls_back_to_repository_metadata_when_the_hint_is_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"default_branch": "stable"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/git/ref/heads/stable"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ref_json("stable", "sha-two")))
            .mount(&server)
            .await;

        let resolved = resolve_default_branch(&client(&server), None)
            .await
            .unwrap();
        assert_eq!(resolved.name, "stable");
        assert_eq!(resolved.sha, "sha-two");
    }

    #[tokio::test]
    async fn missing_directory_is_not_configured() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/contents/.slash"))
            .and(query_param("ref", "sha-one"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(serde_json::json!({"message": "Not Found"})),
            )
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
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(serde_json::json!({"message": "Forbidden"})),
            )
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
                .respond_with(
                    ResponseTemplate::new(status)
                        .set_body_json(serde_json::json!({"message": "request failed"})),
                )
                .mount(&server)
                .await;

            let error = load_catalog(&client(&server), "sha-one").await.unwrap_err();
            assert!(matches!(error, CatalogError::Unavailable { .. }));
            assert_eq!(error.status_code(), Some(status));
        }
    }

    #[tokio::test]
    async fn invalid_yaml_invalidates_the_complete_catalog() {
        let server = MockServer::start().await;
        mount_directory(&server, &["broken.yml"]).await;
        mount_file(&server, "broken.yml", "not: valid: yaml").await;

        let error = load_catalog(&client(&server), "sha-one").await.unwrap_err();
        assert!(matches!(error, CatalogError::Invalid { .. }));
        assert!(error.to_string().contains("broken.yml"));
    }

    #[tokio::test]
    async fn duplicate_commands_invalidate_the_complete_catalog() {
        let server = MockServer::start().await;
        mount_directory(&server, &["one.yml", "two.yml"]).await;
        mount_file(&server, "one.yml", "command: deploy\nworkflow: one.yml\n").await;
        mount_file(&server, "two.yml", "command: deploy\nworkflow: two.yml\n").await;

        let error = load_catalog(&client(&server), "sha-one").await.unwrap_err();
        assert!(matches!(error, CatalogError::Invalid { .. }));
        assert!(error.to_string().contains("defined in both"));
    }

    #[tokio::test]
    async fn oversized_directory_is_rejected_before_file_fetches() {
        let server = MockServer::start().await;
        let names: Vec<_> = (0..=slash_config::MAX_FILES)
            .map(|index| format!("command-{index}.yml"))
            .collect();
        let refs: Vec<_> = names.iter().map(String::as_str).collect();
        mount_directory(&server, &refs).await;

        let error = load_catalog(&client(&server), "sha-one").await.unwrap_err();
        assert!(matches!(error, CatalogError::Invalid { .. }));
        assert!(error.to_string().contains("exceeding the limit"));
    }

    #[tokio::test]
    async fn loads_all_commands_from_one_immutable_sha() {
        let server = MockServer::start().await;
        mount_directory(&server, &["deploy.yml"]).await;
        mount_file(
            &server,
            "deploy.yml",
            "command: deploy\nworkflow: deploy.yml\n",
        )
        .await;

        let CatalogOutcome::Loaded(catalog) =
            load_catalog(&client(&server), "sha-one").await.unwrap()
        else {
            panic!("expected a loaded catalog");
        };
        assert_eq!(catalog.find("deploy").unwrap().workflow, "deploy.yml");
        assert_eq!(catalog.names(), vec!["deploy"]);
    }

    fn catalog_with(names: &[&str]) -> CommandCatalog {
        let commands = names
            .iter()
            .map(|name| ValidatedCommand {
                command: name.to_string(),
                description: None,
                permission: slash_config::Permission::Write,
                workflow: format!("{name}.yml"),
                args: Vec::new(),
            })
            .collect();
        CommandCatalog { commands }
    }

    #[test]
    fn catalog_find_returns_the_matching_command() {
        let catalog = catalog_with(&["deploy", "lint"]);
        assert_eq!(catalog.find("deploy").unwrap().workflow, "deploy.yml");
        assert_eq!(catalog.find("lint").unwrap().command, "lint");
    }

    #[test]
    fn catalog_find_returns_none_for_an_unknown_command() {
        let catalog = catalog_with(&["deploy"]);
        assert!(catalog.find("missing").is_none());
    }

    #[test]
    fn catalog_names_lists_commands_in_order() {
        let catalog = catalog_with(&["deploy", "lint", "test"]);
        assert_eq!(catalog.names(), vec!["deploy", "lint", "test"]);
    }

    #[test]
    fn catalog_names_is_empty_for_an_empty_catalog() {
        let catalog = catalog_with(&[]);
        assert!(catalog.names().is_empty());
    }
}
