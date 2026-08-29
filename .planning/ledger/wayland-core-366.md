---
issue: 366
repo: FerroxLabs/wayland-core
kind: defect
title: "Container orphan scan is nonce-scoped, so it can never see a leftover from an earlier run"
status: open
last_verified_commit: a452b639
criteria:
  - id: d1
    text: "The product can enumerate wayland-created containers WITHOUT being given a nonce -- a key-presence scan reachable from an operator-facing surface, not only from a nonce-scoped path"
    state: met
    evidence: "symbol:crates/wcore-exec-backend/src/backends/container.rs::sweep_containers"
    owner: core
    note: "The key-presence query is issued: `docker ps -a --filter label=wayland.task.nonce` with no `=value`, formatted `{{.Names}}\\t{{.Label \"wayland.task.nonce\"}}` so each surface arrives with its own nonce. Reached from a new operator surface, `wayland-core backend sweep` (crates/wcore-cli/src/backend.rs::sweep), which takes no nonce at all -- there is no argument an operator could get wrong. The cloud backend sweeps by the same key-presence rule, filtered in Rust over the full app listing because the vendor filter has no key-presence form. The local and ssh backends return enumerated=false naming why: their nonce travels in the child environment (WAYLAND_TASK_NONCE) and the process table is read for argv, so no unscoped query can match -- reported as NOT MEASURED rather than as a clean zero, which is this module's standing rule."
  - id: d2
    text: "The nonce-scoped scan_orphans(nonce) contract is left intact for the caller that genuinely wants one run's orphans (cancel()); the unscoped scan is an addition, not a widening. State which callers move and which do not"
    state: met
    evidence: "symbol:crates/wcore-exec-backend/src/contract.rs::ExecutionBackend"
    owner: core
    note: "scan_orphans(nonce) is UNCHANGED in signature, body and meaning on all four backends. sweep_orphans() is a separate REQUIRED trait method returning a separate type (OrphanSweep, not OrphanScan -- OrphanScan carries the nonce it was asked about, which an unscoped sweep does not have). Required, not defaulted: a trait default returning enumerated=false would be a fail-open every future backend inherits silently. CALLERS THAT DO NOT MOVE: ContainerBackend::cancel and CloudBackend::cancel keep scan_orphans(entry.nonce) -- a cancellation verifying its own removal must not report another task's surface as its residual; conformance.rs arm 5 keeps its fresh nonce, with its limits now stated (d4); orphan::scan_all / scan_one and `backend orphans --nonce` keep taking a nonce. CALLERS ADDED: orphan::sweep_all and `backend sweep`. Nothing was migrated off the scoped call."
  - id: d3
    text: "An operator surface reports a leftover it did not create in this process -- one whose nonce is not in the live registry"
    state: met
    evidence: "test:crates/wcore-exec-backend/tests/container_orphan_sweep.rs::a_swept_leftover_no_live_task_claims_is_marked_unclaimed"
    owner: core
    note: "orphan::sweep_all reads the live-task registry ONCE up front and marks every swept surface with SweptSurface::unclaimed = the registry holds no entry carrying that nonce. A run that ends calls registry::forget, so a leftover from a finished run lands as unclaimed -- which is exactly the leftover nobody could see: no nonce-scoped scan has its nonce to ask with, and #365's submit-path reclaim only fires if that task id is submitted again. `backend sweep` prints the flag per surface and exits non-zero when any surface is unclaimed, so it is scriptable as a gate. The test proves it against a container planted under a nonce the running process has never used."
  - id: d4
    text: "The conformance check at conformance.rs:340 is re-examined: it asserts enumerated && found.is_empty() for a nonce chosen so nothing can ever be found"
    state: met
    evidence: "symbol:crates/wcore-exec-backend/src/conformance.rs::run_conformance"
    owner: core
    note: "LIMITS STATED, which is the second of the two options the criterion offers. The check's own name now says what it can and cannot prove: 'an orphan scan reports enumerated truthfully and fabricates nothing for an unused nonce (it CANNOT fail on whether a real leftover would be found -- nothing ever ran under this nonce; see tests/container_orphan_sweep.rs)'. The enumerated=true half is real coverage and is the failure mode orphan.rs was built around (a scanner that returned 0 while ps showed the process); the found.is_empty() half is not, and now says so instead of reading as orphan-scan coverage. The plant-and-find arm went to the container test file rather than here on purpose: this harness is backend-generic and planting a surface is not. A second generic arm was added -- the backend must answer an unscoped sweep with an explicit verdict and name its query -- which a backend answering enumerated=false can pass honestly, so it is not a gate that cannot fail in the other direction either."
  - id: d5
    text: "A regression test plants a labelled leftover under a nonce the running process has never used, and asserts the unscoped scan reports it -- creating the leftover itself and cleaning up after itself"
    state: met
    evidence: "test:crates/wcore-exec-backend/tests/container_orphan_sweep.rs::the_unscoped_sweep_reports_a_leftover_from_a_nonce_this_process_never_used"
    owner: core
    note: "Copies the container_wedge.rs pattern per #365 c5: it plants the container itself with `docker create --label wayland.task.nonce=<nonce this process never used>`, and removes it BEFORE asserting so a red leaves the next lane nothing. Carries a CONTROL in the same run -- the nonce-scoped scan is asked for a fresh nonce, the shape every real caller supplies, and must NOT find the leftover; if it did, the test would not be measuring what it claims. A third test is a negative control that passes in both arms: an UNLABELLED container must not be swept up, which is what stops the sweep being satisfied by listing every container on a shared host."
  - id: d6
    text: "Whether reclamation is in scope is DECIDED and recorded: state whether an unscoped scan only reports, or also reclaims, and justify it against #365's guard"
    state: met
    evidence: "file:.planning/DECISIONS.md"
    owner: core
    note: "DECIDED: REPORT ONLY, never reclaim. Recorded as Q-366d6 in .planning/DECISIONS.md with the argument, and repeated on ExecutionBackend::sweep_orphans where an implementer will read it. The justification is the asymmetry the criterion asked for: #365's submit-path reclaim is safe because it holds the exact task id it is about to use and no other process may hold that id concurrently, so a surface wearing that name is provably a dead predecessor. A sweep matched on the PRESENCE of a label and owns no identity at all -- on a shared daemon the surface may be another tenant's or a live task's in a different process. The `unclaimed` flag does not license removal either: it means only that the registry THIS process can read holds no such nonce, which is exactly as blind to another process's live task as the scan it replaces. So the surface reports, marks, exits non-zero, and PRINTS the `docker rm -f` rather than running it."
---

Split out of #365 c6. That criterion asked whether the orphan scan would have found the two
leftover containers; the answer, established by inspecting a real wedged container, is that
it would have -- the label is present, because labels are applied at CREATE time -- and that
nothing ever asks it.

This is a SECOND defect, not a restatement of #365, because it survives #365's fix. The
submit-path reclaim only fires when that exact task id is submitted again, so a task id that
runs once, wedges, and is never resubmitted leaks a container that no submit reclaims and no
scan reports.
