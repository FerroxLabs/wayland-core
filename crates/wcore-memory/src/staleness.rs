//! T2-D2 — BFS cascading staleness propagation over the KG substrate.
//!
//! # Status: WIRED (v0.6.3 — W4 + Wave D Round 1 cleanup)
//!
//! `staleness_enabled()` returns the env-gated bool. `init_staleness` runs
//! in `AgentBootstrap::build()` alongside `init_kg`, and `propagate_staleness`
//! is invoked from `kg::nodes::upsert_node` on every node upsert. The
//! `WAYLAND_STALENESS=off` rollback flag disables the cascade.
//!
//! Ports the ijfw mcp-server `memory/staleness.js` semantics: a node marked
//! stale propagates its staleness to neighbours discovered via a bounded BFS
//! walk over `kg_edges`. We use a NEW sibling table `kg_node_staleness`
//! rather than mutating the `kg_nodes` schema — the existing KG primitives
//! (`crate::kg::*`) stay untouched and this module is purely additive.
//!
//! Table:
//!   kg_node_staleness(
//!     node_id INTEGER PRIMARY KEY,         -- FK kg_nodes(id)
//!     marked_at INTEGER NOT NULL,          -- unix seconds
//!     propagated_to_neighbors_at INTEGER,  -- unix seconds, NULL until cascade
//!     FOREIGN KEY(node_id) REFERENCES kg_nodes(id)
//!   )
//!
//! Rollback: set WAYLAND_STALENESS=off to skip propagation; mark_stale
//! becomes a no-op when this env var is "off". Callers check the env var
//! themselves; the primitive does not.

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{MemoryError, Result};
use crate::kg::{BfsLimit, bfs_neighbors};

/// Env var controlling staleness propagation. Set to `"off"` to disable.
/// Anything else (including unset) keeps propagation enabled.
pub const ENV_STALENESS: &str = "WAYLAND_STALENESS";

/// Returns `true` unless `WAYLAND_STALENESS` is set to (case-insensitive) `"off"`.
/// Mirrors the auto_memorize::consent_granted opt-out pattern.
pub fn staleness_enabled() -> bool {
    let enabled = std::env::var(ENV_STALENESS)
        .map(|v| v.to_lowercase() != "off")
        .unwrap_or(true);
    // #664: staleness propagation is ON by default; when disabled, stale facts
    // are never flagged. Log once so the disabled state is visible.
    if !enabled {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            tracing::info!(
                target: "wcore_memory::staleness",
                "{ENV_STALENESS}=off — stale-fact propagation is disabled this session"
            );
        });
    }
    enabled
}

/// Outcome of a single `propagate_staleness` walk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PropagationReport {
    /// Node ids that were newly inserted into `kg_node_staleness` by this call.
    pub marked_stale: Vec<i64>,
    /// Node ids that were already stale before this call and were left as-is.
    pub already_stale: Vec<i64>,
    /// Maximum BFS depth observed across the emitted frontier (0 if only root).
    pub depth_reached: u32,
}

// ---------------------------------------------------------------------------
// F23-03 — the AGE dimension of staleness
// ---------------------------------------------------------------------------
//
// The KG cascade above answers "was this node invalidated by something that
// changed?". Recall provenance also has to answer "how old is this, and is it
// still inside the retention bound the operator set?". Both are staleness, so
// both live here rather than in a second, competing module — a user shown two
// different staleness answers for one item would be right to distrust both.

use serde::{Deserialize, Serialize};

/// Age verdict for one recalled item.
///
/// The Fresh/Aging/Stale boundaries are reporting bands, not policy: nothing
/// is excluded because it is Stale. `Expired` is different in kind — it means
/// the item is past an operator-set retention bound and WAS excluded from the
/// prompt, which is why it carries the bound it violated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum StalenessVerdict {
    Fresh,
    Aging,
    Stale,
    Expired { max_age_secs: i64 },
}

impl StalenessVerdict {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Aging => "aging",
            Self::Stale => "stale",
            Self::Expired { .. } => "expired",
        }
    }

    /// True only for `Expired`. Retrieval excludes on this, and on nothing
    /// else, so a reporting band can never quietly become a filter.
    #[must_use]
    pub fn excludes_from_recall(self) -> bool {
        matches!(self, Self::Expired { .. })
    }
}

