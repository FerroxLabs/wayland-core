---
issue: 398
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3's lexical gate makes a guard on any path named objects/modules/store cost one syscall per workspace directory (split from #390 c3)"
status: open
last_verified_commit: 4a738f2e
criteria:
  - id: c1
    text: "— The warm per-guard cost of a gate-admitted path is INDEPENDENT of the workspace's directory count. `vfs_guard_cost.rs::a_gate_admitted_path_costs_one_probe_per_workspace_directory` grades this as a slope and currently pins the slope at 1.0; when it is fixed that assertion is INVERTED rather than deleted, so the regression and its closure are graded by the same test."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c2
    text: "— The refusals #390 bought stay bought: `vfs_nested_named_store_deny.rs` stays green in full, including the vendored-gitfile arm and the nested-alternates arm, so the cost fix is not a quiet revert."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c3
    text: "— The ordinary (gate-refused) path is unchanged at one resolution / zero warm scans / three warm probes, still pinned by `one_ordinary_path_guard_resolves_once_and_does_not_rescan`."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c4
    text: "— The figure is re-stated as a number for BOTH call shapes on the tree the fix lands on, base-vs-HEAD, by the two instruments named above (the counted slope and the differential `strace` per-traversed-directory figure), each with its known-positive control green."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c5
    text: "— The `DENY_CACHE_MAX_DIRS` branch of `nested_stores_memoized` is graded by a test or the branch is removed; today no test reaches it and its cost behaviour is asserted only in a comment."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.
