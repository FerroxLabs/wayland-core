// M7 — ConsolidationEngine (dream cycle).
//
// Pipeline: compress (P1→P2) → consolidate (P2→P3) → crystallize (P3→P4) →
// decay. All four stages ship real implementations as of v0.2.x. M3.1 wires
// `run()` into session-end via `DreamThrottle` so the cycle fires
// automatically without running every session.

use std::time::Instant;

use crate::error::Result;
use crate::partition::PartitionDispatcher;
use crate::partition::prefixspan::{PrefixSpan, ToolSequence};
use crate::v2_types::{
    AccessToken, DreamReport, Episode, EpisodeId, EpisodeStatus, Fact, FactId, Tier,
};

pub struct ConsolidationEngine {
    dispatcher: PartitionDispatcher,
}

impl ConsolidationEngine {
    pub fn new(dispatcher: PartitionDispatcher) -> Self {
        Self { dispatcher }
    }

    /// Run the full dream cycle. Each phase is best-effort — errors in
    /// one phase don't block subsequent phases (recorded in report).
    pub async fn run(&self) -> Result<DreamReport> {
        let started = Instant::now();
        let episodes_compressed = self.compress().await?;
        let facts_consolidated = self.consolidate().await?;
        let procedures_crystallized = self.crystallize().await?;
        // v0.6.4 Task 6.6c — run transitive inference AFTER consolidate has
        // landed any new (subject, predicate, object) edges via
        // `semantic.assert` / `kg_ingest_facts`. Skipped when `WAYLAND_KG=off`
        // or no Session-tier connection is configured (best-effort: errors
        // do not abort the dream cycle).
        let kg_edges_inferred = self.infer_kg().await.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "dream cycle: infer_kg failed");
            0
        });
        let episodes_decayed = self.decay().await?;
        Ok(DreamReport {
            episodes_compressed,
            facts_consolidated,
            procedures_crystallized,
            episodes_decayed,
            kg_edges_inferred,
            elapsed_ms: started.elapsed().as_millis(),
        })
    }

    /// v0.6.4 Task 6.6c — run one pass of deductive transitive inference
    /// over the KG against the same Session-tier `Connection` that
    /// `kg_ingest_facts` and `init_kg` own (see
    /// `PartitionDispatcher::kg_ingest_facts` for the canonical accessor
    /// pattern). Returns the number of inferred edges materialized.
    ///
    /// No-ops when `WAYLAND_KG=off` or when the dispatcher has no
    /// Session-tier configured (e.g. test harnesses that only build a
    /// Project tier). Best-effort: callers should treat errors as a
    /// "skip this cycle" signal, not a fatal failure.
    pub async fn infer_kg(&self) -> Result<u64> {
        if !crate::kg::kg_enabled() {
            return Ok(0);
        }
        let Some(tier_conn) = self.dispatcher.db.tier(Tier::Session) else {
            return Ok(0);
        };
        let conn = tier_conn.conn.lock();
        let out = crate::kg::inference::infer_once(&conn)?;
        Ok(out.edges_created)
    }

    /// Compress P1 working memory into P2 episodes. Snapshots the live
    /// queue, summarises every N=20 entries into one episode.
    pub async fn compress(&self) -> Result<u64> {
        let live = self.dispatcher.working.snapshot();
        if live.is_empty() {
            return Ok(0);
        }
        let chunk = 20;
        let mut written = 0u64;
        for batch in live.chunks(chunk) {
            let summary = mock_summarize(batch);
            if summary.trim().is_empty() {
                continue;
            }
            let ep = Episode {
                id: EpisodeId::new(),
                tier: Tier::Project,
                ts: now_secs(),
                episode_type: "session_summary".into(),
                summary,
                atomic_facts: Vec::new(),
                source: "consolidate".into(),
                source_product: "wcore-consolidate".into(),
                session_id: None,
                project_root: None,
                decay_score: 1.0,
                status: EpisodeStatus::Active,
            };
            self.dispatcher.episodic.record(ep).await?;
            written += 1;
        }
        Ok(written)
    }

    /// Cheap heuristic: scan recent P2 summaries for "User prefers X" patterns
    /// and emit a (user, prefers, X) fact.
    pub async fn consolidate(&self) -> Result<u64> {
        let tc = self.dispatcher.db.tier_or_global(Tier::Project);
        let mut summaries: Vec<String> = Vec::new();
        {
            let conn = tc.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT summary FROM episodes WHERE tier = 'project' AND status = 'active' ORDER BY ts DESC LIMIT 50",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for s in rows.flatten() {
                summaries.push(s);
            }
        }

        let mut emitted = 0u64;
        for s in summaries {
            if let Some((subj, pred, obj, conf)) = extract_fact(&s) {
                let f = Fact {
                    id: FactId::new(),
                    tier: Tier::Project,
                    ts: now_secs(),
                    subject: subj,
                    predicate: pred,
                    object: obj,
                    confidence: conf,
                    source_episode: None,
                    superseded_by: None,
                };
                self.dispatcher.semantic.assert(f).await?;
                emitted += 1;
            }
        }
        Ok(emitted)
    }

    /// Crystallize P3 patterns into P4 staged procedures. Cheap stub: if
    /// at least 3 episodes mention the same `episode_type` recently,
    /// crystallize a staged procedure for it.
    pub async fn crystallize(&self) -> Result<u64> {
        let tc = self.dispatcher.db.tier_or_global(Tier::Project);
        let counts: Vec<(String, i64)> = {
            let conn = tc.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT episode_type, COUNT(*) AS c FROM episodes WHERE tier = 'project' AND status = 'active' GROUP BY episode_type HAVING c >= 3",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            let mut out = Vec::new();
            for t in rows.flatten() {
                out.push(t);
            }
            out
        };
        let mut emitted = 0u64;
        for (ep_type, cnt) in counts {
            // Don't crystallize "session_summary" — those are noise.
            if ep_type == "session_summary" || ep_type == "legacy_yaml" {
                continue;
            }
            let proc = crate::v2_types::Procedure {
                id: crate::v2_types::ProcedureId::new(),
                tier: Tier::Project,
                ts: now_secs(),
                name: format!("pattern:{ep_type}"),
                description: format!("auto-crystallized from {cnt} episodes of type {ep_type}"),
                artifact: String::new(),
                status: crate::v2_types::ProcedureStatus::Staged,
                created_by: "evolution".into(),
                thompson_alpha: 1.0,
                thompson_beta: 1.0,
                use_count: 0,
                success_count: 0,
            };
            self.dispatcher.procedural.upsert(proc).await?;
            emitted += 1;
        }

        // PrefixSpan pass (v0.6.4 Task 6.6a): mine frequent tool-call
        // sequences across recent sessions and crystallize any patterns
        // with support ≥ 3 as staged procedures. ToolSequences are
        // reconstructed from `atomic_facts` entries that begin with the
        // literal `"tool:"` prefix (the format emitted by
        // `compact.rs::offload`). One sequence per `session_id`, tools
        // ordered by episode `ts` ascending.
        let sequences = self.collect_tool_sequences().await?;
        let patterns = PrefixSpan::new(2, 10).mine(&sequences);
        for pat in patterns {
            if pat.support < 3 {
                continue;
            }
            let name = format!("seq:{}", pat.pattern.join("\u{2192}"));
            let proc = crate::v2_types::Procedure {
                id: crate::v2_types::ProcedureId::new(),
                tier: Tier::Project,
                ts: now_secs(),
                name,
                description: format!(
                    "auto-mined by PrefixSpan: support={} confidence={:.3}",
                    pat.support, pat.confidence
                ),
                artifact: String::new(),
                status: crate::v2_types::ProcedureStatus::Staged,
                created_by: "evolution".into(),
                thompson_alpha: 1.0,
                thompson_beta: 1.0,
                use_count: 0,
                success_count: 0,
            };
            self.dispatcher.procedural.upsert(proc).await?;
            emitted += 1;
        }

        Ok(emitted)
    }

    /// Reconstruct one `ToolSequence` per session by scanning recent
    /// project-tier episodes for `atomic_facts` entries with a leading
    /// `"tool:"` marker. Tools are appended in episode-`ts` ascending
    /// order so multi-episode sessions concatenate chronologically.
    /// Sessions whose episodes carry no tool markers are dropped (an
    /// empty `ToolSequence` produces no patterns).
    async fn collect_tool_sequences(&self) -> Result<Vec<ToolSequence>> {
        let tc = self.dispatcher.db.tier_or_global(Tier::Project);
        let rows: Vec<(String, String)> = {
            let conn = tc.conn.lock();
            // 500 episodes is enough headroom for the dream cycle; the
            // miner is O(n × items × max_length) and easily handles it.
            let mut stmt = conn.prepare(
                "SELECT session_id, atomic_facts \
                 FROM episodes \
                 WHERE tier = 'project' AND status = 'active' AND session_id IS NOT NULL \
                 ORDER BY ts ASC \
                 LIMIT 500",
            )?;
            let mapped =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            let mut out = Vec::new();
            for t in mapped.flatten() {
                out.push(t);
            }
            out
        };

        use std::collections::BTreeMap;
        let mut by_session: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (session_id, facts_json) in rows {
            let facts: Vec<String> =
                serde_json::from_str(&facts_json).unwrap_or_else(|_| Vec::new());
            let entry = by_session.entry(session_id).or_default();
            for f in facts {
                if let Some(tool) = parse_tool_marker(&f) {
                    entry.push(tool);
                }
            }
        }

        let mut out = Vec::with_capacity(by_session.len());
        for (session_id, tools) in by_session {
            if tools.is_empty() {
                continue;
            }
            out.push(ToolSequence { session_id, tools });
        }
        Ok(out)
    }

    /// Ebbinghaus decay: new_score = old_score * exp(-age_days / 7.0).
    /// Episodes >30 days old flip to archived. Never DELETE.
    pub async fn decay(&self) -> Result<u64> {
        let now = now_secs();
        // Run on all tiers we have.
        let mut decayed = 0u64;
        for tier in [Tier::Session, Tier::Project, Tier::Global] {
            let Some(tc) = self.dispatcher.db.tier(tier) else {
                continue;
            };
            let to_decay: Vec<(String, i64, f64)> = {
                let conn = tc.conn.lock();
                let mut stmt = conn.prepare(
                    "SELECT id, ts, decay_score FROM episodes WHERE tier = ?1 AND status = 'active'",
                )?;
                let rows = stmt.query_map([tier.as_str()], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, f64>(2)?,
                    ))
                })?;
                let mut out = Vec::new();
                for t in rows.flatten() {
                    out.push(t);
                }
                out
            };
            for (id, ts, _old) in to_decay {
                let age_days = ((now - ts).max(0) as f64) / 86400.0;
                let new_score = (-age_days / 7.0).exp();
                let archive = age_days >= 30.0;
                {
                    let conn = tc.conn.lock();
                    if archive {
                        conn.execute(
                            "UPDATE episodes SET decay_score = ?1, status = 'archived' WHERE id = ?2",
                            rusqlite::params![new_score, id],
                        )?;
                    } else {
                        conn.execute(
                            "UPDATE episodes SET decay_score = ?1 WHERE id = ?2",
                            rusqlite::params![new_score, id],
                        )?;
                    }
                }
                if archive && let Ok(uuid) = uuid::Uuid::parse_str(&id) {
                    self.dispatcher
                        .cdc
                        .append_decay_archive(tier, &uuid, new_score)?;
                }
                decayed += 1;
            }
        }
        Ok(decayed)
    }
}