/// One day in seconds — the Fresh boundary.
pub const AGE_FRESH_SECS: i64 = 24 * 60 * 60;
/// Seven days in seconds — the Aging boundary.
pub const AGE_AGING_SECS: i64 = 7 * 24 * 60 * 60;

/// Classify an item's age, honouring an operator retention bound when one is
/// set. A bound of zero expires everything with a non-zero age, which is the
/// literal reading of "retain nothing" and is the behaviour an operator
/// setting zero is asking for.
#[must_use]
pub fn verdict_for_age(age_secs: i64, max_age_secs: Option<i64>) -> StalenessVerdict {
    if let Some(max) = max_age_secs {
        if age_secs > max {
            return StalenessVerdict::Expired { max_age_secs: max };
        }
    }
    if age_secs < AGE_FRESH_SECS {
        StalenessVerdict::Fresh
    } else if age_secs < AGE_AGING_SECS {
        StalenessVerdict::Aging
    } else {
        StalenessVerdict::Stale
    }
}

/// Idempotently create the sibling staleness table. Safe to call on an
/// existing memory DB. Caller must have already invoked
/// `crate::kg::init_kg(conn)` so the FK target table exists.
pub fn init_staleness(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS kg_node_staleness (
            node_id INTEGER PRIMARY KEY,
            marked_at INTEGER NOT NULL,
            propagated_to_neighbors_at INTEGER,
            FOREIGN KEY(node_id) REFERENCES kg_nodes(id)
        )",
        [],
    )?;
    Ok(())
}

/// Mark `node_id` stale (or re-mark — INSERT OR REPLACE resets the propagation
/// timestamp). `marked_at` is set to the current unix epoch seconds.
pub fn mark_stale(conn: &Connection, node_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO kg_node_staleness
            (node_id, marked_at, propagated_to_neighbors_at)
         VALUES (?1, CAST(strftime('%s','now') AS INTEGER), NULL)",
        params![node_id],
    )?;
    Ok(())
}

/// True iff `node_id` has a row in `kg_node_staleness`.
pub fn is_stale(conn: &Connection, node_id: i64) -> rusqlite::Result<bool> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM kg_node_staleness WHERE node_id = ?1",
            params![node_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(exists.is_some())
}

/// Remove the staleness row for `node_id`. Returns true if a row was deleted.
pub fn clear_stale(conn: &Connection, node_id: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM kg_node_staleness WHERE node_id = ?1",
        params![node_id],
    )?;
    Ok(n > 0)
}

