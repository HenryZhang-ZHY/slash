//! GitHub App user authentication and account connection.
//!
//! GitHub App user access tokens use the OAuth web application transport but
//! are authorized by fine-grained App permissions, not OAuth scopes. Sign-in
//! and connection deliberately use different account-resolution policies. See
//! `docs/design/authentication.md`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::header::{COOKIE, HeaderValue, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{self, AuthSecret};
use crate::github_user_access;
use crate::identity::{self, AuthenticatedIdentity, IdentityError};
use crate::userapi::{SessionUserId, UserId, api_error};

type HmacSha256 = Hmac<Sha256>;

const GITHUB_AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_URL: &str = "https://api.github.com/user";
const GITHUB_API_VERSION: &str = "2026-03-10";
const GITHUB_CONNECTION_ID: Uuid = Uuid::from_u128(1);
const STATE_COOKIE: &str = "slash_github_oauth";
const STATE_TTL_SECS: u64 = 10 * 60;

#[derive(Clone)]
struct OauthEndpoints {
    authorize: Arc<str>,
    token: Arc<str>,
    user: Arc<str>,
}

impl Default for OauthEndpoints {
    fn default() -> Self {
        Self {
            authorize: Arc::from(GITHUB_AUTHORIZE_URL),
            token: Arc::from(GITHUB_TOKEN_URL),
            user: Arc::from(GITHUB_USER_URL),
        }
    }
}

/// Configuration for the GitHub App user authorization flow.
#[derive(Clone)]
pub struct OauthState {
    client_id: Arc<str>,
    client_secret: Arc<str>,
    base_url: Arc<str>,
    auth_secret: AuthSecret,
    endpoints: OauthEndpoints,
}

