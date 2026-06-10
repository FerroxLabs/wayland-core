//! `DedupeCache` — bounded TTL + LRU duplicate-suppression cache.
//!
//! Platforms re-deliver the same inbound message on retries, reconnects,
//! and at-least-once gateway semantics. This cache suppresses
//! reprocessing by remembering message ids that were already admitted,
//! keyed by `(platform, account, message_id)`.
//!
//! Time is caller-supplied (`now_ms`, monotonic millis) so the cache is
//! fully deterministic under test — it never reads the wall clock.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Identity of a single inbound message for dedup purposes.
///
/// Equality is on all three fields: two messages collide only when they
/// share the same platform, receiving account, and platform message id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DedupeKey {
    /// Platform tag (`"slack"`, `"telegram"`, …).
    pub platform: String,
    /// Receiving bot/account identity (empty when single-account).
    pub account_id: String,
    /// Platform-assigned message id.
    pub message_id: String,
}

impl DedupeKey {
    /// Convenience constructor.
    pub fn new(
        platform: impl Into<String>,
        account_id: impl Into<String>,
        message_id: impl Into<String>,
    ) -> Self {
        Self {
            platform: platform.into(),
            account_id: account_id.into(),
            message_id: message_id.into(),
        }
    }
}

/// Bounded duplicate-suppression cache with TTL expiry and LRU eviction.
///
/// Two independent bounds:
///
/// - `ttl_ms`: entries older than this (by `now_ms - first_seen`) are
///   treated as expired and removed lazily on the next call. Duplicates do
///   NOT slide this window — suppression is measured from first sight.
///   `ttl_ms == 0` disables time expiry — entries never expire by age.
/// - `max_size`: when recording a new entry would exceed this count, the
///   least-recently-used entry is evicted first. `max_size == 0` disables
///   size-capping.
///
/// With both `ttl_ms == 0` and `max_size == 0` the cache is effectively
/// unbounded (grows with the number of distinct keys ever seen) — only
/// suitable for short-lived or test scenarios.
#[derive(Debug, Clone)]
pub struct DedupeCache {
    ttl_ms: u64,
    max_size: usize,
    /// key -> entry. TTL expiry is measured from `first_seen` (fixed at
    /// insert), while LRU eviction picks the entry with the smallest
    /// `last_seen` (refreshed on every hit, including duplicates).
    entries: HashMap<DedupeKey, Entry>,
}

/// A tracked key's timestamps. `first_seen` anchors TTL expiry (so a
/// duplicate never slides the suppression window); `last_seen` tracks
/// recency for LRU eviction.
#[derive(Debug, Clone, Copy)]
struct Entry {
    first_seen: u64,
    last_seen: u64,
}

impl DedupeCache {
    /// Construct a cache. See the type docs for the `ttl_ms == 0` /
    /// `max_size == 0` "disabled" semantics.
    pub fn new(ttl_ms: u64, max_size: usize) -> Self {
        Self {
            ttl_ms,
            max_size,
            entries: HashMap::new(),
        }
    }

    /// Drop every entry whose age (`now_ms - last_seen`) exceeds `ttl_ms`.
    /// No-op when `ttl_ms == 0`. Uses saturating arithmetic so a `now_ms`
    /// that runs backwards (clock skew) never underflows into a false
    /// "expired" sweep.
    fn evict_expired(&mut self, now_ms: u64) {
        if self.ttl_ms == 0 {
            return;
        }
        let ttl = self.ttl_ms;
        self.entries
            .retain(|_, e| now_ms.saturating_sub(e.first_seen) < ttl);
    }

    /// True if a live (non-expired) entry exists for `key`. With
    /// `ttl_ms == 0` any present entry is considered live.
    fn is_live(&self, key: &DedupeKey, now_ms: u64) -> bool {
        match self.entries.get(key) {
            None => false,
            Some(e) => self.ttl_ms == 0 || now_ms.saturating_sub(e.first_seen) < self.ttl_ms,
        }
    }

    /// Check-and-record. Returns `true` if `key` is NEW (never seen, or
    /// its prior entry has expired) and records it at `now_ms`. Returns
    /// `false` if `key` is a live duplicate — in which case the entry's
    /// recency is refreshed to `now_ms` (a re-seen key counts as recently
    /// used for LRU purposes).
    ///
    /// On every call: expired entries are swept first, then if recording a
    /// brand-new key would exceed `max_size` the least-recently-used entry
    /// is evicted before insertion.
    pub fn check(&mut self, key: DedupeKey, now_ms: u64) -> bool {
        self.evict_expired(now_ms);

        if self.is_live(&key, now_ms) {
            // Live duplicate: refresh LRU recency only (NOT the TTL anchor),
            // report not-new.
            if let Some(e) = self.entries.get_mut(&key) {
                e.last_seen = now_ms;
            }
            return false;
        }

        // New (or expired) key: make room under the size cap, then record.
        if self.max_size > 0 {
            // The key isn't currently a live entry; if a stale copy lingers
            // it will be overwritten by the insert below, so only evict when
            // adding a genuinely new key would push us over the cap.
            let adding_new = !self.entries.contains_key(&key);
            while adding_new && self.entries.len() >= self.max_size {
                if let Some(lru) = self.lru_key() {
                    self.entries.remove(&lru);
                } else {
                    break;
                }
            }
        }

        self.entries.insert(
            key,
            Entry {
                first_seen: now_ms,
                last_seen: now_ms,
            },
        );
        true
    }

