//! Per-provider-instance cache of which models accept a `tools` array.
//!
//! This is the shared decision surface (layers 1 + 3) of the #389 / #97
//! "tools-unsupported" capability system. Some backends — local Ollama /
//! llama.cpp models, and a handful of hosted ids — reject a request that
//! carries a `tools` array, 400-ing the whole turn instead of just ignoring
//! the field. To avoid that, the request-builder consults this cache before
//! attaching `body["tools"]` and drops the array when the model is positively
//! known to reject it.
//!
//! Three independent signals feed the cache, all writing through [`set`](ToolSupportCache::set):
//!
//! * **Name-gate** — the static prefix predicate (`model_supports_tool_calling`
//!   in `openai_compat`) gates tools for families we already know
//!   (e.g. Groq Compound).
//! * **Ollama probe** — a `/api/show` capability check ([`crate::ollama_probe`])
//!   records the model's true `tools` support before the first turn.
//! * **Reactive net** — when a turn 400s with a tools-unsupported error, the
//!   provider records `false` so the retry — and every later turn for that
//!   model — drops the array.
//!
//! [`allows`](ToolSupportCache::allows) is the gate the request-builder calls.
//! It is **optimistic**: tools are attached unless the cache positively holds
//! `Some(false)` for the model. An unknown model (`None`) still gets tools, so
//! the very first request can probe capability reactively rather than
//! pre-emptively stripping tools from a model that would have accepted them.
//!
//! The cache is keyed by the model id string and shared across clones via an
//! inner `Arc<Mutex<…>>`, mirroring the `Arc<Mutex<…>>` field pattern on
//! `OpenAIProvider` (`keys`, `pinned_base_url`): a clone of the provider — and
//! thus of the cache — observes writes made through any other handle.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Per-provider-instance record of model → "accepts a `tools` array".
///
/// Cheap to [`Clone`] (an `Arc` bump); all clones share one map, so a write
/// through any handle is visible through the others. See the module docs for
/// how the name-gate, Ollama probe, and reactive 400 net populate it and how
/// [`allows`](Self::allows) gates `body["tools"]`.
#[derive(Clone, Debug, Default)]
pub(crate) struct ToolSupportCache {
    /// `true`  → model is known to accept `tools`.
    /// `false` → model is known to reject `tools` (drop the array).
    /// absent  → unknown; treated optimistically as "accepts" by `allows`.
    inner: Arc<Mutex<HashMap<String, bool>>>,
}

impl ToolSupportCache {
    /// Construct an empty cache. Every model starts unknown, so [`allows`](Self::allows)
    /// returns `true` until a signal records otherwise.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Return the cached capability for `model`, or `None` when it has never
    /// been recorded.
    pub(crate) fn get(&self, model: &str) -> Option<bool> {
        // A poisoned lock means a writer panicked mid-update; the map itself is
        // still a valid `HashMap`, so recover the guard rather than propagate a
        // panic into a request path. Same recovery on every access below.
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        guard.get(model).copied()
    }

    /// Record (or overwrite) whether `model` accepts a `tools` array. Called by
    /// the name-gate seed, the Ollama probe, and the reactive 400 handler.
    pub(crate) fn set(&self, model: &str, supports_tools: bool) {
        // Recover from a poisoned lock instead of panicking (see `get`).
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        guard.insert(model.to_string(), supports_tools);
    }

    /// The request-builder gate: may we attach a `tools` array for `model`?
    ///
    /// Returns `false` ONLY when the cache positively holds `Some(false)`.
    /// Both `Some(true)` and unknown (`None`) models return `true` — the
    /// optimistic default that lets a first request probe capability
    /// reactively rather than stripping tools pre-emptively.
    pub(crate) fn allows(&self, model: &str) -> bool {
        self.get(model) != Some(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- defaults ---------------------------------------------------------

    #[test]
    fn unknown_model_is_allowed_and_uncached() {
        let cache = ToolSupportCache::new();
        // Optimistic default: never seen ⇒ attach tools, and nothing cached.
        assert!(cache.allows("never-seen-model"));
        assert_eq!(cache.get("never-seen-model"), None);
    }

    // --- set / get round-trip --------------------------------------------

    #[test]
    fn set_false_blocks_and_caches() {
        let cache = ToolSupportCache::new();
        cache.set("llama3-local", false);
        assert!(!cache.allows("llama3-local"));
        assert_eq!(cache.get("llama3-local"), Some(false));
    }

    #[test]
    fn set_true_allows_and_caches() {
        let cache = ToolSupportCache::new();
        cache.set("gpt-4o", true);
        assert!(cache.allows("gpt-4o"));
        assert_eq!(cache.get("gpt-4o"), Some(true));
    }

    // --- overwrite --------------------------------------------------------

    #[test]
    fn set_overwrites_previous_value() {
        let cache = ToolSupportCache::new();
        // A reactive 400 records `false`; a later probe corrects it to `true`.
        cache.set("flip-model", false);
        assert!(!cache.allows("flip-model"));
        cache.set("flip-model", true);
        assert!(cache.allows("flip-model"));
        assert_eq!(cache.get("flip-model"), Some(true));
    }

    // --- shared state across clones --------------------------------------

    #[test]
    fn clone_shares_state_with_original() {
        let original = ToolSupportCache::new();
        let clone = original.clone();
        // A write through the clone is visible through the original — proves the
        // inner Arc<Mutex<…>> is shared, not deep-copied.
        clone.set("shared-model", false);
        assert_eq!(original.get("shared-model"), Some(false));
        assert!(!original.allows("shared-model"));
    }
}
