//! API key rotation pool — ported from openclaw MIT (c) Peter Steinberger 2025.
//!
//! Holds N keys per provider. On each call, returns the `last_good` key first
//! (stickiness), then rotates round-robin on failure. On success, updates
//! `last_good`. Cooldown markers keep failed keys out of rotation for
//! a configurable window.

use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
struct KeyState {
    key: String,
    last_failed_at: Option<Instant>,
}

/// Stateful rotation over a pool of API keys per provider.
///
/// Use [`KeyPool::next_key`] to get the current best key. Call
/// [`KeyPool::mark_failure`] on any provider error to demote that key for the
/// cooldown window, and [`KeyPool::mark_success`] to set it as `last_good`.
/// Duplicate keys are filtered at construction (matches openclaw's
/// `dedupeApiKeys` invariant).
///
/// # Concurrency
///
/// `next_key`, `mark_success`, and `mark_failure` all take `&mut self`, so
/// a single `KeyPool` cannot be shared across tasks or threads by reference.
/// Callers that need to share rotation state across concurrent providers
/// must wrap the pool in `Arc<Mutex<KeyPool>>` (or
/// `Arc<tokio::sync::Mutex<KeyPool>>` for async paths) themselves — this
/// type does NOT enforce that contract internally.
pub struct KeyPool {
    keys: Vec<KeyState>,
    last_good_idx: Option<usize>,
    cursor: usize,
    cooldown: Duration,
}

/// Split a configured credential string into individual API keys.
///
/// Providers accept multiple keys in a single `api_key` value separated by
/// commas or ASCII whitespace (spaces, tabs, newlines). This splits on either,
/// trims each token, and drops empties. A single key (the common case) yields a
/// one-element vector — so a `KeyPool` built from it behaves exactly like the
/// pre-rotation single-key path. Order is preserved; deduplication is left to
/// [`KeyPool::with_cooldown`], which already dedupes at construction.
pub fn split_keys(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c.is_ascii_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

impl KeyPool {
    pub fn new(keys: impl IntoIterator<Item = String>) -> Self {
        Self::with_cooldown(keys, Duration::from_secs(60))
    }

    pub fn with_cooldown(keys: impl IntoIterator<Item = String>, cooldown: Duration) -> Self {
        let mut seen = std::collections::HashSet::new();
        let keys: Vec<KeyState> = keys
            .into_iter()
            .filter(|k| !k.trim().is_empty())
            .filter(|k| seen.insert(k.clone()))
            .map(|key| KeyState {
                key,
                last_failed_at: None,
            })
            .collect();
        Self {
            keys,
            last_good_idx: None,
            cursor: 0,
            cooldown,
        }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Return the next key to try. Prefers `last_good`, then rotates round-robin
    /// skipping keys still in cooldown. Returns None if every key is cooling.
    pub fn next_key(&mut self) -> Option<&str> {
        if self.keys.is_empty() {
            return None;
        }
        let now = Instant::now();

        if let Some(idx) = self.last_good_idx
            && !self.is_in_cooldown(idx, now)
        {
            return Some(self.keys[idx].key.as_str());
        }

        for _ in 0..self.keys.len() {
            let idx = self.cursor % self.keys.len();
            self.cursor = self.cursor.wrapping_add(1);
            if !self.is_in_cooldown(idx, now) {
                return Some(self.keys[idx].key.as_str());
            }
        }
        None
    }

    fn is_in_cooldown(&self, idx: usize, now: Instant) -> bool {
        match self.keys[idx].last_failed_at {
            Some(t) => now.duration_since(t) < self.cooldown,
            None => false,
        }
    }

    pub fn mark_success(&mut self, key: &str) {
        if let Some(idx) = self.keys.iter().position(|k| k.key == key) {
            self.last_good_idx = Some(idx);
            self.keys[idx].last_failed_at = None;
        }
    }

    pub fn mark_failure(&mut self, key: &str) {
        if let Some(idx) = self.keys.iter().position(|k| k.key == key) {
            self.keys[idx].last_failed_at = Some(Instant::now());
            if self.last_good_idx == Some(idx) {
                self.last_good_idx = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pool_returns_none() {
        let mut p = KeyPool::new(Vec::<String>::new());
        assert!(p.is_empty());
        assert!(p.next_key().is_none());
    }

    #[test]
    fn empty_strings_filtered() {
        let p = KeyPool::new(vec!["".into(), "  ".into(), "real".into()]);
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn rotates_round_robin() {
        let mut p = KeyPool::new(vec!["a".into(), "b".into(), "c".into()]);
        let first = p.next_key().unwrap().to_string();
        let second = p.next_key().unwrap().to_string();
        let third = p.next_key().unwrap().to_string();
        assert!(
            [first, second, third]
                .iter()
                .all(|k| ["a", "b", "c"].contains(&k.as_str()))
        );
    }

    #[test]
    fn last_good_sticky() {
        let mut p = KeyPool::new(vec!["a".into(), "b".into(), "c".into()]);
        p.mark_success("b");
        assert_eq!(p.next_key(), Some("b"));
        assert_eq!(p.next_key(), Some("b"));
    }

    #[test]
    fn mark_failure_demotes_last_good() {
        let mut p = KeyPool::new(vec!["a".into(), "b".into()]);
        p.mark_success("a");
        assert_eq!(p.next_key(), Some("a"));
        p.mark_failure("a");
        assert_ne!(p.next_key(), Some("a"));
    }

    #[test]
    fn all_failed_returns_none_until_cooldown() {
        let mut p = KeyPool::with_cooldown(vec!["a".into(), "b".into()], Duration::from_secs(60));
        p.mark_failure("a");
        p.mark_failure("b");
        assert!(p.next_key().is_none());
    }

    #[test]
    fn cooldown_expiry_unblocks() {
        let mut p = KeyPool::with_cooldown(vec!["a".into()], Duration::from_millis(10));
        p.mark_failure("a");
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(p.next_key(), Some("a"));
    }

    #[test]
    fn mark_success_resets_failure() {
        let mut p = KeyPool::new(vec!["a".into()]);
        p.mark_failure("a");
        p.mark_success("a");
        assert_eq!(p.next_key(), Some("a"));
    }

    #[test]
    fn unknown_key_marks_are_noops() {
        let mut p = KeyPool::new(vec!["a".into()]);
        p.mark_success("nonexistent");
        p.mark_failure("nonexistent");
        assert_eq!(p.next_key(), Some("a"));
    }

    #[test]
    fn split_keys_single_key_is_one_element() {
        // The common case: a lone key yields a one-element pool — identical to
        // the pre-rotation single-key behavior.
        assert_eq!(split_keys("sk-abc123"), vec!["sk-abc123".to_string()]);
    }

    #[test]
    fn split_keys_splits_on_commas_and_whitespace() {
        assert_eq!(
            split_keys("a,b c\td\ne"),
            vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
                "e".to_string()
            ]
        );
    }

    #[test]
    fn split_keys_trims_and_drops_empties() {
        assert_eq!(
            split_keys("  a , , b ,, "),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(split_keys("").is_empty());
        assert!(split_keys("   ,  , ").is_empty());
    }
}
