//! The spec §6.2 `workflow_run` conclusion → check-run conclusion mapping.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckConclusion {
    Success,
    Failure,
    Cancelled,
    TimedOut,
    ActionRequired,
    Neutral,
}

impl CheckConclusion {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::ActionRequired => "action_required",
            Self::Neutral => "neutral",
        }
    }
}

/// Maps a `workflow_run.conclusion` value to a check-run conclusion (spec
/// §6.2). An unrecognized or missing value maps to `Neutral`, with the raw
/// value returned for inclusion in the check-run summary — the mapping
/// itself never fails on unfamiliar input.
pub fn map_conclusion(workflow_run_conclusion: Option<&str>) -> (CheckConclusion, Option<String>) {
    match workflow_run_conclusion {
        Some("success") => (CheckConclusion::Success, None),
        Some("failure") => (CheckConclusion::Failure, None),
        Some("cancelled") => (CheckConclusion::Cancelled, None),
        Some("timed_out") => (CheckConclusion::TimedOut, None),
        Some("action_required") => (CheckConclusion::ActionRequired, None),
        Some("skipped") | Some("neutral") | Some("stale") => (CheckConclusion::Neutral, None),
        Some("startup_failure") => (CheckConclusion::Failure, None),
        Some(other) => (CheckConclusion::Neutral, Some(other.to_string())),
        None => (CheckConclusion::Neutral, Some("null".to_string())),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn maps_every_documented_conclusion() {
        assert_eq!(
            map_conclusion(Some("success")),
            (CheckConclusion::Success, None)
        );
        assert_eq!(
            map_conclusion(Some("failure")),
            (CheckConclusion::Failure, None)
        );
        assert_eq!(
            map_conclusion(Some("cancelled")),
            (CheckConclusion::Cancelled, None)
        );
        assert_eq!(
            map_conclusion(Some("timed_out")),
            (CheckConclusion::TimedOut, None)
        );
        assert_eq!(
            map_conclusion(Some("action_required")),
            (CheckConclusion::ActionRequired, None)
        );
        assert_eq!(
            map_conclusion(Some("skipped")),
            (CheckConclusion::Neutral, None)
        );
        assert_eq!(
            map_conclusion(Some("neutral")),
            (CheckConclusion::Neutral, None)
        );
        assert_eq!(
            map_conclusion(Some("stale")),
            (CheckConclusion::Neutral, None)
        );
        assert_eq!(
            map_conclusion(Some("startup_failure")),
            (CheckConclusion::Failure, None)
        );
    }

    #[test]
    fn maps_unknown_or_missing_to_neutral_with_the_raw_value() {
        assert_eq!(
            map_conclusion(Some("something_new")),
            (CheckConclusion::Neutral, Some("something_new".to_string()))
        );
        assert_eq!(
            map_conclusion(None),
            (CheckConclusion::Neutral, Some("null".to_string()))
        );
    }
}
