//! Lexer/parser for the slash-command grammar (spec §3). Pure, no IO: this
//! crate only turns a PR comment string into structured data or a typed
//! error; permission, config and charset enforcement live in `slash-config`
//! and `slash-core`.

use std::collections::{BTreeSet, HashMap};

/// The §3.1 default value charset length cap: `^[A-Za-z0-9._@:/+=,-]{0,256}$`.
pub const MAX_VALUE_LENGTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    pub name: String,
    pub positionals: Vec<String>,
    pub named: HashMap<String, String>,
    pub raw_line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MisplacedCommand {
    pub name: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("invalid command name at column {column}")]
    InvalidCommandName { column: usize },
    #[error("unterminated quoted value starting at column {column}")]
    UnterminatedQuote { column: usize },
    #[error(
        "'--{key}' at column {column}: keys starting with 'slash_' are reserved for Slash-injected inputs"
    )]
    ReservedKey { key: String, column: usize },
    #[error("duplicate key '--{key}' at column {column}")]
    DuplicateKey { key: String, column: usize },
    #[error("invalid key '--{key}' at column {column}: keys must match [a-z][a-z0-9_-]*")]
    InvalidKey { key: String, column: usize },
}

impl ParseError {
    /// 1-based column, for a human-readable "column N" position in the comment's first line.
    pub fn column(&self) -> usize {
        match self {
            ParseError::InvalidCommandName { column }
            | ParseError::UnterminatedQuote { column }
            | ParseError::ReservedKey { column, .. }
            | ParseError::DuplicateKey { column, .. }
            | ParseError::InvalidKey { column, .. } => *column,
        }
    }
}

/// The §3.1 default value charset: `^[A-Za-z0-9._@:/+=,-]{0,256}$`. Entries
/// with `free_text: true` (§4.1) opt out of this and are validated by the
/// config/binder layer instead.
pub fn is_safe_value(value: &str) -> bool {
    value.chars().count() <= MAX_VALUE_LENGTH
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '.' | '_' | '@' | ':' | '/' | '+' | '=' | ',' | '-')
        })
}

/// Parses only the first line of `comment` (spec §3). Returns `Ok(None)` when
/// the first line does not start with `/`; once it does, grammar violations
/// are reported as a typed, positioned [`ParseError`] rather than silently
/// falling back to "not a command".
pub fn parse_comment(comment: &str) -> Result<Option<ParsedCommand>, ParseError> {
    let raw_line = comment.lines().next().unwrap_or("");
    if !raw_line.starts_with('/') {
        return Ok(None);
    }

    let chars: Vec<char> = raw_line.chars().collect();
    let (name, name_end) = scan_command_name(&chars)?;
    let raw_tokens = tokenize(&chars, name_end)?;

    let mut positionals = Vec::new();
    let mut named = HashMap::new();
    let mut tokens = raw_tokens.into_iter();
    let mut pending = tokens.next();

    while let Some((token, col)) = pending.take() {
        pending = tokens.next();

        let Some(key_token) = strip_double_dash(&token) else {
            positionals.push(dequote(&token));
            continue;
        };

        let (key_chars, inline_value) = split_key_value(&key_token);
        let key = dequote(&key_chars);
        validate_key(&key, col)?;

        let value = if let Some(v) = inline_value {
            dequote(&v)
        } else {
            match pending.take() {
                Some((value_token, _)) if strip_double_dash(&value_token).is_none() => {
                    pending = tokens.next();
                    dequote(&value_token)
                }
                Some(other) => {
                    pending = Some(other);
                    "true".to_string()
                }
                None => "true".to_string(),
            }
        };

        if named.contains_key(&key) {
            return Err(ParseError::DuplicateKey { key, column: col });
        }
        named.insert(key, value);
    }

    Ok(Some(ParsedCommand {
        name,
        positionals,
        named,
        raw_line: raw_line.to_string(),
    }))
}

