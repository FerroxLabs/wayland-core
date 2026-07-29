# F23A-C1-H3 — kill-mid-restore evidence

Host `hetzner-dsm`, worktree `/root/wayland-23a-c1-atomic`, commit
`ccad2c9a5982c5937085101c666103e2f4a94c85`. Driver
`kill-23a-c1-atomic.sh`, harness `crates/wcore-skills/examples/f23a_c1_kill_restore.rs`.
Both modes run the **same** grader, the **same** payload (320 files + `SKILL.md` +
`manifest.json`, ~20 MB, retained by the real `GovernanceStore::revoke`) and the **same**
deterministic kill schedule. Per-trial records: `kill-atomic-trials.tsv`,
`kill-legacy-trials.tsv` (35 lines each).

## The result

```
===== F23A-C1-H3 KILL DISTRIBUTION: mode=legacy trials=35 =====
window_ms=28
not_started=0  completed_before_kill=3  IN_WINDOW=32
of the IN_WINDOW kills:  ABSENT=0  WHOLE=0  PARTIAL=32
staging_left=0  retry_to_whole=32  retry_failed=0

===== F23A-C1-H3 KILL DISTRIBUTION: mode=atomic trials=35 =====
window_ms=81
not_started=0  completed_before_kill=0  IN_WINDOW=35
of the IN_WINDOW kills:  ABSENT=35  WHOLE=0  PARTIAL=0
staging_left=35  retry_to_whole=35  retry_failed=0
```

Counted back off the committed per-trial files with `/usr/bin/grep -c`, unproxied:

| file | `GRADE=PARTIAL` |
|---|---|
| `kill-legacy-trials.tsv` | **32** ← known-positive: the grep and the grader both work |
| `kill-atomic-trials.tsv` | **0** ← the claim |

**Every one of 35 SIGKILLs landing inside an atomic restore left the skill wholly absent.
The identical harness, killing the pre-fix restore, produced a partial skill 32 times out of
32.** That is the whole finding.

## Why this is not a self-passing zero

`PARTIAL=0` is a known-negative, and LANE-BRIEF §3b-i is explicit that a broken instrument,
a wrong path, an unquoted glob or a search of the wrong tree all yield one for free. So:

1. **The reddening control is the same binary, one flag apart.** `--mode legacy` reproduces
   the pre-fix restore (`create_dir_all` on the live directory, then per-file copy) and the
   grader calls it PARTIAL 32/32. A grader that could not see a partial state would have
   reported 0 there too.
2. **`IN_WINDOW` is asserted, not assumed.** A kill before the restore starts, or after it
   finishes, tells you nothing about atomicity. The driver counts those separately
   (`not_started` / `completed_before_kill`) and **exits 2 with `INVALID MEASUREMENT` if
   `IN_WINDOW` is zero**. Atomic scored 35/35 in-window; legacy 32/35.
3. **The window is calibrated per mode, not guessed.** An uninterrupted restore is timed
   first (81 ms atomic, 28 ms legacy) and the 35 kill offsets are spread across it. A fixed
   sleep would have put every legacy kill past the 28 ms mark and produced a vacuous run.
4. **The grader is checked against a known-good state before any trial.** The calibration
   restore must grade `WHOLE`; if it does not the driver exits 2, because a grader that
   miscalls a complete directory makes every PARTIAL below meaningless.
5. **The markers are files, fsync'd, not stdout.** `marks/BEGIN` and `marks/DONE` are
   syscalls. A SIGKILL loses a buffered `println!`; it cannot lose an fsync'd file. **Exit
   status is never read** — a killed process's status is one bit that says nothing about
   where in the restore it died (LANE-BRIEF §3b-ii).
6. **The driver's own exit code does not encode the verdict.** It exits 0 for "the
   measurement is valid" and 2 for "it is not". The counts are the result. A driver that
   exited 0 on `PARTIAL=0` would pass on a broken pipe.

## What this does NOT establish — stated, not buried

- **The `WHOLE`-after-kill branch was never observed.** All 35 atomic kills landed before the
  `rename(2)`, so all 35 graded `ABSENT`. The post-rename window is a single syscall wide and
  is not reachable by a `sleep`-scheduled kill. The claim proven is *"never PARTIAL, and 35/35
  land on the wholly-old side"*; the wholly-new side rests on `rename(2)` being atomic, which
  is a POSIX guarantee, not something this harness measured.
- **Power loss is not tested — only process kill.** `fsync_tree`/`fsync_dir` exist for the
  power-loss case and a SIGKILL cannot exercise them: the page cache outlives the process, so
  an unsynced restore would still grade `WHOLE`. That part of the fix is argued, not measured,
  and `govern.rs` says so in the `fsync_tree` doc comment rather than letting this file imply
  otherwise.
- **`retry_to_whole=32` for legacy overstates the pre-fix product.** The harness's legacy path
  copies over whatever it finds, so it appears recoverable. The *real* pre-fix
  `GovernanceStore::rollback` would refuse a second attempt with `RestoreTargetOccupied`,
  because the partial directory occupies the target — leaving the user stuck with a broken
  skill and no supported way back. The harness flatters the defect here; the product was worse.
- **Linux only.** Run on `hetzner-dsm`. `rename(2)` semantics on Windows go through
  `MoveFileEx`; the atomicity argument holds on the same volume but is not measured here.

## Leftover staging directories: expected, and separately covered

`staging_left=35` — every killed atomic restore leaves `.wl-rollback-<id>.partial` in the
skills root. That is the design, not a defect, and it is covered twice:

- `a_staging_directory_is_never_discovered_as_a_skill` (in `govern_revoke_rollback.rs`) proves
  the loader skips the prefix, **with a live control skill in the same invocation** so the
  negative is not passing on a loader that loaded nothing. Removing the skip turns that test
  red (verified: `test result: FAILED. 17 passed; 1 failed; 0 ignored; 0 filtered out`).
- `retry_to_whole=35` proves the retry path clears the stale staging directory and completes.

## In-CI regression cover

`cargo test -p wcore-skills --test govern_revoke_rollback` →
`test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.

Both new mechanisms were shown to fail with the fix reverted, at the same commit:

| control | mutation | result |
|---|---|---|
| A | `rollback()` restored to the pre-fix bare `copy_tree` into `source_dir` | `FAILED. 17 passed; 1 failed; 0 ignored; 0 filtered out` — `a_restore_that_fails_part_way_leaves_no_directory_at_all` |
| B | loader's `ROLLBACK_STAGING_PREFIX` skip deleted | `FAILED. 17 passed; 1 failed; 0 ignored; 0 filtered out` — `a_staging_directory_is_never_discovered_as_a_skill` |

Both mutations were reverted from a backup copy and `git diff --stat` came back empty.