impl OauthState {
    pub fn new(
        client_id: Arc<str>,
        client_secret: Arc<str>,
        base_url: Arc<str>,
        auth_secret: AuthSecret,
    ) -> Self {
        Self {
            client_id,
            client_secret,
            base_url,
            auth_secret,
            endpoints: OauthEndpoints::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum OauthIntent {
    SignIn,
    Connect,
    RepositoryAccess,
}

#[derive(Debug, Deserialize, Serialize)]
struct StateClaims {
    intent: OauthIntent,
    nonce: String,
    pkce_verifier: String,
    redirect_uri: String,
    exp: u64,
    user_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubTokenResponse {
    access_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
}

struct GithubUserCredential {
    access_token: String,
    ttl_secs: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GithubUser {
    id: i64,
    login: String,
    name: Option<String>,
}

/// `GET /api/auth/github/sign-in` starts an unauthenticated GitHub sign-in.
pub async fn start_github_sign_in(State(state): State<crate::AppState>) -> Response {
    start_oauth(state, OauthIntent::SignIn, None).await
}

/// `POST /api/auth/github/connect` starts connection for the current user.
pub async fn start_github_connect(State(state): State<crate::AppState>, user: UserId) -> Response {
    start_oauth(state, OauthIntent::Connect, Some(user.0)).await
}

/// `POST /api/auth/github/repository-access` refreshes the short-lived
/// repository-discovery credential without changing the linked identity.
pub async fn start_github_repository_access(
    State(state): State<crate::AppState>,
    user: SessionUserId,
) -> Response {
    start_oauth(state, OauthIntent::RepositoryAccess, Some(user.0)).await
}

async fn start_oauth(
    state: crate::AppState,
    intent: OauthIntent,
    user_id: Option<Uuid>,
) -> Response {
    let oauth = match &state.github_oauth {
        Some(oauth) => oauth,
        None => return api_error(StatusCode::NOT_FOUND, "github login is not configured"),
    };
    let redirect_uri = format!(
        "{}/api/auth/github/callback",
        oauth.base_url.trim_end_matches('/')
    );
    let nonce = random_token();
    let pkce_verifier = random_token();
    let pkce_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(pkce_verifier.as_bytes()));
    let claims = StateClaims {
        intent,
        nonce: nonce.clone(),
        pkce_verifier,
        redirect_uri: redirect_uri.clone(),
        exp: now_secs() + STATE_TTL_SECS,
        user_id,
    };
    let state_cookie = match sign_state(&oauth.auth_secret, &claims) {
        Ok(cookie) => cookie,
        Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "auth setup failed"),
    };
    let location = authorization_url(oauth, &redirect_uri, &nonce, &pkce_challenge);
    let mut response = redirect(&location);
    append_cookie(&mut response, &state_cookie_value(oauth, &state_cookie));
    response
}

fn authorization_url(
    oauth: &OauthState,
    redirect_uri: &str,
    nonce: &str,
    pkce_challenge: &str,
) -> String {
    format!(
        "{}?client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256&prompt=select_account",
        oauth.endpoints.authorize,
        urlencoding::encode(&oauth.client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(nonce),
        urlencoding::encode(pkce_challenge),
    )
}

/// GitHub callback for both intents. All browser-facing failures are bounded
/// redirects; internal and GitHub response details are logged only server-side.
pub async fn handle_github_callback(
    State(state): State<crate::AppState>,
    axum::extract::Query(params): axum::extract::Query<CallbackParams>,
    parts: axum::http::request::Parts,
) -> Response {
    let oauth = match &state.github_oauth {
        Some(oauth) => oauth,
        None => return api_error(StatusCode::NOT_FOUND, "github login is not configured"),
    };
    let cookie_header = parts
        .headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok());
    let signed_state = cookie_header.and_then(extract_state_cookie);
    let claims = match signed_state
        .as_deref()
        .ok_or(auth::AuthError::MissingToken)
        .and_then(|token| verify_state(&oauth.auth_secret, token))
    {
        Ok(claims) => claims,
        Err(error) => {
            tracing::warn!(%error, "github oauth callback rejected state cookie");
            return callback_error(oauth, None, "invalid_state");
        }
    };
    if params.state.as_deref() != Some(claims.nonce.as_str()) {
        tracing::warn!(intent = ?claims.intent, "github oauth callback state mismatch");
        return callback_error(oauth, Some(claims.intent), "invalid_state");
    }
    if let Some(error) = params.error.as_deref() {
        tracing::info!(intent = ?claims.intent, github_error = %error, "github oauth was not authorized");
        return callback_error(oauth, Some(claims.intent), "access_denied");
    }
    let code = match params.code.as_deref() {
        Some(code) => code,
        None => return callback_error(oauth, Some(claims.intent), "missing_code"),
    };

    let connected_user = if claims.intent != OauthIntent::SignIn {
        match connection_session_user(&oauth.auth_secret, cookie_header, claims.user_id) {
            Ok(user_id) => Some(user_id),
            Err(error) => {
                tracing::warn!(%error, "github connection callback lost its Slash session");
                return callback_error(oauth, Some(claims.intent), "session_expired");
            }
        }
    } else {
        None
    };

    let credential = match exchange_code(oauth, code, &claims).await {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(%error, intent = ?claims.intent, "github oauth token exchange failed");
            return callback_error(oauth, Some(claims.intent), "github_unavailable");
        }
    };
    let profile = match fetch_profile(oauth, &credential.access_token).await {
        Ok(profile) => profile,
        Err(error) => {
            tracing::error!(%error, intent = ?claims.intent, "github oauth profile fetch failed");
            return callback_error(oauth, Some(claims.intent), error);
        }
    };

    match claims.intent {
        OauthIntent::SignIn => finish_sign_in(&state, oauth, &profile, &credential).await,
        OauthIntent::Connect | OauthIntent::RepositoryAccess => {
            let Some(user_id) = connected_user else {
                return callback_error(oauth, Some(claims.intent), "session_expired");
            };
            finish_connection(
                &state.pool,
                oauth,
                user_id,
                &profile,
                &credential,
                claims.intent,
            )
            .await
        }
    }
}

async fn exchange_code(
    oauth: &OauthState,
    code: &str,
    claims: &StateClaims,
) -> Result<GithubUserCredential, &'static str> {
    let response = reqwest::Client::new()
        .post(oauth.endpoints.token.as_ref())
        .header("accept", "application/json")
        .form(&[
            ("client_id", oauth.client_id.as_ref()),
            ("client_secret", oauth.client_secret.as_ref()),
            ("code", code),
            ("redirect_uri", claims.redirect_uri.as_str()),
            ("code_verifier", claims.pkce_verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|_| "token_request")?;
    if !response.status().is_success() {
        return Err("token_status");
    }
    let payload: GithubTokenResponse = response.json().await.map_err(|_| "token_payload")?;
    if payload.error.is_some() {
        return Err("token_rejected");
    }
    Ok(GithubUserCredential {
        access_token: payload.access_token.ok_or("token_missing")?,
        ttl_secs: payload.expires_in,
    })
}

async fn fetch_profile(
    oauth: &OauthState,
    access_token: &str,
) -> Result<AuthenticatedIdentity, &'static str> {
    let client = reqwest::Client::new();
    let user_response = github_get(&client, oauth.endpoints.user.as_ref(), access_token)
        .await
        .map_err(|_| "github_unavailable")?;
    if !user_response.status().is_success() {
        return Err("github_unavailable");
    }
    let user: GithubUser = user_response.json().await.map_err(|_| "invalid_profile")?;
    let profile = serde_json::to_value(&user).map_err(|_| "invalid_profile")?;
    let display_name = user
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| user.login.clone());
    Ok(AuthenticatedIdentity {
        connection_id: GITHUB_CONNECTION_ID,
        subject: user.id.to_string(),
        username: user.login,
        display_name,
        profile,
    })
}

async fn github_get(
    client: &reqwest::Client,
    url: &str,
    access_token: &str,
) -> Result<reqwest::Response, reqwest::Error> {
    client
        .get(url)
        .bearer_auth(access_token)
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", GITHUB_API_VERSION)
        .header("user-agent", "slash-server")
        .send()
        .await
}

async fn finish_sign_in(
    state: &crate::AppState,
    oauth: &OauthState,
    profile: &AuthenticatedIdentity,
    credential: &GithubUserCredential,
) -> Response {
    let user_id = match identity::sign_in_or_create(&state.pool, profile).await {
        Ok(user_id) => user_id,
        Err(IdentityError::UserUnavailable) => {
            return callback_error(oauth, Some(OauthIntent::SignIn), "account_unavailable");
        }
        Err(error) => {
            tracing::error!(%error, "github sign-in persistence failed");
            return callback_error(oauth, Some(OauthIntent::SignIn), "internal");
        }
    };
    let token = match auth::sign_token(&state.auth_secret, user_id) {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(%error, "github sign-in session creation failed");
            return callback_error(oauth, Some(OauthIntent::SignIn), "internal");
        }
    };
    let destination = match account_destination(&state.pool, user_id).await {
        Ok(destination) => destination,
        Err(error) => {
            tracing::error!(%error, "github sign-in destination lookup failed");
            return callback_error(oauth, Some(OauthIntent::SignIn), "internal");
        }
    };
    let mut response = redirect(destination);
    append_cookie(&mut response, &auth::set_cookie_value(&token));
    if let Err(error) = append_access_cookie(&mut response, oauth, user_id, profile, credential) {
        tracing::error!(%error, "github discovery credential encryption failed");
        return callback_error(oauth, Some(OauthIntent::SignIn), "internal");
    }
    append_cookie(&mut response, &clear_state_cookie_value(oauth));
    response
}

