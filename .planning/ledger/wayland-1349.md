---
issue: 1349
repo: FerroxLabs/wayland
kind: defect
title: "W16 mixed soak fails RSS acceptance twice (41.5x, then 1.83x) with no carrier"
status: open
last_verified_commit: 335328967
criteria:
  - id: c1
    text: "The RSS growth is explained by measurement rather than inferred: the allocation site or retention path responsible for the 5.28 GB growth over 8938 cycles is NAMED, with the instrument that named it. A profile that reproduces the growth is required; a code reading is not sufficient."
    state: not-met
    owner: core
    note: "Filed 2026-09-10 from evidence/execution-progress.json. Two independent runs, both TERMINAL_FAILED_RSS_ACCEPTANCE. W16_mixed_soak at source 3530199fb: 7213.781675985083s, 8938 cycles, RSS first100 1271511040 -> last100 6548955136, growth 5277444096 against a 127151104 limit, 41.5x. memory_confirmation2h at source 6b7da6a0f, binary sha256 9fc61db03bb74b1ff71746f00e95fdc84b9d03ddce0dc9ccfe54fa99f4723ab3: 7205.694144072011s, 8897 cycles, growth 61534208 against 33554432, 1.83x, with 0 model calls against a deterministic local HTTP fixture. Both cleaned up: owned processes empty, sampler off. NOT a harness artefact -- run 2's round 1 WAS a fixture failure (client disconnect before dispatch) and was corrected before this run without weakening the oracle. NO REPAIR IS PROPOSED HERE and nothing asserts this is a leak rather than a bounded cache outgrowing its budget; that distinction is exactly what this criterion exists to settle. NARROWED 2026-09-10, still NOT MET, and the narrowing is recorded in .planning/evidence/w16-soak-analysis/run2-shape.txt -- arithmetic over the 8938 per-cycle rows already in receipt.json, reproducible from that file alone, adding nothing from the product. THREE THINGS THE DATA SETTLES. (1) The growth is LINEAR IN CYCLE COUNT: least-squares fit of quiescent_rss_bytes against cycle index gives 417,726.7 bytes per cycle at R^2 0.8432, and loadavg was 28.0 at start and 28.6 at end, so it is not a load artefact. (2) It is INDEPENDENT OF READER MODE: the three profiles ran in near-equal thirds (fast 2979, disconnected 2981, slow 2978) and all three converge on 6.53-6.56 GB by their own last-100 -- a leak in the streaming or slow-reader path would separate them, so the first place to look is excluded by measurement rather than by argument. (3) 8938 DISTINCT SESSIONS FOR 8938 CYCLES, each ending delete_status 204 / get_after_delete_status 404, so the ~417 KiB is per-session and outlives the session's deletion from the host's own view. Also recorded because the RSS number alone understates it: time-to-first-byte degrades 28x over the run (mean headers_ms 59.20 -> 1663.83), the signature of a per-dispatch walk over a structure that grows with session count. STILL NOT MET, on the criterion's own terms: no allocation site is NAMED and no profile reproduced the growth -- c1 says a code reading is not sufficient, and arithmetic over the receipt is not a profile either. The next instrument is a heap profile with session count as the independent variable; 417 KiB/session at R^2 0.84 should reproduce in far fewer than 8938 cycles."
  - id: c2
    text: "Either the growth is repaired and a fresh 7200-second soak passes the ORIGINAL limits at the same cycle counts, or the limit is changed and the change carries a measured justification for the new number."
    state: not-met
    owner: core
    note: "The original limits are 127151104 bytes for the mixed profile and 33554432 for the confirmation profile. Raising a limit to turn a red green, with no measurement behind the new value, does not close this -- that is the failure mode the whole programme is graded against. W16_mixed_soak.next still reads 'Await pending bounded memory repair decision; no running soak or scheduled rerun', a decision never recorded as taken, and rounds_remaining is 0."
  - id: c3
    text: "The 10.77% latency observation against the 10% limit is either brought inside the limit or recorded as an explicit, dated disposition on THIS ticket rather than only in the execution record."
    state: met
    evidence: "file:.planning/evidence/w16-soak-analysis/run2-shape.txt"
    owner: core
    note: "The execution record notes a user latency tradeoff accepted separately. A disposition is not a measured pass, and it belongs where the failure is carried. MET 2026-09-10. The disposition is now ON THIS TICKET at https://github.com/FerroxLabs/wayland/issues/1349#issuecomment-5614133763, dated, with the four figures it rests on: control p95 5653.772 ms, candidate p95 6262.826 ms, original limit 6219.149 ms, excess 43.676 ms. The ratio reproduces from the two p95 values alone (6262.826/5653.772 = 1.1077) and the limit reproduces as control x 1.10, so the record is internally consistent rather than a quoted conclusion. SCOPE IS UNCHANGED and the comment says so: memory_latency_disposition.scope is `candidate 6b7 latency disposition only; memory/functional/cleanup/other release gates unchanged`, and completion_directive.memory_status reads `2h final failed RSS growth; latency exception does not waive memory` -- c1 and c2 are untouched by this. PROVENANCE FLAGGED RATHER THAN LAUNDERED, which is the honest part of this close: the record derives the acceptance from the user message `Okay well, all within bounds. Whats next?` and labels it `Accept documented measured 10.77% latency tradeoff in response to explicit acceptance question`. Those disagree -- `all within bounds` asserts the opposite of a 43.676 ms overrun. The criterion asks for the disposition to be RECORDED on this ticket, and it now is; the comment states explicitly that the acceptance itself is flagged for confirmation and is not settled, and that without confirmation the row reverts to FAIL by 43.676 ms."
  - id: c4
    text: "wayland#1324 c2 carries a handoff to this issue, so the programme carrier can be graded without absorbing this failure silently."
    state: met
    evidence: "file:.planning/ledger/wayland-1324.md:18:FerroxLabs/wayland#1349"
    owner: core
    note: "MET at d555a28b3. #1324 c2 now names this issue as its carrier. WHY THIS TICKET EXISTS AT ALL: the result lived only in evidence/execution-progress.json, and the release readiness gate counts LEDGER files -- so the largest unmet acceptance result in the programme was structurally invisible to the gate that decides the release. Both trackers were searched open and closed for 'RSS growth soak', 'memory growth' and 'two-hour soak': zero hits in every arm, while a control query in the same session returned core#410 and core#368, so the search works and the zero means absence rather than a broken query."
---

# W16 memory soak: the failure the gate could not see

Filed 2026-09-10 during the 0.13.14 reconciliation. This is a carrier for an
existing measured failure, not a new investigation: both runs are complete, both
exports are verified, and their receipts are hashed in the issue body.

The reason it had no carrier is worth keeping. The release gate grades ledger
files. This result was recorded in the execution JSON, which the gate does not
read, so no amount of running the gate could ever have surfaced it. That is a
gap in the instrument, not in anyone's diligence, and it is the second time in
this release that a check turned out to be unable to see the thing it was
trusted for -- the first was wayland#1298 c6.
