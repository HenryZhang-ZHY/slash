//! The spec §6.4 anti-spam rules that don't require durable state:
//! edit-distance matching for the "unknown command" reply gate, and a token
//! bucket for the mutating-call budget. Per-PR comment dedup and the
//! trusted-actor requirement are decided from data `slash-server` already
//! holds (posted-causes, resolved permission) and don't need dedicated types
//! here.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Levenshtein edit distance, used to decide whether an unrecognized first
/// token is "close enough" to a configured command name to be worth a
/// suggestion (spec §6.4: within edit distance 2, or no commands configured
/// at all).
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());

    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr = vec![0usize; m + 1];

    for i in 1..=n {
        if let Some(first) = curr.first_mut() {
            *first = i;
        }
        for j in 1..=m {
            let cost = usize::from(a.get(i - 1) != b.get(j - 1));
            let up = prev.get(j).copied().unwrap_or(0) + 1;
            let left = curr.get(j - 1).copied().unwrap_or(0) + 1;
            let diag = prev.get(j - 1).copied().unwrap_or(0) + cost;
            if let Some(slot) = curr.get_mut(j) {
                *slot = up.min(left).min(diag);
            }
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev.get(m).copied().unwrap_or(0)
}

/// Spec §6.4: reply with the command list only when the typed name is
/// within edit distance 2 of a configured command, or the repository has no
/// commands configured at all (the "installed but not configured" case).
pub fn should_suggest_commands(typed: &str, configured: &[String]) -> bool {
    if configured.is_empty() {
        return true;
    }
    configured
        .iter()
        .any(|name| edit_distance(typed, name) <= 2)
}

/// A per-key token bucket (spec §6.4): mutating GitHub calls are rate
/// limited per `(installation, repo, actor)` and per `installation`, so one
/// attacker on one public repo cannot degrade Slash for an entire org.
/// Comments are suppressed before reactions when the budget runs out.
pub struct TokenBucket {
    capacity: u32,
    refill_period: Duration,
    buckets: HashMap<String, (u32, Instant)>,
}

impl TokenBucket {
    pub fn new(capacity: u32, refill_period: Duration) -> Self {
        Self {
            capacity,
            refill_period,
            buckets: HashMap::new(),
        }
    }

    /// Attempts to spend one token for `key`. Returns `true` if the call
    /// should proceed.
    pub fn try_spend(&mut self, key: &str, now: Instant) -> bool {
        let entry = self
            .buckets
            .entry(key.to_string())
            .or_insert((self.capacity, now));

        if now.duration_since(entry.1) >= self.refill_period {
            *entry = (self.capacity, now);
        }

        if entry.0 == 0 {
            return false;
        }

        entry.0 -= 1;
        true
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn edit_distance_of_identical_strings_is_zero() {
        assert_eq!(edit_distance("deploy", "deploy"), 0);
    }

    #[test]
    fn edit_distance_counts_substitutions_insertions_deletions() {
        assert_eq!(edit_distance("deploy", "deploi"), 1);
        assert_eq!(edit_distance("deploy", "deployy"), 1);
        assert_eq!(edit_distance("deploy", "deplo"), 1);
        assert_eq!(edit_distance("deploy", "dploy"), 1);
    }

    #[test]
    fn edit_distance_is_symmetric() {
        assert_eq!(
            edit_distance("kitten", "sitting"),
            edit_distance("sitting", "kitten")
        );
        assert_eq!(edit_distance("kitten", "sitting"), 3);
    }

    #[test]
    fn suggests_commands_within_edit_distance_2() {
        let configured = vec!["deploy".to_string(), "echo".to_string()];
        assert!(should_suggest_commands("deplyo", &configured)); // transposition-ish, distance 2
        assert!(should_suggest_commands("deploy", &configured));
        assert!(!should_suggest_commands(
            "completely-different",
            &configured
        ));
    }

    #[test]
    fn always_suggests_when_nothing_is_configured() {
        assert!(should_suggest_commands("anything", &[]));
    }

    #[test]
    fn token_bucket_allows_up_to_capacity_then_blocks() {
        let mut bucket = TokenBucket::new(2, Duration::from_secs(60));
        let now = Instant::now();
        assert!(bucket.try_spend("k", now));
        assert!(bucket.try_spend("k", now));
        assert!(!bucket.try_spend("k", now));
    }

    #[test]
    fn token_bucket_refills_after_the_period() {
        let mut bucket = TokenBucket::new(1, Duration::from_millis(10));
        let now = Instant::now();
        assert!(bucket.try_spend("k", now));
        assert!(!bucket.try_spend("k", now));

        let later = now + Duration::from_millis(11);
        assert!(bucket.try_spend("k", later));
    }

    #[test]
    fn token_bucket_keys_are_independent() {
        let mut bucket = TokenBucket::new(1, Duration::from_secs(60));
        let now = Instant::now();
        assert!(bucket.try_spend("a", now));
        assert!(bucket.try_spend("b", now));
        assert!(!bucket.try_spend("a", now));
    }
}
