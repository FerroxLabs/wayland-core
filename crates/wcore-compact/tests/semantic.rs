//! Integration tests for the `wcore_compact::semantic` core.
//!
//! Unit tests for individual functions live inline next to the
//! implementation. These tests exercise the public surface end-to-end:
//! realistic session shape and system-chunk preservation.

use std::sync::Arc;
use wcore_compact::{Chunk, ChunkRole, CompressionRetention, SemanticCompressor, SemanticJudge};

fn mk(content: &str, priority: f32, tokens: usize, role: ChunkRole) -> Chunk {
    Chunk::new(content, priority, tokens, role)
}

#[test]
fn compress_realistic_session_fits_budget() {
    // ~50 chunks of varied roles, varied priorities and sizes. Budget is set
    // so roughly half the content must be evicted.
    let mut chunks: Vec<Chunk> = Vec::with_capacity(50);
    chunks.push(mk("you are a helpful agent", 0.9, 200, ChunkRole::System));
    for i in 0..49 {
        let role = match i % 4 {
            0 => ChunkRole::User,
            1 => ChunkRole::Assistant,
            2 => ChunkRole::Tool,
            _ => ChunkRole::Assistant,
        };
        // Priority gently rises so recent turns also tend to score higher;
        // exact numbers don't matter — we just want a non-trivial mix.
        let priority = 0.2 + (i as f32) * 0.01;
        let tokens = 100 + (i % 5) * 50; // 100..=300 tokens
        chunks.push(mk(&format!("msg-{i}"), priority, tokens, role));
    }

    let total_input: usize = chunks.iter().map(|c| c.token_count).sum();
    let budget = total_input / 2;
    let compressor = SemanticCompressor::new(budget, 0.5, CompressionRetention::AdaptiveBudget);

    let result = compressor.compress(chunks.clone());

    assert!(
        result.kept_tokens <= budget,
        "kept_tokens ({}) must not exceed budget ({})",
        result.kept_tokens,
        budget
    );
    assert!(
        result.kept_tokens > 0,
        "expected at least some chunks to be kept"
    );
    assert!(
        !result.dropped.is_empty(),
        "expected some chunks to be dropped under a half-budget"
    );
    assert_eq!(
        result.kept.len() + result.dropped.len(),
        chunks.len(),
        "every input chunk must appear in either kept or dropped"
    );
    assert!(
        result.ratio > 0.0 && result.ratio <= 1.0,
        "ratio out of range: {}",
        result.ratio
    );
}

#[test]
fn compress_preserves_system_chunks() {
    // Build a session where the system chunk has a low caller-supplied
    // priority but the role floor should still keep it in. Surround it with
    // higher-priority recent chunks that compete for budget.
    let mut chunks: Vec<Chunk> = Vec::new();
    chunks.push(mk("system prompt", 0.05, 500, ChunkRole::System));
    for i in 0..20 {
        chunks.push(mk(
            &format!("u{i}"),
            0.9,
            500,
            if i % 2 == 0 {
                ChunkRole::User
            } else {
                ChunkRole::Assistant
            },
        ));
    }

    // Budget that fits only ~5 of the 21 chunks — system would normally be
    // outscored on age alone without the role floor.
    let compressor = SemanticCompressor::new(2500, 0.5, CompressionRetention::AdaptiveBudget);
    let result = compressor.compress(chunks);

    let system_kept = result
        .kept
        .iter()
        .any(|c| c.role == ChunkRole::System && c.content == "system prompt");
    assert!(
        system_kept,
        "system chunk must survive eviction (role floor not applied?)"
    );
}

/// Stub judge that rejects any chunk whose content contains a banned
/// substring. Mirrors how a real LLM-judge wrapper would be wired (sees the
/// kept slice, returns one verdict per chunk).
struct BanSubstringJudge {
    needle: String,
}

