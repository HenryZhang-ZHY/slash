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
        let yaml = b"command: Bad\npermission: read\nworkflow: bad\n";
        let errors = load_command_file("deploy.yml", yaml).unwrap_err();
        assert_eq!(errors.len(), 3);
    }
}
