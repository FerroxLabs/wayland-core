---
issue: 1284
repo: FerroxLabs/wayland
kind: defect
title: "Flaky on macOS only: the_live_backend_timeout_bounds compares two wall-clock samples taken under different load"
status: open
last_verified_commit: e16de82cd
criteria:
  - id: c1
    text: "The test stops deciding on a ratio between two wall-clock samples taken at different moments under different load, or its allowlist entry is deleted because the ratio was measured stable."
    state: not-met
    evidence: "file:.config/flaky-allowlist.txt"
    owner: core
    note: "STILL NOT MET, and deliberately so. 2026-09-10: the ratio IS GONE FROM THE TREE -- `the_live_backend_timeout_bounds_the_manifest_build_and_names_it` no longer asserts `bounded * 3 < walk`, and `timer_allowance` (its only caller) is deleted. It now grades three things, none of them a duration: (a) the BOUND, as an event -- the manifest-named timeout string is emitted at exactly one site, the `Err(_)` arm of `timeout_at(deadline, build)` in bash.rs, so its presence proves the deadline fired while the build was outstanding; (b) NO CHILD RAN, as an observation -- CHILD_TOKEN can only reach the result through the child's stdout, so its absence is the claim, and the second call (default 120s budget, same policy, same posture) is its POSITIVE CONTROL; (c) anti-vacuity -- `secret_deny_walk_count() >= 1`, read AFTER the control call because the timed call's build is detached on the blocking pool and might not have entered the walk when the deadline fires. Load can now only make the walk slower, which makes the deadline fire during the build MORE reliably, so the remaining failure direction is one-sided. WHAT IS OWED, and why this stays not-met: NOTHING WAS COMPILED OR RUN. No build slot was available (all three Hetzner slots held) and cargo is banned on the Mac; only `cargo fmt --check` was run, and it is clean. Owed, in order: (1) `cargo nextest run -p wcore-tools --test bash_manifest_bound_live_backend --retries 0` on an enforcing host -- this is the FIRST compile of the change and the first exercise of the new positive control, which depends on the child's stdout reaching ToolResult.content under a bare `contained` policy (precedent: bash_false_cause_test.rs `real_sandbox_write_denial_is_quoted_back_by_the_annotation` runs a real shell under exactly that posture, but this specific control has never executed); (2) FAIL-BEFORE at v0.13.4 (0ccaa90b), where the file header records `timeout message names a cause: no` and the child ran after the walk, so BOTH new assertions should red -- not re-run in this pass; (3) the macOS population arm this criterion has always needed: n>=20 at --retries 0 on a native macOS host, which is what would let line 65 of .config/flaky-allowlist.txt be DELETED rather than renewed. Its expiry is 2026-10-01, NOT 2026-09-20. Until (1) and (2) are observed, this is an unverified edit, not a repair."
---

# A ratio test on a shared runner

Recorded so the 0.13.12 coverage gate has a ledger for it. The repair is 0.13.13 work.
