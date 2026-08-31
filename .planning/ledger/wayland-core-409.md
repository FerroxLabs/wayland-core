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
    state: met
    evidence: "file:crates/wcore-tools/tests/issue_1248_conflict_notice_test.rs:169:const MEM_DEST: &str = r"
    owner: core
    note: "MET 2026-08-31 on the real Windows host (SeanD@seandesktop, D:\\w409, x86_64-pc-windows-msvc, cargo 1.95.0 / nextest 0.9.138), by lane/f13-409-c5-abspath @ b9140d042. TEST DEFECT, not a product defect, and the direction was decided before the edit. The fixture passed `/w/f.txt` as `Write`'s `file_path`; that string has a root but NO drive prefix, so `Path::is_absolute()` is FALSE on Windows, and `write.rs:140` runs `validate_user_path` on the RAW argument before any backend is reached -- `path_validation.rs:166` returns `NotAbsolute`. The test therefore measured the path guard, not the `InMemoryFs` conflict renderer it names: green on Unix, grading nothing on Windows. The guard is CORRECT as written and was not touched: a rooted-but-driveless path resolves against whichever drive is current, so refusing it is the tool's contract, `archive_tool.rs:616-624` already carries a `cfg(windows)` arm that exists precisely because `Path::new(\"/etc/passwd\").is_absolute()` is false there, and `crates/wcore-cli/tests/child_authority_corpus/surfaces.rs:1302` records `path must be absolute` as the expected answer for a non-absolute `Write`. The fix is a `cfg`-selected `MEM_DEST` const (`C:\\w\\f.txt` on Windows, `/w/f.txt` elsewhere) used for the seeded key, the `file_path` argument and the wording assertion; nothing that is graded changed, because the backend is a `HashMap<PathBuf, _>` and the assertion compares against the raw argument the tool echoes back. EVIDENCE, three arms on the Windows host. RED at base 911ac89f4: full `cargo nextest run -p wcore-tools --no-fail-fast`, 1707 tests, this test failed all attempts with the issue's panic verbatim -- `left: \"Refused to write /w/f.txt: path must be absolute: \\\"/w/f.txt\\\"\"`. GREEN at b9140d042: the same binary, 5 tests run, 5 passed, exit 0 -- the other four tests in the file are the known-positive control that the filter selected a non-empty set. SENSITIVITY at b9140d042: mutating the PRODUCTION renderer (`unsaved_work.rs:1523`, `Nothing was changed.` -> `Nothing at all was changed.`) turned this test RED on Windows (3 of 5 failed, `the unchanged wording lost its ending`), and restoring the file -- with an mtime touch, `git status` clean, no leftover string -- returned 5/5. So the pass grades the production renderer on Windows and is not vacuous. Linux control still green (5/5 on hetzner-dsm). `cargo fmt --all` clean, `cargo clippy -p wcore-tools --all-targets -- -D warnings` exit 0. LIMIT, stated because the host is not representative: Developer Mode is ON on seandesktop, but nothing in this criterion is privilege-dependent -- the path guard and the in-memory backend are pure string and hashmap work, so the result carries to an ordinary Windows machine. SCOPE: this row only. The same full-suite Windows run also measured c2's `grep_vcs_content_store_deny` (2 tests) and `inv2_round5_adversarial_test an_in_place_save_is_not_lost_to_the_final_rename` still failing, plus 5 timeouts in `wcore-tools bash::tests::*`; none of those is this criterion and none was touched."
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

C5, MEASURED 2026-08-31. The one row in this file that has been run on Windows.
It was a FIXTURE defect and the direction was established before the edit, since
the opposite reading -- that the absoluteness check is wrong on Windows -- would
have been a real product defect papered over by editing its test. The full
`wcore-tools` suite was run on the Windows host at the base commit rather than
only the named test, so the sibling question ("does one fixture repair leave
three?") is answered by measurement instead of by grep: 1707 tests, and this was
the ONLY hardcoded-POSIX-absolute fixture in the crate that fails that way. The
other POSIX-absolute fixtures in `wcore-tools/tests` are already `cfg(unix)`-gated
by an earlier Windows sweep, or expect a refusal and get one.

One OBSERVATION recorded and deliberately not acted on, because it is a different
defect class from c5 and is nobody's criterion here:
`legacy_execute_path_validation_test write_legacy_refuses_traversal` passes
`/tmp/../etc/shadow` and asserts only `is_error`. On Windows that path is refused
as `NotAbsolute` (the `is_absolute` guard runs BEFORE the `..` component check),
so the test passes for a reason other than the traversal it names. It is green,
not red, so it is not in scope for c5 and was left alone.
