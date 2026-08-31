---
issue: 356
repo: FerroxLabs/wayland-core
kind: defect
title: "Two path resolvers with different escape properties 70 lines apart, and the weaker one guards the skill-source write refusal"
status: open
last_verified_commit: 50c6aad6
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
    evidence: test:crates/wcore-tools/src/workspace_policy/tests.rs::every_strong_resolver_site_states_which_resolver_and_why
    owner: core
    note: "Both resolvers remain, so the obligation is symmetric and was being graded on ONE side only. #383 built every_weak_resolver_site_states_which_resolver_and_why over canon_for_scope call sites; a reader standing at a canon_existing_ancestor call could no less see that a choice had been made. The instrument is now a shared function resolver_sites_without_a_reason(source, needle, marker) driven from a table, so a THIRD resolver is one row, and all six canon_existing_ancestor call sites carry `resolver: `canon_existing_ancestor`` with the reason at the site: fn resolve (both SecretDenyFs guard predicates are refusals, so they must judge where a path lands), is_repo_control_path, is_skill_source_path, ensure_write_target_readable (both sides of the comparison). Enclosing-function look-back, not a fixed window, for the reason #383 records. RED ARM: one marker removed from fn resolve; cargo check exit 0; the gate names the site. RE-DERIVED 13d36be65 (not inherited): the `resolver: `canon_existing_ancestor`` marker deleted from fn resolve; cargo check -p wcore-tools --tests exit 0; every_strong_resolver_site_states_which_resolver_and_why fails naming the site. Restored, blob identity verified equal to the commit under test."
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
