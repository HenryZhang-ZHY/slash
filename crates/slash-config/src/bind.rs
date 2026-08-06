//! Binds a parsed command line (`slash-command`) against a [`ValidatedCommand`]
//! into the final user-input map, or a typed error that renders as a usage
//! block (spec §5, plan M2). Positionals fill, in config order, whichever
//! args were *not* supplied in `--key` form; named/flag values and defaults
//! take the rest.

use std::collections::{HashMap, HashSet};

use slash_command::{ParsedCommand, is_safe_value};

use crate::validate::ValidatedCommand;

/// A conservative proxy for the §4.1 65,535-character `inputs` payload cap:
/// the sum of key/value byte lengths for the args this binder knows about.
/// `slash-core` (M5) re-checks the full payload once injected inputs (§4.2)
/// are merged in, since those aren't known here.
pub const MAX_INPUTS_PAYLOAD_BYTES: usize = 65_535;

#[derive(Debug, Clone, thiserror::Error)]
pub enum BindError {
    #[error("missing required argument '{name}'")]
    MissingRequired { name: String },
    #[error("unknown option '--{name}'")]
    UnknownOption { name: String },
    #[error("too many positional arguments; only {max} are accepted")]
    TooManyPositionals { max: usize },
    #[error("value for '{name}' must be one of: {choices}")]
    NotInChoices { name: String, choices: String },
    #[error("value for '{name}' contains characters outside the allowed set (spec §3.1)")]
    UnsafeValue { name: String },
    #[error("serialized inputs would be {size} bytes, exceeding the limit of {max}")]
    PayloadTooLarge { size: usize, max: usize },
}

pub fn bind(
    parsed: &ParsedCommand,
    command: &ValidatedCommand,
) -> Result<HashMap<String, String>, Vec<BindError>> {
    let mut errors = Vec::new();
    let mut bound: HashMap<String, String> = HashMap::new();

    let declared: HashSet<&str> = command.args.iter().map(|a| a.name.as_str()).collect();
    for key in parsed.named.keys() {
        if !declared.contains(key.as_str()) {
            errors.push(BindError::UnknownOption { name: key.clone() });
        }
    }

    let mut positionals = parsed.positionals.iter();
    let mut positional_slots = 0usize;

    for arg in &command.args {
        let value = if let Some(v) = parsed.named.get(&arg.name) {
            Some(v.clone())
        } else if let Some(p) = positionals.next() {
            positional_slots += 1;
            Some(p.clone())
        } else {
            arg.default.clone()
        };

        let Some(value) = value else {
            if arg.required {
                errors.push(BindError::MissingRequired {
                    name: arg.name.clone(),
                });
            }
            continue;
        };

        if let Some(choices) = &arg.choices {
            if !choices.contains(&value) {
                errors.push(BindError::NotInChoices {
                    name: arg.name.clone(),
                    choices: choices.join(", "),
                });
                continue;
            }
        }
        if !arg.free_text && !is_safe_value(&value) {
            errors.push(BindError::UnsafeValue {
                name: arg.name.clone(),
            });
            continue;
        }

        bound.insert(arg.name.clone(), value);
    }

    if positionals.count() > 0 {
        errors.push(BindError::TooManyPositionals {
            max: positional_slots,
        });
    }

    let payload_size: usize = bound.iter().map(|(k, v)| k.len() + v.len()).sum();
    if payload_size > MAX_INPUTS_PAYLOAD_BYTES {
        errors.push(BindError::PayloadTooLarge {
            size: payload_size,
            max: MAX_INPUTS_PAYLOAD_BYTES,
        });
    }

    if errors.is_empty() {
        Ok(bound)
    } else {
        Err(errors)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::schema::parse_raw;
    use crate::validate::validate_command;

    fn deploy_command() -> ValidatedCommand {
        let raw = parse_raw(
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
  - name: reason
    free_text: true
"#,
        )
        .unwrap();
        validate_command("deploy.yml", &raw).unwrap()
    }

    fn parse(line: &str) -> ParsedCommand {
        slash_command::parse_comment(line).unwrap().unwrap()
    }

    #[test]
    fn binds_positionals_named_and_defaults() {
        let parsed = parse("/deploy staging --reason=\"weekly release\"");
        let bound = bind(&parsed, &deploy_command()).unwrap();
        assert_eq!(bound["env"], "staging");
        assert_eq!(bound["timeout"], "15m");
        assert_eq!(bound["reason"], "weekly release");
    }

    #[test]
    fn named_args_are_skipped_by_positional_binding() {
        let parsed = parse("/deploy --timeout=30m staging");
        let bound = bind(&parsed, &deploy_command()).unwrap();
        assert_eq!(bound["env"], "staging");
        assert_eq!(bound["timeout"], "30m");
    }

    #[test]
    fn missing_required_argument_is_an_error() {
        let parsed = parse("/deploy");
        let errors = bind(&parsed, &deploy_command()).unwrap_err();
        assert!(matches!(errors[0], BindError::MissingRequired { .. }));
    }

    #[test]
    fn unknown_option_is_an_error() {
        let parsed = parse("/deploy staging --bogus=1");
        let errors = bind(&parsed, &deploy_command()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, BindError::UnknownOption { name } if name == "bogus"))
        );
    }

    #[test]
    fn too_many_positionals_is_an_error() {
        let parsed = parse("/deploy staging 30m extra reason four");
        let errors = bind(&parsed, &deploy_command()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, BindError::TooManyPositionals { .. }))
        );
    }

    #[test]
    fn value_not_in_choices_is_an_error() {
        let parsed = parse("/deploy nonexistent-env");
        let errors = bind(&parsed, &deploy_command()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, BindError::NotInChoices { .. }))
        );
    }

    #[test]
    fn unsafe_value_is_rejected_unless_free_text() {
        let parsed = parse("/deploy staging --timeout=\"30 minutes\"");
        let errors = bind(&parsed, &deploy_command()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, BindError::UnsafeValue { name } if name == "timeout"))
        );

        let parsed = parse("/deploy staging --reason=\"has spaces, and stuff\"");
        assert!(bind(&parsed, &deploy_command()).is_ok());
    }

    #[test]
    fn collects_every_bind_error_together() {
        let parsed = parse("/deploy --bogus=1");
        let errors = bind(&parsed, &deploy_command()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, BindError::UnknownOption { .. }))
        );
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, BindError::MissingRequired { .. }))
        );
    }
}
