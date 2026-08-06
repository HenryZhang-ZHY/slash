//! GitHub App authentication and per-repository installation tokens
//! (spec §7.5). JWT signing is octocrab's own (`AuthState::App` mints and
//! attaches a fresh App JWT on every request); this module adds what
//! octocrab does not provide: minting a token scoped to exactly one
//! repository with exactly the requested permissions, a cache keyed by
//! `(installation_id, repository_id, permission_set)`, and the 401
//! invalidate-and-remint path.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use octocrab::Octocrab;
use octocrab::models::AppId;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Debug, Clone, thiserror::Error)]
pub enum AppAuthError {
    #[error("failed to read private key file {path}: {reason}")]
    ReadKeyFile { path: String, reason: String },
    #[error("invalid RSA private key: {0}")]
    InvalidKey(String),
    #[error("failed to build GitHub client: {0}")]
    ClientBuild(String),
    #[error("failed to mint installation token: {0}")]
    Mint(String),
}

/// A minted, per-repository, least-permission installation token. Carries no
/// `Debug`/`Display` impl that would print the token value.
#[derive(Clone)]
pub struct InstallationToken {
    value: String,
    expires_at: DateTime<Utc>,
}

impl InstallationToken {
    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn is_valid(&self, buffer: chrono::Duration) -> bool {
        Utc::now() + buffer < self.expires_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TokenCacheKey {
    installation_id: u64,
    repository_id: u64,
    permissions: Vec<(String, String)>,
}

impl TokenCacheKey {
    pub fn new(installation_id: u64, repository_id: u64, permissions: &[(&str, &str)]) -> Self {
        let mut permissions: Vec<(String, String)> = permissions
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        permissions.sort();
        Self {
            installation_id,
            repository_id,
            permissions,
        }
    }
}

#[derive(Serialize)]
struct CreateAccessTokenBody<'a> {
    repository_ids: [u64; 1],
    permissions: HashMap<&'a str, &'a str>,
}

#[derive(Deserialize)]
struct CreateAccessTokenResponse {
    token: String,
    expires_at: DateTime<Utc>,
}

/// A GitHub App's identity plus its installation-token cache. One instance
/// is shared across a whole `slash-server` process.
pub struct GithubApp {
    app_client: Octocrab,
    tokens: Mutex<HashMap<TokenCacheKey, InstallationToken>>,
    /// The App's own bot login (`{slug}[bot]`), lazily fetched once and
    /// cached for the process lifetime — it never changes. Used as the
    /// `triggering_actor` predicate in the spec §6.3 missing-run-id poll,
    /// the one thing that keeps it from ever claiming a human-started run.
    bot_login: Mutex<Option<String>>,
}

impl GithubApp {
    pub fn new(app_id: u64, rsa_pem: &[u8]) -> Result<Self, AppAuthError> {
        Self::with_base_uri(app_id, rsa_pem, None)
    }

