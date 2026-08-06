//! All `.slash/` configuration errors, both structural and semantic (spec
//! §4.1). Kept in one enum so `slash validate` (plan M2) can print every
//! error it collects, one per line, rather than stopping at the first.

#[derive(Debug, Clone, thiserror::Error)]
pub enum ConfigError {
    #[error("{file}: {message}")]
    Deserialize { file: String, message: String },

    #[error(
        "{file}: command names must match /[a-z][a-z0-9-]*/ and 'help'/'slash' are reserved for built-ins, got '{name}'"
    )]
    InvalidCommandName { file: String, name: String },

    #[error("command '{name}' is defined in both {first} and {second}")]
    DuplicateCommand {
        name: String,
        first: String,
        second: String,
    },

    #[error("{file}: permission must be one of write|maintain|admin, got '{value}'")]
    InvalidPermission { file: String, value: String },

    #[error(
        "{file}: workflow must match /^[A-Za-z0-9._-]{{1,100}}\\.(ya?ml)$/ with no path separators, got '{value}'"
    )]
    InvalidWorkflowName { file: String, value: String },

    #[error("{file}: arg names must match /[a-z][a-z0-9_-]*/, got '{name}'")]
    InvalidArgName { file: String, name: String },

    #[error(
        "{file}: arg name '{name}' starts with 'slash_', which is reserved for Slash-injected inputs"
    )]
    ReservedArgName { file: String, name: String },

    #[error("{file}: duplicate arg name '{name}'")]
    DuplicateArgName { file: String, name: String },

    #[error("{file}: required arg '{name}' must not be declared after optional arg '{after}'")]
    RequiredAfterOptional {
        file: String,
        name: String,
        after: String,
    },

    #[error("{file}: arg '{name}' is required and must not declare a default")]
    DefaultOnRequiredArg { file: String, name: String },

    #[error("{file}: default value for arg '{name}' is not one of its declared choices")]
    DefaultNotInChoices { file: String, name: String },

    #[error("{file}: default value for arg '{name}' fails the value charset (spec §3.1)")]
    DefaultFailsCharset { file: String, name: String },

    #[error(
        "{file}: {count} total inputs (args plus Slash-injected inputs) exceed the workflow_dispatch limit of {max}"
    )]
    TooManyInputs {
        file: String,
        count: usize,
        max: usize,
    },

    #[error(".slash/ contains {count} files, exceeding the limit of {max}")]
    TooManyFiles { count: usize, max: usize },

    #[error(".slash/ totals {total} bytes, exceeding the limit of {max} bytes")]
    DirectoryTooLarge { total: u64, max: u64 },

    #[error("{file}: YAML aliases and anchors are not permitted")]
    AliasesNotPermitted { file: String },

    #[error("{file}: YAML nesting depth {depth} exceeds the limit of {max}")]
    NestingTooDeep {
        file: String,
        depth: usize,
        max: usize,
    },
}
