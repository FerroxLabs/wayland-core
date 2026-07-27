// M5 — HybridRetriever (skeleton for Group C, full impl in Group D).
//
// Group C ships a BM25-only `search_basic` fallback so the dispatcher's
// `search` method compiles + roundtrips. Group D adds the full FTS5 +
// vector + KG fusion + RRF + session-diversity capping.

use rusqlite::OptionalExtension;

use crate::db::{Db, vec_table_name_for_dim};
use crate::embed::{Embedder, cosine, decode_blob, encode_blob};
use crate::error::{MemoryError, Result};
use crate::provenance::{
    ExclusionCause, ModalityContribution, RecallExclusion, RecallModality, RecallReport,
    provenance_for, read_privacy_scope, read_retention,
};
use crate::staleness::StalenessVerdict;
use crate::v2_types::{Hit, Partition, Query, Tier};

/// Cheap search used by Group C's dispatcher tests. Combines BM25 over
/// `episodes_fts` with a vector top-k pass, fuses by RRF, applies session
/// diversity, and trims to limit_per_modality.
///
/// Delegates to [`search_basic_with_provenance`] and drops the provenance, so
/// there is exactly one retrieval implementation.
pub async fn search_basic(db: &Db, embedder: &dyn Embedder, q: &Query) -> Result<Vec<Hit>> {
    Ok(search_basic_with_provenance(db, embedder, q).await?.0)
}