    pub fn from_pem_file(app_id: u64, path: &Path) -> Result<Self, AppAuthError> {
        let bytes = std::fs::read(path).map_err(|e| AppAuthError::ReadKeyFile {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
        Self::new(app_id, &bytes)
    }

    /// `base_uri` overrides the default `https://api.github.com`; used in
    /// tests to point at a mock server.
    pub fn with_base_uri(
        app_id: u64,
        rsa_pem: &[u8],
        base_uri: Option<&str>,
    ) -> Result<Self, AppAuthError> {
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(rsa_pem)
            .map_err(|e| AppAuthError::InvalidKey(e.to_string()))?;

        let mut builder = Octocrab::builder().app(AppId(app_id), key);
        if let Some(uri) = base_uri {
            builder = builder
                .base_uri(uri)
                .map_err(|e| AppAuthError::ClientBuild(e.to_string()))?;
        }
        let app_client = builder
            .build()
            .map_err(|e| AppAuthError::ClientBuild(e.to_string()))?;

        Ok(Self {
            app_client,
            tokens: Mutex::new(HashMap::new()),
            bot_login: Mutex::new(None),
        })
    }

    /// The App's own bot login, e.g. `slash[bot]` (spec §6.3). Fetched via
    /// `GET /app` (App-JWT authenticated, never an installation token) and
    /// cached after the first call.
    pub async fn bot_login(&self) -> Result<String, AppAuthError> {
        {
            let cached = self.bot_login.lock().await;
            if let Some(login) = cached.as_ref() {
                return Ok(login.clone());
            }
        }

        let app = self
            .app_client
            .current()
            .app()
            .await
            .map_err(|e| AppAuthError::Mint(e.to_string()))?;
        let slug = app
            .slug
            .ok_or_else(|| AppAuthError::Mint("the authenticated app has no slug".to_string()))?;
        let login = format!("{slug}[bot]");

        let mut cached = self.bot_login.lock().await;
        *cached = Some(login.clone());
        Ok(login)
    }

    /// Mints (or reuses a cached, still-valid) installation token scoped to
    /// exactly one repository with exactly the requested permissions (spec
    /// §7.5). Never mint a whole-installation token — the least-permission
    /// scoping is structural, not optional.
    pub async fn installation_token(
        &self,
        installation_id: u64,
        repository_id: u64,
        permissions: &[(&str, &str)],
    ) -> Result<String, AppAuthError> {
        let key = TokenCacheKey::new(installation_id, repository_id, permissions);

        if let Some(token) = self.cached_valid(&key).await {
            return Ok(token);
        }

        self.mint_and_cache(installation_id, repository_id, permissions, key)
            .await
    }

    /// The spec §7.5 401 path: invalidate the cached token and mint a fresh
    /// one. Callers should attempt this at most once per request.
    pub async fn remint_after_401(
        &self,
        installation_id: u64,
        repository_id: u64,
        permissions: &[(&str, &str)],
    ) -> Result<String, AppAuthError> {
        let key = TokenCacheKey::new(installation_id, repository_id, permissions);
        {
            let mut cache = self.tokens.lock().await;
            cache.remove(&key);
        }
        self.mint_and_cache(installation_id, repository_id, permissions, key)
            .await
    }

    async fn cached_valid(&self, key: &TokenCacheKey) -> Option<String> {
        let cache = self.tokens.lock().await;
        cache
            .get(key)
            .filter(|t| t.is_valid(chrono::Duration::seconds(60)))
            .map(|t| t.value().to_string())
    }

    async fn mint_and_cache(
        &self,
        installation_id: u64,
        repository_id: u64,
        permissions: &[(&str, &str)],
        key: TokenCacheKey,
    ) -> Result<String, AppAuthError> {
        let body = CreateAccessTokenBody {
            repository_ids: [repository_id],
            permissions: permissions.iter().copied().collect(),
        };

        let response: CreateAccessTokenResponse = self
            .app_client
            .post(
                format!("/app/installations/{installation_id}/access_tokens"),
                Some(&body),
            )
            .await
            .map_err(|e| AppAuthError::Mint(e.to_string()))?;

        let token = InstallationToken {
            value: response.token,
            expires_at: response.expires_at,
        };
        let value = token.value().to_string();

        let mut cache = self.tokens.lock().await;
        cache.insert(key, token);
        Ok(value)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TEST_KEY_PEM: &[u8] = include_bytes!("../tests/fixtures/test-app-key.pem");

    async fn app_against(server: &MockServer) -> GithubApp {
        GithubApp::with_base_uri(123, TEST_KEY_PEM, Some(&server.uri())).unwrap()
    }

    fn mint_response(token: &str) -> ResponseTemplate {
        ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "token": token,
            "expires_at": (Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
        }))
    }

    #[tokio::test]
    async fn mints_a_token_scoped_to_one_repo_and_permissions() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/app/installations/42/access_tokens"))
            .and(body_json(serde_json::json!({
                "repository_ids": [99],
                "permissions": {"checks": "write"}
            })))
            .respond_with(mint_response("tok_abc"))
            .expect(1)
            .mount(&server)
            .await;

        let app = app_against(&server).await;
        let token = app
            .installation_token(42, 99, &[("checks", "write")])
            .await
            .unwrap();
        assert_eq!(token, "tok_abc");
    }

