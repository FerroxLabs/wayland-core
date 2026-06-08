//! Identifier-preservation policy for semantic compaction.
//!
//! Ports the spirit of `IdentifierPolicy` from openclaw `agents/compaction.ts`,
//! adapted to wayland-core's Rust chunk-priority compressor. The openclaw
//! original is a *prompt-level* instruction policy (off / strict / custom)
//! telling the summarizer LLM to preserve opaque identifiers (UUIDs, hashes,
//! IDs, tokens, hostnames, IPs, ports, URLs, file names). Here we implement
//! the same intent at the *chunk-selection* layer that runs *before* any LLM
//! call: scan chunk contents for important identifiers and boost the priority
//! of chunks that contain them, so the budget-bounded selector in
//! [`crate::semantic::SemanticCompressor`] is less likely to evict them.
//!
//! The policy is pure re-prioritisation — it **never** rewrites chunk content.
//!
//! Defaults match three high-signal patterns:
//!   * file paths (anything with at least one `/` separator),
//!   * qualified identifiers like `foo::bar` or `pkg::module::Item`,
//!   * SCREAMING_SNAKE_CASE error codes / constants.
//!
//! Per-match boost defaults to `0.1`, capped at `1.0` per chunk so a chunk
//! with hundreds of identifiers cannot starve the rest of the transcript.

use regex::Regex;

use crate::semantic::Chunk;

/// Re-prioritisation policy: scan chunk contents for important identifiers
/// and boost the chunk's `priority` so it survives semantic compaction.
#[derive(Debug, Clone)]
pub struct IdentifierPolicy {
    /// Regex patterns whose matches in chunk content should be preserved.
    patterns: Vec<Regex>,
    /// Score boost added per identifier-match found in the chunk.
    per_match_boost: f32,
    /// Cap on total boost per chunk (so a chunk with many identifiers
    /// doesn't dominate selection).
    max_boost: f32,
}

impl IdentifierPolicy {
    /// Construct a policy with caller-supplied patterns and default boosts
    /// (`per_match_boost = 0.1`, `max_boost = 1.0`).
    pub fn with_patterns(patterns: Vec<Regex>) -> Self {
        Self {
            patterns,
            per_match_boost: 0.1,
            max_boost: 1.0,
        }
    }

    /// Override the boost weights on an existing policy.
    pub fn with_boost(mut self, per_match: f32, max: f32) -> Self {
        self.per_match_boost = per_match;
        self.max_boost = max;
        self
    }

    /// Count the total number of identifier matches in `content` across all
    /// configured patterns. A single byte range may match multiple patterns;
    /// each pattern contributes independently (this matches the policy's
    /// "more signals → higher boost" intent and is the simplest stable
    /// behaviour).
    pub fn count_identifiers(&self, content: &str) -> usize {
        let mut total = 0usize;
        for pat in &self.patterns {
            total = total.saturating_add(pat.find_iter(content).count());
        }
        total
    }

    /// Compute the priority boost for a single chunk's content:
    /// `min(count * per_match_boost, max_boost)`.
    pub fn boost_for_content(&self, content: &str) -> f32 {
        let count = self.count_identifiers(content) as f32;
        (count * self.per_match_boost).min(self.max_boost)
    }

    /// Apply the policy in place: for each chunk, add the computed boost to
    /// its `priority`. Content is never modified.
    pub fn apply_to_chunks(&self, chunks: &mut [Chunk]) {
        for chunk in chunks.iter_mut() {
            let boost = self.boost_for_content(&chunk.content);
            chunk.priority += boost;
        }
    }
}