async fn finish_connection(
    pool: &PgPool,
    oauth: &OauthState,
    user_id: Uuid,
    profile: &AuthenticatedIdentity,
    credential: &GithubUserCredential,
    intent: OauthIntent,
) -> Response {
    match identity::connect(pool, user_id, profile).await {
        Ok(()) => {
            let mut response = callback_success(oauth, intent);
            if let Err(error) =
                append_access_cookie(&mut response, oauth, user_id, profile, credential)
            {
                tracing::error!(%error, "github discovery credential encryption failed");
                return callback_error(oauth, Some(intent), "internal");
            }
            response
        }
        Err(IdentityError::IdentityInUse) => callback_error(oauth, Some(intent), "identity_in_use"),
        Err(IdentityError::ConnectionAlreadyLinked) => {
            callback_error(oauth, Some(intent), "different_identity_connected")
        }
        Err(IdentityError::UserUnavailable) => {
            callback_error(oauth, Some(intent), "session_expired")
        }
        Err(error) => {
            tracing::error!(%error, "github account connection persistence failed");
            callback_error(oauth, Some(OauthIntent::Connect), "internal")
        }
    }
}

fn append_access_cookie(
    response: &mut Response,
    oauth: &OauthState,
    user_id: Uuid,
    profile: &AuthenticatedIdentity,
    credential: &GithubUserCredential,
) -> Result<(), auth::AuthError> {
    let exp = github_user_access::expires_at(credential.ttl_secs);
    let sealed = github_user_access::seal(
        &oauth.auth_secret,
        &github_user_access::Credential {
            user_id,
            github_subject: profile.subject.clone(),
            access_token: credential.access_token.clone(),
            exp,
        },
    )?;
    let ttl_secs = exp.saturating_sub(now_secs());
    append_cookie(
        response,
        &github_user_access::cookie_value(&sealed, ttl_secs),
    );
    Ok(())
}

