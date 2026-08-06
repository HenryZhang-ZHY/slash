//! serde model for `.slash/*.yml` (spec §4.1). Structurally permissive on
//! purpose: an invalid `permission`/`workflow`/arg shape still deserializes
//! (as a plain `String`) so `validate` can collect *every* semantic problem
//! in one pass instead of stopping at the first deserialize error.

use serde::Deserialize;

use crate::ConfigError;
use crate::limits::scan_yaml_resource_risks;

fn default_permission() -> String {
    "write".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCommandConfig {
    pub command: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_permission")]
    pub permission: String,
    pub workflow: String,
    #[serde(default)]
    pub args: Vec<RawArgConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawArgConfig {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub choices: Option<Vec<String>>,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub free_text: bool,
}

/// Parses one `.slash/*.yml` file's raw text into its unvalidated shape.
/// `file` is used only to attribute errors. The §4.1 resource-limit scan
/// always runs first: this crate never hands attacker-influenced YAML to the
/// real parser before it has passed that check.
pub fn parse_raw(file: &str, text: &str) -> Result<RawCommandConfig, ConfigError> {
    scan_yaml_resource_risks(file, text)?;

    serde_norway::from_str(text).map_err(|e| ConfigError::Deserialize {
        file: file.to_string(),
        message: e.to_string(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_spec_example() {
        let yaml = r#"
command: deploy
description: Deploy the PR to an environment
permission: write
workflow: deploy.yml
args:
  - name: env
    description: Target environment
    required: true
    choices: [staging, production]
  - name: timeout
    default: "15m"
  - name: reason
    free_text: true
"#;
        let raw = parse_raw("deploy.yml", yaml).unwrap();
        assert_eq!(raw.command, "deploy");
        assert_eq!(raw.permission, "write");
        assert_eq!(raw.workflow, "deploy.yml");
        assert_eq!(raw.args.len(), 3);
        assert_eq!(raw.args[0].name, "env");
        assert!(raw.args[0].required);
        assert_eq!(
            raw.args[0].choices.as_deref(),
            Some(["staging".to_string(), "production".to_string()].as_slice())
        );
        assert_eq!(raw.args[1].default.as_deref(), Some("15m"));
        assert!(raw.args[2].free_text);
    }

    #[test]
    fn defaults_permission_to_write_when_absent() {
        let yaml = "command: echo\nworkflow: echo.yml\n";
        let raw = parse_raw("echo.yml", yaml).unwrap();
        assert_eq!(raw.permission, "write");
        assert!(raw.args.is_empty());
    }

    #[test]
    fn rejects_unknown_top_level_keys() {
        let yaml = "command: echo\nworkflow: echo.yml\nnotarealkey: true\n";
        assert!(matches!(
            parse_raw("echo.yml", yaml),
            Err(ConfigError::Deserialize { .. })
        ));
    }

    #[test]
    fn rejects_unknown_arg_keys() {
        let yaml =
            "command: echo\nworkflow: echo.yml\nargs:\n  - name: x\n    slash_reserved: true\n";
        assert!(matches!(
            parse_raw("echo.yml", yaml),
            Err(ConfigError::Deserialize { .. })
        ));
    }

    #[test]
    fn rejects_yaml_aliases_before_deserializing() {
        let yaml = "command: &c echo\nworkflow: echo.yml\n";
        assert!(matches!(
            parse_raw("echo.yml", yaml),
            Err(ConfigError::AliasesNotPermitted { .. })
        ));
    }
}
