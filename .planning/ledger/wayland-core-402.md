---
issue: 402
repo: FerroxLabs/wayland-core
kind: defect
title: "core#356 c4's resolver gate is keyed to two literal names, so a third path resolver arrives ungated"
status: open
last_verified_commit: 4a738f2e
criteria:
  - id: c1
    text: "Adding a path-resolving function to `workspace_policy.rs` that is not one of the two named resolvers fails a gate, rather than passing silently -- shown RED by adding one."
    state: met
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::resolver_inventory_covers_every_pathbuf_returning_fn"
    owner: core
    note: "MET AS WRITTEN, and shown RED by adding one exactly as the criterion asks. workspace_policy.rs now carries an explicit RESOLVER INVENTORY block and the gate enumerates every `-> PathBuf` / `-> Option<PathBuf>` function the file defines, comparing that set against the inventory in BOTH directions (a function with no entry fails; an entry naming a function the file no longer defines fails, so the inventory cannot rot into a lie). RED ARM: a third resolver `fn canon_third(path: &Path) -> PathBuf` was ADDED (not substituted) and called from is_skill_source_path with no note; the mutation compiles (cargo check RC=0) and the gate fails verbatim with `these `-> PathBuf` functions are not in the RESOLVER INVENTORY block of workspace_policy.rs ... ['canon_third']`."
  - id: c2
    text: "The existing two gates keep their anti-vacuity site-count controls and stay green; `every_strong_resolver_site_states_which_resolver_and_why` and `every_weak_resolver_site_states_which_resolver_and_why` are not weakened to make c1 pass."
    state: met
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::every_weak_resolver_site_states_which_resolver_and_why"
    owner: core
    note: "MET AS WRITTEN, and measured under the SAME mutation rather than asserted. Both site gates keep their anti-vacuity site-count controls (canon_for_scope sites>=5, canon_existing_ancestor sites>=6) and neither was weakened: not one line of every_weak_resolver_site_states_which_resolver_and_why changed, and every_strong_resolver_site_states_which_resolver_and_why (added for #356 c4) is its mirror. Under the additive third-resolver mutation both stayed GREEN while the inventory gate went red -- `5 passed; 0 failed` -- which is exactly the blindness #402 describes: a gate keyed to one literal name cannot notice a name it was not given. The mutation was made ADDITIVE on purpose: a substituting mutation instead tripped the strong gate's own site count (6 -> 5), which is the control working but would not have demonstrated c2."
  - id: c3
    text: "The file's resolver inventory is stated explicitly, and `canon_ancestor_only` and `canon` are each classified as resolver or helper with the reason recorded where the gate reads it."
    state: met
    evidence: "file:crates/wcore-tools/src/workspace_policy.rs"
    owner: core
    note: "MET AS WRITTEN. The RESOLVER INVENTORY block in workspace_policy.rs classifies all twelve `-> PathBuf` functions as `resolver` or `helper` with a reason on each, and the gate reads the block from that same file. The two the criterion names BY NAME are classified and the gate asserts both are present: `canon_ancestor_only = helper` (it IS the walk-UP-and-append-verbatim shape #1097 abandoned, and it is a private step of canon_existing_ancestor / resolve_prefix -- it exists so the hop walk can canonicalize what already exists without recursing into itself, and must never be called from a predicate) and `canon = helper` (canonicalize().unwrap_or(p), no missing-component handling and no link hop, used only for roots that already exist at construction time). Resolvers: canon_for_scope, canon_existing_ancestor, resolve, resolve_against."

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
