---
issue: 384
repo: FerroxLabs/wayland-core
kind: defect
title: "is_session_write_granted documents itself as the write-grant enforcement predicate and has no production call site"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "Either is_session_write_granted is the predicate the mutating VFS path actually asks, or it is deleted together with the two tests that grade it"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D12, found while verifying wayland-core#356). Nothing has been done. The measured finding, verbatim: `is_session_write_granted` (workspace_policy.rs:851) has NO production call site. Its doc comment states it is 'the predicate `SandboxedFs`'s mutating operations ask', but `grep -rn is_session_write_granted crates/` returns only its definition plus two references in crates/wcore-tools/tests/path_write_grant_test.rs. The live write-grant check is `SandboxedFs::contain_granted` / `live_grant_roots` in vfs.rs:1687/1749, which does its own dangling-boundary resolution."
  - id: c2
    text: "If it is deleted, the doc comment claiming it is the enforcement point goes with it and SandboxedFs::contain_granted/live_grant_roots carries that documentation instead"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D12). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "A grep gate or a test proves no other documented-but-uncalled enforcement predicate remains in workspace_policy.rs"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D12). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

`is_session_write_granted` (workspace_policy.rs:851) has NO production call site. Its doc comment states it is 'the predicate `SandboxedFs`'s mutating operations ask', but `grep -rn is_session_write_granted crates/` returns only its definition plus two references in crates/wcore-tools/tests/path_write_grant_test.rs. The live write-grant check is `SandboxedFs::contain_granted` / `live_grant_roots` in vfs.rs:1687/1749, which does its own dangling-boundary resolution.

**Where.** crates/wcore-tools/src/workspace_policy.rs:845-859

**Why it matters.** Not an exploitable hole today — the real enforcement path is the VFS one and it resolves correctly — but it is a documented enforcement predicate that enforces nothing, graded by two tests that therefore grade nothing reachable. A future author reading that doc comment could reasonably believe write grants are checked here, and a future change routed through it would be dead on arrival.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
