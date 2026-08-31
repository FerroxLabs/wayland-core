---
issue: 356
repo: FerroxLabs/wayland-core
kind: defect
title: "Two path resolvers with different escape properties 70 lines apart, and the weaker one guards the skill-source write refusal"
status: open
last_verified_commit: 10de774f
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
    note: "MET 2026-08-31 (lane f13-s2-policy-resolvers). The conditional DOES arise: both resolvers remain -- canon_existing_ancestor and canon_for_scope -- and at ca15a48bf only the WEAK one's call sites were labelled, by every_weak_resolver_site_states_which_resolver_and_why. Half of c4's sentence (\"the reason EACH call site picked one\") was therefore ungraded. Every strong site now carries a `#356 c4 - resolver: `canon_existing_ancestor`` note stating what the weaker resolver would get wrong THERE: is_repo_control_path, is_skill_source_path (x2), ensure_write_target_readable (x2), the counted wrapper WorkspacePolicy::resolve, and the three self.resolve callers (is_project_secret, is_vcs_content_store, denies_read_content) -- 9 sites, and the gate's anti-vacuity control is sites >= 8. self.resolve is scanned as a second needle because it IS canon_existing_ancestor under a counter, so gating only the bare name left that door open. RED ARM, cargo check -p wcore-tools --tests RC=0 first: deleting the marker from ensure_write_target_readable made every_strong_resolver_site_states_which_resolver_and_why RED while every_weak_resolver_site_states_which_resolver_and_why stayed GREEN -- the pairing is the proof the new gate is the load-bearing one and the existing one was not weakened to pass it. The residual #383 named (is_project_secret / is_vcs_content_store on the weak resolver) is already closed in this base: both go through self.resolve. Full suite after: cargo nextest run -p wcore-tools -p wcore-cli, 5701 passed, 0 failed."
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


---

**Updated 2026-08-31, lane `f13-s2-policy-resolvers` @ `10de774f`.** c4 is now met, and
the prose above is corrected: the file holds TWO resolvers, not one.
`canon_deep` is gone, but `canon_for_scope` is a resolver with weaker escape
properties than `canon_existing_ancestor` and it is still chosen at six sites.
c4's conditional therefore applies, and its second half — the reason each call
site picked one — was only stated on the weak resolver's sites. Every strong
site now states it too, gated by
`every_strong_resolver_site_states_which_resolver_and_why`, and the class is
backed by `the_resolver_inventory_covers_every_path_resolving_function_in_this_file`
so a THIRD resolver cannot arrive unclassified (FerroxLabs/wayland-core#402).
