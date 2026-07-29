---
lane: 23a-c1-atomic
branch: lane/23a-c1-atomic
graded_against: 23A-PHASE-VERDICT.md (lane/grade-23a, 2026-07-29)
merge_base: 4a8724134f83f901971d1cecb8db510e9b114f42   # gh/plan/f20-unified-audit-repair
new_high: "F23A-C1-H4 — a killed rollback leaves a half-built tree with a SKILL.md inside the skills root, and the loader discovers it. Found in the train, fixed in this lane."
status: complete
---

# 23A-C1-ATOMIC — lane summary

## Headline

**I was dispatched to fix F23A-C1-H3 and close clauses (b) and (c). Two of those three were
already done in the integration train** — `lane/23a-c1-governed` landed at `4a872413` with an
atomic staged-`rename(2)` restore and `--skills-govern` / `--skills-revoke` /
`--skills-rollback` on the shipped binary. My independent implementations of both were
discarded at the merge.

What the lane produced instead:

1. **An independent re-execution of the kill proof** the verdict recorded as *"audited but NOT
   re-executed"* — with a harness written before I had seen the train's code. **0 partial
   states across 29 in-window SIGKILLs.**
2. **A new HIGH, `F23A-C1-H4`**, found by pointing that harness at the train: a killed restore
   leaves a half-built skill tree *inside the skills root*, and the loader discovers it.
   Measured, fixed, and regression-tested in this lane.
3. **`/skill govern | revoke <name> | rollback <id>`** — an in-session surface the train's
   CLI flags cannot provide. Complementary, not required.

## Per-clause grade after this lane

| Clause | Verdict at `861d1b1a` | After the train + this lane |
|---|---|---|
| (a) cannot execute before governed promotion | MET-WITH-STATED-EXCEPTIONS | **MET** — the train's `promote.rs` supplies the promotion the verdict said did not exist. Not re-graded by me. |
| (b) can be observed | PARTIAL | **MET** — train's `--skills-govern`; plus `/skill govern` in-session. Live-driven. |
| (c) can be revoked | PARTIAL | **MET** — train's `--skills-revoke`; plus `/skill revoke`. Live-driven end-to-end. |
| (d) can be rolled back | PARTIAL + F23A-C1-H3 | **MET** — H3 independently re-measured closed (0/29 partial); H4 found and fixed. |

**SC-1: MET, for the clauses I can speak to.** I did not re-grade clause (a) — I did not drive
the promotion path, and un-driven is un-driven.

## What I actually built

| File | Why |
|---|---|
| `crates/wcore-skills/src/loader.rs` | F23A-C1-H4 fix: `collect_skill_md` skips `.promote-staging`. |
| `crates/wcore-skills/src/promote.rs` | `STAGING` made `pub(crate)`; doc corrected to say the location is best-effort and the loader fence is the guarantee. |
| `crates/wcore-skills/tests/govern_staging_discovery.rs` | 2 tests: the consequence (discovery) and the cause (location). Both red against the train. |
| `crates/wcore-skills/examples/f23a_c1_kill_restore.rs` | The kill harness: `prepare` / `restore --mode atomic\|legacy` / `grade`. |
| `.../evidence/23A-C1-ATOMIC/kill-23a-c1-atomic.sh` | Kill-distribution driver with calibration, vacuity guard and in-window accounting. |
| `crates/wcore-agent/src/slash/skill.rs` | `/skill govern\|revoke\|rollback`, +418 lines incl. 6 tests. |

**Not touched:** `wcore-cli/src/lib.rs`, `wcore-cli/src/main.rs`. The shared fence is clean —
`git diff $BASE -- crates/wcore-cli/src/{lib,main}.rs` is empty. No contract corpus drift; I
never ran `wcore-contract generate`.

## The mid-restore kill proof

Full detail and caveats: `evidence/23A-C1-ATOMIC/23A-C1-ATOMIC-KILL-EVIDENCE.md`.

```
mode=legacy (pre-fix control)   IN_WINDOW=28   ABSENT=1   WHOLE=0   PARTIAL=27
mode=atomic (the train)         IN_WINDOW=29   ABSENT=28  WHOLE=1   PARTIAL=0
                                staging_left=29   staging_with_SKILL_md=28
```

Read back off the committed per-trial files with unproxied `/usr/bin/grep -c`: legacy **27**
`GRADE=PARTIAL`, atomic **0**.

The `WHOLE=1` is the most valuable single trial. It is a kill that landed *after* the
`rename(2)* and before the completion marker — **the wholly-new side of the disjunction,
observed rather than inferred**. My first run of this harness (against my own pre-merge fix)
scored 35/35 `ABSENT` and never reached that branch; I recorded then that the other side rested
on the POSIX guarantee. It no longer does.

Why `PARTIAL=0` is not a free zero: same binary one flag apart produces 27 partials; kills
outside the restore window are excluded rather than counted as passes and the driver exits 2 if
none land inside; the window is calibrated per mode; the grader must first grade a known-good
restore `WHOLE` or the run is voided; markers are fsync'd files, not stdout; exit status is
never read.

## F23A-C1-H4 — the new HIGH

`promote::staging_root_for` takes the parent of the directory being written. For a flat
`<root>/<name>` skill that is beside the skills root, which is what `govern.rs` intends and
says. For `skills/auto/auto-<sig>/` — **the only layout this module exists to govern** — it
resolves to `skills/.promote-staging`, inside the tree `collect_skill_md` walks. `govern.rs`
names this exact hazard in its own comment and relies on a mitigation that does not hold there.

Measured against the train at `d8008f65`, cause and consequence, each with a live control:

```
the staging directory was discovered under its own name:
  [".promote-staging:0f8b-uuid-like", "auto:control-visible"]   <- control also found