/// F23-03 — the retrieval that answers "what is in my context window, and
/// why".
///
/// Returns the same hits [`search_basic`] returns plus, for each, the
/// modalities that selected it, its rank inside each, its fused score, its age
/// and its staleness verdict; and separately the items and cells that were
/// EXCLUDED, so a privacy scope or a retention bound is visible rather than
/// silent.
pub async fn search_basic_with_provenance(
    db: &Db,
    embedder: &dyn Embedder,
    q: &Query,
) -> Result<(Vec<Hit>, RecallReport)> {
    let mut report = RecallReport::default();

    // A privacy scope is decided before any row is read: an excluded cell
    // must not have its contents loaded into this process at all, let alone
    // ranked.
    if let Some(scope) = read_privacy_scope(db, Partition::Episodic, q.tier)? {
        report.exclusions.push(RecallExclusion {
            partition: Partition::Episodic,
            tier: q.tier,
            id: None,
            cause: ExclusionCause::PrivacyScope {
                reason: scope.reason,
            },
        });
        return Ok((Vec::new(), report));
    }
    let retention = read_retention(db, Partition::Episodic, q.tier)?.map(|r| r.max_age_secs);

    let tc = db.tier_or_global(q.tier);

    // BM25 pass (FTS5).
    let bm25 = {
        let conn = tc.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT episodes.id, episodes.summary, episodes.session_id, bm25(episodes_fts) AS s
             FROM episodes_fts
             JOIN episodes ON episodes.rowid = episodes_fts.rowid
             WHERE episodes_fts MATCH ?1 AND episodes.tier = ?2
             ORDER BY s ASC
             LIMIT ?3",
        )?;
        let limit = q.limit_per_modality.max(1) as i64;
        let mut hits = Vec::new();
        let rows = stmt.query_map(
            rusqlite::params![fts5_query(&q.text), q.tier.as_str(), limit],
            |r| {
                let id: String = r.get(0)?;
                let summary: String = r.get(1)?;
                let session: Option<String> = r.get(2)?;
                let _score: f64 = r.get(3)?;
                Ok((id, summary, session))
            },
        )?;
        for r in rows {
            hits.push(r.map_err(MemoryError::Db)?);
        }
        hits
    };

    // Vector pass — prefer the dim-aware sqlite-vec KNN path
    // (M5.7) when the per-dim virtual table exists and has rows;
    // fall back to the legacy O(n) cosine over `episodes.embedding`
    // otherwise. The fallback is load-bearing: rows written before
    // M5.7 via `EpisodicPartition::record` (no vec0 mirror) are still
    // matchable via cosine even though the KNN table doesn't see them.
    let qvec = embedder.embed(&q.text).await?;
    let vector = {
        let dim = embedder.dim();
        let limit = q.limit_per_modality.max(1) as i64;
        let knn = knn_pass(&tc.conn, dim, &qvec, q.tier, limit)?;
        if !knn.is_empty() {
            knn
        } else {
            legacy_cosine_pass(&tc.conn, &qvec, q.tier, q.limit_per_modality)?
        }
    };

    // KG pass — depth-bounded BFS from query entities, then resolve
    // the touched node names to episodes via the `summary LIKE`
    // join. Returns an empty vec when `q.entities` is None/empty so
    // RRF degrades cleanly to the BM25 + vector fusion.
    let kg = kg_pass(&tc.conn, q)?;

    // RRF fuse (BM25 + vector + KG, k=60 canonical).
    let (mut combined, contributions) = rrf_fuse_with_contributions(&bm25, &vector, &kg, 60);
    // Session-diversity cap (max 3 per session_id).
    diversify_by_session(&mut combined, 3);

    // Retention: an item past the operator's bound is excluded and REPORTED
    // as expired. It is not deleted, so the exclusion is reversible by
    // relaxing the bound — which is what makes reporting it worth anything.
    let now = crate::audit::now_secs();
    let timestamps = episode_timestamps(&tc.conn, &combined)?;
    if retention.is_some() {
        let mut kept = Vec::with_capacity(combined.len());
        for hit in combined {
            let ts = timestamps.get(&hit.id).copied().unwrap_or(now);
            let age = crate::provenance::age_secs_at(now, ts);
            match crate::staleness::verdict_for_age(age, retention) {
                StalenessVerdict::Expired { max_age_secs } => {
                    report.exclusions.push(RecallExclusion {
                        partition: hit.partition,
                        tier: hit.tier,
                        id: Some(hit.id.clone()),
                        cause: ExclusionCause::RetentionExpired {
                            max_age_secs,
                            age_secs: age,
                        },
                    });
                }
                _ => kept.push(hit),
            }
        }
        combined = kept;
    }

    // Token budget — approximate via summary length.
    if let Some(budget) = q.token_budget {
        let mut used = 0u32;
        combined.retain(|h| {
            used += h.preview.split_whitespace().count() as u32;
            used <= budget
        });
    }

    // Provenance is built LAST, over exactly the surviving list, so the rank
    // it reports is the rank in the list that reached the prompt.
    report.provenance = combined
        .iter()
        .enumerate()
        .map(|(rank, hit)| {
            let ts = timestamps.get(&hit.id).copied().unwrap_or(now);
            provenance_for(
                &hit.id,
                hit.partition,
                hit.tier,
                contributions.get(&hit.id).cloned().unwrap_or_default(),
                rank,
                hit.score,
                now,
                ts,
                retention,
            )
        })
        .collect();

    Ok((combined, report))
}

/// Read the recorded timestamp of each surviving hit. This is a read of the
/// same rows the fusion already selected, never a second ranking, so it
/// cannot change which items are recalled.
fn episode_timestamps(
    conn: &parking_lot::Mutex<rusqlite::Connection>,
    hits: &[Hit],
) -> Result<std::collections::HashMap<String, i64>> {
    let mut out = std::collections::HashMap::with_capacity(hits.len());
    if hits.is_empty() {
        return Ok(out);
    }
    let conn = conn.lock();
    let mut stmt = conn
        .prepare("SELECT ts FROM episodes WHERE id = ?1")
        .map_err(MemoryError::Db)?;
    for hit in hits {
        if let Some(ts) = stmt
            .query_row(rusqlite::params![hit.id], |r| r.get::<_, i64>(0))
            .optional()
            .map_err(MemoryError::Db)?
        {
            out.insert(hit.id.clone(), ts);
        }
    }
    Ok(out)
}

