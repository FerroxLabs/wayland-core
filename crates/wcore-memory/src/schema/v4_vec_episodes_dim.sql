-- M5.7 Memory v2 — schema v4 (dim-aware vec_episodes registry)
--
-- Background. v3 (M4.8) created `vec_episodes` as `vec0(embedding
-- float[384])` — a single hardcoded-dim virtual table. That works for
-- the deterministic hashed/bge-local backends (both 384-dim) but
-- forbids mixing in OpenAI (1536) or Voyage (1024) backends without a
-- destructive migration.
--
-- v4 ships the *registry table* that records which per-dim `vec0`
-- virtual tables exist. The per-dim tables themselves are created
-- lazily by `db::ensure_vec_table_for_dim(dim)` on the first
-- `record_with_embedding` call that uses a new dim — sqlite-vec does
-- not allow `CREATE VIRTUAL TABLE` inside a transaction, and forcing
-- every release to pre-create 1536-dim + 1024-dim tables would mean
-- carrying empty 6 KiB / 4 KiB structures on every fresh db forever.
--
-- Forward-only: existing 384-dim `vec_episodes` (created by v3) keeps
-- working — `ensure_vec_table_for_dim(384)` simply records the entry
-- in the registry, since the table already exists.
--
-- Per-dim naming scheme: `vec_episodes_<dim>` — matches the call
-- pattern `ensure_vec_table_for_dim(embedder.dim())`. Operators can
-- inspect with `SELECT name FROM sqlite_master WHERE
-- name GLOB 'vec_episodes_*'`.

CREATE TABLE IF NOT EXISTS vec_episodes_registry (
    dim         INTEGER PRIMARY KEY,
    table_name  TEXT NOT NULL UNIQUE,
    created_at  INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

-- Seed: the legacy v3 table is treated as the canonical 384-dim
-- entry so callers don't double-create. The seed is INSERT OR IGNORE
-- because re-applying v4 (idempotency) must not error and because a
-- brand-new db without v3 will also fail without the IGNORE (no
-- vec_episodes table to point at).
INSERT OR IGNORE INTO vec_episodes_registry (dim, table_name)
    VALUES (384, 'vec_episodes');
