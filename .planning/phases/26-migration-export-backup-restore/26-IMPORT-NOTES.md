# 26-IMPORT — running notes (lane `26-import`)

Base `861d1b1a`. Branch `lane/26-import`. Append-and-commit after every measurement;
never batch to the end (LANE-BRIEF §6b-i).

---

## t0 — what I inherited, before touching code

Read in full: `.planning/LANE-BRIEF.md`;
`26-PHASE-VERDICT.md` from `lane/grade-26` (HEAD `7f5cdbc3`, unmerged);
`COMPETITIVE-LEDGER.md:156` (`PORT-*`).

**The measurement I have to move** (grade-26, Criterion 2, panel 3-0 PARTIAL):
against a real peer home of 13 profiles / 542 skills / 13 personas, a real-binary
`migrate hermes --yes` left **4 files** in the Wayland home:

```
HOME/config.toml
HOME/migrate-quarantine/index.json
HOME/migrate-quarantine/payloads/mcp_server_ijfw-memory-dc8580321779/mcp-server.json
HOME/migrate-quarantine/payloads/skill_skills_probe-exec-a11e5e4fc1b6/SKILL.md
TOTAL: 4
```

Per-class, as graded: profiles YES (13, into `config.toml`); credentials YES (opt-in);
provenance YES (on contained items); non-exec MCP YES. persona / memory / settings /
assets / data-skill-bodies = **0 files each**. The containment positive control (Q1)
passed in the same probe, so those zeros are measurements and not a dead instrument.

**Open HIGH I must fix first — `F26-GRADE-H1`.** `crates/wcore-cli/src/migrate/mod.rs:621`
records `Outcome::Imported` for a `Classification::Data` skill that is never written;
`print_report` (`mod.rs:1026`) counts it in `imported=`, so the same run prints
`imported=14` *and* `542 skill directories — Detected but NOT imported`.
The guarding test `migrate_quarantine.rs:217`
`t2_skill_without_a_directive_is_data_and_needs_no_promotion` asserts only a
known-negative (`!store.contains(...)`) plus a sibling-quarantine count — **a no-op
import passes t2.** Nothing in that 21-test file asserts a data skill EXISTS in the
Wayland home after an apply.

**Companion MEDIUM I must close in the same change — `F26-GRADE-M1`.**
`peer_skill_roots()` scans `<home>/skills`, `<home>/plugin-skills`,
`<home>/profiles/*/skills`, `<home>/agents/*/skills`; measured coverage against the real
`~/.hermes` is **274 of 1909 SKILL.md — 14%**. Not a live hole today only because
unscanned skills are not imported either. It converts to a containment hole the moment
import widens. Grade lane's instruction: widen roots in the same change.

`F26-GRADE-M2` (MEDIUM): committed hermes fixture has 540 skill dirs and **zero
SKILL.md**, so at-scale conservation classifies 0 of 540. Relevant to me because it
means the committed corpus cannot prove any skill-body import I write.

### Ledger position (`COMPETITIVE-LEDGER.md:156`)

Both peers migrate from each other; Core is the only party with no reciprocal path.
Hermes ships `hermes_cli/migrate.py`, `codex_runtime_plugin_migration.py`,
`optional-skills/migration/openclaw-migration/scripts/openclaw_to_hermes.py`;
OpenClaw ships `docs/install/migrating-hermes.md`, `extensions/codex/src/migration/*`.
Grade lane's refresh: row is stale on "26-02/26-04 never started" (false at HEAD) but
its conclusion survives — apply imports nothing outside profiles + MCP.

---

## Plan of record for this lane (in priority order, from the dispatch)

1. **`F26-GRADE-H1` first.** Accounting must match the filesystem. Replace t2 with a
   test that **fails on a no-op**, three assertions incl. one the old test would have
   missed (§6b-ii).
2. **Widen intake** to the zero categories — persona, memory, settings, assets,
   data-skill bodies — deciding per class *importable* vs *deliberately not*, and
   saying which. Silence is not an answer; an argued refusal is.
3. **Widen `peer_skill_roots` in the same change.**
4. **Preserve provenance; keep quarantine inert.** That property holds today; do not
   break it.

### Proof obligations I have accepted

- Real binary on `hetzner-dsm`, realistic peer corpus, **files-written-per-category
  before AND after** as numbers.
- Quarantine inertness with a **positive control proving the harness can observe
  execution**.
- **If I touch the write path, re-run the 35-kill distribution** (pre-fix 5/35
  unrecovered, post-fix 0/35 via `atomic_write` at `quarantine.rs:369-374`). No
  assuming the old fix covers new code.
- **Sean's real peer homes are read-only.** `/Users/seandonahoe/dev/resources/{hermes-agent,openclaw}`
  and `~/.hermes` / `~/.openclaw`: copy, never write; prove byte-identical before/after.

### Instrument discipline I am bound by

- Assert **existence before absence**; a known-negative is self-passing on a dead grep.
- `/usr/bin/grep`, `/usr/bin/find`, `/usr/bin/git`, absolute-path `cargo` — `rtk`
  rewrites output and **strips `0 ignored` / `0 filtered out`**, the exact fields the
  anti-vacuity rule needs. Every number in my summary comes from an unproxied tool.
- Assert executed test counts explicitly (`N passed`), never exit status.

### Fences

No merge, no PR, no tag, no publish, no issue close, no `wcore-contract generate`, no
`.github/workflows/*`. Shared fence `crates/wcore-cli/src/{lib,main}.rs` — additive
contiguous only, line delta reported. Fence exposure reported vs `861d1b1a`.

---

## Log

- **t0** — brief + verdict + ledger read; this file written and committed. Nothing
  measured by me yet; every number above is **inherited from `lane/grade-26`** and is
  labelled as such. Next: read `migrate/mod.rs` and `migrate_quarantine.rs` at my base
  and re-derive H1 from source myself rather than trusting the citation.
