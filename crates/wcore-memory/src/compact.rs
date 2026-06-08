// M6 — Letta conversation-window compaction.
//
// Non-destructive: oldest P1 turns are summarised into one or more P2
// episodes (source_product="wcore-compact") and replaced in P1 by a
// Bookmark entry pointing at the absorbed episode.
//
// SCOPE GUARD (audit F1): this is internal to wcore-memory. It does NOT
// touch the peer `wcore-compact` crate (tool-output sanitization).

use async_trait::async_trait;

use crate::error::Result;
use crate::partition::PartitionDispatcher;
use crate::partition::working::WorkingEntry;
use crate::v2_types::{CompactReport, Episode, EpisodeId, EpisodeStatus, Tier};

#[async_trait]
pub trait Summarizer: Send + Sync {
    async fn summarize(&self, prompt: &str, turns: &[WorkingEntry]) -> Result<String>;
}

pub struct MockSummarizer {
    pub fixed_output: String,
}

#[async_trait]
impl Summarizer for MockSummarizer {
    async fn summarize(&self, _prompt: &str, _turns: &[WorkingEntry]) -> Result<String> {
        Ok(self.fixed_output.clone())
    }
}

/// Production placeholder. Real router wiring is W6/W7 scope.
pub struct PlaceholderSummarizer;

#[async_trait]
impl Summarizer for PlaceholderSummarizer {
    async fn summarize(&self, _prompt: &str, turns: &[WorkingEntry]) -> Result<String> {
        // Produces a deterministic 1-line summary of the offloaded turns.
        let mut s = String::from("[Letta compaction: ");
        for (i, t) in turns.iter().take(8).enumerate() {
            if i > 0 {
                s.push_str(" | ");
            }
            match t {
                WorkingEntry::Turn { role, text, .. } => {
                    s.push_str(&format!("{role}: {}", first_words(text, 6)));
                }
                WorkingEntry::ToolCall { tool, summary, .. } => {
                    s.push_str(&format!("{tool}: {}", first_words(summary, 4)));
                }
                WorkingEntry::Bookmark {
                    summary_preview, ..
                } => {
                    s.push_str(&format!("bookmark: {}", first_words(summary_preview, 4)));
                }
            }
        }
        if turns.len() > 8 {
            s.push_str(&format!(" | …+{} more", turns.len() - 8));
        }
        s.push(']');
        Ok(s)
    }
}

fn first_words(text: &str, n: usize) -> String {
    text.split_whitespace()
        .take(n)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Approximate token count: whitespace-delimited words. Replace with a
/// tokenizer when the candle swap lands.
pub fn approx_tokens(text: &str) -> u64 {
    text.split_whitespace().count() as u64
}

pub fn entry_tokens(e: &WorkingEntry) -> u64 {
    match e {
        WorkingEntry::Turn { text, role, .. } => approx_tokens(text) + approx_tokens(role),
        WorkingEntry::ToolCall { tool, summary, .. } => {
            approx_tokens(tool) + approx_tokens(summary)
        }
        WorkingEntry::Bookmark {
            summary_preview, ..
        } => approx_tokens(summary_preview),
    }
}

pub async fn compact(
    dispatcher: &PartitionDispatcher,
    target_tokens: u64,
) -> Result<CompactReport> {
    compact_with(dispatcher, target_tokens, &PlaceholderSummarizer).await
}

pub async fn compact_with(
    dispatcher: &PartitionDispatcher,
    target_tokens: u64,
    summarizer: &dyn Summarizer,
) -> Result<CompactReport> {
    // 1. Snapshot live P1.
    let live = dispatcher.working.snapshot();
    let tokens_before: u64 = live.iter().map(entry_tokens).sum();
    if tokens_before <= target_tokens {
        return Ok(CompactReport {
            tokens_before,
            tokens_after: tokens_before,
            turns_offloaded: 0,
            bookmarks_inserted: 0,
        });
    }

    // 2. Walk oldest-first until removing them brings total <= target.
    let mut tokens_left = tokens_before;
    let mut offload: Vec<WorkingEntry> = Vec::new();
    for e in &live {
        if tokens_left <= target_tokens {
            break;
        }
        let t = entry_tokens(e);
        offload.push(e.clone());
        tokens_left = tokens_left.saturating_sub(t);
    }
    if offload.is_empty() {
        return Ok(CompactReport {
            tokens_before,
            tokens_after: tokens_before,
            turns_offloaded: 0,
            bookmarks_inserted: 0,
        });
    }

    // 3. Summarize offloaded turns.
    let prompt = "Summarize the following turns into a single P2 episode";
    let summary = summarizer.summarize(prompt, &offload).await?;
    // 4. Persist as a P2 episode.
    let ep = Episode {
        id: EpisodeId::new(),
        tier: Tier::Project,
        ts: now_secs(),
        episode_type: "letta_compaction".into(),
        // Embed the FULL contents of the offloaded turns in atomic_facts
        // so retrieval can recover them later (the "non-destructive"
        // guarantee).
        summary: summary.clone(),
        atomic_facts: offload
            .iter()
            .map(|e| match e {
                WorkingEntry::Turn { role, text, .. } => format!("{role}: {text}"),
                WorkingEntry::ToolCall { tool, summary, .. } => format!("tool:{tool} {summary}"),
                WorkingEntry::Bookmark {
                    summary_preview, ..
                } => format!("bookmark: {summary_preview}"),
            })
            .collect(),
        source: "compact".into(),
        source_product: "wcore-compact-internal".into(), // NOT wcore-compact (tool sanitization)
        session_id: None,
        project_root: None,
        decay_score: 1.0,
        status: EpisodeStatus::Active,
    };
    let ep_id = dispatcher.episodic.record(ep.clone()).await?;

    // 5. Replace P1 with a Bookmark entry pointing at the absorbed episode.
    let bookmark = WorkingEntry::Bookmark {
        ts: now_secs(),
        episode_id: ep_id.0.to_string(),
        summary_preview: summary.chars().take(120).collect(),
    };
    {
        let mut buf = dispatcher.working.buf.write();
        // Drop the oldest `offload.len()` entries, push bookmark at front.
        for _ in 0..offload.len().min(buf.len()) {
            buf.pop_front();
        }
        buf.push_front(bookmark);
    }

    let tokens_after: u64 = dispatcher.working.snapshot().iter().map(entry_tokens).sum();

    Ok(CompactReport {
        tokens_before,
        tokens_after,
        turns_offloaded: offload.len() as u64,
        bookmarks_inserted: 1,
    })
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
