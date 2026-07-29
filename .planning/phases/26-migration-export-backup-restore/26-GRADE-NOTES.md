# Phase 26 — grading notes (lane `grade-26`, live, append-only)

Purpose: produce `26-PHASE-VERDICT.md`, which has never been written. This file is
committed early and re-committed after every measurement, per LANE-BRIEF §6b-i.

Base: `861d1b1a`. Branch `lane/grade-26`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-grade-26`.

## The four criteria (verbatim, `.planning/ROADMAP.md:139-143`)

1. Hermes/OpenClaw discovery and dry-run are typed, deterministic, secret-redacted, and non-mutating.
2. Selective import/export preserves provenance and quarantines executable content.
3. Backup, restore, profile migration, and reciprocal portability survive interruption and restore exact pre-operation state on rollback.
4. Hostile fixture corpora prove conflict, secret-source remapping, isolation, and recovery semantics.

## What exists to grade (inventory, minute 5)

- `26-01`..`26-04` PLAN+SUMMARY, `26-01-BASELINE.md`, `26-04-CERTIFICATION.md`, `26-GAPS-SUMMARY.md`.
- No `26-PHASE-VERDICT.md`. Confirmed absent.
- `26-04-CERTIFICATION.md` self-grades: SC1 CLOSED, SC2 CLOSED, SC3 OPEN, SC4 CLOSED;
  F26-01/02/04/05 CLOSED, F26-03 OPEN (first clause: F23 envelope → portable session corpus, unstarted).
- `26-GAPS-SUMMARY.md` claims SC3's interruption clause was then attacked and produced
  **F26-GAPS-H1 HIGH** — `QuarantineStore::save_index` truncating `fs::write` on the LIVE index
  once per admitted item; kill mid-window ⇒ 143,360-byte partial JSON, `migrate quarantined`
  exit 1, re-run refuses all 440 items. Claims fixed + re-proved. **Must verify the fix in
  source AND that the re-proof is a kill distribution, not inspection.**

## Claims I must NOT inherit

- [ ] C1: macOS real-install run (7 real secrets, 0 hits, homes unmutated) — and the
      certification's own admission that it ran at ancestor `b671f9ad`, NOT the certified tree.
- [ ] C2: "0 secret hits" is a **known-negative assertion** (§3b-i) — needs a planted-secret
      positive control in the SAME invocation or it is self-passing.
- [ ] C3: F26-GAPS-H1 fix present in `crates/` at HEAD + kill distribution re-proof.
- [ ] C4: **The import half.** PORT-* ledger says both peers migrate from each other and Core
      has no reciprocal path; import untouched. SC2 says "Selective import/export". Determine
      whether `migrate` actually WRITES into a Wayland home, or only plans + quarantines.
- [ ] C5: F26-03 first clause (F23 envelope) — certification says unstarted; re-derive.
- [ ] C6: nextest `no-tests = "fail"` silently ignored; EMFILE mis-read as flakiness.
      Downgrade confidence wherever a green suite is the only evidence.

## Measurements (appended as taken)

(none yet — minute 12)

---

## M1 — the import half, measured in source (minute ~35)

**The ledger row is STALE, and in a direction that matters both ways.**
`.planning/intel/COMPETITIVE-LEDGER.md:156` (dated 2026-07-28) says "plans 26-02 and
26-04 were never started" and "Nothing has yet imported anything." Both are false at
HEAD: `26-02-SUMMARY.md`, `26-04-SUMMARY.md` and `26-04-CERTIFICATION.md` are all on
disk, and `migrate` has a real apply path that writes. So the ledger UNDER-states.

But it also over-states in the other direction when it is read as "import is now fine",
which the certification does. What `apply_plan` actually writes:

`crates/wcore-cli/src/migrate/mod.rs:786` — `patch_global_config` writes exactly two
things: `f.profiles` entries and `f.mcp.servers` entries (non-executable only).
`QuarantineStore::admit` writes executable skills / executable MCP defs into containment.
**That is the complete production write set.**

Instrument-alive check for the absence (§3b-i):
- `/usr/bin/grep -c "patch_global_config" crates/wcore-cli/src/migrate/mod.rs` → **2** (positive)
- `/usr/bin/grep -rn "fs::write|fs::copy|create_dir_all|File::create|fs::rename|write_all" crates/wcore-cli/src/migrate/`
  excluding `quarantine.rs` → 8 hits, **all 8 inside `#[cfg(test)]` fixture setup**
  (`hermes.rs:506`, `openclaw.rs:522,523,524,534,546,552,562`). Zero production writes.