async fn account_destination(pool: &PgPool, user_id: Uuid) -> Result<&'static str, sqlx::Error> {
    let has_team: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM team_members WHERE user_id = $1)")
            .bind(user_id)
            .fetch_one(pool)
            .await?;
    Ok(if has_team { "/" } else { "/onboarding" })
}

fn connection_session_user(
    secret: &AuthSecret,
    cookie_header: Option<&str>,
    expected_user_id: Option<Uuid>,
) -> Result<Uuid, auth::AuthError> {
    let expected = expected_user_id.ok_or(auth::AuthError::InvalidToken)?;
    let token =
        auth::session_token_from_header(cookie_header).ok_or(auth::AuthError::MissingToken)?;
    let actual = auth::verify_token(secret, &token)?;
    if actual != expected {
        return Err(auth::AuthError::InvalidToken);
    }
    Ok(actual)
}

fn sign_state(secret: &AuthSecret, claims: &StateClaims) -> Result<String, auth::AuthError> {
    let json = serde_json::to_vec(claims).map_err(|_| auth::AuthError::Encode)?;
    let payload = URL_SAFE_NO_PAD.encode(json);
    let signature = state_mac(secret, payload.as_bytes());
    Ok(format!("{payload}.{signature}"))
}

fn verify_state(secret: &AuthSecret, token: &str) -> Result<StateClaims, auth::AuthError> {
    let (payload, signature) = token.split_once('.').ok_or(auth::AuthError::InvalidToken)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| auth::AuthError::InvalidToken)?;
    let mut mac =
        HmacSha256::new_from_slice(secret.0.as_bytes()).map_err(|_| auth::AuthError::Internal)?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| auth::AuthError::InvalidToken)?;
    let json = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| auth::AuthError::InvalidToken)?;
    let claims: StateClaims =
        serde_json::from_slice(&json).map_err(|_| auth::AuthError::InvalidToken)?;
    if claims.exp < now_secs() {
        return Err(auth::AuthError::ExpiredToken);
    }
    Ok(claims)
}

fn state_mac(secret: &AuthSecret, payload: &[u8]) -> String {
    #[allow(clippy::expect_used)]
    let mut mac =
        HmacSha256::new_from_slice(secret.0.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload);
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn random_token() -> String {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut bytes = [0_u8; 32];
    bytes[..16].copy_from_slice(first.as_bytes());
    bytes[16..].copy_from_slice(second.as_bytes());
    URL_SAFE_NO_PAD.encode(bytes)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn extract_state_cookie(cookie_header: &str) -> Option<String> {
    cookie_header.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == STATE_COOKIE).then(|| value.to_string())
    })
}

fn state_cookie_value(oauth: &OauthState, token: &str) -> String {
    format!(
        "{STATE_COOKIE}={token}; Path=/api/auth/github/callback; HttpOnly; SameSite=Lax; Max-Age={STATE_TTL_SECS}{}",
        secure_cookie_suffix(oauth)
    )
}

fn clear_state_cookie_value(oauth: &OauthState) -> String {
    format!(
        "{STATE_COOKIE}=; Path=/api/auth/github/callback; HttpOnly; SameSite=Lax; Max-Age=0{}",
        secure_cookie_suffix(oauth)
    )
}

fn secure_cookie_suffix(oauth: &OauthState) -> &'static str {
    if oauth.base_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    }
}

fn append_cookie(response: &mut Response, value: &str) {
    #[allow(clippy::expect_used)]
    let value = HeaderValue::from_str(value).expect("generated cookie is valid ASCII");
    response.headers_mut().append(SET_COOKIE, value);
}

fn redirect(location: &str) -> Response {
    let mut headers = HeaderMap::new();
    let value = HeaderValue::from_str(location)
        .unwrap_or_else(|_| HeaderValue::from_static("/login?github_error=internal"));
    headers.insert(LOCATION, value);
    (StatusCode::SEE_OTHER, headers).into_response()
}

fn callback_success(oauth: &OauthState, intent: OauthIntent) -> Response {
    let location = match intent {
        OauthIntent::SignIn => "/",
        OauthIntent::Connect => "/settings?github=connected",
        OauthIntent::RepositoryAccess => "/activity?github=authorized",
    };
    let mut response = redirect(location);
    append_cookie(&mut response, &clear_state_cookie_value(oauth));
    response
}

