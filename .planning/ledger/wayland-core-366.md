---
issue: 366
repo: FerroxLabs/wayland-core
kind: defect
title: "Container orphan scan is nonce-scoped, so it can never see a leftover from an earlier run"
status: open
last_verified_commit: bc63e94ad
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
    evidence: "test:crates/wcore-exec-backend/tests/orphan_scope_callers.rs::no_surface_outside_the_contract_crate_asks_the_scanner_without_a_scope"
    owner: core
    note: "THE FIRST VERSION OF THIS NOTE WAS AN ENUMERATION WRITTEN FROM MEMORY. It named three callers; the tree held five, and one of the two it missed -- orphan::scan_all, behind `wayland-core backend scan`, the F25-05 gate whose non-zero exit is the scriptable one -- printed `count 0 (MEASURED)` with a labelled leftover sitting in `docker ps -a`. A grep for `scan_orphans` cannot find it either, because it reaches the scanner through the aggregate. SO THE LIST IS GONE. contract::OrphanScope {Nonce(&str), AnyNonce} is now a required parameter of every entry point that is not a backend enumerating its own surfaces -- orphan::scan_all, orphan::scan_one and ExecutionBackend::scan_orphans_in_scope -- so a caller that does not state its scope does not COMPILE, and `cargo check --workspace --all-targets` (exit 0) enumerates the caller set instead of a note. DERIVED SET, `grep -rnE '.scan_orphans(_any_nonce|_in_scope)?\\(|orphan::scan_(all|one)\\(' --include=*.rs crates/`, control 11 files match the bare symbols so the query works. MOVES (asks AnyNonce by default now): wcore-cli `backend orphans`, and wcore-cli `backend scan`, whose --task-id became optional -- MEASURED on hetzner against a planted `wayland.task.nonce=s2fix-nonce-nobody-holds` container with an empty state dir: `backend scan` reports `count 1 (MEASURED)`, names the row LEFTOVER and exits 1, where the same command reported `count 0` before. DOES NOT MOVE (asks Nonce, deliberately): ContainerBackend::cancel(), which re-enumerates by the cancelled task's own nonce to verify its docker rm -f and would report other tasks' containers as its residual if widened -- contract.rs still says `core#366 d2: this contract does not move`; the conformance scoped arm (conformance.rs:358), which keeps its own fresh nonce as a LIVENESS check and says so; tests/fail_closed_matrix.rs, which plants the process itself and holds the nonce; and tests/container_wedge.rs, which exercises both contracts on purpose. NOT A WIDENING: scan_orphans(nonce) is byte-identical and scan_orphans_in_scope only dispatches. ANTI-ROT, because the type closes the aggregate path but the raw method is still pub: tests/orphan_scope_callers.rs refuses any scope-implicit call from OUTSIDE wcore-exec-backend -- a path predicate, not a list of names, so the sixth caller needs no maintenance. RED ARM: re-adding the pre-fix `reference.backend.scan_orphans(..)` to wcore-cli compiled (cargo check exit 0) and reddened it, naming crates/wcore-cli/src/backend.rs."
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

Closing the GitHub issue is Sean's action, not a lane's.
