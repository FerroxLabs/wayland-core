-- Schema v7 (#694) — mark whether `evolved_prompts.score` was ever measured.
--
-- The problem: `score REAL NOT NULL` forced a writer with nothing to report to
-- supply a number anyway. The auto-skill drafter satisfied that constraint with
-- a hardcoded 0.7, which then flowed through `PromptStore::seed_pairs_for` into
-- live skill routing indistinguishably from a measured GEPA pass ratio.
--
-- The obvious fix is to rebuild the table with a nullable `score`. This
-- migration deliberately does NOT do that, because it would break rollback.
-- Downgrade is a supported route here, and every already-released <=v6 binary
-- runs a migration ladder of bare `installed < N` arms: opened against a v7
-- store it matches no arm, runs nothing, raises nothing, and proceeds. It then
-- reads `score` into `pub score: f64`, which cannot map SQL NULL — so a
-- nullable `score` would hand every rolled-back user a hard read failure on
-- every retired row. A guard added in v7 cannot help: it only exists in
-- binaries shipped after v7.
--
-- So v7 is purely additive and forward-readable instead:
--
--   * `score` stays `REAL NOT NULL`. Retired rows store 0.0, not NULL.
--   * A companion `score_measured` flag carries the real meaning. 0 = no
--     scorer has ever measured this row; 1 = one has.
--
-- An older reader ignores the unknown column entirely — every SELECT in
-- `PromptStore` names its columns explicitly (`SELECT id, skill_name, ...`)
-- rather than `SELECT *`, so positional `row.get(i)` indexes are bound to that
-- list and do not shift. It reads 0.0, which is a value it has always been able
-- to handle: `seed_pairs_for` scales 0.0 x 5 to 0 simulated successes and skips
-- the row, and `ORDER BY score DESC` puts it below every real measurement.
-- That is the same outcome the new reader produces, reached a different way.
--
-- The DEFAULT 1 is for old *writers*: a rolled-back binary inserting with the
-- v6 column list gets `score_measured = 1`, i.e. "this number means what it
-- always meant". A rolled-back binary still running the old drafter will write
-- fabricated 0.7 rows flagged as measured — that is that binary's own defect
-- reappearing while it is in charge, not one this migration introduces, and
-- re-upgrading does not retroactively retire them.

ALTER TABLE evolved_prompts ADD COLUMN score_measured INTEGER NOT NULL DEFAULT 1;

-- Retire the values the drafter fabricated. 0.0 is a placeholder chosen for
-- what old readers do with it, not a measurement; `score_measured = 0` is the
-- statement of record and every reader in this build keys off that flag.
UPDATE evolved_prompts
   SET score = 0.0,
       score_measured = 0
 WHERE scorer = 'auto_drafter';

-- `best_for_skill` now orders by (score_measured DESC, score DESC) so an
-- unmeasured row can never outrank a measured one, whatever placeholder it
-- stores. Keep that ordering index-covered.
CREATE INDEX IF NOT EXISTS idx_evolved_prompts_skill_scorer_measured
    ON evolved_prompts (skill_name, scorer, score_measured DESC, score DESC);

INSERT OR IGNORE INTO schema_version (version) VALUES (7);
