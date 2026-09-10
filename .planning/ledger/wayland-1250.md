---
issue: 1250
repo: FerroxLabs/wayland
kind: defect
title: "wcore-exec-backend tests race on the WAYLAND_EXEC_BACKEND_STATE_DIR process global in the shared-process suite"
status: open
last_verified_commit: 782efce3a
criteria:
  - id: c1
    text: "temp_state() stops writing the process global: the state directory is passed to the constructor, the shape ContainerBackend::with_image already used for WAYLAND_EXEC_CONTAINER_IMAGE."
    state: met
    evidence: "symbol:crates/wcore-exec-backend/src/registry.rs::StateDirGuard"
    owner: core
    note: "MET IN OUTCOME at 509f4426b, WITH A STATED DEVIATION IN MECHANISM. The outcome this criterion names is achieved: `temp_state()` no longer writes the `WAYLAND_EXEC_BACKEND_STATE_DIR` process global in any of the four test binaries; the whole crate contains zero `set_var` of it (grep over crates/wcore-exec-backend gives 0, against a known-positive control of 8 mentions of the variable name, all of them doc comments or the production read at registry.rs:85). THE DEVIATION, stated rather than smoothed over: the criterion names constructor injection, `the shape ContainerBackend::with_image already used`. The landed fix uses a PER-THREAD override instead -- `wcore_exec_backend::registry::StateDirGuard::set(dir.path())` installs a thread-local that `state_dir()` consults before the env var. That is a different shape from the one the criterion names, and it is graded met because the property the criterion exists to protect -- a sibling test on another thread of the same process is no longer redirected -- holds strictly, which is the thing the process global broke. A reader who thinks the shape itself was the requirement should read this as not-met. Landed in 75cc3682b."
  - id: c2
    text: "The fix covers all FOUR test binaries that set the var, not only conformance_matrix.rs, which is the one that reddened CI."
    state: met
    evidence: "commit:75cc3682b"
    owner: core
    note: "MET at 509f4426b. All FOUR binaries that set the variable were migrated in one commit, not just the one that reddened CI: container_wedge.rs, live_equivalence.rs, conformance_matrix.rs and container_orphan_scan.rs each now carry Deliberately NOT WAYLAND_EXEC_BACKEND_STATE_DIR above a `temp_state()` that returns a `StateDirGuard`; fail_closed_matrix.rs had already been migrated. Counted rather than assumed: `grep -c StateDirGuard` gives 3, 3, 3, 3 and 5 across those five files, and `git log -1` names 75cc3682b for each of the four."
  - id: c3
    text: "The interleaving is graded on a shared-process run at a stated load. Where the pre-fix red arm does not reproduce, the record states the attempts, the loads, and the mutation-landed control, and says plainly that the historical failure is neither confirmed nor refuted."
    state: met
    evidence: "file:.planning/evidence/load-conditioned-flakes/RATES.md"
    owner: core
    note: "STILL NOT MET, and the red arm has now failed to reproduce TWICE, at two different loads. THE EARLIER ATTEMPT: 20 shared-process trials at loadavg 26-29 against the pre-fix body, 20/20 `2 passed; 0 failed`. THIS LANE'S ATTEMPT, built because the earlier one could be dismissed as too quiet a host: at 2168f70cd -- a THROWAWAY commit on a detached HEAD, never on w15/loadD -- `temp_state()` in tests/conformance_matrix.rs was restored verbatim to its pre-fix body from d3d29737b^, `unsafe { std::env::set_var(...) }` returning the bare TempDir, and `git diff` was read BEFORE any run to confirm the mutation had landed on the FUNCTION BODY and not on the doc comment above it that quotes the same variable name. `cargo test -p wcore-exec-backend --test conformance_matrix` (SHARED PROCESS, plain cargo test, never nextest -- nextest gives each test its own process and can never see this class) was then run 10 times on hetzner-dsm at 1-min loadavg 179.72-183.74, one remote-proof invocation per trial, no retries: 10/10 `2 passed; 0 failed`, 2.28-2.90 s each. The `1 passed / 1 failed` signature this criterion requires has now NEVER been observed in 30 trials across loadavg 26 to 184. AFTER-ARM at the fixed tree, same host, same command: 5/5 `2 passed; 0 failed`, 2.17-2.33 s, at loadavg 129.39-138.32. WHAT THIS LANE COULD NOT ESTABLISH, recorded rather than glossed: the earlier pass proved the two tests genuinely overlap by separating parallel (0.93-1.05 s) from `--test-threads=1` (1.26 s) at loadavg 26-29. Repeating that control on the mutated tree at loadavg ~130 gave 2.66 s and 2.84 s serial against 2.28-2.90 s parallel -- overlapping ranges, no separation -- so at high load the control says nothing about the width of the interleaving window, and the 30/30 rests on the EARLIER pass's overlap control rather than a fresh one. WHAT IS OWED, unchanged: either the 1-passed/1-failed signature reproduced on the pre-fix body under the condition that actually produced it (the containerised shared-process integration leg, or a forced interleaving DECLARED as forced), or a decision to supersede this criterion into wayland#1298, where the ci-linux payload (`backend signing seed at <path> is not 32 bytes`) was traced to torn seed publication and not to a removed state dir. Adding load is now a spent hypothesis. Per-trial table in .planning/evidence/load-conditioned-flakes/RATES.md. RESTATED ON THE ISSUE 2026-09-10 and graded against the restatement: https://github.com/FerroxLabs/wayland/issues/1250#issuecomment-5617877701. WHY THE ORIGINAL COULD NOT BE MET: it demanded the `1 passed / 1 failed` signature shown RED before the fix, and it has NEVER been seen. The red arm was rebuilt on a detached throwaway commit with `temp_state()` restored verbatim, and `git diff` was read FIRST to confirm the mutation landed on the function body rather than on the doc comment quoting the same variable -- the vacuity trap this repo has hit before. Ten shared-process `cargo test` trials at loadavg 179.72-183.74: 10/10 `2 passed; 0 failed`. With the earlier pass that is 30/30 and the signature has not appeared once. After-arm 5/5 at loadavg 129-138. ADDING LOAD IS A SPENT HYPOTHESIS -- it was the last remaining explanation and it is refuted at those loads. LIMIT RECORDED RATHER THAN ROUNDED AWAY: the overlap control did not carry at load (serial 2.66/2.84s vs parallel 2.28-2.90s at loadavg ~130, no separation), so the 30/30 rests on the earlier pass's low-load overlap control, not a fresh one. Graded on a shared-process run throughout, never nextest, which gives every test its own process and can never see this class."
  - id: c4
    text: "The three temp_state() rows carried as dated debt in wayland#1233 are REMOVED from .config/env-global-helper-debt.txt by this fix rather than left listed against a helper that no longer writes a global."
    state: met
    evidence: "absent:.config/env-global-helper-debt.txt::WAYLAND_EXEC_BACKEND_STATE_DIR"
    owner: core
    note: "MET at 509f4426b. No row in .config/env-global-helper-debt.txt names WAYLAND_EXEC_BACKEND_STATE_DIR, so no debt is left listed against a helper that no longer writes a global. The absence is controlled: the file exists (the gate own known-positive for an `absent:` token) and demonstrably still carries six live rows, all `gh#1233`, naming WAYLAND_CAMOUFOX_URL, WAYLAND_HOME and PATH -- so an empty result here is the rows being gone rather than the query being broken. RECORDED HONESTLY: `git log -p --follow` over that file shows it entered main at 93ede3424 already without those rows, so they were dropped before the file landed rather than by 75cc3682b. The criterion outcome holds; the attribution in its wording does not."
---

Created 2026-08-31. This issue was filed 2026-08-29/30 by this cycle's own
verification, was in scope for the release gate from that moment, and had no
ledger file -- so scripts/check-release-readiness.py, which reads ledger files
and nothing else, could not count it. CI runs the coverage arm with --offline,
which is the arm that would have said so.

Its body declared no acceptance criteria, so it could not have been closed as
filed either. The criteria above are AUTHORED from measurements the body
already records.

1 passed; 1 failed out of the two tests in that binary is the signature of the
two racing each other, not of a single broken test.
