---
issue: 356
repo: FerroxLabs/wayland-core
kind: defect
title: "Two path resolvers with different escape properties 70 lines apart, and the weaker one guards the skill-source write refusal"
status: open
last_verified_commit: e7cb6679
criteria:
  - id: c1
    text: "is_skill_source_path is graded against dotdot-after-a-missing-component and the dangling-symlink hop, the two escapes #1097 was written for"
    state: met
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::a_dangling_symlink_into_the_skill_load_path_is_refused"
    owner: core
    note: "MERGED as e4cef7c2 via 2165c30a, and it did NOT hold: the dangling-symlink hop genuinely escaped canon_deep. Both call sites moved to canon_existing_ancestor and canon_deep is DELETED - only canon_existing_ancestor survives in workspace_policy.rs. The dotdot-after-missing-component case is separately graded by a_parent_dir_after_a_missing_component_still_reaches_the_skill_load_path, and the second call site by a_dangling_symlink_into_the_repo_control_surface_is_refused."
  - id: c2
    text: "A red arm is quoted verbatim for whichever escape lands, if either does"
    state: met
    evidence: "commit:e4cef7c2"
    owner: core
    note: "The red arm is quoted verbatim and MEASURED in the commit body: resolving <ws>/brief.html where that name is a dangling link into <ws>/.wayland-core/skills/ gives canon_existing_ancestor -> .../skills/linked-skill/brief.html versus canon_deep -> .../ws/brief.html. That divergence IS the escape."
  - id: c3
    text: "The negative controls pass in both arms, so a fix that refuses too much fails here"
    state: met
    evidence: "test:crates/wcore-agent/tests/skill_source_write_refusal.rs::the_rest_of_the_config_dir_is_still_writable"
    owner: core
    note: "Both controls the ticket names are present and green; the sibling is an_ordinary_project_directory_named_skills_stays_writable in the same file. They stayed green through the resolver swap, and skill_source_write_refusal.rs gained two more end-to-end arms (a_traversal_through_a_directory_that_does_not_exist_yet_is_refused, a_dangling_symlink_into_a_skill_source_dir_is_refused)."
  - id: c4
    text: "If both resolvers remain, the reason each call site picked one is stated AT the call site"
    state: met
    evidence: "test:crates/wcore-tools/tests/workspace_policy_resolver_class.rs::a_dangling_link_to_a_not_yet_created_project_secret_is_a_project_secret"
    owner: core
    note: "FALSE WHEN WRITTEN, TRUE NOW. canon_deep was deleted, but the file still held a THIRD resolver, canon_for_scope, with the same escape property - canonicalize the parent, re-attach the leaf - and it guarded three live security refusals plus the escalation-prompt path. MEASURED at origin/integ/f13, each escape paired with two controls (the name spelled directly, and a link whose target EXISTS) so a predicate answering true to everything could not pass: is_project_secret returned FALSE for <root>/notes.txt -> <root>/.env with .env absent, which is the Full-posture WRITE direction its own doc says has no SandboxedFs to pre-canonicalize; is_vcs_content_store returned FALSE for a dangling link into .git/objects; is_read_reachable called a dangling link out of the workspace REACHABLE, which also suppresses the path_boundary escalation prompt for it. canon_for_scope is now deleted and canon_existing_ancestor is the only resolver in the file, which is what this criterion asserts. Its call sites were is_project_secret, is_vcs_content_store, is_read_reachable, is_session_read_granted, three home lookups, and path_boundary.rs. RED ARM RUN: crates/wcore-tools/src/ checked out at the pre-fix e7cb6679b and touched, 3/3 fail; restored, touched, 3/3 pass, wcore-tools 1828/1828 and wcore-agent 3895/3895. COST, measured rather than assumed away: SecretDenyFs::guard asks two of these predicates on EVERY vfs read, write, exists, list and metadata call, and the component walk costs one canonicalize PER COMPONENT against canon_for_scope one. canon_existing_ancestor therefore keeps a canonicalize fast path for a path that fully exists - not a shortcut, because for such a path canonicalize IS the authoritative resolution and the walk converges on it; the walk exists for the case canonicalize FAILS, which is the missing component and the dangling link. INTERLEAVED A/B over 20,000 calls on a deep existing path, two rounds, because a first non-interleaved run read 123 us/call and was pure host load: is_project_secret 24.42 -> 19.56 and 29.67 -> 23.48 us/call, is_read_reachable 22.51 -> 16.91 and 29.41 -> 23.05. At or slightly faster than the shape it replaced, in both rounds."
---

Found while grading the superseded `lane/finish-criteria` orphan branches. Not a
defect with a known exploit — a structural inconsistency that leaves one guard
weaker than its neighbour for no stated reason.

`crates/wcore-tools/src/workspace_policy.rs` carries two path resolvers about
seventy lines apart with different escape properties. `canon_existing_ancestor`
is the walk-DOWN form, rewritten under `#1097` precisely to abandon the
walk-UP-and-append-verbatim shape. `canon_deep` IS that abandoned shape, and it
is what the `#1096` skill-source write refusal still uses.

The gap was not a proven hole, it was an ungraded one. Grading it turned one of
the two into a REAL hole: the dangling-symlink hop genuinely escaped `canon_deep`,
measured and quoted in `e4cef7c2`. Both call sites moved to
`canon_existing_ancestor` and `canon_deep` is deleted.

That was two of three. `canon_for_scope` — same abandoned shape, same escape —
survived the sweep and still guarded `is_project_secret`,
`is_vcs_content_store`, `is_read_reachable` and the escalation-prompt path in
`path_boundary.rs`. Each was measured escaping (see c4). It is deleted now, so
the claim this ticket makes is finally true of the whole file.

`is_session_write_granted` was deleted in the same change. Its doc comment
called it "the predicate `SandboxedFs`'s mutating operations ask"; it had no
production call site at all — `grep -rn` found only its definition and two
assertions in `path_write_grant_test.rs`, whose real subject is the live path.
The enforcement that actually runs is `SandboxedFs::contain_granted` /
`live_grant_roots` in `vfs.rs`, which does its own dangling-boundary
resolution. A documented enforcement predicate that enforces nothing is the
same trap this ticket is about, one level further on: the next author reads the
doc, routes a change through it, and the change is dead on arrival.

Graded against `origin/integ/next` at `43848f75`, after `lane/session-tickets`
merged in as `2165c30a`.
