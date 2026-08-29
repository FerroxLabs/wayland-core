---
issue: 366
repo: FerroxLabs/wayland-core
kind: defect
title: "Container orphan scan is nonce-scoped, so it can never see a leftover from an earlier run"
status: open
last_verified_commit: df53b9ab
criteria:
  - id: d1
    text: "The product can enumerate wayland-created containers WITHOUT being given a nonce -- a key-presence scan reachable from an operator-facing surface, not only from a nonce-scoped path"
    state: not-met
    evidence: ""
    owner: core
    note: "MEASURED while closing #365 c6, by inspecting a real wedged container rather than reasoning about one. `docker ps -a --filter label=wayland.task.nonce` (key presence, no =value) returns the leftover; `--filter label=wayland.task.nonce=<a fresh nonce>` returns nothing. The query that would answer 'are there wayland containers left over from any run' is one character shorter than the one in use, and nothing in the product issues it."
  - id: d2
    text: "The nonce-scoped scan_orphans(nonce) contract is left intact for the caller that genuinely wants one run's orphans (cancel()); the unscoped scan is an addition, not a widening. State which callers move and which do not"
    state: not-met
    evidence: ""
    owner: core
    note: "cancel() legitimately wants ONE run's orphans -- it re-enumerates by the cancelled task's nonce to verify its own `docker rm -f`, and widening that would make it report other tasks' containers as its own residual. The two callers must be dispositioned separately, not merged."
  - id: d3
    text: "An operator surface reports a leftover it did not create in this process -- one whose nonce is not in the live registry"
    state: not-met
    evidence: ""
    owner: core
    note: "This is the criterion that actually closes the class. A scan that can only report containers the current process already holds a nonce for answers a question nobody needed asked: the two leftovers in #365 sat for a day and were found by a human running `docker ps -a` by hand."
  - id: d4
    text: "The conformance check at conformance.rs:340 is re-examined: it asserts enumerated && found.is_empty() for a nonce chosen so nothing can ever be found"
    state: not-met
    evidence: ""
    owner: core
    note: "A check that cannot fail on the axis it appears to cover. It either gains an arm that plants a labelled container and requires the scan to FIND it, or its limits are stated so it is not read as orphan-scan coverage. As written it is the enumerated=true half doing all the work and the found.is_empty() half doing none."
  - id: d5
    text: "A regression test plants a labelled leftover under a nonce the running process has never used, and asserts the unscoped scan reports it -- creating the leftover itself and cleaning up after itself"
    state: not-met
    evidence: ""
    owner: core
    note: "Per #365 c5, which is already discharged: a test that waits for a dirty host has the same blind spot as the thing it replaces. crates/wcore-exec-backend/tests/container_wedge.rs is the working pattern to copy -- it wedges the daemon itself, is proven red on a fresh host, and cleans up before every assertion so a failing run leaves the next lane nothing."
  - id: d6
    text: "Whether reclamation is in scope is DECIDED and recorded: state whether an unscoped scan only reports, or also reclaims, and justify it against #365's guard"
    state: not-met
    evidence: ""
    owner: core
    note: "Not left open. #365's submit-path reclaim can prove removal is safe because it holds the task id it is about to use; an unscoped background scan holds no such claim, and a leftover may belong to another tenant or to a live task in a different process. The decision must be argued against that asymmetry rather than inherited from #365."
---

Split out of #365 c6. That criterion asked whether the orphan scan would have found the two
leftover containers; the answer, established by inspecting a real wedged container, is that
it would have -- the label is present, because labels are applied at CREATE time -- and that
nothing ever asks it.

This is a SECOND defect, not a restatement of #365, because it survives #365's fix. The
submit-path reclaim only fires when that exact task id is submitted again, so a task id that
runs once, wedges, and is never resubmitted leaks a container that no submit reclaims and no
scan reports.