fn callback_error(oauth: &OauthState, intent: Option<OauthIntent>, code: &'static str) -> Response {
    let location = match intent {
        Some(OauthIntent::Connect) => format!("/settings?github=error&reason={code}"),
        Some(OauthIntent::RepositoryAccess) => {
            format!("/activity?github=error&reason={code}")
        }
        _ => format!("/login?github_error={code}"),
    };
    let mut response = redirect(&location);
    append_cookie(&mut response, &clear_state_cookie_value(oauth));
    response
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn secret() -> AuthSecret {
        AuthSecret(Arc::from("test-oauth-secret"))
    }

    fn oauth() -> OauthState {
        OauthState::new(
            Arc::from("client-id"),
            Arc::from("client-secret"),
            Arc::from("https://slash.example"),
            secret(),
        )
    }

    fn claims(intent: OauthIntent, user_id: Option<Uuid>) -> StateClaims {
        StateClaims {
            intent,
            nonce: "nonce".into(),
            pkce_verifier: "verifier".into(),
            redirect_uri: "https://slash.example/api/auth/github/callback".into(),
            exp: now_secs() + 60,
            user_id,
        }
    }

    fn profile(subject: &str) -> AuthenticatedIdentity {
        AuthenticatedIdentity {
            connection_id: GITHUB_CONNECTION_ID,
            subject: subject.into(),
            username: format!("user-{subject}"),
            display_name: format!("User {subject}"),
            profile: serde_json::json!({"id": subject, "login": format!("user-{subject}")}),
        }
    }

    #[test]
    fn oauth_state_started_on_one_replica_verifies_on_another() {
        let user_id = Uuid::new_v4();
        let token = sign_state(&secret(), &claims(OauthIntent::Connect, Some(user_id))).unwrap();
        let decoded = verify_state(&secret(), &token).unwrap();
        assert_eq!(decoded.intent, OauthIntent::Connect);
        assert_eq!(decoded.user_id, Some(user_id));
        assert_eq!(decoded.pkce_verifier, "verifier");
    }

    #[test]
    fn authorization_url_exposes_only_nonce_and_pkce_challenge() {
        let location = authorization_url(
            &oauth(),
            "https://slash.example/api/auth/github/callback",
            "public-nonce",
            "public-challenge",
        );
        assert!(location.contains("state=public-nonce"));
        assert!(location.contains("code_challenge=public-challenge"));
        assert!(location.contains("code_challenge_method=S256"));
        assert!(!location.contains("scope="));
        assert!(!location.contains("client-secret"));
        assert!(!location.contains("pkce_verifier"));
    }

    #[test]
    fn unknown_intent_and_tampered_state_fail_closed() {
        let json = serde_json::json!({
            "intent": "legacy_link",
            "nonce": "nonce",
            "pkce_verifier": "verifier",
            "redirect_uri": "https://slash.example/api/auth/github/callback",
            "exp": now_secs() + 60,
            "user_id": null
        });
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json).unwrap());
        let token = format!("{payload}.{}", state_mac(&secret(), payload.as_bytes()));
        assert!(verify_state(&secret(), &token).is_err());

        let valid = sign_state(&secret(), &claims(OauthIntent::SignIn, None)).unwrap();
        assert!(verify_state(&secret(), &format!("{valid}x")).is_err());
    }

    #[test]
    fn connection_is_bound_to_the_same_live_session() {
        let user_id = Uuid::new_v4();
        let token = auth::sign_token(&secret(), user_id).unwrap();
        let cookie = format!("slash_session={token}");
        assert_eq!(
            connection_session_user(&secret(), Some(&cookie), Some(user_id)).unwrap(),
            user_id
        );
        assert!(connection_session_user(&secret(), Some(&cookie), Some(Uuid::new_v4())).is_err());
        assert!(connection_session_user(&secret(), None, Some(user_id)).is_err());
    }

    #[test]
    fn discovery_credential_cookie_is_bound_to_user_and_subject() {
        let user_id = Uuid::new_v4();
        let mut response = redirect("/activity");
        let credential = GithubUserCredential {
            access_token: "ghu_discovery".into(),
            ttl_secs: Some(60),
        };

        append_access_cookie(
            &mut response,
            &oauth(),
            user_id,
            &profile("42"),
            &credential,
        )
        .unwrap();

        let set_cookie = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .find_map(|value| {
                let value = value.to_str().unwrap();
                value
                    .strip_prefix("slash_github_access=")
                    .and_then(|rest| rest.split(';').next())
            })
            .unwrap();
        let opened = github_user_access::open(&secret(), set_cookie).unwrap();
        assert_eq!(opened.user_id, user_id);
        assert_eq!(opened.github_subject, "42");
        assert_eq!(opened.access_token, "ghu_discovery");
    }

    #[test]
    fn repository_access_has_activity_success_and_error_destinations() {
        let success = callback_success(&oauth(), OauthIntent::RepositoryAccess);
        assert_eq!(
            success.headers().get(LOCATION).unwrap(),
            "/activity?github=authorized"
        );
        let error = callback_error(
            &oauth(),
            Some(OauthIntent::RepositoryAccess),
            "access_denied",
        );
        assert_eq!(
            error.headers().get(LOCATION).unwrap(),
            "/activity?github=error&reason=access_denied"
        );
    }

    #[tokio::test]
    async fn github_profile_requires_only_the_authenticated_user_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 42,
                "login": "octocat",
                "name": "The Octocat"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let mut oauth = oauth();
        oauth.endpoints.user = Arc::from(format!("{}/user", server.uri()));

        let identity = fetch_profile(&oauth, "user-access-token").await.unwrap();

        assert_eq!(identity.connection_id, GITHUB_CONNECTION_ID);
        assert_eq!(identity.subject, "42");
        assert_eq!(identity.username, "octocat");
        assert_eq!(identity.display_name, "The Octocat");
    }

    async fn test_pool() -> Option<PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = crate::db::connect(&url).await.unwrap();
        crate::db::migrate(&pool).await.unwrap();
        sqlx::query("TRUNCATE users CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        Some(pool)
    }

    async fn password_user(pool: &PgPool, email: &str) -> Uuid {
        let user_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, display_name, status)
             VALUES ($1, 'Existing', 'active')",
        )
        .bind(user_id)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO password_credentials (user_id, normalized_email, password_hash)
             VALUES ($1, $2, 'hash')",
        )
        .bind(user_id)
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
        user_id
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn github_sign_in_does_not_use_email_credentials_as_identity() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let password_user_id = password_user(&pool, "same@example.com").await;
        let github_user_id = identity::sign_in_or_create(&pool, &profile("101"))
            .await
            .unwrap();
        assert_ne!(github_user_id, password_user_id);
        let identities: i64 = sqlx::query_scalar("SELECT count(*) FROM user_identities")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(identities, 1);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn github_sign_in_creates_then_reuses_the_stable_subject() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let first = identity::sign_in_or_create(&pool, &profile("102"))
            .await
            .unwrap();
        let mut renamed = profile("102");
        renamed.username = "renamed".into();
        let second = identity::sign_in_or_create(&pool, &renamed).await.unwrap();
        assert_eq!(first, second);
        let password_credentials: i64 =
            sqlx::query_scalar("SELECT count(*) FROM password_credentials WHERE user_id = $1")
                .bind(first)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(password_credentials, 0);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn equal_subjects_from_different_connections_are_distinct_identities() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let first = identity::sign_in_or_create(&pool, &profile("same-subject"))
            .await
            .unwrap();
        let connection_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO auth_connections
                (id, connection_key, kind, protocol, issuer)
             VALUES ($1, $2, 'oidc', 'oidc', 'https://idp.example')",
        )
        .bind(connection_id)
        .bind(format!("other-oidc-{connection_id}"))
        .execute(&pool)
        .await
        .unwrap();
        let mut other = profile("same-subject");
        other.connection_id = connection_id;

        let second = identity::sign_in_or_create(&pool, &other).await.unwrap();

        assert_ne!(first, second);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn connection_is_idempotent_but_never_replaces_an_identity() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let user_id = password_user(&pool, "owner@example.com").await;
        identity::connect(&pool, user_id, &profile("103"))
            .await
            .unwrap();
        identity::connect(&pool, user_id, &profile("103"))
            .await
            .unwrap();
        let result = identity::connect(&pool, user_id, &profile("104")).await;
        assert!(matches!(
            result,
            Err(IdentityError::ConnectionAlreadyLinked)
        ));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn one_github_identity_cannot_connect_to_two_users() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let first = password_user(&pool, "first@example.com").await;
        let second = password_user(&pool, "second@example.com").await;
        identity::connect(&pool, first, &profile("105"))
            .await
            .unwrap();
        let result = identity::connect(&pool, second, &profile("105")).await;
        assert!(matches!(result, Err(IdentityError::IdentityInUse)));
    }
}
