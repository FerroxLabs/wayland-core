---
issue: 402
repo: FerroxLabs/wayland-core
kind: defect
title: "core#356 c4's resolver gate is keyed to two literal names, so a third path resolver arrives ungated"
status: open
last_verified_commit: 7e159c955
criteria:
  - id: c1
    text: "Adding a path-resolving function to `workspace_policy.rs` that is not one of the two named resolvers fails a gate, rather than passing silently -- shown RED by adding one."
    state: met
    evidence: test:crates/wcore-tools/src/workspace_policy/tests.rs::resolver_inventory_covers_every_pathbuf_returning_fn
    owner: core
    note: "MET at HEAD 7e159c955 (lane f13-s3-vcs-gate). Rebuilt against THIS tree, not restored from lane/f13-authority, whose 453-line rewrite was dropped at merge -- and the RESOLVER INVENTORY block core#402 c3 was graded on went with it. `grep -rn 'RESOLVER INVENTORY' crates/wcore-tools/src/` returned 0 hits at c7f188c49 with a known-positive control (`grep -c canon_ancestor_only` -> 6) in the same call, so c3's `file:` anchor was resolving to a file that no longer held the content. Both are real again here.\nTHE GATE IS STRUCTURAL, not a third literal name -- which is the point, since a gate keyed to literals IS the defect this ticket reports. `resolver_inventory_covers_every_pathbuf_returning_fn` parses workspace_policy.rs, joins each function signature up to its body brace (so a `cargo fmt`-wrapped four-argument signature is read whole), takes the return type after the LAST `->` before the brace (so an `impl Fn(&Path) -> bool` argument cannot make a function classify by its callback), and keeps every function returning a SINGLE path: `PathBuf`, `Option<PathBuf>` or `Result<PathBuf, _>`. That set is compared with the inventory in BOTH directions -- a function with no row fails, a row with no function fails -- and every row must be classified `resolver` or `helper` with a reason of real length.\nA `resolver` ROW CARRIES core#356 c4 FROM THE TABLE: for each one the gate runs the existing `resolver_sites_without_a_reason` instrument over its own name, so a third resolver is ONE ROW and not a fourth hand-written test -- which is what the existing gate's own doc comment already promised and could not deliver.\nRED ARM, run as the criterion demands (`shown RED by adding one`) and ADDITIVE so it cannot pass by tripping an existing site count: add `fn canon_third(path: &Path) -> PathBuf` and call it from the predicate `is_repository_dir`. `cargo check -p wcore-tools --tests` RC=0, so the mutation genuinely compiles, and\n    these `-> PathBuf` functions are not in the RESOLVER INVENTORY block of workspace_policy.rs -- ... : canon_third (line 4248) -> PathBuf\nwhile `every_strong_resolver_site_states_which_resolver_and_why` and `every_weak_resolver_site_states_which_resolver_and_why` BOTH PASS in the same run (`3 passed; 1 failed`) -- the blindness this ticket describes, measured rather than modelled. A SECOND red arm proves the table half is load-bearing: strip the core#356 c4 note from the one call site of the third resolver row and the gate names that site while both name-keyed gates stay green.\nFOUND WHILE BUILDING IT, and the reason the inventory names THREE resolvers rather than two: `grantable_read_root_shape` resolves a host-supplied folder with bare `std::fs::canonicalize`, which is neither named resolver. Correct there -- a grant must name a folder that is there now -- and now stated at its call site. The parser also found two functions the eye missed, `vcs_store_entry` and `secret_entry`, whose multi-line signatures carry an `impl Fn(&Path) -> bool` argument.\nRestored after both mutations, `sha256sum -c` verified, `git status --porcelain` empty."
  - id: c2
    text: "The existing two gates keep their anti-vacuity site-count controls and stay green; `every_strong_resolver_site_states_which_resolver_and_why` and `every_weak_resolver_site_states_which_resolver_and_why` are not weakened to make c1 pass."
    state: met
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::every_weak_resolver_site_states_which_resolver_and_why"
    owner: core
    note: "MET AS WRITTEN, and measured under the SAME mutation rather than asserted. Both site gates keep their anti-vacuity site-count controls (canon_for_scope sites>=5, canon_existing_ancestor sites>=6) and neither was weakened: not one line of every_weak_resolver_site_states_which_resolver_and_why changed, and every_strong_resolver_site_states_which_resolver_and_why (added for #356 c4) is its mirror. Under the additive third-resolver mutation both stayed GREEN while the inventory gate went red -- `5 passed; 0 failed` -- which is exactly the blindness #402 describes: a gate keyed to one literal name cannot notice a name it was not given. The mutation was made ADDITIVE on purpose: a substituting mutation instead tripped the strong gate's own site count (6 -> 5), which is the control working but would not have demonstrated c2."
  - id: c3
    text: "The file's resolver inventory is stated explicitly, and `canon_ancestor_only` and `canon` are each classified as resolver or helper with the reason recorded where the gate reads it."
    state: met
    evidence: file:crates/wcore-tools/src/workspace_policy.rs
    owner: core
    note: "MET AS WRITTEN, and the block it is graded on is IN THE TREE again as of 7e159c955. It was not: the previous grade was earned on lane/f13-authority, whose rewrite was dropped at merge, and a `file:` anchor cannot notice that the content left. Verified absent at c7f188c49 by `grep -rn 'RESOLVER INVENTORY' crates/wcore-tools/src/` -> 0 hits with a known-positive control in the same call.\nThe RESOLVER INVENTORY block now classifies all EIGHTEEN single-path-returning functions -- not twelve; the structural parser found `vcs_store_entry`, `secret_entry` and the four `Result<PathBuf, _>` grant functions that a `-> PathBuf` eye scan misses -- as `resolver` or `helper`, each with its reason, and the gate reads the block from that same file.\nThe two the criterion names BY NAME are classified with the reason the previous decision recorded, unchanged: `canon_ancestor_only = helper` (the walk-UP-and-append-verbatim shape core#1097 abandoned; it follows no symlink, which is why `resolve_prefix` can call it without recursing, and it must never be reached from a predicate) and `canon = helper` (`canonicalize().unwrap_or(p)`, no missing-component handling and no link hop, used only for roots that already exist at construction). Resolvers: `canon_for_scope`, `canon_existing_ancestor`, and `grantable_read_root_shape`."

