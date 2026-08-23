//! API key rotation pool.
//!
//! Holds N keys per provider. On each call, returns the `last_good` key first
//! (stickiness), then rotates round-robin on failure. On success, updates
//! `last_good`. Cooldown markers DEPRIORITISE a failed key for a configurable
//! window: it loses to any healthy key, but a pool where everything is cooling
//! still offers the one closest to recovery rather than reporting itself
//! empty. See [`KeyPool::next_key`] — the difference is the whole reason a
//! rate-limited single-key run used to die claiming the user had no API key.

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
/// Duplicate keys are filtered at construction, so a key supplied twice
/// cannot occupy two rotation slots.
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

    /// Return the next key to try. Prefers `last_good`, then rotates
    /// round-robin skipping keys still in cooldown. Returns `None` only when
    /// no key is CONFIGURED.
    ///
    /// # Why a fully-cooling pool still yields a key
    ///
    /// Cooldown is a preference among keys, not a block on the pool. It used
    /// to be a block, and with the single key that is the normal case that
    /// made a 429 fatal: the 429 demoted the only key, the retry called
    /// `select_key`, the pool answered `None`, and every provider maps that to
    /// [`crate::ProviderError::MissingApiKey`]. The user read "No API key. Set
    /// an api_key via --api-key, the config file, or an API-key environment
    /// variable" for a transient rate limit, and the turn died after ONE
    /// physical send with the blame pointed at their credential.
    ///
    /// Measured on v0.13.5 against a local server answering 429 with
    /// `Retry-After: 7`, n=3: exactly one arrival per run, whole run 0.68 s.
    /// Known-positive control on the same harness, same n: the HTTP 500 arm
    /// produced 3 arrivals per run. So the pool, not the retry loop, was
    /// swallowing the rate-limit retry.
    ///
    /// When every key is cooling, offer the one closest to leaving cooldown —
    /// the least recently failed. Rotation is unchanged whenever a healthy key
    /// exists, and `None` now means exactly what the error it produces says.
    /// Waiting out the cooldown is the retry loop's job, and it does it on a
    /// schedule that honours the server's own `Retry-After`.
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

        // Every key is cooling. Offer the one closest to leaving cooldown
        // rather than reporting a configured pool as unconfigured.
        let idx = self
            .keys
            .iter()
            .enumerate()
            .min_by_key(|(_, state)| state.last_failed_at)
            .map(|(idx, _)| idx)?;
        Some(self.keys[idx].key.as_str())
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

    /// A pool where every key is cooling still yields a key — the one closest
    /// to leaving cooldown. `None` is reserved for "nothing configured",
    /// which is what the `MissingApiKey` it produces actually claims.
    ///
    /// This replaces `all_failed_returns_none_until_cooldown`, which pinned
    /// the behaviour that made a 429 fatal on a single-key pool. See
    /// [`KeyPool::next_key`] for the measurement.
    #[test]
    fn a_fully_cooling_pool_offers_the_key_closest_to_recovery() {
        let mut p = KeyPool::with_cooldown(vec!["a".into(), "b".into()], Duration::from_secs(60));
        p.mark_failure("a");
        std::thread::sleep(Duration::from_millis(5));
        p.mark_failure("b");
        assert_eq!(
            p.next_key(),
            Some("a"),
            "with both keys cooling the older failure is closer to recovery"
        );
    }

    /// The single-key case the product actually runs, stated on its own: one
    /// rate-limited key is still the key, because there is nowhere to rotate.
    #[test]
    fn a_lone_rate_limited_key_is_still_offered() {
        let mut p = KeyPool::with_cooldown(vec!["solo".into()], Duration::from_secs(60));
        assert_eq!(p.next_key(), Some("solo"), "control: the key is configured");
        p.mark_failure("solo");
        assert_eq!(
            p.next_key(),
            Some("solo"),
            "a transient failure on the only key must not read as \"no API key\""
        );
    }

    /// Control for the two tests above: cooldown must still STEER rotation.
    /// A fix that simply always returns something would pass them both and
    /// destroy the reason the pool exists.
    #[test]
    fn a_cooling_key_still_loses_to_a_healthy_one() {
        let mut p = KeyPool::with_cooldown(vec!["a".into(), "b".into()], Duration::from_secs(60));
        p.mark_failure("a");
        for _ in 0..4 {
            assert_eq!(
                p.next_key(),
                Some("b"),
                "a healthy key must win over a cooling one, every time"
            );
        }
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
