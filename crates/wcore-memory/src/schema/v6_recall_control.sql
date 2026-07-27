-- W5 Memory v2 — schema v6 (F23-03 recall control)
--
-- Operator control over what memory may be recalled into a prompt. Two
-- tables, both keyed by the (partition, tier) grid cell the access gate
-- already governs, so a control can never name a cell the gate does not
-- recognise.
--
-- Neither table stores content. A privacy scope and a retention bound are
-- both statements ABOUT a cell, so they carry no episode text, no fact
-- triple and no prompt fragment.

-- Cells the operator has excluded from retrieval. Presence of a row IS the
-- exclusion; removing the scope is a DELETE. `reason` is operator-supplied
-- and exists so the exclusion can be reported rather than being silent.
CREATE TABLE IF NOT EXISTS memory_privacy_scope (
    partition       TEXT NOT NULL,
    tier            TEXT NOT NULL,
    excluded_at     INTEGER NOT NULL,
    reason          TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (partition, tier)
);

-- Maximum age, in seconds, that an item in a cell may reach before it is
-- reported as expired. Expiry excludes an item from retrieval; it does NOT
-- delete it, so an expired recall is reported as expired rather than
-- vanishing without trace.
CREATE TABLE IF NOT EXISTS memory_retention (
    partition       TEXT NOT NULL,
    tier            TEXT NOT NULL,
    max_age_secs    INTEGER NOT NULL,
    set_at          INTEGER NOT NULL,
    PRIMARY KEY (partition, tier)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (6);
