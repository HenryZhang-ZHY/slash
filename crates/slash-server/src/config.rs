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
        // Secrets are file-only (never inline env), mirroring the GitHub App
        // private key — so they never show up in `ps aux`/env dumps or
        // `docker inspect`. One way to configure a secret, the secure one.
        let file_reader = |path: &str| std::fs::read_to_string(path);
        Self::from_lookup_with_reader(&lookup, file_reader)
    }

    fn from_lookup_with_reader(
        lookup: &impl Fn(&str) -> Option<String>,
        mut read_file: impl FnMut(&str) -> std::io::Result<String>,
    ) -> Result<Self, ConfigError> {
        // Secrets are file-only: each has exactly one way to be set, a
        // `*_PATH` env var pointing at a file read at startup (mirroring the
        // GitHub App private key). Inline env variants are not recognized.
        let webhook_secret = resolve_secret(lookup, &mut read_file, "SLASH_WEBHOOK_SECRET_PATH")?;
        let database_url = resolve_secret(lookup, &mut read_file, "SLASH_DATABASE_URL_PATH")?;
        let auth_secret = resolve_secret(lookup, &mut read_file, "SLASH_AUTH_SECRET_PATH")?;
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

/// Resolves a secret from a file: reads `<PATH_VAR>`'s file and trims it, so
/// it never leaks via env/`docker inspect`. This is the only way a secret is
/// configured — there is no inline fallback. Fail-closed: a missing/empty
/// `<PATH_VAR>` or an unreadable/empty file is a startup error; a secret is
/// never defaulted or skipped.
fn resolve_secret(
    lookup: &impl Fn(&str) -> Option<String>,
    read_file: &mut impl FnMut(&str) -> std::io::Result<String>,
    path_var: &'static str,
) -> Result<String, ConfigError> {
    let path = lookup(path_var).ok_or(ConfigError::MissingEnvVar(path_var))?;
    let content =
        read_file(&path).map_err(|e| ConfigError::UnreadableFile(path_var, e.to_string()))?;
    let secret = content.trim().to_string();
    if secret.is_empty() {
        return Err(ConfigError::UnreadableFile(
            path_var,
            "file is empty".to_string(),
        ));
    }
    Ok(secret)
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

    /// Writes a temp secret file and returns its path string. Callers pass
    /// the path as the `*_PATH` env var; the reader is the real filesystem.
    fn secret_file(content: &str) -> String {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("slash-secret-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, content).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// Env for a complete config: all three secrets come from files.
    fn file_env(env: &mut HashMap<String, String>) -> HashMap<String, String> {
        env.insert(
            "SLASH_WEBHOOK_SECRET_PATH".to_string(),
            secret_file("webhook-secret\n"),
        );
        env.insert(
            "SLASH_DATABASE_URL_PATH".to_string(),
            secret_file("postgres://slash:slash@db/slash\n"),
        );
        env.insert(
            "SLASH_AUTH_SECRET_PATH".to_string(),
            secret_file("auth-secret\n"),
        );
        env.clone()
    }

    fn load(env: &HashMap<String, String>) -> Result<ServerConfig, ConfigError> {
        ServerConfig::from_lookup_with_reader(&lookup(env), |p| std::fs::read_to_string(p))
    }

    #[test]
    fn refuses_to_start_without_a_secret_path() {
        let env = env_of(&[
            ("SLASH_GITHUB_APP_ID", "1"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
        ]);
        // The webhook secret's path env var is unset: file-only means no
        // secret can be resolved, and startup fails closed.
        match load(&env) {
            Err(ConfigError::MissingEnvVar("SLASH_WEBHOOK_SECRET_PATH")) => {}
            _ => panic!("expected a missing webhook secret path error"),
        }
    }

    #[test]
    fn loads_a_complete_configuration_from_files() {
        let mut env = env_of(&[
            ("SLASH_GITHUB_APP_ID", "42"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
        ]);
        let env = file_env(&mut env);
        let config = load(&env).unwrap();
        assert_eq!(config.github_app_id, 42);
        assert_eq!(config.webhook_secret, "webhook-secret");
        assert_eq!(config.database_url, "postgres://slash:slash@db/slash");
        assert_eq!(config.auth_secret, "auth-secret");
    }

    #[test]
    fn rejects_a_non_numeric_app_id() {
        let mut env = env_of(&[
            ("SLASH_GITHUB_APP_ID", "not-a-number"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
        ]);
        let env = file_env(&mut env);
        match load(&env) {
            Err(ConfigError::InvalidInt("SLASH_GITHUB_APP_ID", _)) => {}
            _ => panic!("expected an InvalidInt error"),
        }
    }

    #[test]
    fn auth_secret_is_read_from_the_path_when_set() {
        let mut env = env_of(&[
            ("SLASH_GITHUB_APP_ID", "42"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
        ]);
        let mut env = file_env(&mut env);
        env.insert(
            "SLASH_AUTH_SECRET_PATH".to_string(),
            secret_file("file-secret-value\n"),
        );
        let config = load(&env).unwrap();
        assert_eq!(config.auth_secret, "file-secret-value");
    }

    #[test]
    fn auth_secret_path_missing_file_is_an_error() {
        let mut env = env_of(&[
            ("SLASH_GITHUB_APP_ID", "42"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
        ]);
        let mut env = file_env(&mut env);
        env.insert(
            "SLASH_AUTH_SECRET_PATH".to_string(),
            "/does/not/exist/secret.txt".to_string(),
        );
        match load(&env) {
            Err(ConfigError::UnreadableFile("SLASH_AUTH_SECRET_PATH", _)) => {}
            _ => panic!("expected an unreadable-file error"),
        }
    }

    #[test]
    fn webhook_secret_is_read_from_the_path_when_set() {
        let mut env = env_of(&[
            ("SLASH_GITHUB_APP_ID", "42"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
        ]);
        let mut env = file_env(&mut env);
        env.insert(
            "SLASH_WEBHOOK_SECRET_PATH".to_string(),
            secret_file("webhook-secret-value\n"),
        );
        let config = load(&env).unwrap();
        assert_eq!(config.webhook_secret, "webhook-secret-value");
    }

    #[test]
    fn database_url_is_read_from_the_path_when_set() {
        let mut env = env_of(&[
            ("SLASH_GITHUB_APP_ID", "42"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
        ]);
        let mut env = file_env(&mut env);
        env.insert(
            "SLASH_DATABASE_URL_PATH".to_string(),
            secret_file("postgres://user:pass@db/slash\n"),
        );
        let config = load(&env).unwrap();
        assert_eq!(config.database_url, "postgres://user:pass@db/slash");
    }

    #[test]
    fn webhook_secret_path_empty_file_is_an_error() {
        let mut env = env_of(&[
            ("SLASH_GITHUB_APP_ID", "42"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
        ]);
        let mut env = file_env(&mut env);
        env.insert("SLASH_WEBHOOK_SECRET_PATH".to_string(), secret_file("  \n"));
        match load(&env) {
            Err(ConfigError::UnreadableFile("SLASH_WEBHOOK_SECRET_PATH", _)) => {}
            _ => panic!("expected an empty-file error"),
        }
    }

    #[test]
    fn inline_env_secret_variants_are_not_recognized() {
        // The old inline fallbacks (SLASH_WEBHOOK_SECRET / SLASH_DATABASE_URL
        // / SLASH_AUTH_SECRET) must NOT be honored — file-only is the one way.
        let env = env_of(&[
            ("SLASH_WEBHOOK_SECRET", "shh"),
            ("SLASH_DATABASE_URL", "postgres://x"),
            ("SLASH_AUTH_SECRET", "authsecret"),
            ("SLASH_GITHUB_APP_ID", "42"),
            ("SLASH_GITHUB_PRIVATE_KEY_PATH", "/key.pem"),
        ]);
        match load(&env) {
            Err(ConfigError::MissingEnvVar("SLASH_WEBHOOK_SECRET_PATH")) => {}
            _ => panic!("expected a missing webhook secret path error"),
        }
    }
}
