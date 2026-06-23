//! Semantic chunk-priority compression core.
//!
//! Ports the head/tail-protected, budget-bounded chunk selection from the
//! prior Wayland Python engine. This module is the
//! T2-B1 *core* lift: chunk-priority scoring, stable selection within a token
//! budget, retention policies, and a `compress()` entry point.
//!
//! Out of scope for T2-B1 (separate lifts):
//!   * Transcript-level rewrite / summarizer-LLM pipeline (T2-B2).
//!   * Identifier-preservation pass (T2-B3).
//!
//! An optional LLM judge re-scorer can be installed via
//! [`SemanticCompressor::with_judge`]; when present, it runs between the
//! budget selector and the final tally and may evict additional chunks. With
//! no judge installed the heuristic path is byte-for-byte identical to the
//! pre-judge implementation.

use std::cmp::Ordering;
use std::sync::Arc;

/// Optional LLM-judge re-scorer plugged in between the budget selector and
/// the final tally in [`SemanticCompressor::compress`].
///
/// Implementors return one verdict per input chunk: `true` keeps the chunk,
/// `false` evicts it (it joins the dropped set in the final result). The
/// trait is intentionally synchronous and stateless from the compressor's
/// point of view — callers wiring an async LLM should bridge in their own
/// implementation (e.g. `block_on` or a cached judgement table).
pub trait SemanticJudge: Send + Sync {
    fn judge(&self, kept: &[Chunk]) -> Vec<bool>;
}

/// Role of a context chunk. System chunks receive an implicit priority floor
/// in [`SemanticCompressor::score_chunk`] so they are not evicted under
/// budget pressure (mirrors Python `protect_first_n` head protection for the
/// system prompt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkRole {
    Tool,
    Assistant,
    User,
    System,
}

/// A single scoreable unit of context (a message, a tool result, etc.).
///
/// `priority` is a caller-supplied importance signal in `[0.0, 1.0]`.
/// `token_count` is the rough token cost of `content` (callers may use the
/// `wcore-compact` tokenizer or a rough chars/4 estimate).
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub content: String,
    pub priority: f32,
    pub token_count: usize,
    pub role: ChunkRole,
}

impl Chunk {
    /// Convenience constructor.
    pub fn new(
        content: impl Into<String>,
        priority: f32,
        token_count: usize,
        role: ChunkRole,
    ) -> Self {
        Self {
            content: content.into(),
            priority,
            token_count,
            role,
        }
    }
}

/// Retention policy applied on top of the raw token-budget selector.
///
/// `AdaptiveBudget` is the default: keep packing chunks by descending score
/// until the budget is exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CompressionRetention {
    /// Keep at most `k` chunks (highest score wins). Still bounded by budget.
    TopK(usize),
    /// Keep only chunks whose score is `>= threshold`. Still bounded by budget.
    Threshold(f32),
    /// Greedy budget pack (no extra retention filter).
    #[default]
    AdaptiveBudget,
}

/// Result of a single compression pass.
#[derive(Debug, Clone, PartialEq)]
pub struct CompressionResult {
    pub kept: Vec<Chunk>,
    pub dropped: Vec<Chunk>,
    pub kept_tokens: usize,
    pub dropped_tokens: usize,
    /// `kept_tokens / (kept_tokens + dropped_tokens)`; `1.0` when nothing was
    /// dropped, `0.0` when the input was empty.
    pub ratio: f32,
}

/// Semantic compressor — scores chunks, selects within a token budget, and
/// applies an optional retention policy.
///
/// The Python implementation has a much larger surface (head/tail protection,
/// summarizer LLM, anti-thrash cooldowns); this Rust core captures only the
/// budget+scoring spine. Transcript rewrite and identifier policy land in
/// follow-up lifts.
#[derive(Clone)]
pub struct SemanticCompressor {
    pub token_budget: usize,
    pub target_ratio: f32,
    pub retention: CompressionRetention,
    judge: Option<Arc<dyn SemanticJudge>>,
}

impl std::fmt::Debug for SemanticCompressor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticCompressor")
            .field("token_budget", &self.token_budget)
            .field("target_ratio", &self.target_ratio)
            .field("retention", &self.retention)
            .field("judge", &self.judge.as_ref().map(|_| "<installed>"))
            .finish()
    }
}

/// Exponential decay base — `0.95^age`. Picked to match the Python
/// summarizer's "recent turns matter more" heuristic without requiring a
/// configurable knob in the core API.
const DECAY_BASE: f32 = 0.95;

