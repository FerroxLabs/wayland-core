-- M4.4 Memory v2 — schema v2 (evolved prompt store)
--
-- Captures winning prompt variants emitted by the GEPA evolution loop
-- (wcore-evolve). The store is intentionally orthogonal to the cognitive
-- partitions: it is operational memory for the learning loop itself,
-- consumed by future evolve runs to bootstrap the seed pool with past
-- winners and by the CLI to inspect convergence per skill.
--
-- Inserted into the global tier only (cross-run persistence), but the
-- schema is identical on every DB so relocation remains possible.

CREATE TABLE IF NOT EXISTS evolved_prompts (
    id              TEXT PRIMARY KEY,             -- uuid v4
    skill_name      TEXT NOT NULL,
    parent_id       TEXT,                          -- nullable; root variants have NULL
    prompt_body     TEXT NOT NULL,
    score           REAL NOT NULL,                 -- pass_ratio (bench) or DefaultScorer.combined
    scorer          TEXT NOT NULL,                 -- "bench" | "default"
    generation      INTEGER NOT NULL,              -- zero-based generation index
    created_at      INTEGER NOT NULL,              -- unix seconds
    metadata        TEXT,                          -- JSON blob for arbitrary extras
    UNIQUE (skill_name, generation, id)
);

CREATE INDEX IF NOT EXISTS idx_evolved_prompts_skill_gen
    ON evolved_prompts (skill_name, generation DESC, score DESC);

CREATE INDEX IF NOT EXISTS idx_evolved_prompts_skill_scorer_score
    ON evolved_prompts (skill_name, scorer, score DESC);