F23A-C1-H4: a namespaced skill stages at …/skills/.promote-staging   <- flat control passed
```

28 of 29 killed restores left such a tree. Fixed by a name fence in the loader — not by moving
the directory, because `rename(2)` needs staging on the target's filesystem and skills roots
nest arbitrarily via `--add-dir`, `$WAYLAND_HOME` and project roots. The cause test asserts the
*current* location as a fact, so if staging ever does move outside every skills root that test
fails and the fence can be revisited on evidence.

## Gates

At `a4b598d8`, on `hetzner-dsm`, all counts read back with `0 filtered out`:

| Gate | Result |
|---|---|
| `cargo test -p wcore-skills` (whole crate) | **693 passed, 0 failed**, 3 ignored — 623 lib + 20 integration binaries |
| `cargo test -p wcore-skills --test govern_staging_discovery` | `2 passed; 0 failed; 0 ignored; 0 filtered out` |
| `cargo test -p wcore-agent --lib slash::skill` | `14 passed; 0 failed; 0 ignored; 2176 filtered out` |
| `cargo clippy -p wcore-skills -p wcore-agent --all-targets` | clean (only the pre-existing `imap-proto` future-incompat note) |
| `cargo fmt --all` | clean |

**Controls run, each shown to redden:** reverting the atomic restore → the restore test fails
(`17 passed; 1 failed`); deleting the loader fence → the discovery test fails
(`17 passed; 1 failed`); removing my H4 fix → both new tests fail with their controls passing.

## Live evidence

`evidence/23A-C1-ATOMIC/23A-C1-ATOMIC-LIVE-EVIDENCE.md`. Full round-trip on the real
`wayland-core` binary, re-driven on the merged tree: `/skill list` → `/skill govern`
(known-negative first) → `/skill revoke auto:auto-livedemo` → directory gone → `/skill govern`
shows id/time/path/signature/retained-bytes → `/skill rollback <id>` → all three files back
including the nested `refs/` → journal shows both events, append-only → the draft is quarantined
again. The leftover `.promote-staging` root is confirmed empty and **not** discovered.

**The live drive caught what the tests could not:** `/skill revoke auto-livedemo` — the name on
disk and the name every unit test uses — returns "no skill named". The loader registers it as
`auto:auto-livedemo`. Unit tests build `SkillRef`s directly and never see the namespace prefix.
Not a product defect (the tool prints the namespaced name), but the suite was green while the
obvious human command did not work.

**Credential:** none. The engine will not boot without an API key, so the literal string
`placeholder-no-network-call-is-made` was passed; slash commands short-circuit before
`engine.run()`, and the transcript shows the process had no usable provider. `WAYLAND_HOME`
isolated the profile from `/root/.wayland/.env`, so the host's injected `ANTHROPIC_API_KEY`
was not in play (LANE-BRIEF §3b-ii).

## Open, and not closed by me

1. **The train does not fsync restored file contents before the rename** — only the staging
   directory (`promote::sync_dir`). After a power loss the renamed directory could hold
   zero-length files. A SIGKILL cannot show this (page cache outlives the process), so my
   harness structurally cannot measure it. My pre-merge branch had a per-file `fsync_tree`; I
   dropped it with the rest of the duplicate rather than re-litigate landed code. **Recommend
   a follow-up.**
2. **`rollback` does not clean up staging on a copy failure** (only on a rename failure), so
   staging trees accumulate over a profile's life. Now unfindable by the loader, so litter
   rather than corruption.
3. **Clause (a) not re-graded.** I did not drive the promotion path.
4. **Windows unmeasured.** `rename(2)` → `MoveFileEx`; the argument holds on one volume, the
   measurement is Linux-only.
5. **`wcore-agent --lib` has pre-existing flaky failures** in `engine::audit_2026_05_22_tests`,
   `session::tests` and `channel_lease` — 4 failed at base `861d1b1a` and 22 on my branch in
   one run, with **zero** in `slash::*` and no overlap with anything I touched. Different
   subsets each run; the known fd/inotify contention family. I did not chase it and I do not
   claim it clean.

## Instrument defects found and repaired in-lane (LANE-BRIEF §6b-ii)

- **The loader control fired on its first run** — `additional_skills_dirs` maps `--add-dir` to
  `<dir>/.wayland-core/skills`, so passing the skills directory made the loader return `[]` and
  every negative pass for free. Repaired, not noted-and-moved-on.
- **zsh ate `:c` as a parameter modifier.** `"$TRAIN:crates/…"` expands as `${TRAIN:c}` and
  silently reported `promote.rs ABSENT` for a file that was present. Caught only because the
  known-positive control in the same invocation errored loudly. **Always brace:
  `"${VAR}:path"`.** The same line also truncated a source file via `>` before the command
  failed — check for damage after any failed redirect.