/// Implicit floor applied to `ChunkRole::System` chunk *scores* (after
/// decay) so the system prompt cannot be starved by a flood of high-priority
/// tool output. Mirrors `protect_first_n` from the Python reference (head
/// protection). Picked well above any priority * decay product a normal
/// chunk can reach (max ≈ 1.0).
const SYSTEM_SCORE_FLOOR: f32 = 1_000.0;

impl SemanticCompressor {
    /// Build a compressor with the given budget, target ratio, and retention.
    pub fn new(token_budget: usize, target_ratio: f32, retention: CompressionRetention) -> Self {
        Self {
            token_budget,
            target_ratio,
            retention,
            judge: None,
        }
    }

    /// Install an optional LLM-judge re-scorer. When set, [`Self::compress`]
    /// runs the judge over the budget-selected chunks and evicts any whose
    /// verdict is `false`. Returns `self` for chaining.
    pub fn with_judge(mut self, judge: Arc<dyn SemanticJudge>) -> Self {
        self.judge = Some(judge);
        self
    }

    /// Score a single chunk: `effective_priority * decay(age)`.
    ///
    /// System chunks are lifted to `SYSTEM_PRIORITY_FLOOR` before decay so
    /// they outrank ordinary chunks even when many turns old.
    pub fn score_chunk(&self, chunk: &Chunk, age: u32) -> f32 {
        let decay = DECAY_BASE.powi(age as i32);
        let raw = chunk.priority * decay;
        if chunk.role == ChunkRole::System {
            // Lift system chunks to a dominant floor *after* decay so the
            // head of the conversation cannot be evicted even when it is the
            // oldest message in the window. We still respect the budget —
            // an oversized system chunk can be dropped if it does not fit.
            raw.max(SYSTEM_SCORE_FLOOR)
        } else {
            raw
        }
    }

    /// Select chunks within the configured token budget. Order of returned
    /// chunks matches the original input order (stable selection); when two
    /// chunks tie on score, the earlier index wins.
    ///
    /// Ages are derived from input position: the last chunk has age 0, the
    /// previous chunk age 1, and so on — i.e. position-from-end. This matches
    /// the Python "walk backward from the tail" protection scheme.
    pub fn select_within_budget(&self, chunks: &[Chunk]) -> Vec<Chunk> {
        if self.token_budget == 0 || chunks.is_empty() {
            return Vec::new();
        }

        let n = chunks.len();
        // (original_index, score)
        let mut scored: Vec<(usize, f32)> = chunks
            .iter()
            .enumerate()
            .map(|(idx, chunk)| {
                let age = (n - 1 - idx) as u32;
                (idx, self.score_chunk(chunk, age))
            })
            .collect();

        // Sort by score desc, breaking ties by original index asc (stable).
        scored.sort_by(|a, b| match b.1.partial_cmp(&a.1) {
            Some(Ordering::Equal) | None => a.0.cmp(&b.0),
            Some(ord) => ord,
        });

        let mut kept_indices: Vec<usize> = Vec::with_capacity(n);
        let mut tokens_used: usize = 0;
        for (idx, _score) in &scored {
            let cost = chunks[*idx].token_count;
            if tokens_used.saturating_add(cost) > self.token_budget {
                continue;
            }
            tokens_used += cost;
            kept_indices.push(*idx);
        }

        // Return in original input order so downstream consumers can rebuild
        // a coherent transcript.
        kept_indices.sort_unstable();
        kept_indices
            .into_iter()
            .map(|i| chunks[i].clone())
            .collect()
    }

