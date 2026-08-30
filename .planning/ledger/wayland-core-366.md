---
issue: 366
repo: FerroxLabs/wayland-core
kind: defect
title: "Container orphan scan is nonce-scoped, so it can never see a leftover from an earlier run"
status: open
last_verified_commit: afc30e3e8
criteria:
  - id: d1
    text: "The product can enumerate wayland-created containers WITHOUT being given a nonce -- a key-presence scan reachable from an operator-facing surface, not only from a nonce-scoped path"
    state: met
    evidence: "symbol:crates/wcore-exec-backend/src/backends/container.rs::list_all_labelled_containers"
    owner: core
    note: "The key-presence filter `label=wayland.task.nonce` with no =value, reached through ContainerBackend::scan_orphans_any_nonce, dispatched by ExecutionBackend::scan_orphans_in_scope(OrphanScope::AnyNonce), through orphan::scan_all, and from BOTH operator surfaces -- `backend orphans`, whose --nonce is now optional, and `backend scan`, whose --task-id is now optional; each defaults to the UNSCOPED scan. RE-DERIVED, because the first version of this note named `orphan::scan_all_any_nonce` as the middle link and that function had ZERO callers: a chain written from memory listed a step the tree did not contain, and the surface it was supposed to serve stayed broken. Every link above is now grep-checkable and the aggregate takes the scope as a parameter, so there is only one of it. The method string is DERIVED from the filter that actually ran rather than restated: this lane's own red arm mutated the filter to a value scope and the hand-written string still claimed KEY PRESENCE, which is an operator-facing account that can disagree with the enumeration it describes."
  - id: d2
    text: "The nonce-scoped scan_orphans(nonce) contract is left intact for the caller that genuinely wants one run's orphans (cancel()); the unscoped scan is an addition, not a widening. State which callers move and which do not"
    state: met
    evidence: "test:crates/wcore-exec-backend/tests/orphan_scope_routing.rs::every_scope_reaches_the_enumeration_it_names"
    owner: core
    note: "REFUTED TWICE, ON THE SAME SHAPE ONE LAYER DOWN EACH TIME, AND THIS NOTE RECORDS BOTH. ROUND 1: the note was an enumeration written from memory -- it named three callers, the tree held five, and one it missed (orphan::scan_all, behind `wayland-core backend scan`, the scriptable F25-05 gate) printed `count 0 (MEASURED)` over a labelled leftover. Remedy: contract::OrphanScope {Nonce(&str), AnyNonce} became a required parameter of every entry point that is not a backend enumerating its own surfaces -- orphan::scan_all, orphan::scan_one, ExecutionBackend::scan_orphans_in_scope -- so a caller that does not state its scope does not COMPILE. ROUND 2: that left exactly ONE hand-written decision, the two arms of scan_orphans_in_scope. A reviewer mutated the unscoped arm to `self.scan_orphans(<a nonce nobody holds>)`; it compiled (cargo check --all-targets exit 0, 0 errors), 4068 tests passed, and the shipped binary reverted to `count 0 (MEASURED)` over a real labelled leftover. Every test of the unscoped scan called ContainerBackend::scan_orphans_any_nonce() DIRECTLY and never routed, and this criterion's own guard checks the SPELLING (nobody writes a bare scan_orphans outside the crate) rather than the MEANING (a caller that says AnyNonce reaches the unscoped enumeration). COMPLETE CALLER SET, DERIVED NOT RECALLED. `grep -rnE '\.scan_orphans(_any_nonce|_in_scope)?\(|orphan::scan_(all|one)\(' --include=*.rs .` over the WHOLE repo, not crates/ -- 17 code lines. CONTROLS: the bare symbol `scan_orphans` returns 62 lines repo-wide, so the query is not looking at an empty tree; and a planted ./plantctl-w2.rs containing `.scan_orphans(` WAS returned by the same command before removal, so it can see a file outside crates/ (an empty result outside crates/ would otherwise read as absence). MOVES, asks AnyNonce by default now: wcore-cli/src/backend.rs `backend orphans` and wcore-cli/src/backend.rs `backend scan` (--task-id became optional), both through the single conversion fn orphan_scope(), which is now PINNED BY A TEST -- OrphanScope::AnyNonce was constructed in exactly one place in the entire tree and never in a test until this pass. MEASURED on hetzner against a planted `wayland.task.nonce=s2fix-live-nonce-nobody-holds` container with an empty state dir: no flag -> `count 1 (MEASURED)`, row named LEFTOVER, exit 1; `--task-id s2fix-some-other-task` -> container `count 0 (MEASURED)`, exit 0. INTERNAL, carry the caller's scope: orphan::scan_all and orphan::scan_one. THE DISPATCHER: contract.rs, two arms, now fail-closed (below). DOES NOT MOVE, asks Nonce deliberately: conformance.rs:358, the scoped liveness arm that says so in its own assertion text (d4); tests/fail_closed_matrix.rs, which plants the process and holds the nonce; tests/container_wedge.rs, the scoped half of the d5 pair. DELIBERATELY UNSCOPED: conformance.rs 5b, container_wedge's two direct calls, and the new routing test. NOT IN THAT GREP AT ALL, and this is the round-1 miss in a different form: every backend's cancel() re-enumerates by the cancelled task's own nonce through a backend-PRIVATE helper (container list_containers_with_nonce, ssh remote_scan, cloud machines_with_nonce, local its registry+pid check), NOT through the trait method. So `the scan_orphans contract does not move` is true for a different reason than the earlier note implied: cancel() never calls it. The boundary still holds because none of those helpers is reachable from outside -- `grep -rnE 'pub (async )?fn (list_|remote_scan|machines_with)' crates/wcore-exec-backend/src/` returns ZERO lines while the unfiltered grep returns all four declarations, so outside the crate the only route to a nonce-scoped enumeration is the pub trait method the guard covers. WHAT CLOSES ROUND 2. (1) OrphanScope::nonce() is the single statement of what a scope MEANS, and scan_orphans_in_scope now checks the scan it got back against it: an answer whose declared scope differs from the scope asked is an ExecError, not a MEASURED zero downstream. (2) tests/orphan_scope_routing.rs asserts, for every scope, that the nonce handed to the backend equals the nonce the scope names -- total over the enum, no case per variant, no per-arm table -- plus a positive control that a disagreeing answer is refused, plus scan_all carrying the caller's scope to all four backends on any host. (3) container_wedge gains the end-to-end arm through orphan::scan_all that the mutation broke. RED ARMS, each COMPILED first (cargo check --all-targets exit 0, 0 `^error`). RA-A the reviewer's own mutation, AnyNonce -> scan_orphans(s2fix-mutant-nonce): orphan_scope_routing::every_scope_reaches_the_enumeration_it_names exit 100 and container_wedge::the_operator_chain_finds_the_leftover_end_to_end exit 100 (both were green under it before). RA-B CLI default None -> Nonce(...): absent_operator_input_means_the_unscoped_scan exit 100 while its sibling control still passes. RA-C scan_all substituting Nonce(s2fix-aggregate-mutant) for the caller's scope: scan_all_carries_the_callers_scope_to_every_backend exit 100, naming backend `cloud`. ANTI-ROT, and the hand-written list inside it is gone too: orphan_scope_callers.rs now reads the forbidden spellings out of the ExecutionBackend declaration -- every trait method returning Result<OrphanScan> whose signature does not name an OrphanScope -- instead of a two-element array whose only control caught a RENAME. RA-D added a THIRD scope-implicit trait method plus a caller in wcore-cli: it compiled and the guard went red naming wcore-cli/src/backend.rs, which the two-element array could not have seen. The guard also walks the 57 Cargo.toml workspace members rather than crates/, which was already blind to a real one: `workspace-hack` is the one member not under crates/. RA-F planted a scope-implicit call in workspace-hack/src/ -- A/B on the identical plant: PRE-FIX guard restored from origin exit 0 (1 test passed), fixed guard exit 100 naming the file."
  - id: d3
    text: "An operator surface reports a leftover it did not create in this process -- one whose nonce is not in the live registry"
    state: met
    evidence: "test:crates/wcore-exec-backend/tests/container_wedge.rs::a_container_this_process_still_holds_is_not_reported_as_a_leftover"
    owner: core
    note: "Each row is GRADED against the live-task registry, not merely listed: a nonce no live task in this process carries is reported as LEFTOVER. This criterion is anchored to the POLARITY test rather than to the find test, because `mark every row LEFTOVER` would satisfy the find test while turning the operator surface into noise that names live work as garbage. RED ARM M3 forced the grading to LEFTOVER unconditionally and reddened this test and only this test."
  - id: d4
    text: "The conformance check at conformance.rs:340 is re-examined: it asserts enumerated && found.is_empty() for a nonce chosen so nothing can ever be found"
    state: met
    evidence: "file:crates/wcore-exec-backend/src/conformance.rs:356:proves no orphan-detection property"
    owner: core
    note: "BOTH remedies the ticket allowed, because either alone leaves half the defect. The arm is renamed and its limit stated in the assertion text itself, so it cannot be read as orphan-scan coverage; and the find-an-orphan arm it could never host -- it needs a labelled surface planted under a stranger's nonce, which a provider-neutral body cannot build -- now exists in container_wedge.rs. ANTI-VACUITY, since this criterion is itself about a check that cannot fail: RED ARM M5 set the container's scoped scan to enumerated=false and this arm went RED, so the surviving half is load-bearing rather than decorative; RED ARM M2 collapsed the key-presence filter to a value scope and the new find arm went RED."
  - id: d5
    text: "A regression test plants a labelled leftover under a nonce the running process has never used, and asserts the unscoped scan reports it -- creating the leftover itself and cleaning up after itself"
    state: met
    evidence: "test:crates/wcore-exec-backend/tests/container_wedge.rs::the_unscoped_scan_finds_a_leftover_from_a_run_this_process_never_made"
    owner: core
    note: "Plants its own Created container under a nonce an empty temp registry guarantees this process has never held, removes it BEFORE any assertion so a red leaves the next lane nothing, and runs the SCOPED scan against the same planted container in the same call -- the pair of answers is the assertion, because asserting only that the new scan finds it would pass against a scan that finds everything and would say nothing about a defect whose shape is `scoped to a nonce no leftover can carry`. RED ARM M2 (filter collapsed to a value scope) reddens it with `the unscoped scan must FIND the planted leftover. It reported []`."
  - id: d6
    text: "Whether reclamation is in scope is DECIDED and recorded: state whether an unscoped scan only reports, or also reclaims, and justify it against #365's guard"
    state: met
    evidence: "file:crates/wcore-exec-backend/src/contract.rs:405:This REPORTS. It does not reclaim"
    owner: core
    note: "DECIDED: it reports and never reclaims. #365's submit-path reclaim may remove a conflicting container because it holds the exact task id it is about to use, which is a claim on that name. An unscoped scan holds no claim on anything it finds -- the label says `some wayland run created this`, not `this run is over` -- and a found surface may belong to a LIVE task in another process (the registry is per-process) or to another tenant of a shared daemon. Destroying either is a worse failure than the leak this exists to surface."
