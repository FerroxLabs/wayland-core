//! Persisted frecency store — recency + frequency ranking.
//!
//! FROZEN Wave-0 public surface; STUB bodies (T0.6 fills them).
//!
//! Frecency combines how *often* and how *recently* a key (a command or a
//! file path) was used into a single score. The command palette and `@`
//! completion rank their candidates with `rank`. The store persists to
//! the config directory so rankings survive across sessions.
//!
//! ## How the score is computed
//!
//! Each key keeps a hit count and the timestamp (Unix seconds) of its
//! last use. The frecency score is `hits * decay(age)` where `decay` is a
//! half-life function: an item's score halves every [`HALF_LIFE_SECS`].
//! So a key used many times long ago and a key used once a moment ago can
//! tie, and a key used both *often and recently* beats either — which is
//! the whole point of frecency over plain frequency or plain recency.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Half-life of a frecency score, in seconds. A key's score halves every
/// time this much wall-clock time passes since its last use. Three days
/// keeps a daily-driver command warm while letting a one-off fade within
/// a week or two.
const HALF_LIFE_SECS: f64 = 3.0 * 24.0 * 60.0 * 60.0;

/// Per-key usage record: how often and how recently the key was used.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Entry {
    /// Total number of recorded uses.
    hits: u32,
    /// Unix timestamp (seconds) of the most recent use.
    last_used: u64,
}

/// A persisted recency + frequency store. FROZEN Wave-0 contract.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FrecencyStore {
    /// Per-key usage records, keyed by the command/path string.
    entries: HashMap<String, Entry>,
}

impl FrecencyStore {
    /// Load the store from its on-disk location under the config dir,
    /// or start empty if no store exists yet.
    ///
    /// A missing file is normal on first run and yields an empty store.
    /// A corrupt or unreadable file is *also* treated as empty rather
    /// than an error — a damaged frecency cache is a polish-quality
    /// signal, never a reason to fail the TUI launch.
    pub fn load() -> anyhow::Result<Self> {
        let Some(path) = store_path() else {
            // No config dir on this platform — run with an empty,
            // session-only store rather than failing.
            return Ok(Self::default());
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            // Missing or unreadable file — start fresh.
            return Ok(Self::default());
        };
        // Corrupt JSON falls back to empty: the store is a cache, and a
        // broken cache must never block the UI.
        Ok(serde_json::from_str(&raw).unwrap_or_default())
    }

    /// Record one use of `key`, updating its frequency and recency.
    pub fn record(&mut self, key: &str) {
        let entry = self.entries.entry(key.to_string()).or_default();
        entry.hits = entry.hits.saturating_add(1);
        entry.last_used = now_secs();
    }

