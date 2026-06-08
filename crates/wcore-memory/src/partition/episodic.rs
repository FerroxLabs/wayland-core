// P2 Episodic — append-only inserts with embedding + FTS5 + tier routing.
// Implementation lands in Group C.2 alongside the trait scaffold.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cdc::CdcWriter;
use crate::db::{Db, vec_table_name_for_dim};
use crate::embed::{Embedder, decode_blob, encode_blob};
use crate::error::{MemoryError, Result};
use crate::v2_types::{Episode, EpisodeId, EpisodeStatus, Tier};

pub struct EpisodicPartition {
    pub(crate) db: Arc<Db>,
    pub(crate) embedder: Arc<dyn Embedder>,
    pub(crate) cdc: Arc<CdcWriter>,
}

impl EpisodicPartition {
    pub fn new(db: Arc<Db>, embedder: Arc<dyn Embedder>, cdc: Arc<CdcWriter>) -> Self {
        Self { db, embedder, cdc }
    }

    /// Insert a P2 episode. The caller has already passed the gate. Tier
    /// routing happens here: the episode's `tier` field decides which DB
    /// receives it. If that tier isn't configured, we fall back to global.
    pub async fn record(&self, mut ep: Episode) -> Result<EpisodeId> {
        if ep.ts == 0 {
            ep.ts = now_secs();
        }
        let embedding = self.embedder.embed(&ep.summary).await?;
        let tc = self.db.tier_or_global(ep.tier);
        let atomic_json = serde_json::to_string(&ep.atomic_facts).unwrap_or_else(|_| "[]".into());
        let blob = encode_blob(&embedding);
        {
            let conn = tc.conn.lock();
            conn.execute(
                "INSERT INTO episodes (id, tier, ts, episode_type, summary, atomic_facts, source, source_product, session_id, project_root, decay_score, status, embedding)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                rusqlite::params![
                    ep.id.0.to_string(),
                    ep.tier.as_str(),
                    ep.ts,
                    ep.episode_type,
                    ep.summary,
                    atomic_json,
                    ep.source,
                    ep.source_product,
                    ep.session_id,
                    ep.project_root,
                    ep.decay_score,
                    ep.status.as_str(),
                    blob,
                ],
            )?;
        }
        self.cdc.append_episode(ep.tier, &ep)?;
        Ok(ep.id)
    }

    pub async fn get(&self, id: &EpisodeId, tier: Tier) -> Result<Episode> {
        let tc = self.db.tier_or_global(tier);
        let conn = tc.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, tier, ts, episode_type, summary, atomic_facts, source, source_product, session_id, project_root, decay_score, status FROM episodes WHERE id = ?1",
        )?;
        let row = stmt.query_row([id.0.to_string()], |r| {
            let id_s: String = r.get(0)?;
            let tier_s: String = r.get(1)?;
            let atomic_s: String = r.get(5)?;
            let status_s: String = r.get(11)?;
            let atomic: Vec<String> = serde_json::from_str(&atomic_s).unwrap_or_default();
            let status = match status_s.as_str() {
                "archived" => EpisodeStatus::Archived,
                _ => EpisodeStatus::Active,
            };
            let parsed_id =
                uuid::Uuid::parse_str(&id_s).map_err(|_| rusqlite::Error::InvalidQuery)?;
            let parsed_tier: Tier = tier_s.parse().map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok(Episode {
                id: EpisodeId(parsed_id),
                tier: parsed_tier,
                ts: r.get(2)?,
                episode_type: r.get(3)?,
                summary: r.get(4)?,
                atomic_facts: atomic,
                source: r.get(6)?,
                source_product: r.get(7)?,
                session_id: r.get(8)?,
                project_root: r.get(9)?,
                decay_score: r.get(10)?,
                status,
            })
        });
        match row {
            Ok(ep) => Ok(ep),
            Err(rusqlite::Error::QueryReturnedNoRows) => Err(MemoryError::Consolidation(format!(
                "episode {} not found",
                id.0
            ))),
            Err(e) => Err(MemoryError::Db(e)),
        }
    }

    /// M5.7 — list recent episodes for `session_id` at `tier`, newest
    /// first, bounded by `limit`. Used by `SwarmMemoryBridge::read_for_child`
    /// to bootstrap a fresh worker with the parent's recent context
    /// WITHOUT round-tripping through the hybrid retriever (no embedder
    /// query needed — direct rowid scan).
    pub async fn list_recent_for_session(
        &self,
        session_id: &str,
        tier: Tier,
        limit: usize,
    ) -> Result<Vec<Episode>> {
        let tc = self.db.tier_or_global(tier);
        let conn = tc.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, tier, ts, episode_type, summary, atomic_facts, source, source_product, session_id, project_root, decay_score, status
             FROM episodes
             WHERE session_id = ?1 AND tier = ?2 AND status = 'active'
             ORDER BY ts DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![session_id, tier.as_str(), limit.max(1) as i64],
            |r| {
                let id_s: String = r.get(0)?;
                let tier_s: String = r.get(1)?;
                let atomic_s: String = r.get(5)?;
                let status_s: String = r.get(11)?;
                let atomic: Vec<String> = serde_json::from_str(&atomic_s).unwrap_or_default();
                let status = match status_s.as_str() {
                    "archived" => EpisodeStatus::Archived,
                    _ => EpisodeStatus::Active,
                };
                let parsed_id =
                    uuid::Uuid::parse_str(&id_s).map_err(|_| rusqlite::Error::InvalidQuery)?;
                let parsed_tier: Tier =
                    tier_s.parse().map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok(Episode {
                    id: EpisodeId(parsed_id),
                    tier: parsed_tier,
                    ts: r.get(2)?,
                    episode_type: r.get(3)?,
                    summary: r.get(4)?,
                    atomic_facts: atomic,
                    source: r.get(6)?,
                    source_product: r.get(7)?,
                    session_id: r.get(8)?,
                    project_root: r.get(9)?,
                    decay_score: r.get(10)?,
                    status,
                })
            },
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(MemoryError::Db)?);
        }
        Ok(out)
    }

    /// Read the raw embedding for an episode (used by retriever).
    pub fn read_embedding(&self, id: &EpisodeId, tier: Tier) -> Result<Vec<f32>> {
        let tc = self.db.tier_or_global(tier);
        let conn = tc.conn.lock();
        let blob: Vec<u8> = conn.query_row(
            "SELECT embedding FROM episodes WHERE id = ?1",
            [id.0.to_string()],
            |r| r.get(0),
        )?;
        decode_blob(&blob)
    }

    /// M5.7 — record an episode AND mirror its embedding into the dim-
    /// aware sqlite-vec `vec_episodes_<dim>` virtual table for KNN
    /// search.
    ///
    /// This is additive on top of [`Self::record`]: callers that want
    /// the KNN substrate populated should switch to this method;
    /// callers that only need BM25 retrieval can keep using `record`.
    /// The legacy BLOB-encoded `episodes.embedding` column is still
    /// written (so the existing O(n) cosine fallback in
    /// `retrieve::search_basic` keeps working — `vec_episodes_<dim>`
    /// is the *fast path*, not the only path).
    ///
    /// Dim mismatch: rejects with `MemoryError::Embedding` if the
    /// embedder produced a vector whose length disagrees with
    /// `embedder.dim()`. That trips the per-dim virtual-table
    /// auto-create path with a clean error instead of a vec0 schema
    /// rejection.
    pub async fn record_with_embedding(&self, mut ep: Episode) -> Result<EpisodeId> {
        if ep.ts == 0 {
            ep.ts = now_secs();
        }
        let embedding = self.embedder.embed(&ep.summary).await?;
        let dim = self.embedder.dim();
        if embedding.len() != dim {
            return Err(MemoryError::Embedding(format!(
                "embedder dim mismatch: {} reported {} but emit {} dims",
                self.embedder.name(),
                dim,
                embedding.len(),
            )));
        }

        // Ensure the per-dim vec0 virtual table exists across every
        // tier connection before we try to INSERT into it on the
        // routed tier. Cheap after the first call: one registry SELECT
        // per tier.
        let table = self.db.ensure_vec_table_for_dim(dim)?;
        debug_assert_eq!(table, vec_table_name_for_dim(dim));

        let tc = self.db.tier_or_global(ep.tier);
        let atomic_json = serde_json::to_string(&ep.atomic_facts).unwrap_or_else(|_| "[]".into());
        let blob = encode_blob(&embedding);

        let rowid: i64 = {
            let conn = tc.conn.lock();
            // S2: wrap both INSERTs in a single transaction so a crash
            // between the episodes row and the vec0 mirror cannot leave
            // an orphan in either table.
            // Note: CREATE VIRTUAL TABLE (vec0) cannot run inside a
            // transaction (sqlite-vec restriction), but plain INSERTs
            // into an already-created vec0 table can — ensured above by
            // `ensure_vec_table_for_dim`.
            let tx = conn.unchecked_transaction().map_err(MemoryError::Db)?;
            tx.execute(
                "INSERT INTO episodes (id, tier, ts, episode_type, summary, atomic_facts, source, source_product, session_id, project_root, decay_score, status, embedding)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                rusqlite::params![
                    ep.id.0.to_string(),
                    ep.tier.as_str(),
                    ep.ts,
                    ep.episode_type,
                    ep.summary,
                    atomic_json,
                    ep.source,
                    ep.source_product,
                    ep.session_id,
                    ep.project_root,
                    ep.decay_score,
                    ep.status.as_str(),
                    blob,
                ],
            )?;
            let rid: i64 = tx.last_insert_rowid();
            // Mirror into the vec0 virtual table keyed on the same
            // rowid so KNN results join back to the canonical row.
            let insert_sql = format!("INSERT INTO {table} (rowid, embedding) VALUES (?1, ?2)");
            tx.execute(&insert_sql, rusqlite::params![rid, blob])?;
            tx.commit().map_err(MemoryError::Db)?;
            rid
        };
        let _ = rowid; // currently unused outside the closure; reserved for future debug logging.

        self.cdc.append_episode(ep.tier, &ep)?;
        Ok(ep.id)
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