    /// Non-mutating liveness probe. True if a live (non-expired) entry
    /// exists for `key` at `now_ms`. Does not record or refresh anything.
    pub fn peek(&self, key: &DedupeKey, now_ms: u64) -> bool {
        self.is_live(key, now_ms)
    }

    /// Find the least-recently-used key (smallest `last_seen`).
    fn lru_key(&self) -> Option<DedupeKey> {
        self.entries
            .iter()
            .min_by_key(|(_, e)| e.last_seen)
            .map(|(k, _)| k.clone())
    }

    /// Current number of tracked entries (post any lazy eviction from the
    /// last mutating call). Primarily for tests / introspection.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no entries are tracked.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str) -> DedupeKey {
        DedupeKey::new("slack", "acct", id)
    }

    #[test]
    fn new_key_is_new_and_recorded() {
        let mut c = DedupeCache::new(1000, 100);
        assert!(c.check(key("m1"), 0), "first sight of a key is new");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn immediate_duplicate_is_not_new() {
        let mut c = DedupeCache::new(1000, 100);
        assert!(c.check(key("m1"), 0));
        assert!(!c.check(key("m1"), 10), "live duplicate must be suppressed");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn key_equality_is_on_all_three_fields() {
        let mut c = DedupeCache::new(1000, 100);
        assert!(c.check(DedupeKey::new("slack", "a", "m1"), 0));
        // Different platform -> distinct key -> new.
        assert!(c.check(DedupeKey::new("discord", "a", "m1"), 0));
        // Different account -> distinct key -> new.
        assert!(c.check(DedupeKey::new("slack", "b", "m1"), 0));
        // Different message id -> distinct key -> new.
        assert!(c.check(DedupeKey::new("slack", "a", "m2"), 0));
        assert_eq!(c.len(), 4);
    }

    #[test]
    fn reappears_as_new_after_ttl_elapses() {
        let mut c = DedupeCache::new(1000, 100);
        assert!(c.check(key("m1"), 0));
        assert!(!c.check(key("m1"), 999), "still live just under ttl");
        // At exactly ttl the age == ttl, which is NOT < ttl -> expired.
        assert!(c.check(key("m1"), 1000), "expired at the ttl boundary -> new again");
    }

    #[test]
    fn ttl_zero_disables_expiry() {
        let mut c = DedupeCache::new(0, 100);
        assert!(c.check(key("m1"), 0));
        // Arbitrarily far in the future, still a duplicate.
        assert!(!c.check(key("m1"), u64::MAX), "ttl=0 means entries never age out");
    }

    #[test]
    fn lru_evicts_oldest_keeps_recently_touched() {
        let mut c = DedupeCache::new(0, 2);
        assert!(c.check(key("m1"), 1));
        assert!(c.check(key("m2"), 2));
        // Touch m1 so it becomes the most-recently-used.
        assert!(!c.check(key("m1"), 3));
        // Inserting m3 overflows cap (2); LRU is m2, which gets evicted.
        assert!(c.check(key("m3"), 4));
        assert_eq!(c.len(), 2);
        assert!(c.peek(&key("m1"), 5), "recently-touched m1 retained");
        assert!(c.peek(&key("m3"), 5), "newest m3 present");
        assert!(!c.peek(&key("m2"), 5), "LRU m2 evicted");
        // m2 now reads as new again (it was evicted).
        assert!(c.check(key("m2"), 6));
    }

    #[test]
    fn max_size_zero_disables_capping() {
        let mut c = DedupeCache::new(0, 0);
        for i in 0..1000 {
            assert!(c.check(key(&format!("m{i}")), i as u64));
        }
        assert_eq!(c.len(), 1000, "no cap -> all distinct keys retained");
    }

    #[test]
    fn peek_does_not_record() {
        let mut c = DedupeCache::new(1000, 100);
        assert!(!c.peek(&key("m1"), 0), "absent key is not live");
        assert_eq!(c.len(), 0, "peek must not insert");
        // Confirm a subsequent check still treats it as new.
        assert!(c.check(key("m1"), 0));
    }

    #[test]
    fn peek_respects_ttl() {
        let mut c = DedupeCache::new(1000, 100);
        c.check(key("m1"), 0);
        assert!(c.peek(&key("m1"), 500), "live within ttl");
        assert!(!c.peek(&key("m1"), 1000), "expired at ttl boundary");
    }

    #[test]
    fn duplicate_refreshes_recency_for_lru() {
        let mut c = DedupeCache::new(0, 2);
        c.check(key("a"), 1);
        c.check(key("b"), 2);
        // Re-seeing "a" refreshes it to now=3 (newer than b@2).
        assert!(!c.check(key("a"), 3));
        // Adding "c" evicts the true LRU, which is now "b".
        assert!(c.check(key("c"), 4));
        assert!(c.peek(&key("a"), 5));
        assert!(!c.peek(&key("b"), 5), "b was LRU after a's refresh");
    }
}