- `/usr/bin/grep -c "Quarantined" .../migrate/mod.rs` → **10** (positive control) while
  `/usr/bin/grep -ci "export" .../migrate/mod.rs` → **0**. `migrate` has no export verb.

**`ImportSurface` (mod.rs:339) carries exactly ONE field: `skills`.** Persona and memory
are `Deferred` counters (mod.rs:142-145, 251-255) rendered under the literal banner
`"Detected but NOT imported in this pass (tracked for a follow-up)"` (mod.rs:942).
The module header (mod.rs:16) still says so in its own words.

### F26-GRADE-H1 (candidate HIGH) — a data skill is REPORTED imported and is never written

`mod.rs:621` — `Classification::Data` ⇒ `acct.record(&found.id, Outcome::Imported)` with
**no write of any kind**. `print_report` (mod.rs:1026) then prints
`Accounting: discovered=… imported=…` counting it. The same run's plan preview prints
`N skill directories` under "Detected but NOT imported". So the product tells the operator
both things about the same item, and the filesystem agrees with neither reading of
"imported" — nothing landed.

**The test that should have caught it is the self-passing shape §3b-i names.**
`migrate_quarantine.rs:217` `t2_..._is_data_and_needs_no_promotion` asserts
`!store.contains("skill:skills/release-notes")` — a KNOWN-NEGATIVE. Its stated
"positive half" (`report.quarantined >= 2`) proves the *siblings were quarantined*, not
that the data skill was *written anywhere*. No assertion in the file checks that a data
skill exists in the Wayland home after apply. A no-op import passes t2.

F26-02 names eight classes: persona, memory, skills, settings, assets, profiles,
credentials, provenance. Written at HEAD: **profiles, credentials (opt-in), provenance
(inside quarantine records)**, plus MCP servers. Not written: **persona, memory, skills,
settings, assets**. The certification's `F26-02: CLOSED` is not supportable on that count.

`profile export` / `profile import` (profile.rs:88-121) DO exist and are Wayland↔Wayland,
with `--select`/`--exclude` on export only. That is the export half and it is real.

## Still to establish
- [ ] Live proof on hetzner that a data skill/persona/memory does not land (decisive).
- [ ] F26-GAPS-H1 `save_index` fix present at HEAD + kill-distribution re-proof.
- [ ] SC1 macOS leg ran at ancestor `b671f9ad`, not the certified tree.
- [ ] SC4 corpora, positive-control shape.

---

## M2 — F26-GAPS-H1: fixed, and proven by a kill distribution (minute ~70)

Fix present at HEAD and unmodified since: `quarantine.rs:369-374` `save_index` calls
`wcore_config::atomic_write`, not `fs::write`. `/usr/bin/git log --oneline -1 --
crates/wcore-cli/src/migrate/quarantine.rs` → `a170ee24 fix(migrate): write the
quarantine index atomically (F26-GAPS-H1)`, and `git merge-base --is-ancestor a170ee24
HEAD` → **ancestor**. So the fix is at my base with no later edit to that file.

**Arithmetic re-derived from the logs myself, not from the summary:**

| log | source SHA | TRIAL lines | mid | corrupt idx | not-recovered | verdict line |
|---|---|---|---|---|---|---|
| `hermes.log` | `c23a08b9` (pre) | 21 | 17 | 1 | **1** | `PROOF: FAIL` |
| `openclaw.log` | `c23a08b9` (pre) | 21 | 18 | 4 | **4** | `PROOF: FAIL` |
| `hermes-fixed.log` | `a170ee24` (post) | 21 | 17 | 0 | 0 | `PROOF: PASS` |
| `openclaw-fixed.log` | `a170ee24` (post) | 21 | 18 | 0 | 0 | `PROOF: PASS` |