fn mock_summarize(batch: &[crate::partition::working::WorkingEntry]) -> String {
    let mut s = String::new();
    for e in batch {
        match e {
            crate::partition::working::WorkingEntry::Turn { role, text, .. } => {
                if !s.is_empty() {
                    s.push_str("; ");
                }
                s.push_str(&format!("{role}: {}", first_n_words(text, 12)));
            }
            crate::partition::working::WorkingEntry::ToolCall { tool, summary, .. } => {
                if !s.is_empty() {
                    s.push_str("; ");
                }
                s.push_str(&format!("tool:{tool} {}", first_n_words(summary, 8)));
            }
            crate::partition::working::WorkingEntry::Bookmark {
                summary_preview, ..
            } => {
                if !s.is_empty() {
                    s.push_str("; ");
                }
                s.push_str(&format!("bookmark: {}", first_n_words(summary_preview, 8)));
            }
        }
    }
    s
}

fn first_n_words(text: &str, n: usize) -> String {
    text.split_whitespace()
        .take(n)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Cheap pattern: "User prefers X" / "user wants X" / "<subject> is <object>".
fn extract_fact(s: &str) -> Option<(String, String, String, f64)> {
    let lower = s.to_lowercase();
    if let Some(rest) = lower.strip_prefix("user prefers ") {
        let obj = rest.trim().trim_end_matches('.').replace(' ', "-");
        if !obj.is_empty() {
            return Some(("user".into(), "prefers".into(), obj, 0.8));
        }
    }
    if let Some(rest) = lower.strip_prefix("user wants ") {
        let obj = rest.trim().trim_end_matches('.').replace(' ', "-");
        if !obj.is_empty() {
            return Some(("user".into(), "wants".into(), obj, 0.7));
        }
    }
    None
}

/// Parse the canonical tool-call marker that `compact.rs::offload` emits
/// into `atomic_facts`: `"tool:<name> <free-form summary>"`. Returns the
/// `<name>` token (everything between the `tool:` prefix and the first
/// whitespace), or `None` if the entry isn't a tool marker or is missing
/// a name. Strict: no fallback to `"tool"` itself when the name slot is
/// empty.
fn parse_tool_marker(fact: &str) -> Option<String> {
    let rest = fact.strip_prefix("tool:")?;
    let name = rest.split_whitespace().next()?;
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---- AccessToken used for internal writes ----
#[allow(dead_code)]
fn system_token() -> AccessToken {
    AccessToken::System
}

// ---- M3.1 — DreamThrottle ----

use std::sync::Mutex;
use std::time::Duration;

/// Concurrency-safe throttle: [`should_run`] returns true at most once per
/// `min_interval`. Marks the last-run timestamp on every `true` return,
/// so concurrent callers within the window all see `false`.
///
/// Used at session-end (`fire_on_session_end` in `wcore-agent::engine`) to
/// prevent the dream cycle running on every 30-second session. The window
/// is configured via [`crate::consolidate::DreamThrottle::new`] using
/// `cfg.memory.dream_cycle_throttle_secs`.
///
/// Thread safety: a single `Mutex<Option<Instant>>` guards the last-run
/// stamp. The mutex is held only across an `Instant::now()` comparison
/// and a single store, so it never blocks an awaiting task long enough to
/// matter. [`should_run`] panics if a prior caller panicked while holding
/// the lock — that's a fatal logic bug.
///
/// [`should_run`]: DreamThrottle::should_run
#[derive(Debug)]
pub struct DreamThrottle {
    last_run: Mutex<Option<Instant>>,
    min_interval: Duration,
}

impl DreamThrottle {
    /// Construct a throttle that releases at most once per `min_interval`.
    /// A `Duration::ZERO` interval makes every call run (useful for tests).
    pub fn new(min_interval: Duration) -> Self {
        Self {
            last_run: Mutex::new(None),
            min_interval,
        }
    }

    /// Returns `true` and marks the timestamp iff at least `min_interval`
    /// has elapsed since the last `true` return (or this is the first call).
    /// Returns `false` and does not touch the timestamp otherwise.
    pub fn should_run(&self) -> bool {
        let mut guard = self.last_run.lock().expect("dream throttle poisoned");
        let now = Instant::now();
        let due = match *guard {
            None => true,
            Some(prev) => now.duration_since(prev) >= self.min_interval,
        };
        if due {
            *guard = Some(now);
        }
        due
    }
}