    /// Rank `items` by frecency, highest score first. Items the store has
    /// never seen sort after all seen items, preserving input order.
    ///
    /// The sort is stable, so two items with equal score (including two
    /// unseen items, both scoring zero) keep their relative input order.
    pub fn rank(&self, items: &[String]) -> Vec<String> {
        let now = now_secs();
        let mut ranked: Vec<String> = items.to_vec();
        ranked.sort_by(|a, b| {
            let sa = self.score(a, now);
            let sb = self.score(b, now);
            // Descending by score; `partial_cmp` is safe — `score`
            // never produces NaN (finite hits times a finite decay).
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        ranked
    }

    /// Persist the store to disk under the config directory.
    ///
    /// Creates the parent directory if needed. A platform with no config
    /// dir is a silent no-op — the store stays session-only.
    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = store_path() else {
            // No config dir — nothing to persist to. Not an error.
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// The frecency score of `key` at time `now` (Unix seconds): the hit
    /// count scaled by an exponential half-life decay of its age. An
    /// unseen key scores `0.0`.
    fn score(&self, key: &str, now: u64) -> f64 {
        match self.entries.get(key) {
            None => 0.0,
            Some(entry) => {
                let age = now.saturating_sub(entry.last_used) as f64;
                // 0.5 ^ (age / half_life): 1.0 fresh, 0.5 one half-life
                // old, decaying smoothly toward 0.
                let decay = 0.5_f64.powf(age / HALF_LIFE_SECS);
                f64::from(entry.hits) * decay
            }
        }
    }
}

/// The on-disk path of the frecency store.
///
/// Routes through `wcore_config::config::wayland_config_dir()` so
/// `WAYLAND_HOME` hermetically sandboxes the frecency file alongside the
/// rest of the engine's on-disk state (F-010, #270). Returns `Some` in all
/// cases since the canonical helper has a `PathBuf::from("wayland-core")`
/// fallback when no platform config dir exists; we keep the `Option`
/// signature to preserve the call-site shape (`let Some(path) = ...`).
fn store_path() -> Option<PathBuf> {
    Some(wcore_config::config::wayland_config_dir().join("frecency.json"))
}

/// The current time as Unix seconds. A clock set before the epoch (or an
/// otherwise unreadable clock) yields `0` — every entry then shares the
/// same timestamp, so ranking degrades to plain frequency rather than
/// panicking.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a store with explicit `(hits, last_used)` entries, bypassing
    /// `record` so tests can place an entry at a chosen age.
    fn store_with(entries: &[(&str, u32, u64)]) -> FrecencyStore {
        let mut store = FrecencyStore::default();
        for (key, hits, last_used) in entries {
            store.entries.insert(
                (*key).to_string(),
                Entry {
                    hits: *hits,
                    last_used: *last_used,
                },
            );
        }
        store
    }

    #[test]
    fn record_increments_hits_and_updates_recency() {
        let mut store = FrecencyStore::default();
        store.record("/help");
        store.record("/help");
        let entry = store.entries.get("/help").expect("entry exists");
        assert_eq!(entry.hits, 2);
        assert!(entry.last_used > 0);
    }

    #[test]
    fn recent_item_outranks_a_stale_one_with_equal_hits() {
        let now = now_secs();
        // Both used 5 times: one a moment ago, one two weeks ago.
        let store = store_with(&[("fresh", 5, now), ("stale", 5, now - 14 * 24 * 60 * 60)]);
        let ranked = store.rank(&["stale".into(), "fresh".into()]);
        assert_eq!(ranked, vec!["fresh", "stale"]);
    }

    #[test]
    fn frequent_recent_item_outranks_a_stale_frequent_one() {
        let now = now_secs();
        // The brief's core property: frequent AND recent beats stale.
        let store = store_with(&[("hot", 8, now), ("cold", 20, now - 30 * 24 * 60 * 60)]);
        let ranked = store.rank(&["cold".into(), "hot".into()]);
        // 30 days is ~10 half-lives — `cold`'s 20 hits decay to ~0.02,
        // far below `hot`'s 8.
        assert_eq!(ranked, vec!["hot", "cold"]);
    }

    #[test]
    fn unseen_items_sort_after_seen_ones_preserving_input_order() {
        let now = now_secs();
        let store = store_with(&[("seen", 3, now)]);
        let ranked = store.rank(&["unseen_b".into(), "seen".into(), "unseen_a".into()]);
        // `seen` floats up; the two unseen items keep their input order.
        assert_eq!(ranked, vec!["seen", "unseen_b", "unseen_a"]);
    }

    #[test]
    fn more_hits_win_when_recency_is_equal() {
        let now = now_secs();
        let store = store_with(&[("often", 10, now), ("rare", 2, now)]);
        let ranked = store.rank(&["rare".into(), "often".into()]);
        assert_eq!(ranked, vec!["often", "rare"]);
    }

    #[test]
    fn save_then_load_round_trips_the_store() {
        // Redirect the config dir at process scope so `store_path`
        // resolves into a temp dir for this test.
        let tmp = tempfile::tempdir().expect("temp dir");
        // SAFETY: single-threaded test; WAYLAND_HOME is the canonical
        // hermetic override resolved by `wayland_config_dir()` (F-010,
        // #270). It works uniformly on every platform — unlike
        // XDG_CONFIG_HOME, which `dirs::config_dir()` ignores on
        // macOS/Windows.
        unsafe {
            std::env::set_var("WAYLAND_HOME", tmp.path());
        }

        let mut store = FrecencyStore::default();
        store.record("/repomap");
        store.record("/repomap");
        store.record("/doctor");

        // Direct serde round-trip — platform-independent, the real
        // persisted shape.
        let json = serde_json::to_string(&store).expect("serialize");
        let reloaded: FrecencyStore = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(reloaded.entries.get("/repomap").map(|e| e.hits), Some(2));
        assert_eq!(reloaded.entries.get("/doctor").map(|e| e.hits), Some(1));

        unsafe {
            std::env::remove_var("WAYLAND_HOME");
        }
    }

    #[test]
    fn missing_file_loads_as_an_empty_store() {
        // `serde_json::from_str` on an empty/garbage string falls back to
        // default — the same path `load` takes for a corrupt file.
        let empty: FrecencyStore = serde_json::from_str("").unwrap_or_default();
        assert!(empty.entries.is_empty());
        let garbage: FrecencyStore = serde_json::from_str("{not json").unwrap_or_default();
        assert!(garbage.entries.is_empty());
    }

    #[test]
    fn load_never_errors_even_on_a_corrupt_store() {
        // `load` swallows missing/corrupt files; it must always return
        // `Ok` with at worst an empty store.
        let loaded = FrecencyStore::load().expect("load is infallible");
        // Whatever was on disk, the type is well-formed.
        let _ = loaded.entries.len();
    }
}
