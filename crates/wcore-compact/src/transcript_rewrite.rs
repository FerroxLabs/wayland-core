//! Transcript-rewrite primitive (T2-B2).
//!
//! Ports the *intent* of `rewriteTranscriptEntries` from openclaw's
//! `src/context-engine/types.ts`: a safe, additive helper that lets engines
//! request scrubbing / replacement passes over a transcript prior to
//! re-assembly. The TypeScript form is an async branch-and-reappend hook
//! owned by the runtime; this Rust port is the synchronous *primitive*
//! that operates on an in-memory `Vec<TranscriptEntry>` against a list of
//! regex-based [`RewriteRule`]s. The on-disk DAG update is left to the
//! caller (mirrors the openclaw split between engine-decides-what and
//! runtime-owns-how).
//!
//! Rollback: set `WAYLAND_TRANSCRIPT_REWRITE=off` to skip the rewrite
//! step at the primitive itself — the function returns the input vector
//! unchanged with `changes = 0` regardless of the rule list. The flag is
//! checked once on every call via `std::env::var`. The function is also
//! additive — old callers that don't invoke it pay no cost (and the
//! `rules.is_empty()` early return preserves a zero-copy fast path).

use regex::Regex;

use crate::semantic::ChunkRole;

/// Single transcript entry passed into the rewrite primitive.
///
/// Mirrors the role/content surface of `TranscriptRewriteReplacement`'s
/// referenced `AgentMessage` while staying provider-neutral. The numeric
/// `id` and `timestamp_ms` are preserved across rewrite so callers can
/// reconcile with their session DAG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    pub id: u64,
    pub role: ChunkRole,
    pub content: String,
    pub timestamp_ms: u64,
}

/// Rewrite directive applied to a transcript entry.
///
/// Rules are applied in order. `pattern.replace_all` is run against the
/// entry's content using `replacement` as the replacement string (regex
/// backrefs like `$1` work). If `role_filter` is `Some(role)`, the rule
/// only applies to entries whose role matches; `None` applies to all.
#[derive(Debug, Clone)]
pub struct RewriteRule {
    pub pattern: Regex,
    pub replacement: String,
    /// Only applies if entry.role matches; None = apply to all.
    pub role_filter: Option<ChunkRole>,
}

/// Outcome of a [`rewrite_transcript_entries`] call.
///
/// `changes` is the total number of regex matches replaced across all
/// entries and rules (not the number of entries touched).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewriteResult {
    pub rewritten: Vec<TranscriptEntry>,
    pub changes: u32,
}

