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
