---
issue: 367
repo: FerroxLabs/wayland-core
kind: defect
title: "A never-merge red-arm instrument reached integ/f13: OwnedTree owns the leaf only again on Unix"
status: open
last_verified_commit: f0060a2e
criteria:
  - id: c1
    text: "The leaf-only instrument is out of the tree, and the guard's own regression test proves it"
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_owns_spawned_trees.rs::dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child"
    owner: core
    note: "The ten lines 8d6add71 added to crates/wcore-cli/tests/support/owned_tree.rs are removed on lane/f13-fin-handoff-audit. This criterion cites the regression test rather than the source because the test is what makes the removal checkable: with the instrument present it fails 3 of 3 in a workspace run AND 3 of 3 alone on an otherwise idle box -- verbatim, `the grandchild 312060 outlived the guard - killing the direct child does not reach a backgrounded descendant, which is exactly the surviving process TREE the ticket reported (FerroxLabs/wayland#1156)` at harness_owns_spawned_trees.rs:121:5 -- and with it removed the binary is 24/24 green. That IS the red arm; it did not have to be constructed, it was running in the integration branch."
  - id: c2
    text: "A red-arm instrument cannot be merged by accident: a test or CI grep fails when a shipped source file under crates/ contains black_box(true) or a RED ARM marker, with a positive control so the grep cannot pass by reading nothing"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c3
    text: "An integration run's failing-test SET is compared against a named allow-list, so `1 failed` can never be read as `the known 1 failed`"
    state: met
    evidence: "file:.github/scripts/grade-failing-set.sh"
    owner: core
    note: "CLOSED. `.github/scripts/grade-failing-set.sh` extracts the failing test IDENTITIES from every leg's JUnit (awk over the enclosing `<testcase>`, because `grep -c '<failure'` is a count with no name attached -- the state of affairs this gate ends, in a different file) and compares them against the SET named in `.config/known-failing-tests.txt`. It fails in BOTH directions: an UNEXPECTED failure (failed, not listed) and a STALE entry (listed, ran, PASSED). An entry absent from the report entirely is NOT an error -- a platform-gated test legitimately does not appear on every leg -- and it is reported so the reader can see the list is not being exercised. It NAMES the failing set on every run, including clean ones: a gate that speaks only when angry trains the reader to accept the number again next time. WIRED WHERE IT CANNOT BE MISSED: invoked BY PATH from `.github/scripts/assert-test-evidence.sh`, which both aggregate `report` jobs already run and `report` is a REQUIRED status context on main, so it sees every leg's uploaded JUnit in one pass; it fails closed on its own absence; and `report-gate-wiring.test.sh` asserts the script, the call, the fail-closed branch, the allowlist file and the lint.yml self-test line all still exist. `just failing-set-gate` runs it against a local `--profile ci` run, which is the form an integrator actually needs. WITHDRAWN AS THE TICKET SAID: no grep for `black_box(true)` or a `RED ARM` comment. ANTI-VACUITY, and the specific case first. (a) SYNTHETIC, 16 arms in `.github/scripts/tests/failing-set.test.sh`, run by lint.yml: allowlist {A} + failing {B} -- SAME COUNT, DIFFERENT TEST -- exits 1 and names B; allowlist {A} + failing {A} exits 0 on the identical fixture shape, so the first arm cannot be passing against a gate that simply always fails. Also graded: empty-allowlist fail-closed, one-known-plus-one-unexpected, STALE, not-collected, `<flakyFailure>` NOT counted (grade-retry-flakes.sh owns it), outer-attempt-*.xml excluded, malformed/unowned/unjustified/expired entries, and a compact single-line `<testcase><failure>` -- that last arm caught a real defect in the first reader, which keyed every case by the enclosing `<testsuite name=...>`. (b) ON REAL DATA, reproducing the incident. The ten lines of 8d6add71 were re-applied to `owned_tree.rs::snapshot` on hetzner-dsm and `cargo nextest run --profile ci -p wcore-cli --test harness_owns_spawned_trees` reproduced the original failure verbatim -- `the grandchild 2085252 outlived the guard - killing the direct child does not reach a backgrounded descendant, which is exactly the surviving process TREE the ticket reported (FerroxLabs/wayland#1156)` -- giving a REAL junit.xml with `tests=\"24\" failures=\"1\"`. Three arms over that one file: allowlist naming the ACTUAL failure -> exit 0; allowlist naming the standing known failure INSTEAD (`wcore-exec-backend::conformance_matrix::every_reference_backend_passes_the_same_harness_or_reports_why_it_did_not`, count 1 against count 1) -> EXIT 1, `UNEXPECTED wcore-cli::harness_owns_spawned_trees::dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child`; empty allowlist -> exit 1. That middle arm IS the incident: same count, different test, red. The instrument was then reverted, the file touched, and the same command re-run -> 24/24 pass, gate exit 0, `git status` clean. THE ALLOWLIST SHIPS EMPTY, which is the fail-closed state: every failure is unexpected until a run names it."
---

Found by the 0.13.12 handoff audit, not by the lane that produced it — which is
the whole point of the second criterion.

`8d6add71` says **"RED ARM (throwaway, never merge)"** in its own subject line
and **"Delete this branch after reading the run; it is an instrument, not a
fix"** in its body. `d03a6e14` merged it into `integ/f13`. Its content reduces
`OwnedTree::snapshot` to leaf-only ownership behind `std::hint::black_box(true)`
— chosen so the dead code below stays reachable to the compiler and `clippy -D
warnings` still passes, which is a reasonable thing for an instrument to do and
is exactly why the static gates could not see it.

The effect is `wayland#1156` reintroduced: the guard owns the direct child and
not the tree, so a backgrounded grandchild survives.

c1 is done. c2 is the reason this is filed as a defect rather than closed with
the revert: the tree was wrong for four commits, a full test run said so every
time, and the signal was a number nobody expanded.