    #[tokio::test]
    async fn reuses_a_cached_valid_token_without_a_second_mint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/app/installations/42/access_tokens"))
            .respond_with(mint_response("tok_abc"))
            .expect(1)
            .mount(&server)
            .await;

        let app = app_against(&server).await;
        let first = app
            .installation_token(42, 99, &[("checks", "write")])
            .await
            .unwrap();
        let second = app
            .installation_token(42, 99, &[("checks", "write")])
            .await
            .unwrap();
        assert_eq!(first, "tok_abc");
        assert_eq!(second, "tok_abc");
    }

    #[tokio::test]
    async fn different_permission_sets_get_different_cache_entries() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/app/installations/42/access_tokens"))
            .and(body_json(serde_json::json!({
                "repository_ids": [99],
                "permissions": {"checks": "write"}
            })))
            .respond_with(mint_response("tok_checks"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/app/installations/42/access_tokens"))
            .and(body_json(serde_json::json!({
                "repository_ids": [99],
                "permissions": {"contents": "read"}
            })))
            .respond_with(mint_response("tok_contents"))
            .expect(1)
            .mount(&server)
            .await;

        let app = app_against(&server).await;
        let checks = app
            .installation_token(42, 99, &[("checks", "write")])
            .await
            .unwrap();
        let contents = app
            .installation_token(42, 99, &[("contents", "read")])
            .await
            .unwrap();
        assert_eq!(checks, "tok_checks");
        assert_eq!(contents, "tok_contents");
    }

    #[tokio::test]
    async fn remint_after_401_evicts_the_cache_and_mints_again() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/app/installations/42/access_tokens"))
            .respond_with(mint_response("tok_first"))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/app/installations/42/access_tokens"))
            .respond_with(mint_response("tok_second"))
            .expect(1)
            .mount(&server)
            .await;

        let app = app_against(&server).await;
        let first = app
            .installation_token(42, 99, &[("checks", "write")])
            .await
            .unwrap();
        assert_eq!(first, "tok_first");

        let second = app
            .remint_after_401(42, 99, &[("checks", "write")])
            .await
            .unwrap();
        assert_eq!(second, "tok_second");

        let third = app
            .installation_token(42, 99, &[("checks", "write")])
            .await
            .unwrap();
        assert_eq!(third, "tok_second");
    }

    fn app_response_json(slug: &str) -> serde_json::Value {
        serde_json::json!({
            "id": 123, "slug": slug, "node_id": "n",
            "owner": {
                "login": "acme", "id": 1, "node_id": "n", "avatar_url": "https://avatars.githubusercontent.com/u/1",
                "gravatar_id": "", "url": "https://api.github.com/users/acme", "html_url": "https://github.com/acme",
                "followers_url": "https://api.github.com/users/acme/followers",
                "following_url": "https://api.github.com/users/acme/following{/other_user}",
                "gists_url": "https://api.github.com/users/acme/gists{/gist_id}",
                "starred_url": "https://api.github.com/users/acme/starred{/owner}{/repo}",
                "subscriptions_url": "https://api.github.com/users/acme/subscriptions",
                "organizations_url": "https://api.github.com/users/acme/orgs",
                "repos_url": "https://api.github.com/users/acme/repos",
                "events_url": "https://api.github.com/users/acme/events{/privacy}",
                "received_events_url": "https://api.github.com/users/acme/received_events",
                "type": "Organization", "site_admin": false
            },
            "name": "Slash", "external_url": "https://slash.example.com", "html_url": "https://github.com/apps/slash",
            "permissions": {"push": true, "pull": true},
            "events": ["issue_comment", "workflow_run", "check_run", "pull_request"]
        })
    }

    #[tokio::test]
    async fn bot_login_fetches_and_formats_the_app_slug_once() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/app"))
            .respond_with(ResponseTemplate::new(200).set_body_json(app_response_json("slash")))
            .expect(1)
            .mount(&server)
            .await;

        let app = app_against(&server).await;
        let first = app.bot_login().await.unwrap();
        let second = app.bot_login().await.unwrap();
        assert_eq!(first, "slash[bot]");
        assert_eq!(second, "slash[bot]");
    }
}