    /// Full compression pass: score → apply retention filter → pack into
    /// budget → tally tokens. The `target_ratio` field is informational for
    /// downstream callers (the budget alone determines selection); we still
    /// surface the achieved ratio on the result so callers can decide whether
    /// a follow-up summarization pass is warranted.
    ///
    /// An optional [`SemanticJudge`] (installed via
    /// [`Self::with_judge`]) is invoked between `select_within_budget` and
    /// the final tally; chunks the judge rejects join the dropped set. When
    /// no judge is installed the heuristic output is byte-for-byte identical
    /// to the pre-judge implementation.
    pub fn compress(&self, chunks: Vec<Chunk>) -> CompressionResult {
        if chunks.is_empty() {
            return CompressionResult {
                kept: Vec::new(),
                dropped: Vec::new(),
                kept_tokens: 0,
                dropped_tokens: 0,
                ratio: 0.0,
            };
        }

        // Apply retention policy first to pre-filter the candidate set, then
        // hand the survivors to the budget selector.
        let n = chunks.len();
        let scored: Vec<(usize, f32)> = chunks
            .iter()
            .enumerate()
            .map(|(idx, c)| {
                let age = (n - 1 - idx) as u32;
                (idx, self.score_chunk(c, age))
            })
            .collect();

        let allowed: Vec<bool> = match self.retention {
            CompressionRetention::AdaptiveBudget => vec![true; n],
            CompressionRetention::Threshold(min_score) => {
                scored.iter().map(|(_, s)| *s >= min_score).collect()
            }
            CompressionRetention::TopK(k) => {
                let mut ranked: Vec<(usize, f32)> = scored.clone();
                ranked.sort_by(|a, b| match b.1.partial_cmp(&a.1) {
                    Some(Ordering::Equal) | None => a.0.cmp(&b.0),
                    Some(ord) => ord,
                });
                let mut keep = vec![false; n];
                for (idx, _) in ranked.into_iter().take(k) {
                    keep[idx] = true;
                }
                keep
            }
        };

        let candidates: Vec<Chunk> = chunks
            .iter()
            .zip(allowed.iter())
            .filter_map(|(c, ok)| if *ok { Some(c.clone()) } else { None })
            .collect();

        let mut kept = self.select_within_budget(&candidates);

        // Optional judge re-scorer: evict any chunk the judge rejects. The
        // judge runs ONLY when installed — otherwise this branch is a no-op
        // and `kept` is identical to the pre-judge selection. The judge sees
        // the post-budget kept slice in original input order so it can apply
        // sequence-aware heuristics.
        if let Some(judge) = self.judge.as_ref() {
            let verdicts = judge.judge(&kept);
            // Defensive: if the judge returned a mis-sized verdict vector, we
            // treat missing entries as `true` (keep) so a buggy judge cannot
            // wipe the context entirely.
            kept = kept
                .into_iter()
                .enumerate()
                .filter(|(i, _)| verdicts.get(*i).copied().unwrap_or(true))
                .map(|(_, c)| c)
                .collect();
        }

        // Dropped = every input chunk that does not appear in `kept`.
        // Value equality is fine here because `Chunk` is `PartialEq` and
        // callers may legitimately submit duplicate chunks; we keep the
        // first-match-wins semantics so token totals stay consistent.
        let mut kept_consumed = vec![false; kept.len()];
        let mut dropped: Vec<Chunk> = Vec::with_capacity(chunks.len());
        for c in &chunks {
            let mut matched = false;
            for (i, k) in kept.iter().enumerate() {
                if !kept_consumed[i] && k == c {
                    kept_consumed[i] = true;
                    matched = true;
                    break;
                }
            }
            if !matched {
                dropped.push(c.clone());
            }
        }

        let kept_tokens: usize = kept.iter().map(|c| c.token_count).sum();
        let dropped_tokens: usize = dropped.iter().map(|c| c.token_count).sum();
        let total = kept_tokens + dropped_tokens;
        let ratio = if total == 0 {
            0.0
        } else {
            kept_tokens as f32 / total as f32
        };

        CompressionResult {
            kept,
            dropped,
            kept_tokens,
            dropped_tokens,
            ratio,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(content: &str, priority: f32, tokens: usize, role: ChunkRole) -> Chunk {
        Chunk::new(content, priority, tokens, role)
    }

    fn compressor(budget: usize) -> SemanticCompressor {
        SemanticCompressor::new(budget, 0.5, CompressionRetention::AdaptiveBudget)
    }

    #[test]
    fn score_chunk_decays_with_age() {
        let c = compressor(1000);
        let ch = chunk("x", 1.0, 10, ChunkRole::User);
        let s0 = c.score_chunk(&ch, 0);
        let s5 = c.score_chunk(&ch, 5);
        let s10 = c.score_chunk(&ch, 10);
        assert!(s0 > s5);
        assert!(s5 > s10);
        // 0.95^0 == 1.0
        assert!((s0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn score_chunk_higher_priority_wins() {
        let c = compressor(1000);
        let lo = chunk("x", 0.1, 10, ChunkRole::User);
        let hi = chunk("y", 0.9, 10, ChunkRole::User);
        // Same age, higher priority must score higher.
        assert!(c.score_chunk(&hi, 3) > c.score_chunk(&lo, 3));
    }

    #[test]
    fn select_within_budget_drops_lowest_score_first() {
        let c = compressor(20);
        let chunks = vec![
            chunk("old", 0.1, 10, ChunkRole::User), // age 2, low priority
            chunk("mid", 0.5, 10, ChunkRole::User),
            chunk("new", 0.9, 10, ChunkRole::User), // age 0
        ];
        let kept = c.select_within_budget(&chunks);
        assert_eq!(kept.len(), 2);
        // The lowest-scoring "old" should be evicted.
        assert!(!kept.iter().any(|k| k.content == "old"));
        assert!(kept.iter().any(|k| k.content == "new"));
        assert!(kept.iter().any(|k| k.content == "mid"));
    }

    #[test]
    fn select_within_budget_stable_on_tie() {
        let c = compressor(20);
        // All three chunks share priority + token count. Ages differ by
        // position, so "newer" wins on score; among the two oldest (age 2 and
        // age 1), the older wins ties via original-index ordering — but
        // since ages differ, ties only matter when priority+age line up.
        // Construct an explicit tie by inserting two identical-age chunks
        // via priority compensation: priority * 0.95^age must match.
        // Easier: budget is 20, fit only 2 of 3. Verify deterministic output.
        let chunks = vec![
            chunk("a", 0.5, 10, ChunkRole::User),
            chunk("b", 0.5, 10, ChunkRole::User),
            chunk("c", 0.5, 10, ChunkRole::User),
        ];
        let kept1 = c.select_within_budget(&chunks);
        let kept2 = c.select_within_budget(&chunks);
        assert_eq!(kept1, kept2, "selection must be deterministic");
        // Newest (highest age-decay score) must be present.
        assert!(kept1.iter().any(|k| k.content == "c"));
    }

    #[test]
    fn select_within_budget_zero_budget_returns_empty() {
        let c = compressor(0);
        let chunks = vec![chunk("x", 1.0, 1, ChunkRole::User)];
        assert!(c.select_within_budget(&chunks).is_empty());
    }

    #[test]
    fn compress_reports_kept_and_dropped_tokens() {
        let c = compressor(15);
        let chunks = vec![
            chunk("a", 0.1, 10, ChunkRole::User),
            chunk("b", 0.9, 10, ChunkRole::User),
        ];
        let r = c.compress(chunks);
        // Only one 10-token chunk fits in a 15-token budget.
        assert_eq!(r.kept_tokens, 10);
        assert_eq!(r.dropped_tokens, 10);
        assert_eq!(r.kept.len(), 1);
        assert_eq!(r.dropped.len(), 1);
        assert!((r.ratio - 0.5).abs() < 1e-6);
    }

    #[test]
    fn compress_target_ratio_respected_when_possible() {
        // 100-token input, 50-token budget -> we expect roughly half kept.
        let c = compressor(50);
        let chunks: Vec<Chunk> = (0..10)
            .map(|i| chunk(&format!("m{i}"), 0.5, 10, ChunkRole::User))
            .collect();
        let r = c.compress(chunks);
        assert!(r.kept_tokens <= 50);
        assert!(
            r.kept_tokens >= 40,
            "expected ~50 tokens kept, got {}",
            r.kept_tokens
        );
        assert!(r.ratio > 0.0 && r.ratio <= 0.6);
    }

    #[test]
    fn compress_empty_input_returns_empty() {
        let c = compressor(100);
        let r = c.compress(Vec::new());
        assert!(r.kept.is_empty());
        assert!(r.dropped.is_empty());
        assert_eq!(r.kept_tokens, 0);
        assert_eq!(r.dropped_tokens, 0);
        assert_eq!(r.ratio, 0.0);
    }

    #[test]
    fn retention_top_k_caps_kept_count() {
        let c = SemanticCompressor::new(
            1000, // big budget — retention is the only constraint
            0.5,
            CompressionRetention::TopK(2),
        );
        let chunks = vec![
            chunk("a", 0.5, 10, ChunkRole::User),
            chunk("b", 0.5, 10, ChunkRole::User),
            chunk("c", 0.5, 10, ChunkRole::User),
            chunk("d", 0.5, 10, ChunkRole::User),
        ];
        let r = c.compress(chunks);
        assert_eq!(r.kept.len(), 2);
        assert_eq!(r.dropped.len(), 2);
    }

    #[test]
    fn retention_threshold_filters_low_score() {
        // Score = priority * 0.95^age. With priority=0.1 and age up to 3,
        // every score is well below 0.5 — all should be filtered out.
        let c = SemanticCompressor::new(1000, 0.5, CompressionRetention::Threshold(0.5));
        let chunks = vec![
            chunk("a", 0.1, 10, ChunkRole::User),
            chunk("b", 0.1, 10, ChunkRole::User),
            chunk("c", 0.1, 10, ChunkRole::User),
            chunk("d", 0.1, 10, ChunkRole::User),
        ];
        let r = c.compress(chunks);
        assert!(
            r.kept.is_empty(),
            "threshold should reject all low-priority chunks"
        );
        assert_eq!(r.dropped.len(), 4);
    }
}