35 mid-apply kills each side. Pre-fix **5/35 unrecovered (14.3%)** — matches the brief's
"5 of 35". Post-fix **0/35**. This is a kill distribution, not an inspection.

**The gate can fail, and did.** The pre-fix logs carry `PROOF: FAIL` from the same
harness — the single most important fact about this instrument. Additional controls I
confirmed present in the logs rather than in the prose: `DETERMINISM-CONTROL: pass`,
`SENSITIVITY-CONTROL: pass (profile-drop, payload-byte, index-entry all detected)`
(three deformations that must each change the fingerprint), and `nokill.log` as the
classifier's negative control (`KILL-ENABLED: no`, `CLASS-MID: 0`, 5 post).
`SENTINEL-UNCHANGED: yes` on all four runs.

Residual, stated: `ORPHAN-PAYLOAD-TRIALS` is still **9 (hermes) / 11 (openclaw)**
post-fix — a payload directory can be written before its index entry exists. Every such
trial still recovers on re-drive, so it is not data loss; it is not "no orphans" either.

## M3 — SC1's real-credential leg: reproduced INDEPENDENTLY, and its missing control supplied

I re-ran the inherited macOS leg myself rather than reading it:
`sh scripts/portability-real-state-check.sh /Users/seandonahoe/f26-artifacts/b671/wayland-core`
→ `extracted 7 real secret values`, `hermes items=14`, `openclaw items=3`,
both `non-mutation confirmed`, **`0 hits`** each, `REAL-STATE CHECK PASSED`, rc=0.
Independent tree digests I took before and after my own runs are byte-identical:
hermes `d385dfa6…d75d` → `d385dfa6…d75d`; openclaw `b43014b2…c71e` → `b43014b2…c71e`.
**Sean's real homes were not mutated by me.**

**But "0 hits" is the self-passing shape (§3b-i), and the script had no planted-secret
positive control.** It guards its INPUTS well (empty extraction, zero items, unparseable
JSON, changed digest are each hard-red) but never demonstrates the MATCHER can fire.
Per §6b-ii I repaired the instrument rather than noting the defect: new
`scripts/portability-redaction-positive-control.sh`, three assertions —

```
EXTRACTED-TOTAL: 7
ACTUALLY-SEARCHED: 7
PLANT-LENGTH: 122   (value never printed)
hermes/openclaw A1 known-positive:        PASS  (matcher fires on a PLANTED real secret)
hermes/openclaw A2 known-negative:        PASS  (untouched document carries none)
hermes/openclaw A3 dead-instrument-misses: PASS  (the guarded failure mode reports 0)
POSITIVE-CONTROL: PASS
```

A3 is the assertion that makes the control mean anything. With it, the 0-hit result is
non-vacuous **for the first time**. The control plants into a COPY; no peer home is written.

**The limit the certification itself names, and it is real.** The macOS leg ran at
`b671f9ad`. `git log b671f9ad..HEAD -- crates/wcore-config/src/portability/
crates/wcore-cli/src/migrate/` returns **10 commits**, and `portability/redact.rs` —
the redaction module itself — is among the changed files. Three are redaction fixes:
`255d06ba` scrub credentials embedded in free-form plan details, `dd8579bc` make the
details scrub a type invariant, `f63da68a` narrow the credential name field. So the
real-credential evidence covers code that has since changed in exactly the module
responsible for the property. It is evidence FOR the property and it is **not evidence
at HEAD**. (The direction is favourable — the later commits harden — but that is an
argument, not a measurement, and the whole point of this leg was to stop arguing.)

---

## M4 — the import half, LIVE on the real binary (minute ~110). This is the decisive leg.

Built `wcore-cli` release at my HEAD on `hetzner-dsm` (`/root/wayland-grade26`,
`wayland-core 0.12.25`). Drove `migrate hermes --yes` against the COMMITTED hermes
fixture plus one sentinel of each content class F26-02 names.

