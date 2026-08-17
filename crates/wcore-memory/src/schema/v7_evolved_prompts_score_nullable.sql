-- Schema v7 (#694) — `evolved_prompts.score` becomes nullable.
--
-- The column was `REAL NOT NULL`, so a writer with nothing to report still had
-- to supply a number. The auto-skill drafter satisfied that constraint with a
-- hardcoded 0.7, which then flowed through `PromptStore::seed_pairs_for` into
-- live skill routing indistinguishably from a measured GEPA pass ratio.
--
-- After this migration NULL means "no scorer has ever measured this row" and a
-- number always means one has. NULL sorts below every value under
-- `ORDER BY score DESC`, so an unmeasured row can never outrank a measured one.
--
-- SQLite cannot drop NOT NULL in place, so the table is rebuilt. `DROP TABLE`
-- takes the old indexes with it; both are recreated below.

CREATE TABLE evolved_prompts_v7 (
    id              TEXT PRIMARY KEY,             -- uuid v4
    skill_name      TEXT NOT NULL,
    parent_id       TEXT,                          -- nullable; root variants have NULL
    prompt_body     TEXT NOT NULL,
    score           REAL,                          -- NULL = never measured
    scorer          TEXT NOT NULL,                 -- "bench" | "default" | "auto_drafter"
    generation      INTEGER NOT NULL,              -- zero-based generation index
    created_at      INTEGER NOT NULL,              -- unix seconds
    metadata        TEXT,                          -- JSON blob for arbitrary extras
    UNIQUE (skill_name, generation, id)
);

-- Rows written by the drafter carry the fabricated constant rather than a
-- measurement, so the value is retired instead of migrated. Every other
-- scorer's rows are copied verbatim.
INSERT INTO evolved_prompts_v7
    (id, skill_name, parent_id, prompt_body, score, scorer, generation, created_at, metadata)
SELECT id, skill_name, parent_id, prompt_body,
       CASE WHEN scorer = 'auto_drafter' THEN NULL ELSE score END,
       scorer, generation, created_at, metadata
FROM evolved_prompts;

DROP TABLE evolved_prompts;
ALTER TABLE evolved_prompts_v7 RENAME TO evolved_prompts;

CREATE INDEX IF NOT EXISTS idx_evolved_prompts_skill_gen
    ON evolved_prompts (skill_name, generation DESC, score DESC);

CREATE INDEX IF NOT EXISTS idx_evolved_prompts_skill_scorer_score
    ON evolved_prompts (skill_name, scorer, score DESC);

INSERT OR IGNORE INTO schema_version (version) VALUES (7);
