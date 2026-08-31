---
issue: 402
repo: FerroxLabs/wayland-core
kind: defect
title: "core#356 c4's resolver gate is keyed to two literal names, so a third path resolver arrives ungated"
status: open
last_verified_commit: 10de774f
criteria:
  - id: c1
    text: "c1: Adding a path-resolving function to `workspace_policy.rs` that is not one of the two named resolvers fails a gate, rather than passing silently -- shown RED by adding one."
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::the_resolver_inventory_covers_every_path_resolving_function_in_this_file"
    state: met
    owner: core
    note: "TEXT REPAIRED 2026-08-31: the transcription that created this ledger truncated every criterion at the first line break, so c1 read \"Adding a path-resolving function to workspace_policy.rs that is not one of the two\" -- half a sentence. Restored verbatim from the issue body; no wording was tightened. MET the same day (lane f13-s2-policy-resolvers). The gate is the INVERSION the ticket asks for: it derives the set of path-resolving functions the file DEFINES (any fn whose signature takes a path-typed parameter AND returns a PathBuf, signatures joined across lines) and requires it to equal RESOLVER_INVENTORY exactly, in both directions -- an undeclared function fails, and so does a stale entry. It is keyed to no name. RED ARM, cargo check -p wcore-tools --tests RC=0 first, and chosen to touch NEITHER gated name: adding `fn canon_v4(path: &Path) -> PathBuf { std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()) }` -> inventory gate RED, every_strong_resolver_site_states_which_resolver_and_why GREEN, every_weak_resolver_site_states_which_resolver_and_why GREEN. That pairing IS the ticket's claim measured: the two name-keyed gates cannot see a name they were not given, and the structural one can. (A second arm using canon_for_scope internally went RED on the weak gate too, which is why the no-shared-name arm is the one that counts.) The scanner also found two multi-line signatures a hand grep missed -- vcs_store_entry and secret_entry -- both now classified."
  - id: c2
    text: "c2: The existing two gates keep their anti-vacuity site-count controls and stay green; every_strong_resolver_site_states_which_resolver_and_why and every_weak_resolver_site_states_which_resolver_and_why are not weakened to make c1 pass."
    evidence: "test:crates/wcore-tools/src/workspace_policy/tests.rs::every_weak_resolver_site_states_which_resolver_and_why"
    state: met
    owner: core
    note: "TEXT REPAIRED 2026-08-31 from the issue body (see c1). MET, with one correction to the ticket's premise, measured: at ca15a48bf only ONE of the two gates it names existed. every_weak_resolver_site_states_which_resolver_and_why is in the tree with its `sites >= 5` control; every_strong_resolver_site_states_which_resolver_and_why was NOT -- `grep -rn every_strong_resolver crates/` returned nothing. So \"the existing two gates\" was one gate. It is now genuinely two: the weak gate is BYTE-FOR-BYTE UNCHANGED (no refactor, no shared helper extracted from it -- that was the cheapest way to make \"not weakened\" checkable rather than argued), and the strong one is new with its own `sites >= 8` control plus the same definition-must-exist known-positive. Both green in the same run as the inventory gate, and each went RED alone under the other's red arm: the strong-site marker deletion (356 c4) leaves the weak gate GREEN, and the canon_v4 arm leaves BOTH green while the inventory gate goes red."
  - id: c3
    text: "c3: The file's resolver inventory is stated explicitly, and `canon_ancestor_only` and `canon` are each classified as resolver or helper with the reason recorded where the gate reads it."
    evidence: "symbol:crates/wcore-tools/src/workspace_policy/tests.rs::RESOLVER_INVENTORY"
    state: met
    owner: core
    note: "TEXT REPAIRED 2026-08-31 from the issue body (see c1). MET. RESOLVER_INVENTORY is a const table IN THE TEST FILE -- which is literally where the gate reads it -- holding all 19 path-resolving functions the file defines, each with a class and a reason the gate requires to be non-trivial. The two the ticket names by hand are decided, not left open: `canon` is NonJudgement (fs::canonicalize with the input as fallback; every call site is a ROOT normalized once at construction, never a path arriving from a tool call, and it has no missing-component handling at all), and `canon_ancestor_only` is WalkInternal to resolve_prefix (it IS the walk-UP-and-append shape #1097 abandoned, sound only as the terminating step of the hop walk). A WalkInternal claim is CHECKED, not taken: the gate resolves each call site's enclosing function and fails if it is not the declared owner. RED ARM, cargo check RC=0 first: rewriting ensure_write_target_readable to call canon_ancestor_only directly -- the exact regression #356 was filed for -- made the inventory gate RED. resolve_prefix and lexical_normalize are classified the same way; the remaining entries are NotAResolver, each with the reason it delegates rather than resolves."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.


---

**Updated 2026-08-31, lane `f13-s2-policy-resolvers` @ `10de774f`.** The paragraph
above is superseded: this ledger now records work. All three criteria are met,
and the criterion TEXT has been repaired — the transcription that created this
file cut every criterion at its first line break, so all three read as half
sentences. They are restored verbatim from the issue body with no tightening.

One correction to the ticket's premise, measured at the base commit: only ONE
of the two gates it names existed. `every_strong_resolver_site_states_which_resolver_and_why`
was not in the tree. It is now, alongside the untouched weak gate and a
structural inventory gate that is keyed to no name at all.
