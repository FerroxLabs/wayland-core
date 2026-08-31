---
issue: 393
repo: FerroxLabs/wayland-core
kind: defect
title: "Windows: a quarantine git abort kills the leaf and leaves its descendants running (split from #379)"
status: open
last_verified_commit: b45f08119
criteria:
  - id: c1
    text: "On Windows, both quarantine abort paths terminate the child's descendants, not the direct process alone"
    state: met
    evidence: "test:crates/wcore-cli/tests/quarantine_process_tree_windows.rs::the_drain_grace_abort_takes_the_whole_process_tree_on_windows"
    owner: core
    note: "MET -- MEASURED ON REAL WINDOWS, the first execution these tests have ever had. GREEN ARM: run https://github.com/FerroxLabs/wayland-core/actions/runs/33352985296, job `CI (Array)`, self-hosted runner `ferrox-win-msvc`, commit b45f08119 on lane/f13-w3-win-ci-exec, whose TREE IS BYTE-IDENTICAL to lane/f13-windows @ 91940861e -- that commit is empty and adds only the `[ci-windows]` `[ci-darwin]` CI opt-in markers, and `git diff` between the two prints nothing. WHY IT HAD NEVER RUN, which is the whole reason this criterion sat not-met against a finished fix: both test files are `#![cfg(windows)]`, so `cargo test -- --list` prints `0 tests` on Linux, and ci.yml gates every Windows leg on a `[ci-windows]` commit-message marker for non-main branches (ci.yml:239, 284, 921) which no lane commit carried. The whole `WindowsJobObject::attach` block could be deleted and every gate that had ever run on the branch stayed green. COLLECTION IS PROVED, NOT INFERRED: the job log is TRUNCATED (12,618 `PASS [` lines against 16,223 tests run), so a verdict read off the log would be unsound; it is taken from the `nextest-junit-Array` artifact, which carries all 16,223 `<testcase>` elements. Both arms are present BY NAME with NO `flaky-runs` attribute, so neither was retried into a pass: `wcore-cli::quarantine_process_tree_windows the_drain_grace_abort_takes_the_whole_process_tree_on_windows` PASS 5.678s and `the_wall_clock_abort_takes_the_whole_process_tree_on_windows` PASS 2.067s. Leg summary: `16223 tests run: 16218 passed (2 slow, 1 leaky), 5 failed, 165 skipped`. NONE of the five failures is a quarantine test; all five are unrelated, none exists on main, and they are filed as FerroxLabs/wayland-core#409 rather than left in a lane summary. BOTH PATHS ARE GRADED SEPARATELY, which is what c1 means by BOTH: drain-grace and wall-clock are separate `#[test]`s, so one cannot mask the other at a first failing assertion. That they DISCRIMINATE rather than pass for free is c2."
  - id: c2
    text: "A test on real Windows spawns a quarantine child that backgrounds a descendant, trips an abort path, and asserts the descendant is gone; shown RED against today's kill-the-leaf code"
    state: met
    evidence: "test:crates/wcore-cli/tests/quarantine_process_tree_windows.rs::the_wall_clock_abort_takes_the_whole_process_tree_on_windows"
    owner: core
    note: "MET -- RED ARM EXECUTED ON REAL WINDOWS. Run https://github.com/FerroxLabs/wayland-core/actions/runs/33353374757, job `CI (Array)`, self-hosted runner `SEANDESKTOP`, branch lane/f13-w3-win-ci-exec-red, commit 1014c1f82. THE ARMS DIFFER IN PRODUCT CODE ONLY, shown rather than asserted: `git diff --name-only` between the green and red branches prints `crates/wcore-cli/src/plugin/quarantine.rs` and nothing else, and `git diff --stat` restricted to crates/wcore-cli/tests, crates/wcore-tools/src and crates/wcore-config is EMPTY -- every test file is byte-identical. THE MUTATION IS THE PRE-FIX STATE, which is what c2's phrase `today's kill-the-leaf code` names: `cmd.creation_flags(QUARANTINE_SPAWN_FLAGS)` removed from `run_hardened`, so the child is spawned with `DETACHED_PROCESS` alone exactly as `harden_against_credential_prompt` set it before #393; and the `WindowsJobObject::attach` block removed, so nothing owns the tree and only `Child::kill` on the leaf remains. DROPPING ONLY THE ATTACH BLOCK WOULD NOT HAVE BEEN A RED ARM and this was checked before running it: `CREATE_SUSPENDED` would still be set, the child would stay frozen forever, no descendant pid would be recorded, and the test would die on its own fixture-vacuity guard instead of on the graded assertion. NOT VACUOUS BY COMPILATION, checked on hetzner before the push: `cargo check --target x86_64-pc-windows-gnu -p wcore-cli --tests` RC=0 and `cargo clippy --target x86_64-pc-windows-gnu -p wcore-cli --all-targets -- -D warnings` RC=0 (the Windows leg runs clippy BEFORE its test step, so a lint-red mutation would have measured nothing). RESULT: both graded arms RED, deterministically -- `failure` plus two `rerunFailure` in the JUnit, so `[profile.ci] retries = 2` did not launder it. THEY FAILED ON THE GRADED ASSERTION, NOT ON A GUARD. drain-grace, quarantine_process_tree_windows.rs:265:5 -- `assertion left == right failed: #393 c1: after the drain-grace abort the descendant (pid 11856) is Live. The leaf was reaped and its tree was left running.` wall-clock, :290:5 -- `#393 c1: after the wall-clock abort the descendant (pid 40200) is Live. abort said: git: git [treeprobe] timed out after 1500 ms`. Lines 265 and 290 are the `assert_eq!(state, ProcessLiveness::Dead)` calls, so everything before them passed first: the liveness CONTROL (a descendant outlives its git when nobody owns the tree), the `no descendant pid was recorded` vacuity guard, and the `err.contains(..)` assertion that pins WHICH abort path was reached. A descendant was spawned, it did outlive its git, the named abort path did fire, and the descendant was LIVE. SPECIFICITY -- the mutation reddens ONLY these two: in the same red run `a_successful_quarantine_run_leaves_its_tree_standing_on_windows` PASSES (correct: with no job there is nothing to kill the tree) and all three `quarantine_console_authority_windows` tests PASS (correct: `harden_against_credential_prompt` still sets `DETACHED_PROCESS`). The red arm's failure set is the green arm's five unrelated failures PLUS exactly these two."
  - id: c3
    text: "The change does not weaken #338: a test asserts the production build_git_command child still does not share the user's console after the fix"
    state: met
    evidence: "test:crates/wcore-cli/tests/quarantine_console_authority_windows.rs::quarantine_child_has_no_console_at_creation_on_windows"
    owner: core
    note: "MET ON REAL WINDOWS, same green run https://github.com/FerroxLabs/wayland-core/actions/runs/33352985296: `wcore-cli::quarantine_console_authority_windows quarantine_child_has_no_console_at_creation_on_windows` PASS 0.410s, with `a_quarantine_git_announces_itself_on_the_operators_console` PASS 0.096s and `the_notice_reaches_the_console_the_prompt_reaches` PASS 0.023s alongside it -- all three by name in the JUnit artifact, none carrying `flaky-runs`. IT GRADES THE COMPOSITION AT THE ONE PLACE THAT COMPOSITION EXISTS: `probe_through_production_spawn` drives a probe through `run_hardened` itself, where `QUARANTINE_SPAWN_FLAGS` OR-s `DETACHED_PROCESS` with `CREATE_SUSPENDED`, and asserts `SHARES_USER_CONSOLE_BEFORE=false` there. The trap this criterion exists for -- `CommandExt::creation_flags` is a SETTER, so calling it twice silently drops `DETACHED_PROCESS` and reopens #338 -- is therefore MEASURED on the box rather than argued, which is what the original note said it would take. Both readings of `the production build_git_command child` are covered: `probe_through_production_git` runs the command `build_git_command` builds, and `probe_through_production_spawn` runs that same command through the production spawn. NON-VACUITY IS BUILT INTO THE TEST AND IT IS LOAD-BEARING HERE: it asserts a NEGATIVE CONTROL first -- an UNHARDENED child MUST land on the driver's own console (`SHARES_USER_CONSOLE_BEFORE=true`, `CONOUT_BEFORE=OPEN`) -- and allocates a console if the driver has none, so a host that could not exhibit #338 at all fails the control instead of passing the grade; and it asserts a liveness arm (hardened `git --version` still runs), so a guard that refused everything could not read as a fix. The red arm confirms this criterion is independent of c1's: with the Job Object gone, this test still PASSES."
