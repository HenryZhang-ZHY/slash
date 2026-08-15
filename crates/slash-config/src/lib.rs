//! `.slash/*.yml` command definitions: schema, validation, and the command
//! line + config binder (spec §4). Pure, no IO — callers (`slash-cli`,
//! `slash-server`) read files or fetch bytes from GitHub and hand them to
//! [`load_command_file`]; this crate never touches a filesystem or network.

mod bind;
mod error;
mod limits;
mod schema;
mod validate;

pub use bind::{BindError, MAX_INPUTS_PAYLOAD_BYTES, bind};
pub use error::ConfigError;
pub use limits::{MAX_FILES, MAX_NESTING_DEPTH, MAX_TOTAL_BYTES, check_directory_limits};
pub use schema::{RawArgConfig, RawCommandConfig};
pub use validate::{
    INJECTED_INPUT_COUNT, MAX_TOTAL_INPUTS, Permission, RESERVED_COMMAND_NAMES, ValidatedArg,
    ValidatedCommand, find_duplicate_commands, validate_command,
};

/// Parses and validates one `.slash/*.yml` file's bytes. `file` is used only
/// to attribute errors. Directory-wide limits ([`check_directory_limits`])
/// and cross-file duplicate detection ([`find_duplicate_commands`]) are the
/// caller's responsibility, since only the caller sees the whole directory.
pub fn load_command_file(file: &str, bytes: &[u8]) -> Result<ValidatedCommand, Vec<ConfigError>> {
    let text = std::str::from_utf8(bytes).map_err(|e| {
        vec![ConfigError::Deserialize {
            file: file.to_string(),
            message: format!("not valid UTF-8: {e}"),
        }]
    })?;

    let raw = schema::parse_raw(file, text).map_err(|e| vec![e])?;
    validate_command(file, &raw)
}

/// Parses and validates a whole `.slash/` directory's files: each entry is a
/// `(file name, decoded bytes)` pair from the caller's content fetch. Runs
/// [`load_command_file`] per file, aggregates per-file validation errors, then
/// runs cross-file duplicate detection ([`find_duplicate_commands`]). Pure and
/// IO-free — the caller does the GitHub fetch and base64 decode.
///
/// Returns the validated commands in file order, or the aggregated error
/// messages (per-file errors joined with duplicate-command errors) when any
/// file or cross-file check fails.
pub fn assemble_directory(
    files: &[(String, Vec<u8>)],
) -> Result<Vec<ValidatedCommand>, Vec<String>> {
    let mut commands = Vec::with_capacity(files.len());
    let mut validation_errors = Vec::new();
    let mut command_sources = Vec::new();

    for (file_name, bytes) in files {
        match load_command_file(file_name, bytes) {
            Ok(command) => {
                command_sources.push((file_name.clone(), command.command.clone()));
                commands.push(command);
            }
            Err(errors) => {
                validation_errors.extend(errors.into_iter().map(|error| error.to_string()));
            }
        }
    }

    validation_errors.extend(
        find_duplicate_commands(&command_sources)
            .into_iter()
            .map(|error| error.to_string()),
    );
    if !validation_errors.is_empty() {
        return Err(validation_errors);
    }

    Ok(commands)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn loads_a_valid_file_end_to_end() {
        let yaml = br#"
command: deploy
workflow: deploy.yml
args:
  - name: env
    required: true
    choices: [staging, production]
"#;
        let command = load_command_file("deploy.yml", yaml).unwrap();
        assert_eq!(command.command, "deploy");
        assert_eq!(command.permission, Permission::Write);
    }

    #[test]
    fn rejects_invalid_utf8() {
        let bytes = [0xff, 0xfe, 0xfd];
        let errors = load_command_file("bad.yml", &bytes).unwrap_err();
        assert!(matches!(errors[0], ConfigError::Deserialize { .. }));
    }

    #[test]
    fn propagates_a_single_structural_error() {
        let yaml = b"command: deploy\nworkflow: deploy.yml\nbogus: true\n";
        let errors = load_command_file("deploy.yml", yaml).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], ConfigError::Deserialize { .. }));
    }

    #[test]
    fn propagates_every_semantic_validation_error() {
        let yaml = b"command: Bad\npermission: maintain\nworkflow: bad\n";
        let errors = load_command_file("deploy.yml", yaml).unwrap_err();
        assert_eq!(errors.len(), 3);
    }

    #[test]
    fn assemble_directory_collects_valid_commands_in_file_order() {
        let deploy = b"command: deploy\nworkflow: deploy.yml\n";
        let lint = b"command: lint\nworkflow: lint.yml\n";
        let commands = assemble_directory(&[
            ("deploy.yml".to_string(), deploy.to_vec()),
            ("lint.yml".to_string(), lint.to_vec()),
        ])
        .unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].command, "deploy");
        assert_eq!(commands[1].command, "lint");
    }

    #[test]
    fn assemble_directory_aggregates_per_file_and_duplicate_errors() {
        let deploy = b"command: deploy\nworkflow: deploy.yml\n";
        let bad = b"command: Bad\npermission: maintain\nworkflow: bad\n";
        let dup = b"command: deploy\nworkflow: other.yml\n";
        let errors = assemble_directory(&[
            ("deploy.yml".to_string(), deploy.to_vec()),
            ("bad.yml".to_string(), bad.to_vec()),
            ("other.yml".to_string(), dup.to_vec()),
        ])
        .unwrap_err();
        assert_eq!(errors.len(), 4);
        assert!(
            errors.iter().any(|e| e.contains("defined in both")),
            "duplicate-command error present: {errors:?}"
        );
    }

    #[test]
    fn assemble_directory_rejects_empty_directory() {
        let commands = assemble_directory(&[]).unwrap();
        assert!(commands.is_empty());
    }
}
