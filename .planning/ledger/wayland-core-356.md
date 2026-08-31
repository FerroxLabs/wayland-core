---
issue: 356
repo: FerroxLabs/wayland-core
kind: defect
title: "Two path resolvers with different escape properties 70 lines apart, and the weaker one guards the skill-source write refusal"
status: open
last_verified_commit: 488fbbae9
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
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::every_strong_resolver_site_states_which_resolver_and_why"
    owner: core
    note: "MET AS WRITTEN, on the refuted reading rather than the original one. Both resolvers DO remain (canon_for_scope for advisory answers, canon_existing_ancestor for every refusal), so the criterion's antecedent holds and its obligation is live. The weak half was already gated by every_weak_resolver_site_states_which_resolver_and_why (#383); the strong half was not, so a reader standing at a canon_existing_ancestor site still could not see that a choice had been made. All six strong sites now carry a `resolver: `canon_existing_ancestor`` note with its reason, and the mirror gate enforces it with the same enclosing-function look-back and a sites>=6 anti-vacuity control. RED ARM, verbatim, with the gate added and the notes not yet written: `these `canon_existing_ancestor` call sites do not say which resolver they use or why` naming lines 1107, 1168, 1220, 1226, 1256 and 1260. The residual #383 named (is_project_secret / is_vcs_content_store on the weak resolver) was closed separately and its guard assertions are still in the weak gate."

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
`canon_existing_ancestor` and `canon_deep` is deleted, so there is now one
resolver and no choice for a later reader to make blind.

Graded against `origin/integ/next` at `43848f75`, after `lane/session-tickets`
merged in as `2165c30a`.
## Independently re-verified 2026-08-31 by lane f13-authority at 488fbbae9

c1/c2's red arm was RE-RUN: `is_skill_source_path` was switched back to the WEAK
resolver (`canon_for_scope` at both of its calls), `cargo check -p wcore-tools
--tests` returned RC=0, and

    panicked at crates/wcore-tools/src/workspace_policy/tests.rs:2373:5:
    a DANGLING symlink into a skill load path was not recognised -- the refusal
    judged where the LINK sits, not where the path leads

Recorded honestly rather than rounded up: under that SAME mutation
`a_parent_dir_after_a_missing_component_still_reaches_the_skill_load_path`
PASSED. Only ONE of the two escapes c1 names actually lands on the weak
resolver. c1 asks that the predicate be GRADED against both, and it is; c2 asks
for a red arm "for whichever escape lands", and exactly one landed.

c4 re-confirmed as written at HEAD rather than against the 0.13.12 sweep comment
that re-graded it `not-met`: both resolvers do remain, and every site of each
now states which one and why. `every_strong_resolver_site_states_which_resolver_and_why`
and `every_weak_resolver_site_states_which_resolver_and_why` are both green, and
the `is_project_secret` site that comment named as unlabelled carries
`#383 c3 -- resolver: `canon_for_scope`` with its reason at
`workspace_policy.rs:1880`.
