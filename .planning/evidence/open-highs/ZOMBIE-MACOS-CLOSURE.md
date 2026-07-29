# Zombie liveness — macOS binary-level proof, taken 2026-07-29

**HIGH CLOSED.** The one gap `ZOMBIE-PROBE.md` §6 named honestly ("macOS is the one platform not
behaviourally proven in Rust") is now measured on real Darwin hardware.

Host: this Mac. `uname -m` = `arm64`, macOS **26.3** (build `25D125`), `rustc 1.95.0`.
Run as uid **501** (`seandonahoe`), NOT root — required, see ARM D below.

## Route taken, and why it is permitted

LANE-BRIEF §0 **Darwin-behaviour exception** (added 2026-07-29): a single-crate, single-test run
IS permitted on the Mac when the thing under test is platform behaviour Darwin alone can
demonstrate. Command run was exactly `cargo test -p wcore-types --test real_zombie` — the verbatim
command `ZOMBIE-PROBE.md` §6 named as the one that would close this. **I qualify:** hetzner cannot
prove it, because no permitted build host executes Darwin code. Disclosed here and in the SUMMARY
per §0.

No workspace build, no clippy, no release build was run on the Mac.

## Instrument first (§3b-i)

Reused `.planning/evidence/zombie-probe/run-capture.sh` (no pipeline; prints `TRUE_RC=`), rather
than writing a fresh one. It had only ever been self-tested on Linux, so it was self-tested **on
this Mac** before being trusted — `.planning/evidence/open-highs/run-capture-selftest-macos.txt`:

```
SELFTEST 1 PASS  known-positive -> TRUE_RC=0
SELFTEST 2 PASS  known-negative -> TRUE_RC=7 (exact code, not collapsed to 1)
SELFTEST 3 PASS  old shape reported rc=0 for a command that exited 7 -- the repair is load-bearing
SELFTEST SUMMARY: 3 checks, 0 failed
```

Assertion 3 passing is the substantive part: **macOS exhibits the stolen-exit-status defect too**,
so the repair is load-bearing here and not a Linux-only formality.

`cargo` was invoked as `/Users/seandonahoe/.cargo/bin/cargo` (absolute, unproxied). Confirmation
that this worked: the result lines below still carry **`0 ignored`** and **`0 filtered out`**,
which `rtk`'s cargo rewrite strips (HANDOFF §2). Their presence is itself the proof the count was
read off an unproxied tool.

## GREEN — `.planning/evidence/open-highs/macos-real-zombie.log`, `TRUE_RC=0`

```
running 5 tests
test a_fully_reaped_process_reads_as_dead ... ok
test a_live_process_owned_by_another_user_reads_as_live_and_the_old_shape_called_it_dead ... ARM D reproduced on Darwin: uid=501 pid=1 new_probe=Live old_shape(kill(1,0))=alive:false errno=1 (EPERM=1)
ok
test a_live_process_reads_as_live ... ok
test a_real_unreaped_corpse_reads_as_dead_and_the_old_shape_would_have_missed_it ... independent oracle for pid 62651: ps state=Z
ok
test the_running_test_process_reads_as_live ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**Five tests, not four.** `real_zombie.rs:349` carries `#[cfg(target_os = "macos")]` ARM D, which
**had never executed on any host** — Linux and Windows both compile it out. So this run is the
first execution anywhere of the assertion covering the macOS-specific direction.

Two things this establishes that the C measurement could not:

1. **ARM D reproduced in Rust**, verbatim: `uid=501 pid=1 new_probe=Live
   old_shape(kill(1,0))=alive:false errno=1 (EPERM=1)`. The old `kill(pid,0)==0` shape called
   **pid 1, launchd, unambiguously running**, DEAD. That is the false-clean direction — the one
   that makes an orphan reaper believe it has nothing to clean up. The repaired probe reads `Live`.
2. **A real corpse reads `Dead` on Darwin**, corroborated by an oracle independent of the code
   under test: `ps state=Z`.

### The degraded-arm concern, excluded by measurement not by argument

`process_liveness.rs:296` degrades to `Indeterminate` if Apple ever moves the `kinfo_proc` fields,
and `Indeterminate` renders as *alive*. So a pass on a degraded arm was a live possibility worth
excluding — the ABI offsets (`P_STAT_OFFSET=36`, `P_PID_OFFSET=40`, `sizeof=648`) were measured on
an **earlier** macOS than the 26.3 here.

It is excluded: the corpse assertion requires exactly `ProcessLiveness::Dead`, and a degraded arm
returns `Indeterminate`, which fails that assertion. The corpse read `Dead`, so the `p_pid`
readback matched and **the hardcoded offsets still hold on macOS 26.3 / arm64**. Recorded because
it is a new datapoint, not a restatement.

## NEGATIVE CONTROL — proven to redden, one variable

Required by the lane brief: a fix with no failing counterpart is not proven. Mutation is the same
shape the prior lane used on Linux — `process_liveness.rs:300`, the macOS corpse test, forced to
never fire (the old shape: nothing is ever a corpse):

```rust
-        if buffer[P_STAT_OFFSET] as i8 == SZOMB {
+        if false {
```

`.planning/evidence/open-highs/macos-real-zombie-MUTATED.log`:

```
test a_real_unreaped_corpse_reads_as_dead_and_the_old_shape_would_have_missed_it ... FAILED
assertion `left == right` failed: an unreaped corpse (pid 65463) must read as Dead, not Live.
  Oracle said: ps state=Z
  left: Live
 right: Dead

test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

**Exactly one test reddens, and it is the corpse test.** The other four — including both
positive-direction tests and ARM D — stayed green. That second half matters as much as the first:
it shows the macOS arm is **not** universal denial. A probe that answered `Dead` to everything
would have reddened the positive tests instead.

Mutation reverted; `git diff -- crates/` empty; re-run green at
`.planning/evidence/open-highs/macos-real-zombie-REVERTED.log`
(`5 passed; 0 failed; 0 ignored; 0 filtered out`).

## The production sites do call the proven helper

A working probe nothing calls closes nothing, so this was checked rather than assumed. Unproxied
`/usr/bin/grep`, quoted globs, with a known-positive control in the same sweep (`use wcore_types`
→ **632** hits, instrument alive). Searched the **concept** — `process_liveness::`,
`process_is_alive`, `process_liveness(` — not one keyword.

All four production sites named in `ZOMBIE-PROBE.md` §1b delegate to the shared helper:

| site | delegates at |
|---|---|
| `wcore-gateway/src/pidlock.rs` `process_is_alive` | `:315` → `wcore_types::process_liveness::process_is_alive` |
| `wcore-cli/src/cron.rs` `process_is_alive` | `:1161` → same |
| `wcore-exec-backend/src/backends/local.rs` | `:386` → same |
| `wcore-browser/src/supervisor.rs` | `:484` → same |

**Wider than the finding said, and worth recording:** the sweep found **four further production
consumers** that reach the same helper transitively and were never named — `wcore-cli/src/gateway.rs:404`,
`wcore-cli/src/cron.rs:924`, `wcore-cli/src/channel.rs:217` and `:469`, plus
`wcore-cli/src/crash_sentinel.rs:100` and `wcore-cli/src/backup/journal.rs:196` (both of which take
the probe as an injected function). These are covered by the same repair — they are *more* closure,
not a new gap — but the blast radius of the original defect was larger than "4 production sites".

## Verdict

**CLOSED.** macOS binary-level proof taken, against a real corpse, with a one-variable negative
control proven to redden and the positive direction proven not to. All three platforms
(Linux, Windows, macOS) are now behaviourally proven in Rust on real hardware. No residual.
