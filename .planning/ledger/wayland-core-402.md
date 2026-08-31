---
issue: 402
repo: FerroxLabs/wayland-core
kind: defect
title: "core#356 c4's resolver gate is keyed to two literal names, so a third path resolver arrives ungated"
status: closed
last_verified_commit: 7e159c955
criteria:
  - id: c1
    text: "Adding a path-resolving function to `workspace_policy.rs` that is not one of the two named resolvers fails a gate, rather than passing silently -- shown RED by adding one."
    state: met
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::resolver_inventory_covers_every_pathbuf_returning_fn"
    owner: core
    note: "MET, on the SECOND attempt, and the first grading's refutation is the reason this one is trustworthy. Attempt one gated on four literal return-type spellings while its own doc claimed the alphabet was structural; the second instrument compiled `fn canon_third(&self, path: &Path) -> std::io::Result<PathBuf>` -- the shape std::fs::canonicalize returns, and one workspace_policy.rs already uses -- and all three gates plus the whole 1879-test crate stayed green. RED ARM on the structural gate, same counter-example: `cargo check -p wcore-tools --tests` RC=0 FIRST so the red is behaviour and not a build break, then the inventory gate FAILS naming `canon_third (line 3134) -> std::io::Result<std::path::PathBuf>` (RC=101); restored with `git checkout --` plus `touch`, sha256sum -c OK, gate green again, `git status --porcelain` = 0. The gate also found a real pre-existing gap on its first run, which is the evidence it is not merely re-tuned to its own red arm: `dir_stamp` returns `Option<(PathBuf, SystemTime)>` and no whitelist of four spellings can see a path inside a tuple; classified `helper` with its reason. Wrong guesses on the collection list fail CLOSED."
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