/// The §3.2 scan: a configured command name on any line after the first.
/// `configured` must already be lowercased command names.
pub fn find_misplaced_command(
    comment: &str,
    configured: &BTreeSet<String>,
) -> Option<MisplacedCommand> {
    for (offset, line) in comment.lines().enumerate().skip(1) {
        if !line.starts_with('/') {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        if let Ok((name, _)) = scan_command_name(&chars) {
            if configured.contains(&name) {
                return Some(MisplacedCommand {
                    name,
                    line: offset + 1,
                });
            }
        }
    }
    None
}

/// Consumes `/[a-z][a-z0-9-]*` case-insensitively from `chars` (index 0 must
/// be `/`), lowercases it, and returns the name plus the index right after it.
fn scan_command_name(chars: &[char]) -> Result<(String, usize), ParseError> {
    let mut idx = 1usize;
    let mut name = String::new();

    match chars.get(idx) {
        Some(c) if c.is_ascii_alphabetic() => {
            name.push(c.to_ascii_lowercase());
            idx += 1;
        }
        _ => return Err(ParseError::InvalidCommandName { column: idx + 1 }),
    }

    while let Some(c) = chars.get(idx) {
        if c.is_ascii_alphanumeric() || *c == '-' {
            name.push(c.to_ascii_lowercase());
            idx += 1;
        } else {
            break;
        }
    }

    match chars.get(idx) {
        None => {}
        Some(c) if c.is_whitespace() => {}
        Some(_) => return Err(ParseError::InvalidCommandName { column: idx + 1 }),
    }

    Ok((name, idx))
}

/// Splits `chars[start..]` into whitespace-delimited tokens, treating double
/// quotes (with `\"`/`\\` escapes) as grouping spaces into one token. Tokens
/// are returned raw (still quoted/escaped) alongside their 1-based start column.
fn tokenize(chars: &[char], start: usize) -> Result<Vec<(Vec<char>, usize)>, ParseError> {
    let mut tokens = Vec::new();
    let mut idx = start;

    loop {
        while matches!(chars.get(idx), Some(c) if c.is_whitespace()) {
            idx += 1;
        }
        if chars.get(idx).is_none() {
            break;
        }

        let token_start_col = idx + 1;
        let mut buf: Vec<char> = Vec::new();
        let mut in_quotes = false;
        let mut quote_open_col = 0usize;

        loop {
            let Some(c) = chars.get(idx).copied() else {
                if in_quotes {
                    return Err(ParseError::UnterminatedQuote {
                        column: quote_open_col,
                    });
                }
                break;
            };

            if in_quotes {
                if c == '\\' {
                    if let Some(next_c) = chars.get(idx + 1).copied() {
                        if next_c == '"' || next_c == '\\' {
                            buf.push(c);
                            buf.push(next_c);
                            idx += 2;
                            continue;
                        }
                    }
                }
                if c == '"' {
                    in_quotes = false;
                }
                buf.push(c);
                idx += 1;
            } else if c.is_whitespace() {
                break;
            } else {
                if c == '"' {
                    in_quotes = true;
                    quote_open_col = idx + 1;
                }
                buf.push(c);
                idx += 1;
            }
        }

        tokens.push((buf, token_start_col));
    }

    Ok(tokens)
}

fn strip_double_dash(token: &[char]) -> Option<Vec<char>> {
    let mut iter = token.iter();
    if iter.next() == Some(&'-') && iter.next() == Some(&'-') {
        Some(iter.copied().collect())
    } else {
        None
    }
}

/// Splits a `--key`-stripped token on the first `=` seen outside quotes.
fn split_key_value(token: &[char]) -> (Vec<char>, Option<Vec<char>>) {
    let mut key = Vec::new();
    let mut in_quotes = false;
    let mut iter = token.iter().copied().peekable();

    while let Some(c) = iter.peek().copied() {
        if !in_quotes && c == '=' {
            iter.next();
            return (key, Some(iter.collect()));
        }
        if c == '"' {
            in_quotes = !in_quotes;
        }
        key.push(c);
        iter.next();
    }

    (key, None)
}

/// Strips grouping quotes and resolves `\"`/`\\` escapes.
fn dequote(token: &[char]) -> String {
    let mut out = String::new();
    let mut in_quotes = false;
    let mut iter = token.iter().copied().peekable();

    while let Some(c) = iter.next() {
        if in_quotes {
            if c == '\\' {
                match iter.peek().copied() {
                    Some(next) if next == '"' || next == '\\' => {
                        out.push(next);
                        iter.next();
                        continue;
                    }
                    _ => {
                        out.push('\\');
                        continue;
                    }
                }
            }
            if c == '"' {
                in_quotes = false;
            } else {
                out.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else {
            out.push(c);
        }
    }

    out
}

/// `^[a-z][a-z0-9_-]*$`: shared by `--key` command-line keys here and by
/// `args[].name` in `slash-config` (spec §4.1) — a config arg's `name` is
/// exactly the key a commenter types on the command line, so both must
/// accept the same charset.
pub fn is_valid_arg_name(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn validate_key(key: &str, column: usize) -> Result<(), ParseError> {
    if !is_valid_arg_name(key) {
        return Err(ParseError::InvalidKey {
            key: key.to_string(),
            column,
        });
    }
    if key.starts_with("slash_") {
        return Err(ParseError::ReservedKey {
            key: key.to_string(),
            column,
        });
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;

    #[test]
    fn ignores_non_commands_and_parses_only_the_first_line() {
        assert_eq!(parse_comment("hello\n/deploy staging"), Ok(None));

        let parsed = parse_comment("/DePloY staging\n--force").unwrap();
        assert_eq!(
            parsed,
            Some(ParsedCommand {
                name: "deploy".into(),
                positionals: vec!["staging".into()],
                named: HashMap::new(),
                raw_line: "/DePloY staging".into(),
            })
        );
    }

    #[test]
    fn parses_named_forms_flags_and_quotes() {
        let parsed =
            parse_comment("/deploy staging --force --timeout=30m --reason=\"hot \\\"fix\\\"\"")
                .unwrap()
                .unwrap();

        assert_eq!(parsed.positionals, ["staging"]);
        assert_eq!(parsed.named["force"], "true");
        assert_eq!(parsed.named["timeout"], "30m");
        assert_eq!(parsed.named["reason"], "hot \"fix\"");
    }

    #[test]
    fn consumes_the_next_token_as_an_option_value() {
        let parsed = parse_comment("/echo --message hello").unwrap().unwrap();
        assert!(parsed.positionals.is_empty());
        assert_eq!(parsed.named["message"], "hello");
    }

    #[test]
    fn rejects_reserved_duplicate_and_malformed_keys() {
        assert!(matches!(
            parse_comment("/echo --slash_head_sha=bad"),
            Err(ParseError::ReservedKey { .. })
        ));
        assert!(matches!(
            parse_comment("/echo --value=one --value=two"),
            Err(ParseError::DuplicateKey { .. })
        ));
        assert!(matches!(
            parse_comment("/echo --Bad=value"),
            Err(ParseError::InvalidKey { .. })
        ));
    }

    #[test]
    fn reports_human_readable_error_positions() {
        let error = parse_comment("/echo \"unterminated").unwrap_err();
        assert_eq!(error.column(), 7);
        assert!(error.to_string().contains("column 7"));

        let error = parse_comment("/9echo").unwrap_err();
        assert_eq!(error.column(), 2);
    }

    #[test]
    fn validates_the_default_safe_value_policy() {
        assert!(is_safe_value("release-1.2/@team:ok+yes,now"));
        assert!(is_safe_value(""));
        assert!(!is_safe_value("has spaces"));
        assert!(!is_safe_value("$(unsafe)"));
        assert!(!is_safe_value(&"x".repeat(MAX_VALUE_LENGTH + 1)));
    }

    #[test]
    fn finds_only_configured_commands_on_later_lines() {
        let configured = BTreeSet::from(["deploy".to_owned(), "echo".to_owned()]);
        assert_eq!(
            find_misplaced_command("Looks good\n/DEPLOY staging\n/unknown", &configured),
            Some(MisplacedCommand {
                name: "deploy".into(),
                line: 2
            })
        );
        assert_eq!(
            find_misplaced_command("Text\n/deployment staging", &configured),
            None
        );
    }

    proptest! {
        #[test]
        fn quoted_values_round_trip(value in "[A-Za-z0-9 ._@:/+=,\\\\-]{0,128}") {
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            let input = format!("/echo --message=\"{escaped}\"");
            let parsed = parse_comment(&input).unwrap().unwrap();
            prop_assert_eq!(&parsed.named["message"], &value);
        }

        #[test]
        fn arbitrary_input_never_panics(input in any::<String>()) {
            let _ = parse_comment(&input);
        }

        #[test]
        fn reserved_keys_never_reach_output(
            suffix in "[a-z0-9_-]{0,30}",
            value in "[A-Za-z0-9._@:/+=,\\-]{0,30}",
        ) {
            let input = format!("/echo --slash_{suffix}={value}");
            if let Ok(Some(command)) = parse_comment(&input) {
                prop_assert!(command.named.keys().all(|key| !key.starts_with("slash_")));
            }
        }
    }
}
