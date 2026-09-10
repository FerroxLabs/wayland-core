---
issue: 1250
repo: FerroxLabs/wayland
kind: defect
title: "wcore-exec-backend tests race on the WAYLAND_EXEC_BACKEND_STATE_DIR process global in the shared-process suite"
status: open
last_verified_commit: cdd15c3dd
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
    text: "Shown RED: the interleaving reproduces on a shared-process run before the fix, with the 1 passed / 1 failed signature quoted, and does not after. Isolation passes 8/8 today and so proves nothing either way."
    state: not-met
    owner: core
    note: "STILL NOT MET, and now not-met on EVIDENCE rather than on nobody having tried. THE RED ARM WAS BUILT AND IT DID NOT REPRODUCE. At 1b29659a5 `temp_state()` in tests/conformance_matrix.rs was restored VERBATIM to its pre-fix body from d3d29737b^ -- `unsafe { std::env::set_var(\"WAYLAND_EXEC_BACKEND_STATE_DIR\", dir.path()) }` returning the bare TempDir, with the mutation confirmed by `git diff` before any run. `cargo test -p wcore-exec-backend --test conformance_matrix` (SHARED PROCESS, plain cargo test, never nextest -- nextest gives each test its own process and can never see this class) was then run 20 times on hetzner-dsm at loadavg 26-29, one remote-proof invocation per trial, no retries: 20/20 `2 passed; 0 failed`, 0.93-1.05s each. The `1 passed / 1 failed` signature this criterion requires was NEVER OBSERVED. NOT A VACUOUS RUN, controlled: the two tests genuinely overlap in that shared process -- the same binary at `--test-threads=1` takes 1.26s against 0.93-1.05s in parallel, so roughly 0.26s of the ~1s run is concurrent and the interleaving window is real, just narrow. AFTER-ARM for completeness: 2 passed / 0 failed at cdd15c3dd. WHAT THIS MEANS: the redirection hazard c1 fixed is real as a mechanism, but the failure this ticket was filed on has not been shown to be it -- which is consistent with wayland#1298, where the ci-linux payload (`backend signing seed at <path> is not 32 bytes`) was traced to torn seed publication and not to a removed state dir. WHAT IS OWED: either a reproduction of the 1-passed/1-failed signature on the pre-fix body under the condition that actually produced it (the containerised shared-process integration leg, or a forced interleaving that is declared as forced), or a decision to supersede this criterion into wayland#1298 rather than leave a red arm nobody can raise."
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