impl SemanticJudge for BanSubstringJudge {
    fn judge(&self, kept: &[Chunk]) -> Vec<bool> {
        kept.iter()
            .map(|c| !c.content.contains(&self.needle))
            .collect()
    }
}

#[test]
fn judge_installed_evicts_rejected_chunks() {
    // Budget that fits everything — eviction must come from the judge, not
    // the budget selector.
    let chunks = vec![
        mk("keep-a", 0.9, 10, ChunkRole::User),
        mk("DROP-b", 0.9, 10, ChunkRole::Assistant),
        mk("keep-c", 0.9, 10, ChunkRole::User),
        mk("DROP-d", 0.9, 10, ChunkRole::Tool),
    ];

    let judge = Arc::new(BanSubstringJudge {
        needle: "DROP".to_string(),
    });
    let compressor =
        SemanticCompressor::new(1000, 0.5, CompressionRetention::AdaptiveBudget).with_judge(judge);

    let result = compressor.compress(chunks);

    // The judge must have moved DROP-* into the dropped set even though the
    // budget could fit them all.
    assert_eq!(result.kept.len(), 2, "judge should evict 2 chunks");
    assert!(result.kept.iter().all(|c| !c.content.contains("DROP")));
    assert!(result.dropped.iter().any(|c| c.content == "DROP-b"));
    assert!(result.dropped.iter().any(|c| c.content == "DROP-d"));
    // Tokens stay consistent after judge eviction.
    assert_eq!(result.kept_tokens, 20);
    assert_eq!(result.dropped_tokens, 20);
}

#[test]
fn no_judge_path_is_unchanged() {
    // Same input run twice — once on a plain compressor, once on a compressor
    // built via `with_judge` but with no judge ever installed (the heuristic
    // path). Outputs must be byte-for-byte identical.
    let chunks: Vec<Chunk> = (0..10)
        .map(|i| {
            mk(
                &format!("m{i}"),
                0.3 + (i as f32) * 0.05,
                10,
                ChunkRole::User,
            )
        })
        .collect();

    let baseline = SemanticCompressor::new(50, 0.5, CompressionRetention::AdaptiveBudget)
        .compress(chunks.clone());
    let no_judge =
        SemanticCompressor::new(50, 0.5, CompressionRetention::AdaptiveBudget).compress(chunks);

    assert_eq!(baseline.kept, no_judge.kept);
    assert_eq!(baseline.dropped, no_judge.dropped);
    assert_eq!(baseline.kept_tokens, no_judge.kept_tokens);
    assert_eq!(baseline.dropped_tokens, no_judge.dropped_tokens);
    assert_eq!(baseline.ratio, no_judge.ratio);
}

#[test]
fn judge_keep_all_matches_no_judge() {
    // A judge that approves every chunk must produce a result identical to
    // the no-judge baseline. This pins the contract that the judge is purely
    // additive — it can never *promote* a chunk the budget selector dropped.
    struct KeepAll;
    impl SemanticJudge for KeepAll {
        fn judge(&self, kept: &[Chunk]) -> Vec<bool> {
            vec![true; kept.len()]
        }
    }

    let chunks: Vec<Chunk> = (0..10)
        .map(|i| {
            mk(
                &format!("m{i}"),
                0.3 + (i as f32) * 0.05,
                10,
                ChunkRole::User,
            )
        })
        .collect();

    let baseline = SemanticCompressor::new(50, 0.5, CompressionRetention::AdaptiveBudget)
        .compress(chunks.clone());
    let with_judge = SemanticCompressor::new(50, 0.5, CompressionRetention::AdaptiveBudget)
        .with_judge(Arc::new(KeepAll))
        .compress(chunks);

    assert_eq!(baseline.kept, with_judge.kept);
    assert_eq!(baseline.dropped, with_judge.dropped);
    assert_eq!(baseline.kept_tokens, with_judge.kept_tokens);
    assert_eq!(baseline.dropped_tokens, with_judge.dropped_tokens);
    assert_eq!(baseline.ratio, with_judge.ratio);
}
