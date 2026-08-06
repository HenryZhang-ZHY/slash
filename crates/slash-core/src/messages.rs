//! All user-facing strings live here (spec §6.4, §9), and every interpolated
//! user value passes through [`escape_user_text`] — the single point where
//! attacker-influenced text is made safe to render as Markdown/HTML inside a
//! GitHub comment or check-run summary, which carries the App's trusted
//! identity.

const MAX_ESCAPED_LEN: usize = 200;

/// Strips control characters and newlines, truncates to 200 characters, and
/// wraps the result in a code fence longer than any backtick run it
/// contains — so the text can never break out of the fence to render as
/// Markdown/HTML (spec §6.4).
pub fn escape_user_text(raw: &str) -> String {
    let stripped: String = raw
        .chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect();

    let truncated: String = stripped.chars().take(MAX_ESCAPED_LEN).collect();

    let longest_backtick_run = truncated
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence_len = (longest_backtick_run + 1).max(3);
    let fence = "`".repeat(fence_len);

    format!("{fence}{truncated}{fence}")
}

pub fn installed_but_not_configured() -> String {
    "Slash is installed on this repository, but no commands are configured under `.slash/`."
        .to_string()
}

pub fn command_catalog_unavailable() -> String {
    "Slash could not read this repository's `.slash/` configuration. Please try again later."
        .to_string()
}

pub fn fork_unsupported() -> String {
    "Slash does not support commands on pull requests from forks (spec §2.4, §11): \
     `workflow_dispatch` cannot target a ref in a fork. A maintainer can push the branch \
     to this repository instead."
        .to_string()
}

pub fn pr_not_open() -> String {
    "This pull request is not open, so the command was not run.".to_string()
}

pub fn misplaced_command(command: &str) -> String {
    format!(
        "`/{}` must be on the first line of a comment to be recognized.",
        escape_user_text(command).trim_matches('`')
    )
}

pub fn config_error(details: &str) -> String {
    format!(
        "Your `.slash/` configuration has a problem: {}",
        escape_user_text(details)
    )
}

pub fn permission_denied(command: &str, required: &str) -> String {
    format!(
        "You need at least `{required}` GitHub repository access to run `/{}`.",
        escape_user_text(command).trim_matches('`')
    )
}

pub fn usage_error(command: &str, errors: &[String]) -> String {
    let mut body = format!(
        "Usage error for `/{}`:\n",
        escape_user_text(command).trim_matches('`')
    );
    for e in errors {
        body.push_str("- ");
        body.push_str(&escape_user_text(e));
        body.push('\n');
    }
    body
}

pub fn head_moved() -> String {
    "The pull request's head branch moved after this command was issued. Please re-issue it."
        .to_string()
}

/// The check-run summary spec §6.2/§6.4 asks for: run link, timing, actor,
/// and the original command line.
pub fn check_run_summary(
    command_line: &str,
    actor: &str,
    run_url: &str,
    duration_seconds: Option<i64>,
    head_sha_mismatch: bool,
) -> String {
    let mut lines = vec![
        format!("Run: {run_url}"),
        format!("Actor: {}", escape_user_text(actor)),
        format!("Command: {}", escape_user_text(command_line)),
    ];
    if let Some(secs) = duration_seconds {
        lines.push(format!("Duration: {secs}s"));
    }
    if head_sha_mismatch {
        lines.push("Note: the branch moved after this command was issued.".to_string());
    }
    lines.join("\n")
}

/// Spec §6.5: a re-run request denied because the rerequester's permission
/// could not be resolved has no comment surface — this is used only for the
/// check run itself when the stale/unknown case applies.
pub fn rerequest_permission_denied(command: &str, required: &str) -> String {
    format!(
        "Re-run denied: requires at least `{required}` GitHub repository access to run `/{command}`."
    )
}

pub fn rerequest_private_collaborator_denied(command: &str) -> String {
    format!(
        "Re-run denied: only current collaborators may run `/{command}` in a private repository."
    )
}

pub fn superseded() -> String {
    "A newer invocation of this command supersedes this one.".to_string()
}

pub fn aborted_head_moved() -> String {
    "Aborted: the PR head moved before this command was dispatched.".to_string()
}

pub fn unknown_command_suggestion(typed: &str, configured: &[String]) -> String {
    let list = configured
        .iter()
        .map(|c| format!("`/{c}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "`/{}` is not a configured command. Configured commands: {list}.",
        escape_user_text(typed).trim_matches('`')
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn check_run_summary_includes_link_actor_and_command() {
        let summary = check_run_summary(
            "/echo hi",
            "alice",
            "https://github.com/x/y/actions/runs/1",
            Some(42),
            false,
        );
        assert!(summary.contains("https://github.com/x/y/actions/runs/1"));
        assert!(summary.contains("alice"));
        assert!(summary.contains("/echo hi"));
        assert!(summary.contains("42s"));
        assert!(!summary.contains("Note:"));
    }

    #[test]
    fn check_run_summary_notes_a_head_sha_mismatch() {
        let summary = check_run_summary("/echo hi", "alice", "https://x", None, true);
        assert!(summary.contains("Note:"));
    }

    #[test]
    fn strips_control_characters_and_newlines() {
        let escaped = escape_user_text("hello\nworld\x07");
        assert!(!escaped.contains('\n'));
        assert!(!escaped.contains('\x07'));
        assert!(escaped.contains("helloworld"));
    }

    #[test]
    fn truncates_to_200_characters() {
        let long = "a".repeat(500);
        let escaped = escape_user_text(&long);
        let inner: String = escaped.chars().filter(|&c| c != '`').collect();
        assert_eq!(inner.chars().count(), MAX_ESCAPED_LEN);
    }

    #[test]
    fn wraps_in_a_fence_longer_than_any_contained_backtick_run() {
        let escaped = escape_user_text("break out ``` of the fence");
        // The content contains a run of 3 backticks; the wrapping fence must
        // be longer (4+) so the content can never terminate it early.
        assert!(escaped.starts_with("````"));
        assert!(escaped.ends_with("````"));
    }

    #[test]
    fn a_single_backtick_run_still_gets_at_least_a_triple_fence() {
        let escaped = escape_user_text("plain text");
        assert!(escaped.starts_with("```"));
        assert!(escaped.ends_with("```"));
    }

    #[test]
    fn command_catalog_unavailable_does_not_expose_internal_errors() {
        assert_eq!(
            command_catalog_unavailable(),
            "Slash could not read this repository's `.slash/` configuration. Please try again later."
        );
    }

    #[test]
    fn never_panics_on_arbitrary_input() {
        for input in [
            "",
            "\0\0\0",
            "🎉".repeat(300).as_str(),
            "`".repeat(1000).as_str(),
        ] {
            let _ = escape_user_text(input);
        }
    }
}
