# F23A-C1-H3 / H4 — kill-mid-restore evidence

**Run against the integration train, not against my own fix.** Host `hetzner-dsm`, worktree
`/root/wayland-23a-c1-atomic`, commit `71203c5b`, whose `wcore-skills/src/govern.rs` is
`gh/plan/f20-unified-audit-repair@4a872413` verbatim. Driver `kill-23a-c1-atomic.sh`, harness
`crates/wcore-skills/examples/f23a_c1_kill_restore.rs`. Per-trial records:
`kill-atomic-trials.tsv`, `kill-legacy-trials.tsv` (35 lines each).

This matters for provenance. `23A-PHASE-VERDICT.md` §3 recorded the governed lane's kill
figures as **"audited but NOT re-executed"** — its hetzner worktree was gone and a grading
lane would not rebuild. **This is that independent re-execution**, with a harness written
before I had seen the train's implementation.

Both modes use the same grader, the same payload (320 files + `SKILL.md` + `manifest.json`,
~20 MB, retained by the real `GovernanceStore::revoke`), and the same deterministic kill
schedule. The subject is laid out **namespaced** — `skills/auto/auto-killtest/` — because that
is what `SkillDrafter` writes and because a flat skill would not exercise H4 at all.

## The result

```
===== F23A-C1-H3 KILL DISTRIBUTION: mode=legacy trials=35 =====     [pre-fix control]
window_ms=29
not_started=0  completed_before_kill=7  IN_WINDOW=28
of the IN_WINDOW kills:  ABSENT=1  WHOLE=0  PARTIAL=27
staging_left=0  staging_with_SKILL_md=0
retry_to_whole=28  retry_failed=0

===== F23A-C1-H3 KILL DISTRIBUTION: mode=atomic trials=35 =====     [the train's rollback]
window_ms=32
not_started=0  completed_before_kill=6  IN_WINDOW=29
of the IN_WINDOW kills:  ABSENT=28  WHOLE=1  PARTIAL=0
staging_left=29  staging_with_SKILL_md=28   (F23A-C1-H4 exposure)
retry_to_whole=28  retry_failed=1
```

Counted back off the committed per-trial files with unproxied `/usr/bin/grep -c`:

| file | `GRADE=PARTIAL` |
|---|---|
| `kill-legacy-trials.tsv` | **27** ← known-positive: grep and grader both work |
| `kill-atomic-trials.tsv` | **0** ← the claim |

**F23A-C1-H3 is closed in the train, independently re-measured.** 29 SIGKILLs landed inside
the train's restore; 28 left the skill wholly absent, **1 left it wholly restored**, none left
it partial. The same harness killing the pre-fix restore produced a partial skill 27 times out
of 28.

The single `WHOLE` in-window trial is worth more than the 28 `ABSENT`s. It is trial 6
(`27ms  GRADE=WHOLE BEGAN=1 DONE=0`): the kill landed *after* the `rename(2)` and *before* the
completion marker. **That is the other side of the disjunction actually observed**, not
inferred from POSIX. My first run of this harness (against my own implementation, before the
merge) scored 35/35 `ABSENT` and never reached that branch; I recorded then that it rested on
the `rename(2)` guarantee rather than on measurement. It no longer does.

## Why this is not a self-passing zero

`PARTIAL=0` is a known-negative, and LANE-BRIEF §3b-i is explicit that a broken instrument, a
wrong path or a search of the wrong tree yields one for free.

1. **The reddening control is the same binary, one flag apart.** `--mode legacy` reproduces the
   pre-fix restore; the grader calls it PARTIAL 27/28.
2. **`IN_WINDOW` is asserted, not assumed.** Kills landing before the restore starts or after
   it finishes are counted separately and **the driver exits 2 with `INVALID MEASUREMENT` if
   `IN_WINDOW` is zero**.
3. **The window is calibrated per mode.** 32 ms atomic, 29 ms legacy, measured on an
   uninterrupted run before any trial. A fixed sleep would have made one of the two vacuous.
4. **The grader is checked against a known-good state first.** The calibration restore must
   grade `WHOLE` or the driver exits 2 — a grader that miscalls a complete directory makes
   every `PARTIAL` below meaningless.
5. **Markers are fsync'd files, not stdout,** and **exit status is never read**. A killed
   process's status is one bit that says nothing about where in the restore it died.
6. **The driver's exit code does not encode the verdict** — 0 means "valid measurement", 2
   means "not". The counts are the result.

## F23A-C1-H4: what the same run exposed

`staging_with_SKILL_md=28`. **28 of 29 killed restores left a half-built tree containing a
`SKILL.md` inside the skills root** — at `<skills_root>/.promote-staging/<uuid>/`.

`promote::staging_root_for` takes the parent of the directory being written. For a flat
`<root>/<name>` skill that is beside the skills root, as `govern.rs` intends and states. For
`skills/auto/auto-<sig>/` — the only layout this module exists to govern — it resolves *inside*
the tree `collect_skill_md` walks. `govern.rs` names the hazard in its own comment
("`collect_skill_md` does not skip dot-directories") and relies on a mitigation that does not
hold for the drafter's layout.

Measured directly, both cause and consequence, each with a live control in the same
invocation (`tests/govern_staging_discovery.rs`, red against the train at `d8008f65`):

```
the staging directory was discovered under its own name:
  [".promote-staging:0f8b-uuid-like", "auto:control-visible"]     <- control found too
F23A-C1-H4: a namespaced skill stages at /tmp/.tmp85OdXY/skills/.promote-staging,
  INSIDE the skills root the loader walks                          <- flat-skill control passed
```

Fixed by a name fence in `loader::collect_skill_md`. `2 passed; 0 failed; 0 ignored; 0
filtered out` after the fix; both tests red before it.

The fence is the fix rather than moving the staging directory because the location cannot be
guaranteed in general: `rename(2)` needs staging on the target's filesystem, and skills roots
nest arbitrarily through `--add-dir`, `$WAYLAND_HOME` and project roots. The cause test
therefore asserts the *current* location as a fact, so that if a later change genuinely moves
staging outside every skills root, that test fails and the fence can be revisited on evidence.

## What this does NOT establish — stated, not buried

- **Power loss is not tested, only process kill.** The train fsyncs the staging *directory*
  (`promote::sync_dir`) but not the files inside it, so after a power loss the renamed
  directory could hold zero-length files. A SIGKILL cannot show this — the page cache outlives
  the process, so an unsynced restore still grades `WHOLE`. **This is an open gap in the
  train's implementation that this harness structurally cannot measure.** My pre-merge branch
  had a per-file `fsync_tree`; I dropped it with the rest of my duplicate rather than
  re-litigate the train's code, and record it here instead.
- **`retry_failed=1` is the harness, not the product.** It is trial 6, the `WHOLE` one: the
  restore had already published and cleared its tombstone, so a second `rollback` correctly
  returns "no such revocation". The directory was already whole before the retry ran. The
  driver's retry step does not distinguish "already succeeded" from "needs retry".
- **The train's `rollback` does not clean up staging on a copy failure** (only on a rename
  failure), which is why `staging_left=29`. Litter, not corruption, and now unfindable by the
  loader — but it accumulates across a profile's life.
- **Linux only.** `rename(2)` on Windows goes through `MoveFileEx`; the argument holds on the
  same volume but is not measured here.

## In-CI regression cover

At `71203c5b`:

- `cargo test -p wcore-skills --test govern_staging_discovery` → `2 passed; 0 failed; 0
  ignored; 0 measured; 0 filtered out`. Both tests verified red at `d8008f65` (pre-fix), with
  their controls passing in the same run.