---

Created 2026-08-31 to close a COVERAGE gap; GRADED 2026-08-31 by lane
f13-authority, which took the work.

The criteria are the issue body's, restored to their full text -- the first
transcription truncated each at its first line break, so `c1` read "Adding a
path-resolving function to `workspace_policy.rs` that is not one of the two"
and stopped. Nothing was tightened or loosened; the missing halves were put
back.

The decision the ticket declined to make has been made and recorded where the
gate reads it: `canon_ancestor_only` and `canon` are HELPERS.
`canon_ancestor_only` is the walk-UP-and-append-verbatim shape `#1097`
abandoned and exists only so `resolve_prefix`'s hop walk can canonicalize what
already exists without recursing into itself; `canon` is
`canonicalize().unwrap_or(p)` with no missing-component handling and no link
hop, used only for roots that already exist at construction time. Neither
answers "where does this caller-supplied path land", so neither carries the
`#356` c4 call-site obligation.

The gate that enforces it is the inverted question, not a third literal name:
`resolver_inventory_covers_every_pathbuf_returning_fn` enumerates what the file
DEFINES and fails when the inventory and the file disagree in either
direction.
## Independently re-verified 2026-08-31 by lane f13-authority at 488fbbae9

c1's red arm was RE-RUN exactly as the criterion asks -- by ADDING a resolver,
not by substituting one. A third `fn canon_third(path: &Path) -> PathBuf` was
added and called from `is_skill_source_path` with no inventory entry and no
call-site note. `cargo check -p wcore-tools --tests` returned RC=0, so the
mutation genuinely compiled, and

    panicked at crates/wcore-tools/src/workspace_policy/tests.rs:3030:5:
    these `-> PathBuf` functions are not in the RESOLVER INVENTORY block of
    workspace_policy.rs -- classify each as `resolver` or `helper` with its
    reason, and if it is a resolver give its call sites the #356 c4 note
    (FerroxLabs/wayland-core#402 c1):
    ["canon_third"]

c2 measured UNDER THE SAME MUTATION rather than asserted separately. In that one
run `every_strong_resolver_site_states_which_resolver_and_why`,
`every_weak_resolver_site_states_which_resolver_and_why` and
`every_path_predicate_in_this_file_has_a_production_call_site` all PASSED while
the inventory gate alone went red -- `4 tests run: 3 passed, 1 failed`. Neither
site gate was weakened to make c1 pass, and their blindness to a name they were
not given is exactly what #402 describes, now measured rather than modelled.
