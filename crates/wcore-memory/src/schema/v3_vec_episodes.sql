-- M4.8 Memory v2 — schema v3 (sqlite-vec KNN scaffold)
--
-- Creates the `vec_episodes` virtual table backed by sqlite-vec's
-- `vec0` module. The extension is loaded process-wide via
-- `sqlite3_auto_extension` in `db.rs::register_sqlite_vec`, so by the
-- time this migration runs the `vec0` module is already registered
-- on every connection.
--
-- Dimensionality is HARDCODED at 384 to match `HashedEmbedder` (the
-- default) and the M4.7b bge-small stub. Backends with different dims
-- (OpenAI 1536, Voyage 1024) will trip a clean dim-mismatch error
-- when M5.7 wires the insert path; today the table simply isn't
-- written to under those backends.
--
-- Insert + retrieve wiring is M5.x scope. M4.8 establishes the
-- substrate (extension loaded + table exists) so M5 can wire vec0
-- KNN into `retrieve::search_basic` without revisiting the migration
-- runner. The legacy BLOB-encoded `embedding` column on `episodes`
-- remains the canonical storage; vec0 will mirror it for fast KNN.

CREATE VIRTUAL TABLE IF NOT EXISTS vec_episodes USING vec0(
    embedding float[384]
);
