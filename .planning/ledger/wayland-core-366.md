---
issue: 366
repo: FerroxLabs/wayland-core
kind: defect
title: "Container orphan scan is nonce-scoped, so it can never see a leftover from an earlier run"
status: open
last_verified_commit: 483a4dcaf
criteria:
  - id: d1
    text: "The product can enumerate wayland-created containers WITHOUT being given a nonce -- a key-presence scan reachable from an operator-facing surface, not only from a nonce-scoped path"
    state: met
    evidence: symbol:crates/wcore-exec-backend/src/backends/container.rs::scan_all_orphans
    owner: core
    note: "MET. `ContainerBackend::scan_all_orphans` issues `docker ps -a --filter label=wayland.task.nonce` -- key PRESENCE, no value -- and is reached from the operator surface `wayland backend orphans` with NO --nonce (crates/wcore-cli/src/backend.rs:488, the only production call site). PROVEN ON THE REAL BINARY, not the helper: with a container planted under nonce lane-f13-s2-linux-sandbox-d3-1788178023, `wayland-core backend orphans --nonce <fresh>` reported `found=0` and `wayland-core backend orphans` reported `found=2` including that container. Wrong-refusal control in the same exercise: an UNLABELLED container planted alongside is NOT reported (the_unscoped_scan_ignores_a_container_this_backend_did_not_label), so the widened filter does not sweep in a co-tenant's work."
  - id: d2
    text: "The nonce-scoped scan_orphans(nonce) contract is left intact for the caller that genuinely wants one run's orphans (cancel()); the unscoped scan is an addition, not a widening. State which callers move and which do not"
    state: met
    evidence: file:crates/wcore-exec-backend/src/backends/container.rs:852:WHICH CALLERS MOVE: NONE
    owner: core
    note: "MET, and the disposition is stated at the method. NO CALLER MOVES. Verified by grepping every production call site of `.scan_orphans(` (4: wcore-cli/src/backend.rs:446, wcore-exec-backend/src/orphan.rs:230 and :246, wcore-exec-backend/src/conformance.rs:367) -- all unchanged; `.scan_all_orphans(` has exactly 1 production call site, the new CLI arm. CORRECTION MADE IN THIS LANE: the first draft of the doc said cancel() calls `scan_orphans(entry.nonce)`. It does not -- it calls `list_containers_with_nonce(&entry.nonce)` directly at container.rs:790, the same query scan_orphans wraps. The comment now says that, because d2 asks for the disposition to be STATED and a stated falsehood is worse than silence."
  - id: d3
    text: "An operator surface reports a leftover it did not create in this process -- one whose nonce is not in the live registry"
    state: met
    evidence: symbol:crates/wcore-cli/src/backend.rs::all_orphans
    owner: core
    note: "MET on the operator surface, not on a helper. Each row is graded against `registry::list()` and rendered as `-- NOT held by any live task in this process`. MEASURED with the built binary against a planted container whose nonce this process never used: `orphan366-d3proof-lane-f13-s2 (nonce lane-f13-s2-linux-sandbox-d3-1788178023) -- NOT held by any live task in this process`. The scoped scan for a fresh nonce reported found=0 in the same exercise, which is the defect stated as a measurement."
  - id: d4
    text: "The conformance check at conformance.rs:340 is re-examined: it asserts enumerated && found.is_empty() for a nonce chosen so nothing can ever be found"
    state: met
    evidence: symbol:crates/wcore-exec-backend/src/conformance.rs::SCOPED_SCAN_CHECK
    owner: core
    note: "MET, and the answer is: YES, IT WAS VACUOUS, and it is now said so in the source. The nonce is `{id_prefix}-nonce-never-used`, chosen so nothing can ever have run under it, so `found.is_empty()` was true BY CONSTRUCTION -- no state of any backend and no state of the host, however dirty, could falsify it. The whole verdict was the `enumerated` half. The vacuous conjunct is REMOVED from the assertion (the count is still reported in the detail, as an observation rather than a claim) and the check is renamed to state its limit so it is not read as orphan-scan coverage. The find-a-real-leftover arm it could never carry is tests/unscoped_orphan_scan.rs -- backend-specific by necessity, since planting a leftover means creating one the way that one backend creates them."
  - id: d5
    text: "A regression test plants a labelled leftover under a nonce the running process has never used, and asserts the unscoped scan reports it -- creating the leftover itself and cleaning up after itself"
    state: met
    evidence: test:crates/wcore-exec-backend/tests/unscoped_orphan_scan.rs::the_unscoped_scan_finds_a_leftover_from_a_nonce_this_process_never_used
    owner: core
    note: "MET. `the_unscoped_scan_finds_a_leftover_from_a_nonce_this_process_never_used` plants a labelled container under a per-pid, per-nanosecond nonce, requires the unscoped scan to report it with in_live_registry=false, and removes it from a Drop guard so an unwinding assertion cannot leave residue. RED ARM, and it COMPILES (cargo check -p wcore-exec-backend --tests RC=0 with the mutation applied, so the red is behaviour and not a build break): with the filter reverted to key-EQUALITY on a fresh nonce -- the pre-fix shape -- both tests FAIL with `the unscoped scan did not report the planted leftover orphan366-leftover-4023122 ... rows=[]`. Restored + touched, both PASS. Two controls in the bodies: the scoped scan given the leftover's OWN nonce DOES find it (so an empty scoped result reads as 'not this nonce', never as 'the query is broken'), and an unlabelled container is NOT reported."
  - id: d6
    text: "Whether reclamation is in scope is DECIDED and recorded: state whether an unscoped scan only reports, or also reclaims, and justify it against #365's guard"
    state: met
    evidence: file:crates/wcore-exec-backend/src/backends/container.rs:871:REPORT ONLY, DECIDED, not left open
    owner: core
    note: "MET -- DECIDED AND RECORDED: REPORT ONLY, no reclamation, and the operator surface says so in its own summary line. Justified against #365's guard by the asymmetry #365 itself relies on: the submit-path reclaim can PROVE removal is safe because it holds the task id it is about to use, so the name it clears is the name it is about to take -- and it still refuses a RUNNING holder and an unlabelled one. An unscoped scan holds no such claim over ANY row: the live-task registry is per-WAYLAND_EXEC_BACKEND_STATE_DIR, so in_live_registry=false does NOT mean nobody owns it (a live task in another process reads false), and on a shared daemon a labelled container may be another tenant's. Removing on that evidence would destroy real work to tidy a report."
---

Split out of #365 c6. That criterion asked whether the orphan scan would have found the two
leftover containers; the answer, established by inspecting a real wedged container, is that
it would have -- the label is present, because labels are applied at CREATE time -- and that
nothing ever asks it.

This is a SECOND defect, not a restatement of #365, because it survives #365's fix. The
submit-path reclaim only fires when that exact task id is submitted again, so a task id that
runs once, wedges, and is never resubmitted leaks a container that no submit reclaims and no
scan reports.