/// Embedding-cosine recall over the P3 Semantic `facts` table.
///
/// `assert_fact` writes (subject, predicate, object) triples into the
/// `facts` table (P3 Semantic), which the episodic [`search_basic`] passes
/// (BM25 / vector / KG over `episodes_fts` + `episodes`) never touch.
/// Without this, a fact stored in a prior session is unreachable through
/// `search` — the cross-session recall gap. We embedding-rank the live
/// (non-superseded) facts at `q.tier` against the query and return the top
/// matches as `Partition::Semantic` hits so `session_search` and the
/// session-start recall both surface them.
///
/// The preview is the natural-language triple (`"subject predicate object"`)
/// — exactly what `assert_fact` embedded — so a fresh session re-surfaces a
/// stored preference verbatim.
///
/// Gating is the CALLER's responsibility: the dispatcher's `search` performs
/// the `Partition::Semantic` ACL check before invoking this, preserving the
/// per-partition read-scope boundary for sub-agents.
///
/// Returns an empty `Vec` (NOT an error) when the table is empty or no row
/// carries an embedding, so callers degrade cleanly to episodic-only results.
pub async fn facts_search(db: &Db, embedder: &dyn Embedder, q: &Query) -> Result<Vec<Hit>> {
    let tc = db.tier_or_global(q.tier);
    let qvec = embedder.embed(&q.text).await?;
    facts_cosine_pass(&tc.conn, &qvec, q.tier, q.limit_per_modality)
}

fn facts_cosine_pass(
    conn: &parking_lot::Mutex<rusqlite::Connection>,
    qvec: &[f32],
    tier: Tier,
    limit_per_modality: usize,
) -> Result<Vec<Hit>> {
    let conn = conn.lock();
    let mut stmt = conn.prepare(
        "SELECT id, subject, predicate, object, embedding FROM facts
         WHERE tier = ?1 AND superseded_by IS NULL AND embedding IS NOT NULL",
    )?;
    let rows = stmt.query_map([tier.as_str()], |r| {
        let id: String = r.get(0)?;
        let subject: String = r.get(1)?;
        let predicate: String = r.get(2)?;
        let object: String = r.get(3)?;
        let blob: Vec<u8> = r.get(4)?;
        Ok((id, subject, predicate, object, blob))
    })?;
    let mut scored: Vec<(f32, Hit)> = Vec::new();
    for r in rows {
        let (id, subject, predicate, object, blob) = r.map_err(MemoryError::Db)?;
        if let Ok(v) = decode_blob(&blob) {
            let s = cosine(qvec, &v);
            scored.push((
                s,
                Hit {
                    partition: Partition::Semantic,
                    tier,
                    id,
                    score: s as f64,
                    session_id: None,
                    preview: format!("{subject} {predicate} {object}"),
                },
            ));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored
        .into_iter()
        .take(limit_per_modality.max(1))
        .map(|(_, h)| h)
        .collect())
}

/// M5.7 — KNN pass against the dim-aware `vec_episodes_<dim>` virtual
/// table created by `EpisodicPartition::record_with_embedding`. Returns
/// an empty `Vec` (NOT an error) when the table is missing or empty so
/// the caller can transparently fall back to the legacy cosine path.
///
/// The vec0 `MATCH` operator with `LIMIT k` is the documented
/// KNN-search syntax; sqlite-vec returns rows ordered by ascending L2
/// distance (smaller = closer). We invert into a 0-1 similarity-ish
/// score via `1.0 / (1.0 + distance)` so the downstream RRF fuser sees
/// "higher is better" the same way the legacy cosine path emits.
fn knn_pass(
    conn: &parking_lot::Mutex<rusqlite::Connection>,
    dim: usize,
    qvec: &[f32],
    tier: Tier,
    limit: i64,
) -> Result<Vec<VecHit>> {
    let table = vec_table_name_for_dim(dim);
    let conn = conn.lock();
    // Cheap existence check — if the per-dim virtual table hasn't been
    // materialized yet (no record_with_embedding calls), bail to the
    // legacy path silently.
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1 AND type = 'table'",
            [&table],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if exists == 0 {
        return Ok(Vec::new());
    }
    let qblob = encode_blob(qvec);

    // sqlite-vec KNN: `embedding MATCH ? AND k = ?` is the canonical
    // syntax. We join to `episodes` so the caller gets the same
    // (id, summary, session_id) tuple shape as the legacy cosine pass.
    // The `tier` filter is applied post-join because vec0 only carries
    // the embedding column (no tier metadata in the virtual table —
    // by design, to keep the index compact).
    let sql = format!(
        "SELECT e.id, e.summary, e.session_id, v.distance
         FROM {table} v
         JOIN episodes e ON e.rowid = v.rowid
         WHERE v.embedding MATCH ?1
           AND k = ?2
           AND e.tier = ?3
           AND e.status = 'active'
         ORDER BY v.distance ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(MemoryError::Db)?;
    let rows = stmt
        .query_map(rusqlite::params![qblob, limit, tier.as_str()], |r| {
            let id: String = r.get(0)?;
            let summary: String = r.get(1)?;
            let session: Option<String> = r.get(2)?;
            let distance: f32 = r.get(3)?;
            // Map L2 distance to a positive "higher-is-better" pseudo-
            // similarity so RRF fusion downstream treats KNN hits the
            // same shape as cosine hits. The actual numeric scale only
            // matters for stable RRF ranking inside this list (we
            // don't compare across tier-pass values).
            let score = 1.0f32 / (1.0f32 + distance);
            Ok((score, id, summary, session))
        })
        .map_err(MemoryError::Db)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(MemoryError::Db)?);
    }
    Ok(out)
}

