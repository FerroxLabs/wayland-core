---
issue: 390
repo: FerroxLabs/wayland-core
kind: defect
title: "is_vcs_content_store arm 2 reads only <root>/.git, so a VENDORED gitfile's object store is VFS-readable (split from #244 c1)"
status: open
last_verified_commit: 30fd6cfde
criteria:
  - id: c1
    text: "A VFS read of an object under a store named by a gitfile on a VENDORED checkout is refused, with that checkout`s own working tree still readable as the wrong-refusal control"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_store_deny.rs::a_vendored_gitfiles_object_store_is_refused
    owner: core
    note: "MEASURED at lane/f13-vcs-store. `vfs_nested_store_deny.rs::a_vendored_gitfiles_object_store_is_refused`: `<root>/vendor/pkg/.git` = `gitdir: ../pkg-git`, object at `<root>/vendor/pkg-git/objects/12/3456`, driven through the production `SandboxedFs(SecretDenyFs(RealFs, contained(root)))` stack -- refused, with TWO controls green (an ordinary source file, and that checkout's own working tree `vendor/pkg/README.md`). Closed by arm 4 (`discover_nested_content_stores`), which reads a gitfile where it LIES instead of only at `<root>/.git`. RED ARM (compiled first, check clean): remove arms 3+4 and 6 of the 8 tests in that file go red with the canary in the bytes; the negative control and the recorded residual stay green."
  - id: c2
    text: "The same holds for an objects/info/alternates borrow declared by a NESTED checkout, not only by the workspace root"
    state: met
    evidence: test:crates/wcore-tools/tests/vfs_nested_store_deny.rs::a_nested_alternates_borrow_is_refused_whatever_its_target_is_named
    owner: core
    note: "`a_nested_alternates_borrow_is_refused_whatever_its_target_is_named`: `<root>/vendor/pkg/.git/objects/info/alternates` = `../../../../odb`, object at `<root>/odb/ab/cd1234` -- a target carrying NO store component. Refused. Arm 4 resolves borrow targets eagerly at scan time and the set is tested by prefix, so the target's directory name never enters the decision; that is also what closes #394 c1 with the same mechanism. Declaring checkout's working tree readable as the control."
  - id: c3
    text: "Whatever caching the fix introduces is measured against #376`s complaint: the per-operation cost of is_vcs_content_store does not get worse than it is today, stated as a number"
    state: not-met
    owner: core
    note: "RE-GRADED not-met 2026-08-31 (lane/f13-w3-vcs-regrade). The criterion is `the per-operation cost of is_vcs_content_store does not get worse than it is today, stated as a number`. It carries NO `warm` or `steady-state` qualifier, and the cost DID get worse.\nRE-MEASURED HERE with the SAME GuardCounters instrument on both trees: one source file (`tests/zz_regrade_probe.rs`, byte-identical on both arms, deleted before commit) driving the production `SandboxedFs(SecretDenyFs(RealFs, contained(root)))` stack; `cargo check -p wcore-tools --tests` RC=0 on each arm BEFORE running; a SEPARATE `CARGO_TARGET_DIR` per arm, because a shared one silently linked the base rlib into the HEAD test binary and reported the base numbers back as HEAD (the two arms are fingerprinted apart by their own cold figure below, and by base failing to compile HEAD`s `nested_walk_count`):\n  base 07ee39f63: `modules/vpc/main.tf` warm = 3 probes/guard at 8 workspace directories and 3 at 48 (slope 0); ordinary path 3; first guard 17 at both sizes.\n  HEAD 0ed5d4707: same path warm = 4 probes/guard at 8 and 4 at 48 (slope 0); ordinary path 3; first guard 83 at 8 directories and 403 at 48.\nSo the per-operation cost of a store-leaf-named path rose 3 -> 4 (+33%) for the life of the session, on every path with an ancestor named `objects`, `modules`, `store`, `lfs`, `pristine` or `repository` -- a Terraform `modules/`, a Redux `store/`. The ordinary path is genuinely unchanged at 3; that is the control showing the +1 is the new arm-3 repository probe and not the fixture.\nALSO NEWLY MEASURED, and it makes the retained one-off figure worse rather than better: the arm-4 walk is not a flat +24. It SCALES with the workspace -- 83 -> 403 first-guard probes for 40 extra `pkg{i}/src` pairs, i.e. ~4 probes per workspace directory, paid once per policy. The retained `17 -> 41` is that same walk on the smaller `vfs_guard_cost` fixture and is not a size-independent number.\nThe measurements retained below are not withdrawn -- they are the most valuable part of this entry and they are correct as far as they go. What is wrong is the GRADE: the warm figure was compared against `lane/f13-w3-vcs-residuals`, which is NOT an ancestor of `integ/f13` -- the same ground this ledger already uses to grade #398 c1/c3/c5 and #394 c2/c3/c4 not-met. To close this AS WRITTEN either the arm-3 probe leaves the warm path, or the criterion is restated ON THE ISSUE to exempt the store-leaf-named shape and state the new figure. Not rewritten here.\n--- retained, verbatim ---\nSTATED AS NUMBERS, base (integ/f13 @ 07ee39f63, code-identical to 11eb00097) vs HEAD, two instruments.\n(1) Counted `GuardCounters`, `vfs_guard_cost.rs`: the warm ordinary-path guard is UNCHANGED at 1 resolution / 0 rescans / exactly 3 probes per guard over 50 guards. A store-leaf-named path (`modules/vpc/main.tf`) costs 4 warm probes at 8 workspace directories AND at 48 -- slope 0.\n(2) Differential `strace -f -c` over `probe_vcs_content_stores_per_traversed_directory` (added here; the probe #390's merge used is not in this lineage), at WL_PROBE_DIRS 100/1100 and WL_PROBE_REPS 1/6, `1 passed` and the probe's known-positive control asserted in every run. Steady-state per traversed directory: base 35.009 -> HEAD 34.997 syscalls (-0.012, noise).\nThe COST PAID is a one-off, and it is stated rather than hidden: arm 4's walk runs once per policy, costing +24 probes on the `vfs_guard_cost` fixture (first guard 17 -> 41) and +6.0 syscalls per workspace directory, ONCE. Separated from the steady state by differencing two WL_PROBE_REPS counts at the same directory count; a single-invocation measurement cannot tell the two apart."
  - id: c4
    text: "When the fix lands, grep_vcs_named_store_deny.rs`s `!is_vcs_content_store` assertion is INVERTED rather than deleted, so the two layers are re-tied"
    state: met
    evidence: test:crates/wcore-tools/tests/grep_vcs_named_store_deny.rs::grep_cannot_harvest_a_nested_gitfile_named_store
    owner: core
    note: "INVERTED, not deleted. `grep_vcs_named_store_deny.rs::grep_cannot_harvest_a_nested_gitfile_named_store` carried `assert!(!policy.is_vcs_content_store(&store_file))` as the record that Grep and the point-predicate disagreed; it now asserts the predicate DOES see the vendored gitfile's store, with a comment saying which ticket inverted it and why. 3 passed."
---

Split out of #244 c1 while closing #244 c3. #244 c1's text was the unqualified
"at any nested depth"; that has been rewritten to the scope that actually holds
and this issue carries the remainder, referenced from #244 c7.

Found by building the second-arm fixture for #244 c3, not reported by that
change's verifier. The Grep half of the same class IS closed in that change --
it is only the in-process VFS predicate that still misses this shape.
