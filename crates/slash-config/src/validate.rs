//! Semantic validation of a [`RawCommandConfig`] (spec §4.1). Every rule is
//! checked and every violation collected — `slash validate` (plan M2) prints
//! all of them, not just the first.

use std::collections::BTreeSet;

use slash_command::is_valid_arg_name;

use crate::ConfigError;
use crate::schema::{RawArgConfig, RawCommandConfig};

/// Injected inputs (§4.2): `slash_run_id`, `slash_pr_number`, `slash_head_sha`,
/// `slash_actor`, `slash_actor_id`.
pub const INJECTED_INPUT_COUNT: usize = 5;
pub const MAX_TOTAL_INPUTS: usize = 25;
pub const RESERVED_COMMAND_NAMES: &[&str] = &["help", "slash"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Write,
    Maintain,
    Admin,
}

impl Permission {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "write" => Some(Self::Write),
            "maintain" => Some(Self::Maintain),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedArg {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
    pub choices: Option<Vec<String>>,
    pub default: Option<String>,
    pub free_text: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCommand {
    pub command: String,
    pub description: Option<String>,
    pub permission: Permission,
    pub workflow: String,
    pub args: Vec<ValidatedArg>,
}

fn matches_command_name_pattern(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn matches_workflow_pattern(s: &str) -> bool {
    let Some(stem) = s.strip_suffix(".yml").or_else(|| s.strip_suffix(".yaml")) else {
        return false;
    };
    if stem.is_empty() || s.len() > 100 {
        return false;
    }
    stem.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Runs every §4.1 rule against `raw` and returns either a fully validated
/// command or every violation found (never just the first).
pub fn validate_command(
    file: &str,
    raw: &RawCommandConfig,
) -> Result<ValidatedCommand, Vec<ConfigError>> {
    let mut errors = Vec::new();

    let command_valid = matches_command_name_pattern(&raw.command)
        && !RESERVED_COMMAND_NAMES.contains(&raw.command.as_str());
    if !command_valid {
        errors.push(ConfigError::InvalidCommandName {
            file: file.to_string(),
            name: raw.command.clone(),
        });
    }

    let permission = Permission::parse(&raw.permission);
    if permission.is_none() {
        errors.push(ConfigError::InvalidPermission {
            file: file.to_string(),
            value: raw.permission.clone(),
        });
    }

    if !matches_workflow_pattern(&raw.workflow) {
        errors.push(ConfigError::InvalidWorkflowName {
            file: file.to_string(),
            value: raw.workflow.clone(),
        });
    }

    let mut seen_names = BTreeSet::new();
    let mut seen_optional = false;
    let mut validated_args = Vec::with_capacity(raw.args.len());

    for arg in &raw.args {
        validate_arg(file, arg, &mut seen_names, &mut seen_optional, &mut errors);
        validated_args.push(ValidatedArg {
            name: arg.name.clone(),
            description: arg.description.clone(),
            required: arg.required,
            choices: arg.choices.clone(),
            default: arg.default.clone(),
            free_text: arg.free_text,
        });
    }

    let total_inputs = raw.args.len() + INJECTED_INPUT_COUNT;
    if total_inputs > MAX_TOTAL_INPUTS {
        errors.push(ConfigError::TooManyInputs {
            file: file.to_string(),
            count: total_inputs,
            max: MAX_TOTAL_INPUTS,
        });
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(ValidatedCommand {
        command: raw.command.clone(),
        description: raw.description.clone(),
        // Unwrap is safe: an empty `errors` means the `permission.is_none()`
        // branch above never pushed an error, so `permission` is `Some`.
        permission: permission.unwrap_or(Permission::Write),
        workflow: raw.workflow.clone(),
        args: validated_args,
    })
}

fn validate_arg(
    file: &str,
    arg: &RawArgConfig,
    seen_names: &mut BTreeSet<String>,
    seen_optional: &mut bool,
    errors: &mut Vec<ConfigError>,
) {
    let name_valid = is_valid_arg_name(&arg.name) && !arg.name.starts_with("slash_");
    if !is_valid_arg_name(&arg.name) {
        errors.push(ConfigError::InvalidArgName {
            file: file.to_string(),
            name: arg.name.clone(),
        });
    } else if arg.name.starts_with("slash_") {
        errors.push(ConfigError::ReservedArgName {
            file: file.to_string(),
            name: arg.name.clone(),
        });
    }

    if name_valid && !seen_names.insert(arg.name.clone()) {
        errors.push(ConfigError::DuplicateArgName {
            file: file.to_string(),
            name: arg.name.clone(),
        });
    }

    if arg.required {
        if *seen_optional {
            errors.push(ConfigError::RequiredAfterOptional {
                file: file.to_string(),
                name: arg.name.clone(),
                after: "an earlier optional argument".to_string(),
            });
        }
        if arg.default.is_some() {
            errors.push(ConfigError::DefaultOnRequiredArg {
                file: file.to_string(),
                name: arg.name.clone(),
            });
        }
    } else {
        *seen_optional = true;
    }

    if let Some(default) = &arg.default {
        if let Some(choices) = &arg.choices
            && !choices.contains(default)
        {
            errors.push(ConfigError::DefaultNotInChoices {
                file: file.to_string(),
                name: arg.name.clone(),
            });
        }
        if !arg.free_text && !slash_command::is_safe_value(default) {
            errors.push(ConfigError::DefaultFailsCharset {
                file: file.to_string(),
                name: arg.name.clone(),
            });
        }
    }
}

/// Cross-file check (spec §4.1): a command name defined in more than one file
/// is a configuration error naming both files. `commands` is `(filename,
/// command name)` for every successfully-validated command in a directory.
pub fn find_duplicate_commands(commands: &[(String, String)]) -> Vec<ConfigError> {
    let mut first_seen: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
    let mut errors = Vec::new();

    for (file, name) in commands {
        match first_seen.get(name.as_str()) {
            Some(first) => errors.push(ConfigError::DuplicateCommand {
                name: name.clone(),
                first: (*first).to_string(),
                second: file.clone(),
            }),
            None => {
                first_seen.insert(name.as_str(), file.as_str());
            }
        }
    }

    errors
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::schema::parse_raw;

    fn valid_raw() -> RawCommandConfig {
        parse_raw(
            "deploy.yml",
            r#"
command: deploy
workflow: deploy.yml
args:
  - name: env
    required: true
    choices: [staging, production]
  - name: timeout
    default: "15m"
"#,
        )
        .unwrap()
    }

    #[test]
    fn accepts_a_valid_command() {
        let raw = valid_raw();
        let validated = validate_command("deploy.yml", &raw).unwrap();
        assert_eq!(validated.command, "deploy");
        assert_eq!(validated.permission, Permission::Write);
        assert_eq!(validated.args.len(), 2);
    }

    #[test]
    fn rejects_invalid_and_reserved_command_names() {
        for bad in ["Deploy", "9deploy", "help", "slash", ""] {
            let mut raw = valid_raw();
            raw.command = bad.to_string();
            let errors = validate_command("deploy.yml", &raw).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, ConfigError::InvalidCommandName { .. })),
                "expected InvalidCommandName for {bad:?}, got {errors:?}"
            );
        }
    }

    #[test]
    fn rejects_bad_permission_values() {
        for bad in ["read", "triage", "Write", "owner"] {
            let mut raw = valid_raw();
            raw.permission = bad.to_string();
            let errors = validate_command("deploy.yml", &raw).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, ConfigError::InvalidPermission { .. }))
            );
        }
    }

    #[test]
    fn accepts_all_valid_permissions() {
        for good in ["write", "maintain", "admin"] {
            let mut raw = valid_raw();
            raw.permission = good.to_string();
            assert!(validate_command("deploy.yml", &raw).is_ok());
        }
    }

    #[test]
    fn rejects_bad_workflow_names() {
        for bad in [
            "deploy",         // no extension
            "deploy.yml.bak", // wrong extension
            "../deploy.yml",  // path traversal
            "a/deploy.yml",   // path separator
            "de%20ploy.yml",  // percent
        ] {
            let mut raw = valid_raw();
            raw.workflow = bad.to_string();
            let errors = validate_command("deploy.yml", &raw).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, ConfigError::InvalidWorkflowName { .. })),
                "expected InvalidWorkflowName for {bad:?}, got {errors:?}"
            );
        }
    }

    #[test]
    fn accepts_yaml_and_yml_workflow_extensions() {
        for good in ["deploy.yml", "deploy.yaml", "deploy-prod.v2.yml"] {
            let mut raw = valid_raw();
            raw.workflow = good.to_string();
            assert!(validate_command("deploy.yml", &raw).is_ok());
        }
    }

    #[test]
    fn rejects_invalid_and_reserved_arg_names() {
        let mut raw = valid_raw();
        raw.args[0].name = "Env".to_string();
        let errors = validate_command("deploy.yml", &raw).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::InvalidArgName { .. }))
        );

        let mut raw = valid_raw();
        raw.args[0].name = "slash_env".to_string();
        let errors = validate_command("deploy.yml", &raw).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::ReservedArgName { .. }))
        );
    }

    #[test]
    fn rejects_duplicate_arg_names() {
        let mut raw = valid_raw();
        raw.args[1].name = raw.args[0].name.clone();
        let errors = validate_command("deploy.yml", &raw).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::DuplicateArgName { .. }))
        );
    }

    #[test]
    fn rejects_required_after_optional() {
        let mut raw = valid_raw();
        raw.args[0].required = false;
        raw.args[0].default = None;
        raw.args[1].required = true;
        let errors = validate_command("deploy.yml", &raw).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::RequiredAfterOptional { .. }))
        );
    }

    #[test]
    fn rejects_default_on_a_required_arg() {
        let mut raw = valid_raw();
        raw.args[0].default = Some("staging".to_string());
        let errors = validate_command("deploy.yml", &raw).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::DefaultOnRequiredArg { .. }))
        );
    }

    #[test]
    fn rejects_default_not_in_choices() {
        let mut raw = valid_raw();
        raw.args[1].choices = Some(vec!["15m".to_string(), "30m".to_string()]);
        raw.args[1].default = Some("1h".to_string());
        let errors = validate_command("deploy.yml", &raw).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::DefaultNotInChoices { .. }))
        );
    }

    #[test]
    fn rejects_default_failing_the_value_charset_unless_free_text() {
        let mut raw = valid_raw();
        raw.args[1].default = Some("has spaces".to_string());
        let errors = validate_command("deploy.yml", &raw).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::DefaultFailsCharset { .. }))
        );

        let mut raw = valid_raw();
        raw.args[1].default = Some("has spaces".to_string());
        raw.args[1].free_text = true;
        assert!(validate_command("deploy.yml", &raw).is_ok());
    }

    #[test]
    fn rejects_too_many_total_inputs() {
        let mut raw = valid_raw();
        raw.args.clear();
        for i in 0..(MAX_TOTAL_INPUTS - INJECTED_INPUT_COUNT + 1) {
            raw.args.push(RawArgConfig {
                name: format!("arg{i}"),
                description: None,
                required: false,
                choices: None,
                default: None,
                free_text: false,
            });
        }
        let errors = validate_command("deploy.yml", &raw).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::TooManyInputs { .. }))
        );
    }

    #[test]
    fn collects_every_error_instead_of_stopping_at_the_first() {
        let mut raw = valid_raw();
        raw.command = "Bad".to_string();
        raw.permission = "read".to_string();
        raw.workflow = "bad".to_string();
        let errors = validate_command("deploy.yml", &raw).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::InvalidCommandName { .. }))
        );
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::InvalidPermission { .. }))
        );
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::InvalidWorkflowName { .. }))
        );
    }

    #[test]
    fn finds_duplicate_commands_across_files() {
        let commands = vec![
            ("deploy.yml".to_string(), "deploy".to_string()),
            ("echo.yml".to_string(), "echo".to_string()),
            ("deploy2.yml".to_string(), "deploy".to_string()),
        ];
        let errors = find_duplicate_commands(&commands);
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ConfigError::DuplicateCommand { name, first, second }
                if name == "deploy" && first == "deploy.yml" && second == "deploy2.yml"
        ));
    }

    #[test]
    fn no_duplicates_when_all_names_are_unique() {
        let commands = vec![
            ("deploy.yml".to_string(), "deploy".to_string()),
            ("echo.yml".to_string(), "echo".to_string()),
        ];
        assert!(find_duplicate_commands(&commands).is_empty());
    }
}