---

Split out of `FerroxLabs/wayland-core#379` on 2026-08-30 while its unix arm was being closed,
so that #379's wording -- "the whole session/process group it created" -- cannot be read as a
claim about a platform that creates neither.

Searched before filing: the open quarantine issues in this repo are #338, #369, #379, #380,
#385 and #389. #380 and #389 are the Windows arms of #338 and both are about console and
prompt authority, not teardown; a keyword search for "quarantine Windows job object" and for
"descendant process tree Windows" returned nothing, against a control search for "quarantine"
that returned all six. There was no carrier.

## Graded on real Windows, 2026-08-31

All three criteria were `not-met` at `91940861e` against a fix that was already complete. The
gap was never in the code: **the tests had never been executed on any host.** Both files are
`#![cfg(windows)]`, so they report `0 tests` on Linux, and `ci.yml` gates the Windows legs on a
`[ci-windows]` commit-message marker (ci.yml:239, 284, 921) that no lane commit carried. The
branch's only run that executed any job at all sat at `bb1e08dcd`, the commit *before* the fix,
with its Windows rows `skipped`.

Closed by executing them, on both arms, on the two self-hosted Windows runners:

| arm | run | runner | the two `#393` c1 tests |
|---|---|---|---|
| GREEN — the fix as written | [33352985296](https://github.com/FerroxLabs/wayland-core/actions/runs/33352985296) | `ferrox-win-msvc` | **PASS** (5.678s / 2.067s) |
| RED — the fix deleted | [33353374757](https://github.com/FerroxLabs/wayland-core/actions/runs/33353374757) | `SEANDESKTOP` | **FAIL** ×3 attempts, `descendant … is Live` |

The two arms' trees differ in exactly one file, `crates/wcore-cli/src/plugin/quarantine.rs`;
every test file is byte-identical between them, so the flip is attributable to product code and
to nothing else.

The arms ran on *different* runners, which would normally be a confound. It is not one here:
each graded test runs `assert_a_descendant_outlives_its_git_when_nobody_owns_the_tree` FIRST, so
a host that could not exhibit the fixture fails the control rather than the grade. That control
passed on both boxes.

## Not closed here

The green leg is still red overall — `16223 tests run … 5 failed`. **None of the five is a
quarantine test**, none of the five exists on `main` (checked against `main`'s own JUnit roster
at `b26e4058d`, 15,647 tests / **0 failures**), and all five are recorded with verbatim evidence
on **FerroxLabs/wayland-core#409** rather than absorbed into this note. Two of them read as real
Windows product behaviour and want triage before the release.

Closing this issue is Sean's; this ledger only records that all three criteria now hold.
