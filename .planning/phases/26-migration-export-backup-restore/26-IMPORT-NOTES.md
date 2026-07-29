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

- **t1 — H1 re-derived from source at my base, not inherited.** `mod.rs:614-658`:
  the loop over `surface.skills`. `Classification::Data` ⇒ `acct.record(&found.id,
  Outcome::Imported)` and **nothing else** — no `fs::` call, no store call, no
  destination. `Classification::Executable` ⇒ `store.admit(&req)`, which does write.
  `print_report` (`mod.rs:1026`) prints `imported=` straight off `acct.counts()`.
  Corroborated structurally: the complete production write set in `migrate/` is
  `patch_global_config` (`mod.rs:786`) + `QuarantineStore::admit`/`promote`
  (`quarantine.rs:392`/`470`). **H1 confirmed independently. The citation was right.**

  `t2` re-read at `migrate_quarantine.rs:214-247`. Its two assertions are
  `!store.contains("skill:skills/release-notes")` (known-negative) and
  `report.quarantined >= 2` (proves the *siblings* were contained). Neither touches
  the filesystem where a data skill would land. **A no-op import passes t2** —
  confirmed by reading, and I will confirm it by execution before I change it.

- **t2 — the real `~/.hermes`, measured read-only with `/usr/bin/find`.**
  Instrument alive first: `find ~/.hermes -name 'SKILL.md' -type f | wc -l` = **1909**
  (matches grade-26's independent 1909 exactly); negative control
  `-name 'NOTAFILE-zzz.md'` = **0**. Then the shape, which is the actual finding:

  | path shape (relative to home) | skill dir depth | count |
  |---|---|---|
  | `skills/<skill>/` | 2 | 22 |
  | `skills/<group>/<skill>/` | 3 | 157 |
  | `skills/…/<skill>/` | 4 | 21 |
  | `skills/…/<skill>/` | 5 | 66 |
  | `profiles/<p>/skills/<skill>/` | 4 | 252 |
  | `profiles/<p>/skills/<group>/<skill>/` | 5 | 960 |
  | `profiles/<p>/skills/…/<skill>/` | 6 | 252 |
  | `hermes-agent/…` (vendor checkout) | 3,4,5,9 | 176 |
  | `hermes-office/…` (vendor checkout) | 4 | 3 |

  **`scan_peer_skills` inspects only an IMMEDIATE child of a root** (`quarantine.rs:709`
  — `read_dir(root)` then `dir.join("SKILL.md")`), so it sees exactly the depth-2
  `skills/` row (22) and the depth-4 `profiles/*/skills/` row (252) = **274**. That
  reproduces grade-26's 274/1909 from the code path rather than from its report.

  **So M1 is not "missing roots" — it is a missing recursion.** Adding roots alone
  cannot reach the 960-item `profiles/<p>/skills/<group>/<skill>/` row. The fix is a
  bounded recursive walk for "a directory holding `SKILL.md`", from a widened root set.

  **`hermes-agent/` and `hermes-office/` are git checkouts of the Hermes product
  itself** — `~/.hermes/hermes-agent/.git` exists, alongside `cli.py`, `Dockerfile`,
  `CONTRIBUTING.md`. Their `optional-skills/` is the vendor's shipped catalog, not the
  user's setup. **Deliberate exclusion, argued:** those skills arrive with the peer
  product and are re-obtained by installing it; copying them into a Wayland home
  duplicates a vendor library the user never authored, and it is the single largest
  way to inflate an "imported" count without migrating anything of the user's.
  The accounting closes exactly: **1730 user-authored + 179 vendor = 1909.**

- **t3 — the other categories, in the same real home.**
  `SOUL.md` = **14** total: 13 under `profiles/<p>/`, 1 at the home root, 1 in the
  vendor checkout (`hermes-agent/docker/SOUL.md`) — 13 profile personas, matching
  grade-26's "13 personas". Root `HERMES.md` (3118 bytes) is the peer's
  project-context file, the analogue of Core's `WAYLAND.md`/`AGENTS.md`.
  `memories/*.md` = **0** (13 `memories/` directories exist and every one is empty).
  `profiles/*/skins/**` files = **0**.

  **Consequence for my proof: the real corpus cannot prove memory or asset import**,
  because it contains none. Those legs need an augmented corpus (a copy of the real
  home plus planted notes/assets), and every number from it will be labelled as coming
  from the augmented corpus, never presented as a real-home measurement.

- **t4 — Core-side destinations, so "import" means landing somewhere Core reads.**
  - skills → `wayland_config_dir()/skills` (`wcore_skills::paths::user_skills_dir`),
    which is **already the promote target** (`mod.rs:503`). Import and promote sharing
    one destination and one collision policy is the right shape.
  - memory → `wcore_memory::paths::memory_base_dir()` + `store::write_memory`.
  - persona → **Core has no persona surface.** `ProfileConfig` (`config.rs:900`) has
    no `system_prompt`; the only system-prompt field is the single global
    `default.system_prompt` (`config.rs:730`). 13 personas do not fit 1 field, and
    Core's own precedent treats a foreign system prompt as hostile: an untrusted
    project's `system_prompt` is run through `neutralize_trust_delimiters`
    (`config.rs:3998`, the GHSA-8r7g companion) so it cannot inject fake
    `<system-reminder>` trust delimiters. **A peer SOUL.md is the same class of
    content arriving by a different route.**
  - settings → OpenClaw's deferred set (`openclaw.rs:51`) is `agents, flows, identity,
    logs, memory, plugin-skills, plugins, tasks, tui, workspace`. Most have no Core
    semantics at all; a guessed mapping silently changes behaviour.

  Dispositions are being decided against this and will be written up with the argument
  for each, including the ones I decide NOT to import.