---

Split out of #365 c6. The unscoped scan is an ADDITION: `scan_orphans` stays nonce-scoped
for `cancel()`, which wants exactly one run's residue. Backends with no unscoped
enumeration inherit a default that answers `enumerated: false` -- never a silent zero,
because an un-enumerated surface is not a clean surface.

WHAT THE SECOND PASS CHANGED. The first pass fixed the one caller the ticket named and
added `orphan::scan_all_any_nonce` for the rest. That function shipped with ZERO callers
while `wayland-core backend scan` -- the other operator surface, and the scriptable one --
still reported a MEASURED zero over a real labelled leftover. Adding a parallel unscoped
function beside a scoped one leaves the scope a DEFAULT, and a default is what nobody
chooses and nobody reviews. The scope is now a parameter with no default
(`contract::OrphanScope`), the parallel function is deleted, and which callers ask which
question is answered by the compiler rather than by this file.

SWEEP for the same shape elsewhere in this lane: d1 above named a chain link that did not
exist (fixed); 362 c4 claimed a completeness its derivation could not deliver (fixed in
that ledger). One more was found OUTSIDE either ticket's criteria and fixed while the file
was open -- `orphan::tests::only_the_three_proven_mechanism_names_are_ever_spelled`
compared a hand-written `allowed` array against the hand-written string literals in
`mechanism_for`, so neither an upstream rename nor a fourth variant could redden it, and
this crate depends on `wcore-sandbox` so the authority was reachable all along. The names
are now derived from `ProcessTreeMechanism`'s `Debug` behind a wildcard-free match, which
turns both rot modes into compile errors. That string is printed to an operator by
`backend scan` as the mechanism a backend relies on.

THIRD PASS. The scope parameter moved the caller set from a note to the compiler and left one
hand-written decision behind -- the dispatcher's two arms -- which a reviewer mutated to
restore the shipped defect with the whole suite green. That seam is now covered by a property
total over the enum rather than by a case per arm, and the dispatcher refuses an answer whose
declared scope disagrees with the scope it was asked. SWEEP for the same shape again: the
anti-rot guard's own two-element list of forbidden spellings is derived from the trait
declaration; both this ticket's guard and core#362 c4's walk the Cargo.toml members instead of
crates/ (which was already blind to `workspace-hack`); and 362 c4's path-suffix exemption is
now the definition site. One product defect outside either ticket's criteria was found by the
same reviewer and fixed here: the local process-table scan excluded only its own pid, so the
nonce on the scanner's argv came back as a MEASURED orphan for every wrapper that invoked it.
A/B on the deciding condition, two binaries fingerprinted by sha256, same wrapper: pre-fix
predicate `local count 1 (MEASURED)` naming the invoking shell, lineage-walking predicate
`local count 0 (MEASURED)`, with a planted non-ancestor orphan still found by both.

Closing the GitHub issue is Sean's action, not a lane's.
