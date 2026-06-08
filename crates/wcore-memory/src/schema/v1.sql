-- W5 Memory v2 — schema v1
-- Applied uniformly to session, project, and global DBs. Tier-specific
-- partitions skip writes via gate + tier-resolver; the schema is identical
-- so any DB can host any partition's table if needed for relocation.

PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

-- Schema-version tracking (single row).
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL PRIMARY KEY
);
INSERT OR IGNORE INTO schema_version (version) VALUES (1);

-- ----------------------------------------------------------------------------
-- P2 Episodic
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS episodes (
    id              TEXT PRIMARY KEY,                       -- uuid v7
    tier            TEXT NOT NULL,                          -- session|project|global
    ts              INTEGER NOT NULL,                       -- unix epoch sec
    episode_type    TEXT NOT NULL,
    summary         TEXT NOT NULL,
    atomic_facts    TEXT NOT NULL DEFAULT '[]',             -- JSON array
    source          TEXT NOT NULL,                          -- main-agent | sub-agent:<n> | legacy | ...
    source_product  TEXT NOT NULL,                          -- wcore-agent | wcore-consolidate | ...
    session_id      TEXT,
    project_root    TEXT,
    decay_score     REAL NOT NULL DEFAULT 1.0,
    status          TEXT NOT NULL DEFAULT 'active',         -- active | archived
    embedding       BLOB                                    -- f32[384] mean-pooled
);
CREATE INDEX IF NOT EXISTS idx_episodes_tier_ts ON episodes (tier, ts);
CREATE INDEX IF NOT EXISTS idx_episodes_status ON episodes (status);
CREATE INDEX IF NOT EXISTS idx_episodes_session ON episodes (session_id);

-- FTS5 over episodes.summary + atomic_facts
CREATE VIRTUAL TABLE IF NOT EXISTS episodes_fts USING fts5(
    summary,
    atomic_facts,
    content='episodes',
    content_rowid='rowid'
);

-- Triggers keep FTS5 in sync with the content table.
CREATE TRIGGER IF NOT EXISTS episodes_ai AFTER INSERT ON episodes BEGIN
    INSERT INTO episodes_fts (rowid, summary, atomic_facts)
    VALUES (new.rowid, new.summary, new.atomic_facts);
END;
CREATE TRIGGER IF NOT EXISTS episodes_ad AFTER DELETE ON episodes BEGIN
    INSERT INTO episodes_fts (episodes_fts, rowid, summary, atomic_facts)
    VALUES ('delete', old.rowid, old.summary, old.atomic_facts);
END;
CREATE TRIGGER IF NOT EXISTS episodes_au AFTER UPDATE ON episodes BEGIN
    INSERT INTO episodes_fts (episodes_fts, rowid, summary, atomic_facts)
    VALUES ('delete', old.rowid, old.summary, old.atomic_facts);
    INSERT INTO episodes_fts (rowid, summary, atomic_facts)
    VALUES (new.rowid, new.summary, new.atomic_facts);
END;

-- ----------------------------------------------------------------------------
-- P3 Semantic
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS facts (
    id              TEXT PRIMARY KEY,
    tier            TEXT NOT NULL,
    ts              INTEGER NOT NULL,
    subject         TEXT NOT NULL,
    predicate       TEXT NOT NULL,
    object          TEXT NOT NULL,
    confidence      REAL NOT NULL DEFAULT 1.0,
    source_episode  TEXT,
    superseded_by   TEXT,
    embedding       BLOB
);
CREATE INDEX IF NOT EXISTS idx_facts_subject_predicate ON facts (subject, predicate);
CREATE INDEX IF NOT EXISTS idx_facts_object ON facts (object);
CREATE INDEX IF NOT EXISTS idx_facts_supersede ON facts (superseded_by);

CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
    triple,
    content=''
);

-- ----------------------------------------------------------------------------
-- P4 Procedural
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS procedures (
    id              TEXT PRIMARY KEY,
    tier            TEXT NOT NULL,
    ts              INTEGER NOT NULL,
    name            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    artifact        TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'staged',
    created_by      TEXT NOT NULL DEFAULT 'main-agent',
    thompson_alpha  REAL NOT NULL DEFAULT 1.0,
    thompson_beta   REAL NOT NULL DEFAULT 1.0,
    use_count       INTEGER NOT NULL DEFAULT 0,
    success_count   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_procedures_name_tier ON procedures (name, tier);
CREATE INDEX IF NOT EXISTS idx_procedures_status ON procedures (status);

-- ----------------------------------------------------------------------------
-- P5 Core user-model (global only; system-only write enforced in app layer)
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS user_model (
    key             TEXT PRIMARY KEY,
    value_json      TEXT NOT NULL,
    ts              INTEGER NOT NULL
);

-- ----------------------------------------------------------------------------
-- P1 Working spillover (session DB only; idle in project/global DBs)
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS p1_working (
    rowid           INTEGER PRIMARY KEY AUTOINCREMENT,
    ts              INTEGER NOT NULL,
    kind            TEXT NOT NULL,
    payload         BLOB NOT NULL
);

-- ----------------------------------------------------------------------------
-- CDC changelog (M9; written via writer in Group F)
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS cdc_log (
    seq             INTEGER PRIMARY KEY AUTOINCREMENT,
    ts              INTEGER NOT NULL,
    tier            TEXT NOT NULL,
    partition       TEXT NOT NULL,
    op              TEXT NOT NULL,        -- insert | update | supersede | status_transition | delta | spillover | decay_archive
    target_id       TEXT,
    source_product  TEXT,
    payload_json    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cdc_tier_seq ON cdc_log (tier, seq);

-- ----------------------------------------------------------------------------
-- Legacy import marker (idempotency for one-shot YAML→P2 importer)
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS legacy_import_marker (
    yaml_dir        TEXT PRIMARY KEY,
    imported_at     INTEGER NOT NULL,
    episode_count   INTEGER NOT NULL
);