**Two probe generations, and the first one's failure is the point.** v1 hand-authored a
`config.yaml`; the profile failed to parse, so its known-positive went RED and it
correctly declared its own five absences meaningless. v2 used the committed fixture but
wrote the exec directive as ` ```!shell `, which the block regex
`(?s)```!\s*\n` does not match — **my error, not the product's**. v3 uses the real
syntax from `tests/fixtures/portability-exec/skills/repo-status/SKILL.md`.

**v3 result — positive control PASS, so the zeros are real:**

```
Imported 13 profiles (0 skipped), 0 MCP servers, 0 credentials.
Accounting: discovered=16 imported=14 quarantined=2 excluded=0

=== FILES IN WAYLAND HOME ===
HOME/config.toml
HOME/migrate-quarantine/index.json
HOME/migrate-quarantine/payloads/mcp_server_ijfw-memory-dc8580321779/mcp-server.json
HOME/migrate-quarantine/payloads/skill_skills_probe-exec-a11e5e4fc1b6/SKILL.md
TOTAL: 4

Q1 CONTAINMENT: PASS — the executable skill IS in the home (quarantined)
Q2 IMPORT:      data-skill body files_containing=0  (product reported Outcome::Imported)
```

**Q1 is the positive control that makes Q2 mean something**: the probe demonstrably CAN
see a skill-shaped write, because it found the quarantined one. Q2's zero is therefore a
measurement, not a dead instrument.

v2 additionally measured the other four classes with the same control passing
(config.toml present with `[profiles.*]`):

| class | sentinel | files containing it in the Wayland home |
|---|---|---|
| persona (`SOUL.md`) | `SOUL-SENTINEL-9f31` | **0** |
| memory note | `MEMORY-SENTINEL-4b77` | **0** |
| settings | `SETTINGS-SENTINEL-c21a` | **0** |
| asset | `ASSET-SENTINEL-77de` | **0** |
| data skill body | `DATASKILL-SENTINEL-1a2b` | **0** |

**After importing a 13-profile, 542-skill, 13-persona peer home, the Wayland home
contains FOUR files.** One config.toml, one quarantine index, two inert payloads.

### F26-GRADE-H1 confirmed live — "Imported" is reported for content that is never written

The plan preview and the apply report contradict each other about the same items, in the
same run: `Detected but NOT imported in this pass: 542 skill directories` and
`Accounting: … imported=14`, where the 14 includes the data skill whose bytes are
nowhere on disk. Neither reading matches the filesystem.

### Containment is real but its coverage is narrow — measured on Sean's REAL install

`peer_skill_roots()` scans only `<home>/skills`, `<home>/plugin-skills`,
`<home>/profiles/*/skills` and `<home>/agents/*/skills`, and `scan_peer_skills()` only
looks at an IMMEDIATE child directory holding a `SKILL.md`. Against `~/.hermes`,
read-only, with a negative control on the matcher:

```
TOTAL SKILL.md in the real home : 1909
skills/<d>/SKILL.md             :   22
plugin-skills/<d>/SKILL.md      :    0
profiles/<p>/skills/<d>/SKILL.md:  252
agents/<a>/skills/<d>/SKILL.md  :    0
SCANNED: 274 / 1909      UNSCANNED: 1635
negative control (./zzz-not-a-root/*/SKILL.md): 0
```

(My first pass at this used zsh globs and returned all-zeros including for a directory I
could see had 122 — the instrument-death trap in §3b-i. Re-measured with `/usr/bin/find`
exact-shape patterns, which is what the numbers above are.)

**86% of the real install's skills are never classified.** This is NOT a live-execution
hole, and I want to be exact about why: an unscanned skill is not imported either, so
nothing executable escapes containment. The property holds — but it holds substantially
**because almost nothing is imported**, which is the vacuous-satisfaction shape the
ledger already flagged elsewhere in this program. A future lane that wires skill import
without widening `peer_skill_roots` at the same time turns this from a completeness gap
into a containment gap.

Also measured: the committed `tests/fixtures/portability/hermes` corpus has **540 skill
directories and ZERO `SKILL.md` files**, so `t13 conservation holds over both full
committed corpora` exercises skill classification on **0 of 540**. The real install does
use `SKILL.md` (1909), so this is a fixture-fidelity gap, not a product one.