impl Default for IdentifierPolicy {
    fn default() -> Self {
        // unwrap: these are static, hand-written patterns that compile.
        let patterns = vec![
            // File paths: at least one `/` separator with conventional path chars.
            Regex::new(r"(?:[A-Za-z0-9_./-]*/)+[A-Za-z0-9_.-]+").unwrap(),
            // Qualified identifiers: `foo::bar`, `pkg::mod::Item`.
            Regex::new(r"\b[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+").unwrap(),
            // SCREAMING_SNAKE_CASE error codes / constants (3+ chars).
            Regex::new(r"\b[A-Z][A-Z0-9_]{2,}\b").unwrap(),
        ];
        Self {
            patterns,
            per_match_boost: 0.1,
            max_boost: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{Chunk, ChunkRole};

    #[test]
    fn default_policy_constructs_with_three_patterns() {
        let policy = IdentifierPolicy::default();
        assert_eq!(policy.patterns.len(), 3);
        assert!((policy.per_match_boost - 0.1).abs() < 1e-6);
        assert!((policy.max_boost - 1.0).abs() < 1e-6);
    }

    #[test]
    fn count_identifiers_counts_all_kinds() {
        let policy = IdentifierPolicy::default();
        // file path + qualified ident + ERROR_CODE → at least 3 matches.
        let content = "see src/lib.rs where foo::bar fails with E_FATAL_BUG";
        let count = policy.count_identifiers(content);
        assert!(
            count >= 3,
            "expected >=3 identifier matches, got {count} in {content:?}"
        );
    }

    #[test]
    fn count_identifiers_zero_on_empty_content() {
        let policy = IdentifierPolicy::default();
        assert_eq!(policy.count_identifiers(""), 0);
    }

    #[test]
    fn boost_for_content_caps_at_max_boost() {
        let policy = IdentifierPolicy::default();
        // Build content with ~100 SCREAMING_SNAKE matches; unbounded boost
        // would be 10.0 — must be capped at 1.0.
        let mut content = String::new();
        for i in 0..100 {
            content.push_str(&format!("ERR_CODE_{i:03} "));
        }
        let boost = policy.boost_for_content(&content);
        assert!(
            (boost - 1.0).abs() < 1e-6,
            "expected boost capped at max_boost=1.0, got {boost}"
        );
    }

    #[test]
    fn boost_for_content_zero_when_no_identifiers() {
        let policy = IdentifierPolicy::default();
        let boost = policy.boost_for_content("a quiet sentence with nothing special");
        assert!(
            boost.abs() < 1e-6,
            "expected zero boost on plain prose, got {boost}"
        );
    }

    #[test]
    fn apply_to_chunks_increases_priority_for_chunks_with_idents() {
        let policy = IdentifierPolicy::default();
        let mut chunks = vec![Chunk::new(
            "panic at crates/wcore-compact/src/lib.rs in Foo::bar",
            0.2,
            10,
            ChunkRole::Tool,
        )];
        let before = chunks[0].priority;
        policy.apply_to_chunks(&mut chunks);
        assert!(
            chunks[0].priority > before,
            "expected priority boost from {before} but got {}",
            chunks[0].priority
        );
    }

    #[test]
    fn apply_to_chunks_leaves_chunks_without_idents_unchanged() {
        let policy = IdentifierPolicy::default();
        let mut chunks = vec![Chunk::new("hello world", 0.3, 2, ChunkRole::User)];
        let before = chunks[0].priority;
        policy.apply_to_chunks(&mut chunks);
        assert!(
            (chunks[0].priority - before).abs() < 1e-6,
            "expected priority unchanged on plain prose, got {} -> {}",
            before,
            chunks[0].priority
        );
    }

    #[test]
    fn apply_to_chunks_does_not_modify_content() {
        let policy = IdentifierPolicy::default();
        let original = "see src/lib.rs and Foo::bar and E_BAD".to_string();
        let mut chunks = vec![Chunk::new(original.clone(), 0.0, 5, ChunkRole::Assistant)];
        policy.apply_to_chunks(&mut chunks);
        assert_eq!(
            chunks[0].content, original,
            "policy must never rewrite chunk content"
        );
    }

    #[test]
    fn with_boost_overrides_defaults() {
        let policy = IdentifierPolicy::default().with_boost(0.5, 2.0);
        assert!((policy.per_match_boost - 0.5).abs() < 1e-6);
        assert!((policy.max_boost - 2.0).abs() < 1e-6);
        // Two matches × 0.5 = 1.0, well under cap.
        let boost = policy.boost_for_content("Foo::bar and Baz::qux");
        assert!(
            (boost - 1.0).abs() < 1e-6,
            "expected 2*0.5=1.0 boost, got {boost}"
        );
    }

    #[test]
    fn with_patterns_replaces_default_patterns() {
        // Only match lowercase-x tokens; nothing in default set hits this.
        let pat = Regex::new(r"\bxxx\b").unwrap();
        let policy = IdentifierPolicy::with_patterns(vec![pat]);
        assert_eq!(policy.count_identifiers("Foo::bar src/lib.rs E_BAD"), 0);
        assert_eq!(policy.count_identifiers("the xxx token"), 1);
    }
}
