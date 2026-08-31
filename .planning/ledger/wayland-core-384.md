---
issue: 384
repo: FerroxLabs/wayland-core
kind: defect
title: "is_session_write_granted documents itself as the write-grant enforcement predicate and has no production call site"
status: open
last_verified_commit: 10de774f
criteria:
  - id: c1
    text: "Either is_session_write_granted is the predicate the mutating VFS path actually asks, or it is deleted together with the two tests that grade it"
    evidence: "absent:crates/wcore-tools/src/workspace_policy.rs::is_session_write_granted"
    state: met
    owner: core
    note: "MET 2026-08-31 (lane f13-s2-policy-resolvers) by DELETING it, which the ticket offers as one of its two arms and which is the better one: the live write-grant check is SandboxedFs::contain_granted / live_grant_roots, and it resolves the dangling boundary itself through landing_prefix, so routing the mutating path through this predicate would have made a WEAKER check authoritative. Recount at ca15a48bf before deleting, with the test module and tests/ and examples/ excluded and a known-negative control in the same scan: 0 production call sites. CORRECTION TO THE TICKET'S WORDING: it says \"deleted together with the two tests that grade it\"; there are no two such tests -- there were two ASSERTION LINES, path_write_grant_test.rs:94 and :151, inside tests whose remaining bodies grade the real VFS path (the DoD green arm and the read-only-grant refusal). Deleting those enclosing tests would have removed the enforcement coverage this ticket exists to protect, so the two assertions went and the tests stayed. Nothing else in crates/ referenced the predicate; cargo check --workspace --all-targets RC=0 confirms the public-API removal breaks no downstream crate."
  - id: c2
    text: "If it is deleted, the doc comment claiming it is the enforcement point goes with it and SandboxedFs::contain_granted/live_grant_roots carries that documentation instead"
    evidence: "file:crates/wcore-tools/src/vfs.rs:1795:**This is the predicate the mutating operations ask.**"
    state: met
    owner: core
    note: "MET 2026-08-31. The deleted doc comment claimed to be \"the predicate `SandboxedFs`'s mutating operations ask\". That sentence now sits on SandboxedFs::contain_granted (vfs.rs:1795), which is where a mutation is actually refused, and the grant half of it on live_grant_roots (vfs.rs:1871), which is where \"a read-only grant on the same folder still refuses the write\" is decided. Both notes name the deletion and its ticket, so a reader arriving at the old name is not left guessing. The remaining textual mentions of the deleted predicate in the tree are exactly those two explanations plus one in path_write_grant_test.rs saying why the assertion is gone."
  - id: c3
    text: "A grep gate or a test proves no other documented-but-uncalled enforcement predicate remains in workspace_policy.rs"
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::no_public_predicate_in_this_file_is_uncalled_in_production"
    state: met
    owner: core
    note: "MET 2026-08-31, and it FOUND MORE, which is reported rather than filed away. The gate walks every .rs file under crates/ that ships in a lib or bin (tests/, examples/, tests.rs and #[cfg(test)] module bodies stripped, brace-counted) and requires a production call site for every `pub fn -> bool` in workspace_policy.rs. It is written over that SUPERSET on purpose: \"which words count as an enforcement claim\" is the open alphabet that name-keyed gates die of, so it never reads a doc comment to decide whether a predicate is in scope. Two more predicates came back with ZERO production call sites: is_project_secret and is_vcs_content_store. Both are bypassed by the production guard (denies_read_content resolves once and asks is_project_secret_resolved / is_vcs_content_store_resolved), and BOTH doc comments claimed enforcement -- \"Used as the SecretDenyFs read-path predicate\" and \"this refusal is enforced by THIS process\". They are kept (they are the public predicate surface the crate's integration tests drive) but their docs now say NOT AN ENFORCEMENT POINT and name denies_read_content, and the gate's exemption is conditional on that text still being there and on them still having no production caller. INSTRUMENT CONTROLS, all inside the test: >= 200 source files walked, >= 12 predicates found, a known-POSITIVE (denies_read_content must be found) and a known-NEGATIVE (a name that does not exist must return zero) so a zero is not read as absence. RED ARMS, cargo check RC=0 first: (a) adding a new uncalled `pub fn is_write_reachable_zz(..) -> bool` with the old doc sentence -> RED; (b) deleting the NOT AN ENFORCEMENT POINT line from is_project_secret -> RED, so the exemption is not free."
---

`is_session_write_granted` (workspace_policy.rs:851) has NO production call site. Its doc comment states it is 'the predicate `SandboxedFs`'s mutating operations ask', but `grep -rn is_session_write_granted crates/` returns only its definition plus two references in crates/wcore-tools/tests/path_write_grant_test.rs. The live write-grant check is `SandboxedFs::contain_granted` / `live_grant_roots` in vfs.rs:1687/1749, which does its own dangling-boundary resolution.

**Where.** crates/wcore-tools/src/workspace_policy.rs:845-859

**Why it matters.** Not an exploitable hole today — the real enforcement path is the VFS one and it resolves correctly — but it is a documented enforcement predicate that enforces nothing, graded by two tests that therefore grade nothing reachable. A future author reading that doc comment could reasonably believe write grants are checked here, and a future change routed through it would be dead on arrival.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.


---

**Closed 2026-08-31, lane `f13-s2-policy-resolvers` @ `10de774f`.** The predicate is
deleted. The enforcement sentence now sits on `SandboxedFs::contain_granted` and
`live_grant_roots`, which are where a mutation is actually refused.

The c3 gate found TWO more predicates in the same file with no production call
site: `is_project_secret` and `is_vcs_content_store`. Both are bypassed by the
production guard — `denies_read_content` resolves the path once and asks their
`_resolved` siblings — and both doc comments claimed enforcement. They are kept
as the public predicate surface the crate's integration tests drive, their docs
now say `NOT AN ENFORCEMENT POINT` and name `denies_read_content`, and the
gate's exemption is conditional on that text still being there. That is the
finding this ticket's c3 was written to surface, reported rather than filed
away.
