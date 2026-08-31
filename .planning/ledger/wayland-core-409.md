---
issue: 409
repo: FerroxLabs/wayland-core
kind: defect
title: "Five tests are red on real Windows on lane/f13-windows, none of them a regression from main"
status: open
last_verified_commit: a07bf29e5
criteria:
  - id: c1
    text: "`wcore-cli::permission_mode_matrix a_read_outside_the_workspace_escalates_in_every_mode_except_force` passes on real Windows, or its expectation is corrected against measured Windows behaviour with the reason recorded."
    state: not-met
    owner: core
    note: "Transcribed verbatim from the issue body on 2026-08-31. not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task: the gate reserves task for a credential, an account or a platform a human must obtain, and there is code behind this one."
  - id: c2
    text: "`wcore-tools::grep_vcs_content_store_deny a_session_without_a_secret_deny_vfs_still_refuses_a_store` passes on real Windows -- including its own CONTROL arm, which is what failed."
    state: not-met
    owner: core
    note: "Transcribed verbatim from the issue body on 2026-08-31. not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task: the gate reserves task for a credential, an account or a platform a human must obtain, and there is code behind this one."
  - id: c3
    text: "`wcore-cli tests::every_runtime_mcp_add_joins_the_catalog_refresh` finds `src/main.rs` on Windows, so the lint does not grade an empty set there."
    state: not-met
    owner: core
    note: "Transcribed verbatim from the issue body on 2026-08-31. not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task: the gate reserves task for a credential, an account or a platform a human must obtain, and there is code behind this one."
  - id: c4
    text: "`wcore-cli tests::every_runtime_mcp_withdrawal_leaves_the_catalog_refresh` passes on real Windows."
    state: not-met
    owner: core
    note: "Transcribed verbatim from the issue body on 2026-08-31. not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task: the gate reserves task for a credential, an account or a platform a human must obtain, and there is code behind this one."
  - id: c5
    text: "`wcore-tools::issue_1248_conflict_notice_test in_memory_backend_conflict_still_renders_todays_wording` passes on real Windows."
    state: not-met
    owner: core
    note: "Transcribed verbatim from the issue body on 2026-08-31. not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task: the gate reserves task for a credential, an account or a platform a human must obtain, and there is code behind this one."
  - id: c6
    text: "A Windows run is REACHABLE for the branch these arrived on, rather than skipped by the `[ci-windows]` marker gate, so the next five are found by the pipeline instead of by a hand-pushed marker commit."
    state: not-met
    owner: core
    note: "Transcribed verbatim from the issue body on 2026-08-31. not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task: the gate reserves task for a credential, an account or a platform a human must obtain, and there is code behind this one."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-release-readiness.py` reads ledger files and nothing else, so an open in-scope issue with no ledger is invisible to it. `check-criteria-ledger.py`'s
coverage arm is the only thing that reports the gap, and CI runs that arm
`--offline`, which cannot ask the trackers -- so nothing said so.

Criteria are transcribed from the issue body WITHOUT EDIT. Where the wording is
loose it is left loose: sharpening a criterion inside the ledger is how a
criterion quietly becomes an easier adjacent property. Whoever takes this
restates it on the ISSUE first.


CRITERIA ARE NOT VERBATIM HERE and that is flagged rather than hidden: the issue body
is a measured ROSTER of five failures with their panic text, not an acceptance list.
Each criterion above is `this named test passes on real Windows`, which is the
weakest statement that discharges the roster and adds nothing. c6 is drawn from the
body's `How they were invisible` section, which is a finding in its own right and
would otherwise be lost. Restate on the ISSUE before grading any of them.

Measurement carried over so nobody re-derives it. RED: run 33352985296, job
`CI (Array)`, runner `ferrox-win-msvc`, commit `b45f08119` -- 16223 run, 5 failed.
CONTROL: run 33313825017, `main` @ `b26e4058d` -- 15647 run, 0 failed. None of the
five `(binary, test)` pairs exists in `main`'s JUnit roster, so this is not a
Windows regression: it is five tests that had never once run on Windows. Each failed
all three attempts, so `[profile.ci] retries = 2` laundered none of them.

READING THE ROSTER, recorded because two of the five look like one cause: c3 and c4
are both source-walk lints in `main.rs` whose path comparisons are written with `/`
(`path.ends_with("tui/surfaces/mod.rs")`, the `src/main.rs` discovery check) against
paths Windows renders with `\`. That is a hypothesis from the panic text, NOT a
measurement -- it has not been run on Windows -- and it is written here as a lead,
not as a diagnosis.
