---
issue: 384
repo: FerroxLabs/wayland-core
kind: defect
title: "is_session_write_granted documents itself as the write-grant enforcement predicate and has no production call site"
status: open
last_verified_commit: 607d6ba9
criteria:
  - id: c1
    text: "Either is_session_write_granted is the predicate the mutating VFS path actually asks, or it is deleted together with the two tests that grade it"
    state: met
    evidence: symbol:crates/wcore-tools/src/vfs.rs::contain_granted
    owner: core
    note: "DELETED. is_session_write_granted is gone from workspace_policy.rs (31 lines), together with the two assertions that graded it (path_write_grant_test.rs:94 and :151). Deleted rather than wired in because the live check is strictly stronger: SandboxedFs::contain_granted resolves the dangling-link boundary itself through landing_prefix and evaluates grant expiry at use time, where the deleted predicate compared a canon_for_scope path against a snapshot. The two surrounding tests keep their real coverage — they grade fs.write refusal and writable_roots, not the predicate. grep -rn is_session_write_granted crates/ now returns exactly one line: the history note in the moved documentation."
  - id: c2
    text: "If it is deleted, the doc comment claiming it is the enforcement point goes with it and SandboxedFs::contain_granted/live_grant_roots carries that documentation instead"
    state: met
    evidence: symbol:crates/wcore-tools/src/vfs.rs::live_grant_roots
    owner: core
    note: "The claim moved with the deletion. SandboxedFs::contain_granted now opens `THE predicate the mutating VFS path asks about a standing grant`, names live_grant_roots as the other half, and records what #384 found; live_grant_roots carries the reciprocal pointer. The cross-reference in is_read_reachable that pointed at the deleted symbol was rewritten to state the reason in place."
  - id: c3
    text: "A grep gate or a test proves no other documented-but-uncalled enforcement predicate remains in workspace_policy.rs"
    state: met
    evidence: test:crates/wcore-tools/src/workspace_policy/tests.rs::no_documented_enforcement_predicate_is_uncalled
    owner: core
    note: "A gate over the CLASS, not the instance. documented_but_uncalled() pairs every pub fn in workspace_policy.rs with its doc block and flags any whose doc makes an enforcement claim and whose name appears in no production call site (crates/*/src, excluding tests/ and tests.rs). ENFORCEMENT_CLAIMS is a SHAPE — this file own convention of a capitalised definite article (THE read-content refusal, THE exec-time shell gate predicate, THE ONE ANSWER) plus `the predicate `, `must not `, `must be REFUSED`, `must stay denied` — never a list of the predicates that exist today. THREE anti-vacuity controls in the same test: a synthetic source proving the detector flags an uncalled claimant, does not flag a called one and does not flag an unclaimed one; a corpus-size and known-call-site check; and the decisive one, documented_but_uncalled(SOURCE, \"\") must match at least 8 REAL predicates — the first vocabulary matched only denies_read_content and the gate was nearly vacuous, which this control caught. RED ARM: a documented uncalled predicate reintroduced; cargo check exit 0; test fails naming it. RE-DERIVED 13d36be65 (not inherited): a pub fn zz_red_arm_uncalled_enforcement_predicate documented `THE predicate the mutating VFS path asks about a standing grant` added to workspace_policy.rs; cargo check -p wcore-tools --tests exit 0; the gate fails with `these predicates document themselves as enforcement points and nothing in production calls them ... [\"zz_red_arm_uncalled_enforcement_predicate\"]`. Restored, blob identity verified equal to the commit under test."
---

`is_session_write_granted` (workspace_policy.rs:851) has NO production call site. Its doc comment states it is 'the predicate `SandboxedFs`'s mutating operations ask', but `grep -rn is_session_write_granted crates/` returns only its definition plus two references in crates/wcore-tools/tests/path_write_grant_test.rs. The live write-grant check is `SandboxedFs::contain_granted` / `live_grant_roots` in vfs.rs:1687/1749, which does its own dangling-boundary resolution.

**Where.** crates/wcore-tools/src/workspace_policy.rs:845-859

**Why it matters.** Not an exploitable hole today — the real enforcement path is the VFS one and it resolves correctly — but it is a documented enforcement predicate that enforces nothing, graded by two tests that therefore grade nothing reachable. A future author reading that doc comment could reasonably believe write grants are checked here, and a future change routed through it would be dead on arrival.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
