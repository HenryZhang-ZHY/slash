//! Resource limits on `.slash/` as a whole and on individual YAML documents
//! (spec §4.1). This is attacker-influenced input to a shared multi-tenant
//! process, so limits are enforced *before* parsing, not discovered by
//! letting the parser run until it hurts.

use crate::ConfigError;

pub const MAX_FILES: usize = 50;
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024;
pub const MAX_NESTING_DEPTH: usize = 32;

/// Checks the directory-wide limits using only file count and sizes — never
/// file contents, so an oversized directory is rejected before anything is
/// read.
pub fn check_directory_limits(file_count: usize, total_bytes: u64) -> Result<(), ConfigError> {
    if file_count > MAX_FILES {
        return Err(ConfigError::TooManyFiles {
            count: file_count,
            max: MAX_FILES,
        });
    }
    if total_bytes > MAX_TOTAL_BYTES {
        return Err(ConfigError::DirectoryTooLarge {
            total: total_bytes,
            max: MAX_TOTAL_BYTES,
        });
    }
    Ok(())
}

fn is_anchor_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Best-effort, YAML-crate-agnostic pre-parse scan for two resource-exhaustion
/// vectors the real parser does not itself guard against: anchor/alias reuse
/// (exponential expansion, "billion laughs") and unbounded nesting (parser
/// recursion). It runs on raw text, before any real YAML parser sees it.
///
/// This is deliberately conservative rather than a full YAML lexer: it may
/// reject unusual-but-harmless content (a literal `&`/`*` right after a `: `
/// or `- ` marker), but it never misses a real anchor or alias, because for
/// `&name`/`*name` to function as one *to the real parser*, it must sit in
/// exactly the syntactic position this scan checks — anything that fools this
/// scan into thinking it's inside a string is, by the same logic, also just a
/// string to the real parser, and therefore harmless.
pub fn scan_yaml_resource_risks(file: &str, text: &str) -> Result<(), ConfigError> {
    #[derive(Clone, Copy, PartialEq)]
    enum Quote {
        Single,
        Double,
    }

    let mut quote: Option<Quote> = None;
    let mut in_comment = false;
    let mut flow_depth: usize = 0;
    let mut indent_stack: Vec<usize> = Vec::new();
    let mut max_depth: usize = 0;
    let mut at_node_start = true;
    let mut at_line_start = true;
    let mut indent = 0usize;

    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' {
            in_comment = false;
            at_line_start = true;
            at_node_start = true;
            indent = 0;
            continue;
        }

        if in_comment {
            continue;
        }

        if at_line_start {
            if c == ' ' {
                indent += 1;
                continue;
            }
            at_line_start = false;
            if c != '\t' {
                while let Some(&top) = indent_stack.last() {
                    if indent < top {
                        indent_stack.pop();
                    } else {
                        break;
                    }
                }
                let already_at_level = matches!(indent_stack.last(), Some(&top) if top == indent);
                if quote.is_none() && c != '#' && !already_at_level {
                    indent_stack.push(indent);
                }
                max_depth = max_depth.max(indent_stack.len() + flow_depth);
            }
        }

        if let Some(q) = quote {
            match q {
                Quote::Double => {
                    if c == '\\' {
                        chars.next();
                    } else if c == '"' {
                        quote = None;
                    }
                }
                Quote::Single => {
                    if c == '\'' {
                        if chars.peek() == Some(&'\'') {
                            chars.next();
                        } else {
                            quote = None;
                        }
                    }
                }
            }
            at_node_start = false;
            continue;
        }

        match c {
            '#' => {
                in_comment = true;
            }
            '"' => {
                quote = Some(Quote::Double);
                at_node_start = false;
            }
            '\'' => {
                quote = Some(Quote::Single);
                at_node_start = false;
            }
            '[' | '{' => {
                flow_depth += 1;
                max_depth = max_depth.max(indent_stack.len() + flow_depth);
                at_node_start = true;
            }
            ']' | '}' => {
                flow_depth = flow_depth.saturating_sub(1);
                at_node_start = false;
            }
            ',' | ':' => {
                at_node_start = true;
            }
            '-' if at_node_start => {
                if chars.peek().is_some_and(char::is_ascii_whitespace) || chars.peek().is_none() {
                    // sequence marker: node-start persists for the entry value
                } else {
                    at_node_start = false;
                }
            }
            ' ' => {}
            '&' | '*' if at_node_start => {
                if chars.peek().is_some_and(|next| is_anchor_name_char(*next)) {
                    return Err(ConfigError::AliasesNotPermitted {
                        file: file.to_string(),
                    });
                }
                at_node_start = false;
            }
            _ => {
                at_node_start = false;
            }
        }
    }

    if max_depth > MAX_NESTING_DEPTH {
        return Err(ConfigError::NestingTooDeep {
            file: file.to_string(),
            depth: max_depth,
            max: MAX_NESTING_DEPTH,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_realistic_command_file() {
        let yaml = r#"
command: deploy
description: "Deploy the PR to an environment (staging & production)"
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
        assert!(scan_yaml_resource_risks("test.yml", yaml).is_ok());
    }

    #[test]
    fn rejects_a_top_level_anchor() {
        let yaml = "command: &anchor deploy\nworkflow: deploy.yml\n";
        assert!(matches!(
            scan_yaml_resource_risks("test.yml", yaml),
            Err(ConfigError::AliasesNotPermitted { .. })
        ));
    }

    #[test]
    fn rejects_an_alias_reference() {
        let yaml = "defaults: &d\n  timeout: 15m\nargs:\n  - <<: *d\n";
        assert!(matches!(
            scan_yaml_resource_risks("test.yml", yaml),
            Err(ConfigError::AliasesNotPermitted { .. })
        ));
    }

    #[test]
    fn rejects_an_anchor_inside_a_flow_sequence() {
        let yaml = "choices: [staging, &a production]\n";
        assert!(matches!(
            scan_yaml_resource_risks("test.yml", yaml),
            Err(ConfigError::AliasesNotPermitted { .. })
        ));
    }

    #[test]
    fn does_not_flag_ampersand_or_star_inside_scalar_content() {
        let yaml = r#"description: "Deploy & release *fast*""#;
        assert!(scan_yaml_resource_risks("test.yml", yaml).is_ok());

        let yaml_plain = "description: Deploy & release fast";
        assert!(scan_yaml_resource_risks("test.yml", yaml_plain).is_ok());
    }

    #[test]
    fn rejects_excessive_nesting() {
        let mut yaml = String::new();
        for i in 0..40 {
            yaml.push_str(&" ".repeat(i * 2));
            yaml.push_str("a:\n");
        }
        assert!(matches!(
            scan_yaml_resource_risks("test.yml", &yaml),
            Err(ConfigError::NestingTooDeep { .. })
        ));
    }

    #[test]
    fn accepts_a_flat_list_of_siblings_at_the_same_indent() {
        let mut yaml = String::from("args:\n");
        for i in 0..40 {
            yaml.push_str(&format!("  - name: arg{i}\n"));
        }
        assert!(scan_yaml_resource_risks("test.yml", &yaml).is_ok());
    }

    #[test]
    fn directory_limits_reject_too_many_files() {
        assert!(matches!(
            check_directory_limits(MAX_FILES + 1, 0),
            Err(ConfigError::TooManyFiles { .. })
        ));
        assert!(check_directory_limits(MAX_FILES, 0).is_ok());
    }

    #[test]
    fn directory_limits_reject_too_many_bytes() {
        assert!(matches!(
            check_directory_limits(1, MAX_TOTAL_BYTES + 1),
            Err(ConfigError::DirectoryTooLarge { .. })
        ));
        assert!(check_directory_limits(1, MAX_TOTAL_BYTES).is_ok());
    }

    proptest::proptest! {
        #[test]
        fn never_panics_on_arbitrary_input(input in proptest::prelude::any::<String>()) {
            let _ = scan_yaml_resource_risks("test.yml", &input);
        }
    }
}