fn legacy_cosine_pass(
    conn: &parking_lot::Mutex<rusqlite::Connection>,
    qvec: &[f32],
    tier: Tier,
    limit_per_modality: usize,
) -> Result<Vec<VecHit>> {
    let conn = conn.lock();
    let mut stmt = conn.prepare(
        "SELECT id, summary, session_id, embedding FROM episodes
         WHERE tier = ?1 AND embedding IS NOT NULL AND status = 'active'",
    )?;
    let rows = stmt.query_map([tier.as_str()], |r| {
        let id: String = r.get(0)?;
        let summary: String = r.get(1)?;
        let session: Option<String> = r.get(2)?;
        let blob: Vec<u8> = r.get(3)?;
        Ok((id, summary, session, blob))
    })?;
    let mut scored: Vec<(f32, String, String, Option<String>)> = Vec::new();
    for r in rows {
        let (id, summary, session, blob) = r.map_err(MemoryError::Db)?;
        if let Ok(v) = decode_blob(&blob) {
            let s = cosine(qvec, &v);
            scored.push((s, id, summary, session));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit_per_modality.max(1));
    Ok(scored)
}

/// FTS5 needs trigrams or pre-escaped query text. We coarsely OR-join
/// tokens so partial matches still hit.
fn fts5_query(text: &str) -> String {
    let toks: Vec<String> = text
        .split_whitespace()
        .filter(|t| t.chars().any(|c| c.is_alphanumeric()))
        .map(|t| {
            let cleaned: String = t.chars().filter(|c| c.is_alphanumeric()).collect();
            cleaned
        })
        .filter(|t| !t.is_empty())
        .collect();
    if toks.is_empty() {
        return "\"\"".into(); // empty query, won't match
    }
    toks.iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

type BmHit = (String, String, Option<String>);
type VecHit = (f32, String, String, Option<String>);
// KgHit carries (weight, id, summary, session). The weight slot exists
// for symmetry with VecHit (and future expansion to weight by BFS
// depth) but is ignored by RRF — only the rank within the slice
// matters for fusion.
type KgHit = (f32, String, String, Option<String>);

/// Phase 6.5 — KG modality pass for `search_basic`.
///
/// Choice: **(a) path-based via `kg::bfs::bfs_neighbors`** (rather than
/// a direct SQL `kg_edges` join). Rationale:
///   - Reuses the already-shipped + tested BFS primitive with depth
///     and node caps — no new traversal code.
///   - Mirrors Forge's HybridRetriever wiring, per design doc §6.5
///     ("BFS is closer to Forge's HybridRetriever's KG-modality wiring").
///   - The BFS already handles undirected expansion + dedup, which a
///     bespoke SQL join would have to re-derive.
///
/// Returns an empty vec when `q.entities` is `None`/empty (pass-through),
/// when no entity names resolve to KG nodes, or when no touched node
/// names appear in any episode summary. Errors propagate from the
/// underlying KG primitives.
fn kg_pass(conn: &parking_lot::Mutex<rusqlite::Connection>, q: &Query) -> Result<Vec<KgHit>> {
    let Some(entities) = q.entities.as_ref() else {
        return Ok(Vec::new());
    };
    if entities.is_empty() {
        return Ok(Vec::new());
    }
    let conn = conn.lock();

    // 1. Resolve entity names → KG node ids. Use exact-match on name
    //    (find_nodes_by_name does substring; we want only true seeds).
    let mut seeds: Vec<i64> = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT id FROM kg_nodes WHERE name = ?1")
            .map_err(MemoryError::Db)?;
        for ent in entities {
            let rows = stmt
                .query_map(rusqlite::params![ent], |r| r.get::<_, i64>(0))
                .map_err(MemoryError::Db)?;
            for r in rows {
                seeds.push(r.map_err(MemoryError::Db)?);
            }
        }
    }
    if seeds.is_empty() {
        return Ok(Vec::new());
    }

    // 2. BFS from each seed up to q.kg_depth; collect unique node ids.
    //    Cap total nodes at 128 — generous but bounded.
    let limit = crate::kg::BfsLimit::new(q.kg_depth as u32, 128);
    let mut touched: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for seed in seeds {
        let neighbours = crate::kg::bfs_neighbors(&conn, seed, limit)?;
        for (nid, _depth) in neighbours {
            touched.insert(nid);
        }
    }
    if touched.is_empty() {
        return Ok(Vec::new());
    }

    // 3. Resolve touched node ids → names, then join those names to
    //    episodes via `summary LIKE '%name%'`. We cannot rely on a
    //    direct kg_edges→episodes link (the schema doesn't carry one
    //    in v0.6.x), so name-mention is the canonical projection.
    let mut names: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT name FROM kg_nodes WHERE id = ?1")
            .map_err(MemoryError::Db)?;
        for nid in touched {
            let n = stmt
                .query_row(rusqlite::params![nid], |r| r.get::<_, String>(0))
                .map_err(MemoryError::Db)?;
            names.push(n);
        }
    }

    let mut out: Vec<KgHit> = Vec::new();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let limit_per_modality = q.limit_per_modality.max(1) as i64;
    let mut stmt = conn
        .prepare(
            "SELECT id, summary, session_id FROM episodes
             WHERE tier = ?1 AND status = 'active' AND summary LIKE ?2 ESCAPE '\\'
             LIMIT ?3",
        )
        .map_err(MemoryError::Db)?;
    for name in names {
        // Escape LIKE metacharacters (%, _, \) in the user-controllable node
        // name before wrapping in %...%. KG node names come from KgIngest
        // without LIKE-pattern validation; a bare `%` would otherwise match
        // every episode summary and pollute RRF rankings.
        let escaped = name
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let rows = stmt
            .query_map(
                rusqlite::params![q.tier.as_str(), pattern, limit_per_modality],
                |r| {
                    let id: String = r.get(0)?;
                    let summary: String = r.get(1)?;
                    let session: Option<String> = r.get(2)?;
                    Ok((id, summary, session))
                },
            )
            .map_err(MemoryError::Db)?;
        for r in rows {
            let (id, summary, session) = r.map_err(MemoryError::Db)?;
            if seen_ids.insert(id.clone()) {
                out.push((1.0, id, summary, session));
            }
            if out.len() >= q.limit_per_modality.max(1) {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

fn rrf_fuse(bm25: &[BmHit], vector: &[VecHit], kg: &[KgHit], k: usize) -> Vec<Hit> {
    rrf_fuse_with_contributions(bm25, vector, kg, k).0
}

/// F23-03 — the same fusion, additionally returning which modality selected
/// each item and at what rank inside that modality's own list.
///
/// This is the ONLY fusion implementation; [`rrf_fuse`] delegates to it and
/// discards the second return value. That is deliberate: a provenance record
/// computed by a second, parallel pass could describe a ranking that never
/// happened, and a user shown a provenance that does not match their context
/// window is worse off than one shown nothing. There is no second pass to
/// drift from.
///
/// The scoring math is byte-for-byte the pre-existing math — the golden RRF
/// tests below pin it, and they were not touched.
fn rrf_fuse_with_contributions(
    bm25: &[BmHit],
    vector: &[VecHit],
    kg: &[KgHit],
    k: usize,
) -> (
    Vec<Hit>,
    std::collections::HashMap<String, Vec<ModalityContribution>>,
) {
    use std::collections::HashMap;
    let mut scores: HashMap<String, f64> = HashMap::new();
    let mut meta: HashMap<String, (String, Option<String>)> = HashMap::new();
    let mut contributions: HashMap<String, Vec<ModalityContribution>> = HashMap::new();

    for (rank, (id, summary, session)) in bm25.iter().enumerate() {
        let s = 1.0 / (k as f64 + rank as f64 + 1.0);
        *scores.entry(id.clone()).or_insert(0.0) += s;
        meta.entry(id.clone())
            .or_insert_with(|| (summary.clone(), session.clone()));
        contributions
            .entry(id.clone())
            .or_default()
            .push(ModalityContribution {
                modality: RecallModality::Lexical,
                rank,
            });
    }
    for (rank, (_cos, id, summary, session)) in vector.iter().enumerate() {
        let s = 1.0 / (k as f64 + rank as f64 + 1.0);
        *scores.entry(id.clone()).or_insert(0.0) += s;
        meta.entry(id.clone())
            .or_insert_with(|| (summary.clone(), session.clone()));
        contributions
            .entry(id.clone())
            .or_default()
            .push(ModalityContribution {
                modality: RecallModality::Vector,
                rank,
            });
    }
    for (rank, (_w, id, summary, session)) in kg.iter().enumerate() {
        let s = 1.0 / (k as f64 + rank as f64 + 1.0);
        *scores.entry(id.clone()).or_insert(0.0) += s;
        meta.entry(id.clone())
            .or_insert_with(|| (summary.clone(), session.clone()));
        contributions
            .entry(id.clone())
            .or_default()
            .push(ModalityContribution {
                modality: RecallModality::Graph,
                rank,
            });
    }

    let mut out: Vec<Hit> = scores
        .into_iter()
        .filter_map(|(id, s)| {
            meta.remove(&id).map(|(summary, session)| Hit {
                partition: Partition::Episodic,
                tier: Tier::Project,
                id,
                score: s,
                session_id: session,
                preview: summary,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    (out, contributions)
}

pub fn diversify_by_session(hits: &mut Vec<Hit>, max_per_session: usize) {
    use std::collections::HashMap;
    let mut seen: HashMap<String, usize> = HashMap::new();
    hits.retain(|h| match &h.session_id {
        Some(s) => {
            let entry = seen.entry(s.clone()).or_insert(0);
            if *entry < max_per_session {
                *entry += 1;
                true
            } else {
                false
            }
        }
        None => true,
    });
}

#[cfg(test)]
mod rrf_kg_tests {
    //! Phase 6.5 — pinned golden values for the 3-modality RRF fuse.
    //!
    //! Golden math (k=60): rank r → 1 / (60 + r + 1).
    //!   - rank 0 → 1/61 ≈ 0.0163934426
    //!   - rank 1 → 1/62 ≈ 0.0161290323
    //!   - rank 2 → 1/63 ≈ 0.0158730159
    //!
    //! Tie ordering is NOT stable (HashMap-iter source); ties are
    //! asserted via set-equality + score equality per design doc.
    use super::{BmHit, KgHit, VecHit, rrf_fuse};

    fn b(id: &str) -> (String, String, Option<String>) {
        (id.to_string(), format!("summary-{id}"), None)
    }
    fn v(id: &str) -> (f32, String, String, Option<String>) {
        (0.9, id.to_string(), format!("summary-{id}"), None)
    }
    fn g(id: &str) -> (f32, String, String, Option<String>) {
        (1.0, id.to_string(), format!("summary-{id}"), None)
    }

    const EPS: f64 = 1e-9;

    #[test]
    fn bm25_only() {
        let bm25: Vec<BmHit> = vec![b("a"), b("b")];
        let vector: Vec<VecHit> = vec![];
        let kg: Vec<KgHit> = vec![];
        let out = rrf_fuse(&bm25, &vector, &kg, 60);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "a");
        assert_eq!(out[1].id, "b");
        assert!((out[0].score - 1.0 / 61.0).abs() < EPS);
        assert!((out[1].score - 1.0 / 62.0).abs() < EPS);
    }

    #[test]
    fn kg_boosts_to_top() {
        // bm25=[a@0], vec=[], kg=[b@0, a@1].
        // a: 1/61 (bm25 r0) + 1/62 (kg r1) ≈ 0.03252
        // b: 1/61 (kg r0)                    ≈ 0.01639
        // a beats b -> a first.
        let bm25: Vec<BmHit> = vec![b("a")];
        let vector: Vec<VecHit> = vec![];
        let kg: Vec<KgHit> = vec![g("b"), g("a")];
        let out = rrf_fuse(&bm25, &vector, &kg, 60);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "a");
        assert_eq!(out[1].id, "b");
        let expected_a = 1.0 / 61.0 + 1.0 / 62.0;
        let expected_b = 1.0 / 61.0;
        assert!(
            (out[0].score - expected_a).abs() < EPS,
            "a score {} != {}",
            out[0].score,
            expected_a
        );
        assert!(
            (out[1].score - expected_b).abs() < EPS,
            "b score {} != {}",
            out[1].score,
            expected_b
        );
    }

    #[test]
    fn three_modalities_dedupe() {
        // All three modalities hit `x` at rank 0 -> 3/61 ≈ 0.04918.
        let bm25: Vec<BmHit> = vec![b("x")];
        let vector: Vec<VecHit> = vec![v("x")];
        let kg: Vec<KgHit> = vec![g("x")];
        let out = rrf_fuse(&bm25, &vector, &kg, 60);
        assert_eq!(out.len(), 1, "expected dedupe to a single hit for x");
        assert_eq!(out[0].id, "x");
        let expected = 3.0 / 61.0;
        assert!(
            (out[0].score - expected).abs() < EPS,
            "x score {} != {}",
            out[0].score,
            expected
        );
    }

    #[test]
    fn kg_only_no_bm25_match() {
        // Single-modality (KG only): RRF still produces ordered hits.
        let bm25: Vec<BmHit> = vec![];
        let vector: Vec<VecHit> = vec![];
        let kg: Vec<KgHit> = vec![g("b"), g("c")];
        let out = rrf_fuse(&bm25, &vector, &kg, 60);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "b");
        assert_eq!(out[1].id, "c");
        assert!((out[0].score - 1.0 / 61.0).abs() < EPS);
        assert!((out[1].score - 1.0 / 62.0).abs() < EPS);
    }

    #[test]
    fn tie_breaks_set_equality_and_score() {
        // bm25=[a@0,b@1], vec=[b@0,a@1] — a and b both score 1/61 + 1/62.
        // Sort order between two equal-score entries is not stable
        // (HashMap source); assert SET equality + score equality.
        use std::collections::HashSet;
        let bm25: Vec<BmHit> = vec![b("a"), b("b")];
        let vector: Vec<VecHit> = vec![v("b"), v("a")];
        let kg: Vec<KgHit> = vec![];
        let out = rrf_fuse(&bm25, &vector, &kg, 60);
        assert_eq!(out.len(), 2);
        let ids: HashSet<&str> = out.iter().map(|h| h.id.as_str()).collect();
        let expected_ids: HashSet<&str> = ["a", "b"].into_iter().collect();
        assert_eq!(ids, expected_ids);
        let expected = 1.0 / 61.0 + 1.0 / 62.0;
        for h in &out {
            assert!(
                (h.score - expected).abs() < EPS,
                "{} score {} != {}",
                h.id,
                h.score,
                expected
            );
        }
    }
}
