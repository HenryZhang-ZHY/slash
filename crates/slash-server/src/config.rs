//! Process configuration from environment variables (spec §7.1). No
//! `Debug`/`Display` impl: the webhook secret and database URL (which may
//! carry a password) must never end up in a log line via a stray `{:?}`.

use std::path::PathBuf;

#[derive(Debug, Clone, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} must be set")]
    MissingEnvVar(&'static str),
    #[error("{0} must be a valid integer: {1}")]
    InvalidInt(&'static str, String),
    #[error("{0} must be a path; could not read: {1}")]
    UnreadableFile(&'static str, String),
}

pub struct ServerConfig {
    pub github_app_id: u64,
    pub github_private_key_path: PathBuf,
    pub webhook_secret: String,
    pub database_url: String,
    pub auth_secret: String,
}

impl ServerConfig {
    /// Reads and validates configuration. Refuses to start (returns `Err`)
    /// if the webhook secret is unset — signature verification is never
    /// skipped (spec §7.3).
    pub fn from_env() -> Result<Self, ConfigError> {
        let lookup = |name: &str| std::env::var(name).ok();
        // Secrets may come from a file (`*_PATH`) so they never show up in
        // `ps aux`/env dumps — mirroring how the GitHub App private key is
        // handled. Falls back to the inline env var when the path variant is
        // unset.
        let file_reader = |path: &str| std::fs::read_to_string(path);
        Self::from_lookup_with_reader(&lookup, file_reader)
    }

    /// Testable core: takes a lookup function instead of touching the real
    /// process environment, so tests don't need `std::env::set_var` (which
    /// is `unsafe` and process-global, and this workspace forbids `unsafe`
    /// entirely). Test-only: production uses `from_env`.
    #[cfg(test)]
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        Self::from_lookup_with_reader(&lookup, |_path| {
            Err(std::io::Error::other("no file reader available"))
        })
    }

    fn from_lookup_with_reader(
        lookup: &impl Fn(&str) -> Option<String>,
        mut read_file: impl FnMut(&str) -> std::io::Result<String>,
    ) -> Result<Self, ConfigError> {
        // Secrets may come from a file (`*_PATH`) so they never show up in
        // `ps aux`/env dumps / `docker inspect` — mirroring how the GitHub App
        // private key is handled. Falls back to the inline env var when the
        // path variant is unset.
        let webhook_secret = resolve_secret(
            lookup,
            &mut read_file,
            "SLASH_WEBHOOK_SECRET_PATH",
            "SLASH_WEBHOOK_SECRET",
        )?;
        let database_url = resolve_secret(
            lookup,
            &mut read_file,
            "SLASH_DATABASE_URL_PATH",
            "SLASH_DATABASE_URL",
        )?;
        let auth_secret = resolve_secret(
            lookup,
            &mut read_file,
            "SLASH_AUTH_SECRET_PATH",
            "SLASH_AUTH_SECRET",
        )?;
        let github_app_id_raw = require(lookup, "SLASH_GITHUB_APP_ID")?;
        let github_app_id = github_app_id_raw
            .parse()
            .map_err(|_| ConfigError::InvalidInt("SLASH_GITHUB_APP_ID", github_app_id_raw))?;
        let github_private_key_path =
            PathBuf::from(require(lookup, "SLASH_GITHUB_PRIVATE_KEY_PATH")?);

        Ok(Self {
            github_app_id,
            github_private_key_path,
            webhook_secret,
            database_url,
            auth_secret,
        })
    }
}

/// Resolves a secret: prefer `<PATH_VAR>` (read from file, trimmed) so it
/// doesn't leak via env/`docker inspect`; fall back to `<DIRECT_VAR>`. Fail-
/// closed: if the path file is unreadable/empty, or neither var is set, this
/// returns an error — a secret is never defaulted or skipped.
fn resolve_secret(
    lookup: &impl Fn(&str) -> Option<String>,
    read_file: &mut impl FnMut(&str) -> std::io::Result<String>,
    path_var: &'static str,
    direct_var: &'static str,
) -> Result<String, ConfigError> {
    if let Some(path) = lookup(path_var) {
        let content =
            read_file(&path).map_err(|e| ConfigError::UnreadableFile(path_var, e.to_string()))?;
        let secret = content.trim().to_string();
        if secret.is_empty() {
            return Err(ConfigError::UnreadableFile(
                path_var,
                "file is empty".to_string(),
            ));
        }
        return Ok(secret);
    }
    require(lookup, direct_var)
}

fn require(
    lookup: impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<String, ConfigError> {
    lookup(name).ok_or(ConfigError::MissingEnvVar(name))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn lookup(env: &HashMap<String, String>) -> impl Fn(&str) -> Option<String> + '_ {
        move |name| env.get(name).cloned()
    }

    #[test]
    fn refuses_to_start_without_webhook_secret() {
        let env = env_of(&[
            ("SLASH_DATABASE_URL", "postgres://x"),
            ("SLASH_GITHUB_APP_ID", "1"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
        ]);
        // `ServerConfig` deliberately has no `Debug` impl (so the webhook
        // secret can never leak via a stray `{:?}`), so `.unwrap_err()`
        // (which requires `T: Debug`) isn't available here — match instead.
        match ServerConfig::from_lookup(lookup(&env)) {
            Err(err) => assert!(matches!(
                err,
                ConfigError::MissingEnvVar("SLASH_WEBHOOK_SECRET")
            )),
            Ok(_) => panic!("expected a MissingEnvVar error"),
        }
    }

    #[test]
    fn loads_a_complete_configuration() {
        let env = env_of(&[
            ("SLASH_WEBHOOK_SECRET", "shh"),
            ("SLASH_DATABASE_URL", "postgres://x"),
            ("SLASH_GITHUB_APP_ID", "42"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
            ("SLASH_AUTH_SECRET", "authsecret"),
        ]);
        let config = ServerConfig::from_lookup(lookup(&env)).unwrap();
        assert_eq!(config.github_app_id, 42);
        assert_eq!(config.webhook_secret, "shh");
        assert_eq!(config.auth_secret, "authsecret");
    }

    #[test]
    fn rejects_a_non_numeric_app_id() {
        let env = env_of(&[
            ("SLASH_WEBHOOK_SECRET", "shh"),
            ("SLASH_DATABASE_URL", "postgres://x"),
            ("SLASH_AUTH_SECRET", "authsecret"),
            ("SLASH_GITHUB_APP_ID", "not-a-number"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
        ]);
        match ServerConfig::from_lookup(lookup(&env)) {
            Err(err) => assert!(matches!(
                err,
                ConfigError::InvalidInt("SLASH_GITHUB_APP_ID", _)
            )),
            Ok(_) => panic!("expected an InvalidInt error"),
        }
    }

    #[test]
    fn auth_secret_is_read_from_the_path_when_set() {
        // Real file read: write a temp secret and verify config picks it up.
        let dir = std::env::temp_dir();
        let path = dir.join(format!("slash-auth-secret-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "file-secret-value\n").unwrap();

        let env = env_of(&[
            ("SLASH_WEBHOOK_SECRET", "shh"),
            ("SLASH_DATABASE_URL", "postgres://x"),
            ("SLASH_GITHUB_APP_ID", "42"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
            ("SLASH_AUTH_SECRET_PATH", path.to_str().unwrap()),
        ]);
        match ServerConfig::from_lookup_with_reader(&lookup(&env), |p| std::fs::read_to_string(p)) {
            Ok(config) => assert_eq!(config.auth_secret, "file-secret-value"),
            Err(_) => panic!("expected the file secret to be read"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn auth_secret_path_missing_file_is_an_error() {
        let env = env_of(&[
            ("SLASH_WEBHOOK_SECRET", "shh"),
            ("SLASH_DATABASE_URL", "postgres://x"),
            ("SLASH_GITHUB_APP_ID", "42"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
            ("SLASH_AUTH_SECRET_PATH", "/does/not/exist/secret.txt"),
        ]);
        match ServerConfig::from_lookup_with_reader(&lookup(&env), |p| std::fs::read_to_string(p)) {
            Err(ConfigError::UnreadableFile("SLASH_AUTH_SECRET_PATH", _)) => {}
            _ => panic!("expected an unreadable-file error"),
        }
    }

    #[test]
    fn webhook_secret_is_read_from_the_path_when_set() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("slash-webhook-secret-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "webhook-secret-value\n").unwrap();

        let env = env_of(&[
            ("SLASH_DATABASE_URL", "postgres://x"),
            ("SLASH_GITHUB_APP_ID", "42"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
            ("SLASH_AUTH_SECRET", "authsecret"),
            ("SLASH_WEBHOOK_SECRET_PATH", path.to_str().unwrap()),
        ]);
        match ServerConfig::from_lookup_with_reader(&lookup(&env), |p| std::fs::read_to_string(p)) {
            Ok(config) => assert_eq!(config.webhook_secret, "webhook-secret-value"),
            Err(_) => panic!("expected the file secret to be read"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn database_url_is_read_from_the_path_when_set() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("slash-db-url-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "postgres://user:pass@db/slash\n").unwrap();

        let env = env_of(&[
            ("SLASH_WEBHOOK_SECRET", "shh"),
            ("SLASH_GITHUB_APP_ID", "42"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
            ("SLASH_AUTH_SECRET", "authsecret"),
            ("SLASH_DATABASE_URL_PATH", path.to_str().unwrap()),
        ]);
        match ServerConfig::from_lookup_with_reader(&lookup(&env), |p| std::fs::read_to_string(p)) {
            Ok(config) => assert_eq!(config.database_url, "postgres://user:pass@db/slash"),
            Err(_) => panic!("expected the file database URL to be read"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn webhook_secret_path_empty_file_is_an_error() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("slash-webhook-empty-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "  \n").unwrap();

        let env = env_of(&[
            ("SLASH_DATABASE_URL", "postgres://x"),
            ("SLASH_GITHUB_APP_ID", "42"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
            ("SLASH_AUTH_SECRET", "authsecret"),
            ("SLASH_WEBHOOK_SECRET_PATH", path.to_str().unwrap()),
        ]);
        match ServerConfig::from_lookup_with_reader(&lookup(&env), |p| std::fs::read_to_string(p)) {
            Err(ConfigError::UnreadableFile("SLASH_WEBHOOK_SECRET_PATH", _)) => {}
            _ => panic!("expected an empty-file error"),
        }
        let _ = std::fs::remove_file(&path);
    }
}