/// BFS-cascade staleness from `root` outward.
///
/// Walks the KG via [`bfs_neighbors`] (undirected at the read layer; see
/// `kg::bfs::bfs_neighbors`). For every `(node_id, depth)` returned with
/// `depth > 0` (root excluded), we either:
///   * insert a fresh `kg_node_staleness` row via [`mark_stale`] and append
///     `node_id` to `marked_stale`, or
///   * if a row already exists, append `node_id` to `already_stale` and
///     leave it untouched.
///
/// After the walk, the root's `propagated_to_neighbors_at` column is set to
/// the current unix epoch seconds. If the root has no row in
/// `kg_node_staleness`, the timestamp update is a no-op (the root is not
/// implicitly marked stale by propagation).
pub fn propagate_staleness(
    conn: &Connection,
    root: i64,
    limit: BfsLimit,
) -> Result<PropagationReport> {
    let frontier = bfs_neighbors(conn, root, limit)?;

    let mut report = PropagationReport::default();
    for (node_id, depth) in &frontier {
        if *depth > report.depth_reached {
            report.depth_reached = *depth;
        }
        if *node_id == root || *depth == 0 {
            continue; // exclude the start node from cascade
        }
        let already = is_stale(conn, *node_id).map_err(MemoryError::Db)?;
        if already {
            report.already_stale.push(*node_id);
        } else {
            mark_stale(conn, *node_id).map_err(MemoryError::Db)?;
            report.marked_stale.push(*node_id);
        }
    }

    // Best-effort: stamp the root row's propagation timestamp. If the root
    // has no staleness row, UPDATE affects 0 rows — that's fine, the root
    // was never marked stale, propagation just emitted its neighbours.
    conn.execute(
        "UPDATE kg_node_staleness
            SET propagated_to_neighbors_at = CAST(strftime('%s','now') AS INTEGER)
          WHERE node_id = ?1",
        params![root],
    )
    .map_err(MemoryError::Db)?;

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kg::edges::{EdgeKind, upsert_edge};
    use crate::kg::nodes::{NodeKind, upsert_node};
    use crate::kg::schema as kg_schema;

    /// Open in-memory DB, init kg + staleness tables.
    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        kg_schema::init(&conn).unwrap();
        init_staleness(&conn).unwrap();
        conn
    }

    /// Build a star graph: a -> b, a -> c. Returns (conn, [a,b,c]).
    fn star_graph() -> (Connection, Vec<i64>) {
        let conn = fresh_conn();
        let ids: Vec<i64> = ["a", "b", "c"]
            .iter()
            .map(|n| upsert_node(&conn, n, &NodeKind::Entity).unwrap())
            .collect();
        upsert_edge(&conn, ids[0], ids[1], &EdgeKind::Mentions, 1.0).unwrap();
        upsert_edge(&conn, ids[0], ids[2], &EdgeKind::Mentions, 1.0).unwrap();
        (conn, ids)
    }

    #[test]
    fn init_staleness_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        kg_schema::init(&conn).unwrap();
        init_staleness(&conn).unwrap();
        // Second call MUST NOT error — CREATE TABLE IF NOT EXISTS.
        init_staleness(&conn).unwrap();
        // Table is present (sqlite_master query).
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='kg_node_staleness'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "kg_node_staleness must exist after init");
    }

    #[test]
    fn mark_stale_inserts_row() {
        let conn = fresh_conn();
        let n = upsert_node(&conn, "x", &NodeKind::Entity).unwrap();
        mark_stale(&conn, n).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM kg_node_staleness WHERE node_id = ?1",
                params![n],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        // marked_at is non-null and positive.
        let marked_at: i64 = conn
            .query_row(
                "SELECT marked_at FROM kg_node_staleness WHERE node_id = ?1",
                params![n],
                |r| r.get(0),
            )
            .unwrap();
        assert!(marked_at > 0, "marked_at must be a positive unix epoch");
    }

    #[test]
    fn is_stale_returns_true_after_mark() {
        let conn = fresh_conn();
        let n = upsert_node(&conn, "x", &NodeKind::Entity).unwrap();
        mark_stale(&conn, n).unwrap();
        assert!(is_stale(&conn, n).unwrap());
    }

    #[test]
    fn is_stale_returns_false_for_unmarked() {
        let conn = fresh_conn();
        let n = upsert_node(&conn, "x", &NodeKind::Entity).unwrap();
        assert!(!is_stale(&conn, n).unwrap());
        // Also false for a node id that doesn't exist at all.
        assert!(!is_stale(&conn, 99_999).unwrap());
    }

    #[test]
    fn clear_stale_removes_row() {
        let conn = fresh_conn();
        let n = upsert_node(&conn, "x", &NodeKind::Entity).unwrap();
        mark_stale(&conn, n).unwrap();
        assert!(is_stale(&conn, n).unwrap());
        let removed = clear_stale(&conn, n).unwrap();
        assert!(
            removed,
            "clear_stale must report true when a row is deleted"
        );
        assert!(!is_stale(&conn, n).unwrap());
        // Second clear is a no-op and reports false.
        let removed_again = clear_stale(&conn, n).unwrap();
        assert!(!removed_again);
    }

    #[test]
    fn propagate_staleness_marks_direct_neighbors() {
        let (conn, ids) = star_graph();
        let report = propagate_staleness(&conn, ids[0], BfsLimit::new(1, 100)).unwrap();
        // b, c marked. a (root) NOT marked.
        assert!(!is_stale(&conn, ids[0]).unwrap(), "root must NOT be marked");
        assert!(is_stale(&conn, ids[1]).unwrap(), "b must be marked");
        assert!(is_stale(&conn, ids[2]).unwrap(), "c must be marked");
        let marked: std::collections::HashSet<i64> = report.marked_stale.iter().copied().collect();
        assert_eq!(marked.len(), 2);
        assert!(marked.contains(&ids[1]));
        assert!(marked.contains(&ids[2]));
        assert!(report.already_stale.is_empty());
        assert_eq!(report.depth_reached, 1);
    }

    #[test]
    fn propagate_staleness_respects_max_depth() {
        // a -> b -> c -> d. Cascade depth 1 from a should only touch b.
        let conn = fresh_conn();
        let ids: Vec<i64> = ["a", "b", "c", "d"]
            .iter()
            .map(|n| upsert_node(&conn, n, &NodeKind::Entity).unwrap())
            .collect();
        upsert_edge(&conn, ids[0], ids[1], &EdgeKind::Mentions, 1.0).unwrap();
        upsert_edge(&conn, ids[1], ids[2], &EdgeKind::Mentions, 1.0).unwrap();
        upsert_edge(&conn, ids[2], ids[3], &EdgeKind::Mentions, 1.0).unwrap();

        let report = propagate_staleness(&conn, ids[0], BfsLimit::new(1, 100)).unwrap();
        assert!(is_stale(&conn, ids[1]).unwrap(), "b must be marked");
        assert!(
            !is_stale(&conn, ids[2]).unwrap(),
            "c must NOT be marked at depth 1"
        );
        assert!(
            !is_stale(&conn, ids[3]).unwrap(),
            "d must NOT be marked at depth 1"
        );
        assert_eq!(report.marked_stale, vec![ids[1]]);
        assert_eq!(report.depth_reached, 1);
    }

    #[test]
    fn propagate_staleness_skips_already_stale() {
        use std::collections::HashSet;
        let (conn, ids) = star_graph();
        // Pre-mark b stale.
        mark_stale(&conn, ids[1]).unwrap();
        let report = propagate_staleness(&conn, ids[0], BfsLimit::new(1, 100)).unwrap();
        // BFS edge enumeration order is not contracted by sqlite, so partition
        // assertions go through HashSet (mirrors the marks_direct_neighbors test).
        let already: HashSet<i64> = report.already_stale.iter().copied().collect();
        let marked: HashSet<i64> = report.marked_stale.iter().copied().collect();
        assert_eq!(already, HashSet::from([ids[1]]));
        assert_eq!(marked, HashSet::from([ids[2]]));
        // b is still stale.
        assert!(is_stale(&conn, ids[1]).unwrap());
    }

    #[test]
    fn propagate_staleness_updates_root_propagated_at() {
        let (conn, ids) = star_graph();
        // Pre-mark the root so it has a row whose propagation timestamp can be updated.
        mark_stale(&conn, ids[0]).unwrap();
        let before: Option<i64> = conn
            .query_row(
                "SELECT propagated_to_neighbors_at FROM kg_node_staleness WHERE node_id = ?1",
                params![ids[0]],
                |r| r.get(0),
            )
            .unwrap();
        assert!(before.is_none(), "fresh mark_stale leaves propagation NULL");
        let _ = propagate_staleness(&conn, ids[0], BfsLimit::new(1, 100)).unwrap();
        let after: Option<i64> = conn
            .query_row(
                "SELECT propagated_to_neighbors_at FROM kg_node_staleness WHERE node_id = ?1",
                params![ids[0]],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            after.is_some() && after.unwrap() > 0,
            "propagated_to_neighbors_at must be stamped after cascade"
        );
    }

    #[test]
    fn propagate_staleness_works_after_bootstrap_init_sequence() {
        // W4 v0.6.3 regression: AgentBootstrap now calls `init_kg` then
        // `init_staleness` on the same session-tier connection. Before the
        // fix, `init_staleness` was never called in production, so
        // `propagate_staleness` hit a missing `kg_node_staleness` table.
        // This test exercises exactly the bootstrap sequence and asserts
        // propagation succeeds against a real table.
        let conn = Connection::open_in_memory().unwrap();
        // Bootstrap order: init_kg FIRST (creates kg_nodes FK target), then
        // init_staleness.
        crate::kg::init_kg(&conn).unwrap();
        init_staleness(&conn).unwrap();

        let a = upsert_node(&conn, "a", &NodeKind::Entity).unwrap();
        let b = upsert_node(&conn, "b", &NodeKind::Entity).unwrap();
        upsert_edge(&conn, a, b, &EdgeKind::Mentions, 1.0).unwrap();

        // Must NOT error with "no such table: kg_node_staleness".
        let report = propagate_staleness(&conn, a, BfsLimit::new(1, 100))
            .expect("propagate_staleness must succeed after the bootstrap init sequence");
        assert_eq!(report.marked_stale, vec![b]);
        assert!(is_stale(&conn, b).unwrap());
    }

    #[test]
    fn propagate_staleness_empty_graph_returns_empty_report() {
        let conn = fresh_conn();
        let lonely = upsert_node(&conn, "lonely", &NodeKind::Entity).unwrap();
        let report = propagate_staleness(&conn, lonely, BfsLimit::new(3, 100)).unwrap();
        assert!(report.marked_stale.is_empty());
        assert!(report.already_stale.is_empty());
        assert_eq!(report.depth_reached, 0);
        assert!(!is_stale(&conn, lonely).unwrap());
    }

    // -- staleness_enabled env-var opt-out -----------------------------------
    //
    // `WAYLAND_STALENESS` is process-global; tests serialize via the
    // `#[serial(env)]` group and a local mutex (mirroring the
    // `auto_memorize` pattern).

    use serial_test::serial;
    use std::sync::{Mutex, OnceLock};

    fn staleness_env_lock() -> &'static Mutex<()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
    }

    fn restore_env(key: &str, saved: Option<String>) {
        // SAFETY: only called inside staleness_env_lock + #[serial(env)].
        unsafe {
            match saved {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    #[serial(env)]
    fn staleness_enabled_true_when_unset() {
        let _g = staleness_env_lock().lock().unwrap();
        let prior = std::env::var(ENV_STALENESS).ok();
        // SAFETY: serialized via env_lock + #[serial(env)].
        unsafe { std::env::remove_var(ENV_STALENESS) };
        assert!(staleness_enabled());
        restore_env(ENV_STALENESS, prior);
    }

    #[test]
    #[serial(env)]
    fn staleness_enabled_false_when_off() {
        let _g = staleness_env_lock().lock().unwrap();
        let prior = std::env::var(ENV_STALENESS).ok();
        // SAFETY: serialized via env_lock + #[serial(env)].
        unsafe { std::env::set_var(ENV_STALENESS, "off") };
        assert!(!staleness_enabled());
        // Case-insensitive.
        unsafe { std::env::set_var(ENV_STALENESS, "OFF") };
        assert!(!staleness_enabled());
        restore_env(ENV_STALENESS, prior);
    }

    #[test]
    #[serial(env)]
    fn staleness_enabled_true_for_other_values() {
        let _g = staleness_env_lock().lock().unwrap();
        let prior = std::env::var(ENV_STALENESS).ok();
        // SAFETY: serialized via env_lock + #[serial(env)].
        unsafe { std::env::set_var(ENV_STALENESS, "on") };
        assert!(staleness_enabled());
        unsafe { std::env::set_var(ENV_STALENESS, "1") };
        assert!(staleness_enabled());
        restore_env(ENV_STALENESS, prior);
    }

    // ----- F23-03 age dimension -----

    #[test]
    fn age_bands_are_reporting_only_and_never_exclude() {
        assert_eq!(verdict_for_age(0, None), StalenessVerdict::Fresh);
        assert_eq!(
            verdict_for_age(AGE_FRESH_SECS - 1, None),
            StalenessVerdict::Fresh
        );
        assert_eq!(verdict_for_age(AGE_FRESH_SECS, None), StalenessVerdict::Aging);
        assert_eq!(
            verdict_for_age(AGE_AGING_SECS - 1, None),
            StalenessVerdict::Aging
        );
        assert_eq!(verdict_for_age(AGE_AGING_SECS, None), StalenessVerdict::Stale);
        // A very old item is Stale, not Expired, when no bound is set: a
        // reporting band must never quietly become a filter.
        assert_eq!(verdict_for_age(10_000_000, None), StalenessVerdict::Stale);
        for v in [
            StalenessVerdict::Fresh,
            StalenessVerdict::Aging,
            StalenessVerdict::Stale,
        ] {
            assert!(!v.excludes_from_recall(), "{v:?} must not exclude");
        }
    }

    #[test]
    fn retention_bound_expires_and_carries_the_bound_it_violated() {
        let v = verdict_for_age(120, Some(60));
        assert_eq!(v, StalenessVerdict::Expired { max_age_secs: 60 });
        assert!(v.excludes_from_recall());
        assert_eq!(v.as_str(), "expired");
        // Exactly at the bound is still inside it.
        assert_eq!(verdict_for_age(60, Some(60)), StalenessVerdict::Fresh);
    }

    #[test]
    fn a_zero_bound_expires_anything_with_a_nonzero_age() {
        assert_eq!(verdict_for_age(0, Some(0)), StalenessVerdict::Fresh);
        assert_eq!(
            verdict_for_age(1, Some(0)),
            StalenessVerdict::Expired { max_age_secs: 0 }
        );
    }
}
