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
    note: "The key-presence filter `label=wayland.task.nonce` with no =value, reached through the new ExecutionBackend::scan_orphans_any_nonce, through orphan::scan_all_any_nonce, and from the operator surface `wcore backend orphans` -- whose --nonce is now optional, and whose default is the UNSCOPED scan. The method string is DERIVED from the filter that actually ran rather than restated: this lane's own red arm mutated the filter to a value scope and the hand-written string still claimed KEY PRESENCE, which is an operator-facing account that can disagree with the enumeration it describes."
  - id: d2
    text: "The nonce-scoped scan_orphans(nonce) contract is left intact for the caller that genuinely wants one run's orphans (cancel()); the unscoped scan is an addition, not a widening. State which callers move and which do not"
    state: met
    evidence: "file:crates/wcore-exec-backend/src/contract.rs:349:this contract does not move"
    owner: core
    note: "MOVES: the CLI `orphans` command, which now defaults to unscoped and takes --nonce to opt back in. DOES NOT MOVE: ContainerBackend::cancel(), which re-enumerates by the cancelled task's own nonce to verify its docker rm -f and would report other tasks' containers as its residual if widened; and the conformance harness's scoped arm, which keeps its own nonce. OrphanScan.nonce became Option<String> so `every run` and `one named run` are different values rather than one string a reader has to guess at -- a sentinel would have preserved exactly the confusion that let this defect stand."
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
    evidence: "file:crates/wcore-exec-backend/src/contract.rs:365:This REPORTS. It does not reclaim"
    owner: core
    note: "DECIDED: it reports and never reclaims. #365's submit-path reclaim may remove a conflicting container because it holds the exact task id it is about to use, which is a claim on that name. An unscoped scan holds no claim on anything it finds -- the label says `some wayland run created this`, not `this run is over` -- and a found surface may belong to a LIVE task in another process (the registry is per-process) or to another tenant of a shared daemon. Destroying either is a worse failure than the leak this exists to surface."
---

Split out of #365 c6. The unscoped scan is an ADDITION: `scan_orphans` stays nonce-scoped
for `cancel()`, which wants exactly one run's residue. Backends with no unscoped
enumeration inherit a default that answers `enumerated: false` -- never a silent zero,
because an un-enumerated surface is not a clean surface.

Closing the GitHub issue is Sean's action, not a lane's.
