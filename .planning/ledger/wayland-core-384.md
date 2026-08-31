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
    state: met
    evidence: "absent:crates/wcore-tools/src/workspace_policy.rs::pub fn is_session_write_granted"
    owner: core
    note: "MET AS WRITTEN, by the DELETE branch. Wiring it was measured as the wrong answer, not merely the harder one: SandboxedFs::contain_granted resolves the dangling-link boundary itself (canonicalize_existing_prefix + landing_prefix) while the predicate used the weaker canon_for_scope, so routing the mutating path through it would have WEAKENED the live check. The predicate and its doc comment are gone. The two REFERENCES that graded it are gone with it; the two enclosing tests are deliberately KEPT and the reason is stated at the head of path_write_grant_test.rs -- each drives fs.write / fs.read on the real SandboxedFs, which is the enforcement point, so deleting them to satisfy the phrase 'the two tests' would have removed #1104's definition-of-done arms and graded the live path LESS. All 14 tests in that file pass."
  - id: c2
    text: "If it is deleted, the doc comment claiming it is the enforcement point goes with it and SandboxedFs::contain_granted/live_grant_roots carries that documentation instead"
    state: met
    evidence: "symbol:crates/wcore-tools/src/vfs.rs::contain_granted"
    owner: core
    note: "MET. contain_granted now opens with '**The write-grant enforcement point.**', names #384, says the deleted predicate carried that claim with no caller, and explains why a lexical starts_with on a shallow-canonicalised path is not an equivalent question. live_grant_roots carries the matching note. The one remaining prose reference inside workspace_policy.rs (is_read_reachable, which cited the deleted predicate for its resolver reasoning) was restated in place rather than left dangling."
  - id: c3
    text: "A grep gate or a test proves no other documented-but-uncalled enforcement predicate remains in workspace_policy.rs"
    state: met
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::every_path_predicate_in_this_file_has_a_production_call_site"
    owner: core
    note: "MET AS WRITTEN, by a test rather than a grep, and by the INVERTED question so the next one is caught on arrival: it enumerates every `pub fn` in workspace_policy.rs taking a path and returning bool, walks crates/ for production source (excluding tests/ dirs, tests.rs, and column-zero #[cfg(test)] items), and demands a caller for each. RED ARM, verbatim, with the gate added and the predicate not yet deleted: `these `workspace_policy.rs` predicates have NO production call site ... is_session_write_granted`. Anti-vacuity controls in both directions: >=10 predicates enumerated with three named known-positives, >=200 production files read, is_secret_path_static seen called >=4 times, the stripper proven to REMOVE an inline test module and proven NOT to eat the production call sites that follow one -- a first version cut each file at its first #[cfg(test)] and falsely reported is_repo_control_path and is_skill_source_path as uncalled."

---

`is_session_write_granted` (workspace_policy.rs:851) has NO production call site. Its doc comment states it is 'the predicate `SandboxedFs`'s mutating operations ask', but `grep -rn is_session_write_granted crates/` returns only its definition plus two references in crates/wcore-tools/tests/path_write_grant_test.rs. The live write-grant check is `SandboxedFs::contain_granted` / `live_grant_roots` in vfs.rs:1687/1749, which does its own dangling-boundary resolution.

**Where.** crates/wcore-tools/src/workspace_policy.rs:845-859

**Why it matters.** Not an exploitable hole today — the real enforcement path is the VFS one and it resolves correctly — but it is a documented enforcement predicate that enforces nothing, graded by two tests that therefore grade nothing reachable. A future author reading that doc comment could reasonably believe write grants are checked here, and a future change routed through it would be dead on arrival.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
