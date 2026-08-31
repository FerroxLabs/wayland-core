---
issue: 409
repo: FerroxLabs/wayland-core
kind: defect
title: "Five tests are red on real Windows on lane/f13-windows, and the branch's Windows leg has never run to say so"
status: open
last_verified_commit: b45f08119
criteria:
  - id: c1
    text: "Each of the five rows is triaged to a cause and either fixed or given its own carrier issue; row 4 is checked against FerroxLabs/wayland#1213 before a new one is opened"
    state: not-met
    owner: core
    note: "Filed 2026-08-31 by lane/f13-w3-win-ci-exec while executing the #393 and wayland#1268 Windows suites for the first time. This ledger records the ROSTER, not any fix, and no row has been triaged. The five are, verbatim from the JUnit artifact of run 33352985296: (1) wcore-cli::permission_mode_matrix a_read_outside_the_workspace_escalates_in_every_mode_except_force -- 'a boundary read must gate even though Read is allow-listed', left Auto, right Gated('info'); (2) wcore-tools::grep_vcs_content_store_deny a_session_without_a_secret_deny_vfs_still_refuses_a_store -- the test's own CONTROL failed, 'Refused to search \\\\?\\C:\\Windows\\ServiceProfiles\\...\\Temp\\.tmpcEhD2n: path uses a Windows device / verbatim namespace'; (3) wcore-cli::bin/wayland-core tests::every_runtime_mcp_add_joins_the_catalog_refresh -- 'the McpManager construction walk did not find src/main.rs', while its own diagnostic prints that very path; (4) wcore-cli::bin/wayland-core tests::every_runtime_mcp_withdrawal_leaves_the_catalog_refresh -- names FerroxLabs/wayland#1213 c4 in its own assertion message, so it may belong there rather than here; (5) wcore-tools::issue_1248_conflict_notice_test in_memory_backend_conflict_still_renders_todays_wording -- 'path must be absolute: /w/f.txt', a POSIX path in the fixture. Each failed on all three attempts (failure + two rerunFailure), so retries = 2 did not launder any of them."
  - id: c2
    text: "Rows 1 and 2 are graded as product-or-fixture explicitly, with the evidence, because both currently read as product behaviour on Windows and would be real user-facing defects if they are"
    state: not-met
    owner: core
    note: "Not started. Row 1 says a read outside the workspace is Auto rather than Gated on Windows, which if it is product behaviour is an authority gap and not a test bug. Row 2 says the product's own device / verbatim-namespace path guard refuses an ordinary search when the temp directory resolves through \\\\?\\, which if it is product behaviour reaches any user whose TEMP resolves that way. Neither has been reproduced on a workstation yet, and neither should be dismissed as a fixture bug without that."
  - id: c3
    text: "A Windows verdict for this branch family stops depending on someone remembering the [ci-windows] marker: either the marker is required before integ/f13 merges, or the gate reports the branch's Windows leg as ABSENT rather than silently skipped"
    state: not-met
    owner: core
    note: "This is the cause-of-invisibility, and it is the reason a whole lane's Windows work was gradeable only by hand. ci.yml gates every Windows leg on a [ci-windows] commit-message marker for non-main, non-integ branches (ci.yml:239, 284, 921). A lane that never types it gets Windows rows reported as `skipped`, which in a checks list is indistinguishable from a leg that ran clean. ci.yml already carries the precedent for the fix: the `Assert this leg produced test signal (zero tests is not a pass)` step exists because a silent zero-test leg had previously been read as a pass, and the same reasoning applies one level up to a leg that never ran at all."
---

Filed while closing `FerroxLabs/wayland-core#393` and `FerroxLabs/wayland#1268`, both of which
were stuck for exactly one reason: their Windows tests had never been executed. Executing them
required a `[ci-windows]` push, and that push swept up five failures that have nothing to do
with either ticket.

## Why this is one issue and not five

They share a single cause of invisibility, and that cause is itself the defect worth fixing
(`c3`). Each individual red still needs its own disposition, which is what `c1` demands — this
issue must not be closed by fixing some of them.

## Attribution, measured

* RED: run 33352985296, `CI (Array)`, `ferrox-win-msvc`, commit `b45f08119` (tree byte-identical
  to `lane/f13-windows` @ `91940861e`). `16223 tests run: 16218 passed, 5 failed, 165 skipped`.
* CONTROL — `main` is clean: run 33313825017, `CI (Array)`, `main` @ `b26e4058d`,
  **15,647 tests, 0 failures**.
* **None of the five is a regression from `main`.** Each `(binary, test)` pair was compared
  against `main`'s own JUnit roster and **none of them exists there**. They are not "Windows
  broke" — they are five tests that arrived in the `integ/f13` window, have never once run on
  Windows, and are red the first time they do.

Not bisected below `integ/f13`: which merge introduced each row is unknown, and saying so is
cheaper than guessing.

Verdicts are taken from the run's `nextest-junit-Array` artifact, never from the job log. The
log is truncated — 12,618 `PASS [` lines against 16,223 tests run — so a name's absence there
means nothing.