/// Apply `rules` in order to every entry in `entries`.
///
/// Fast path: if `rules` is empty, return the input vector unchanged with
/// `changes = 0` (no allocation, no copy). Otherwise iterate over entries
/// and rules in order, counting matches via `find_iter().count()` *before*
/// running `replace_all` so the count is accurate even when the
/// replacement text would itself match the next rule's pattern.
pub fn rewrite_transcript_entries(
    entries: Vec<TranscriptEntry>,
    rules: &[RewriteRule],
) -> RewriteResult {
    // Rollback kill-switch (BATTLE-PLAN-v2 migration policy): operators
    // can disable the rewrite step in production without redeploying.
    if std::env::var("WAYLAND_TRANSCRIPT_REWRITE").as_deref() == Ok("off") {
        return RewriteResult {
            rewritten: entries,
            changes: 0,
        };
    }
    if rules.is_empty() {
        return RewriteResult {
            rewritten: entries,
            changes: 0,
        };
    }

    let mut changes: u32 = 0;
    let mut rewritten: Vec<TranscriptEntry> = Vec::with_capacity(entries.len());

    for mut entry in entries.into_iter() {
        for rule in rules {
            if let Some(filter) = rule.role_filter
                && filter != entry.role
            {
                continue;
            }
            let match_count = rule.pattern.find_iter(&entry.content).count();
            if match_count == 0 {
                continue;
            }
            changes = changes.saturating_add(match_count as u32);
            let replaced = rule
                .pattern
                .replace_all(&entry.content, rule.replacement.as_str())
                .into_owned();
            entry.content = replaced;
        }
        rewritten.push(entry);
    }

    RewriteResult { rewritten, changes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64, role: ChunkRole, content: &str) -> TranscriptEntry {
        TranscriptEntry {
            id,
            role,
            content: content.to_string(),
            timestamp_ms: 1_000 + id,
        }
    }

    fn rule(pattern: &str, replacement: &str, role_filter: Option<ChunkRole>) -> RewriteRule {
        RewriteRule {
            pattern: Regex::new(pattern).expect("test regex compiles"),
            replacement: replacement.to_string(),
            role_filter,
        }
    }

    #[test]
    fn rewrite_empty_rules_returns_unchanged_with_zero_changes() {
        let entries = vec![entry(1, ChunkRole::User, "hello world")];
        let result = rewrite_transcript_entries(entries.clone(), &[]);
        assert_eq!(result.changes, 0);
        assert_eq!(result.rewritten, entries);
    }

    #[test]
    fn rewrite_single_replacement_counted() {
        let entries = vec![entry(1, ChunkRole::User, "hello world")];
        let rules = vec![rule("world", "rust", None)];
        let result = rewrite_transcript_entries(entries, &rules);
        assert_eq!(result.changes, 1);
        assert_eq!(result.rewritten[0].content, "hello rust");
    }

    #[test]
    fn rewrite_no_match_zero_changes() {
        let entries = vec![entry(1, ChunkRole::User, "hello world")];
        let rules = vec![rule("zzz", "qqq", None)];
        let result = rewrite_transcript_entries(entries.clone(), &rules);
        assert_eq!(result.changes, 0);
        assert_eq!(result.rewritten, entries);
    }

    #[test]
    fn rewrite_multiple_matches_in_one_entry_all_counted() {
        let entries = vec![entry(1, ChunkRole::Tool, "foo foo foo bar foo")];
        let rules = vec![rule("foo", "X", None)];
        let result = rewrite_transcript_entries(entries, &rules);
        assert_eq!(result.changes, 4);
        assert_eq!(result.rewritten[0].content, "X X X bar X");
    }

    #[test]
    fn rewrite_role_filter_applies_only_to_matching_role() {
        let entries = vec![
            entry(1, ChunkRole::User, "secret hello"),
            entry(2, ChunkRole::Assistant, "secret world"),
        ];
        let rules = vec![rule("secret", "[REDACTED]", Some(ChunkRole::User))];
        let result = rewrite_transcript_entries(entries, &rules);
        assert_eq!(result.changes, 1);
        assert_eq!(result.rewritten[0].content, "[REDACTED] hello");
        // Assistant entry untouched
        assert_eq!(result.rewritten[1].content, "secret world");
    }

    #[test]
    fn rewrite_role_filter_none_applies_to_all() {
        let entries = vec![
            entry(1, ChunkRole::User, "secret hello"),
            entry(2, ChunkRole::Assistant, "secret world"),
            entry(3, ChunkRole::Tool, "secret tool"),
        ];
        let rules = vec![rule("secret", "[X]", None)];
        let result = rewrite_transcript_entries(entries, &rules);
        assert_eq!(result.changes, 3);
        assert_eq!(result.rewritten[0].content, "[X] hello");
        assert_eq!(result.rewritten[1].content, "[X] world");
        assert_eq!(result.rewritten[2].content, "[X] tool");
    }

    #[test]
    fn rewrite_two_rules_applied_in_order() {
        let entries = vec![entry(1, ChunkRole::User, "alpha beta")];
        // Rule 1: alpha -> middle ; Rule 2: middle -> omega
        // Should chain to "omega beta", counting 1 + 1 = 2 changes.
        let rules = vec![rule("alpha", "middle", None), rule("middle", "omega", None)];
        let result = rewrite_transcript_entries(entries, &rules);
        assert_eq!(result.changes, 2);
        assert_eq!(result.rewritten[0].content, "omega beta");
    }

    #[test]
    fn rewrite_preserves_id_and_timestamp() {
        let entries = vec![TranscriptEntry {
            id: 42,
            role: ChunkRole::Assistant,
            content: "hi name".to_string(),
            timestamp_ms: 9_999_999,
        }];
        let rules = vec![rule("name", "world", None)];
        let result = rewrite_transcript_entries(entries, &rules);
        assert_eq!(result.rewritten[0].id, 42);
        assert_eq!(result.rewritten[0].timestamp_ms, 9_999_999);
        assert_eq!(result.rewritten[0].role, ChunkRole::Assistant);
        assert_eq!(result.rewritten[0].content, "hi world");
    }

    #[test]
    fn rewrite_preserves_entry_count() {
        let entries = vec![
            entry(1, ChunkRole::User, "a"),
            entry(2, ChunkRole::Assistant, "b"),
            entry(3, ChunkRole::Tool, "c"),
            entry(4, ChunkRole::System, "d"),
        ];
        let rules = vec![rule("z", "Q", None)];
        let result = rewrite_transcript_entries(entries, &rules);
        assert_eq!(result.rewritten.len(), 4);
        assert_eq!(result.changes, 0);
    }

    #[test]
    fn rewrite_does_not_modify_input_vec_when_rules_empty() {
        // Verifies the no-copy early-return path: contents identical, length identical.
        let entries = vec![
            entry(1, ChunkRole::User, "x"),
            entry(2, ChunkRole::Assistant, "y"),
        ];
        let original = entries.clone();
        let result = rewrite_transcript_entries(entries, &[]);
        assert_eq!(result.changes, 0);
        assert_eq!(result.rewritten, original);
    }
}
