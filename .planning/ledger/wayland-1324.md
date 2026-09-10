---
issue: 1324
repo: FerroxLabs/wayland
kind: task
title: "Core stabilization: verified lifecycle, credential, budget and acceptance repair program"
status: open
last_verified_commit: 2fe7a70df
criteria:
  - id: c1
    text: "Each applicable package has exact source/artifact/fixture identity, fail-before/pass-after controls, actual executed commands/counts, cleanup, current integration evidence and an independent review."
    state: not-met
    owner: core
    note: "Reconciled 2026-09-10 against FINAL-PLAN.md W00 plus W01-W17 and evidence/execution-progress.json (updated_at 2026-09-10), both under /Users/seandonahoe/dev/waylandcore-stabilization-20260905; per-package detail in .planning/RECON-1269-1272-1324-2026-09-10.md. status_reconciliation (2026-09-07) records accepted_packages 12, total_packages 17, published False; completed_execution_packages lists W01-W11 and W14; current_package is W05. Five are NOT accepted and the record says why in its own words. W12: release-gate code integrated at a03a19e3ff7e784b4c12d2b02243da0bac3caba4 with 3 self-tests PASS, actual final evidence and promotion pending, no third-source audit, not whole-package accepted; its W12_final_mutants attempt 2 is 8/8 caught over 16 complete captures with all digests verified (index sha256 e92a2bbf97171f6860c5e3b39bef462d985a0bb66c90a9eb462af0f987038cdc) for five named pairs, which is NOT proof of nightly five-crate mutation coverage. W13: integrated 8b40bff53, foreign consumer receipt outstanding. W15: integrated 8b40bff53, journey gaps recorded explicit. W16: comparative trials, 1000 cycles and the 2h soak still pending, not package accepted. W17: only W17_model_limits exists, state committed-awaiting-parent-verification at 56b19046cadd7f30955eaad2ba582c111ff71342. One row does not reconcile with itself and is flagged rather than smoothed over: W14 is IN completed_execution_packages while its own field reads round-1 non-pass qualified with the final round held for a frozen combined candidate and the VM retained. A package whose final round is held is not accepted; either the list or the field is wrong and one must be corrected before this criterion can be graded. No W18 or new architectural scope is proposed. Partial package evidence does not discharge a whole-programme criterion, and neither does a release having gone out."
  - id: c2
    text: "External/unmeasured criteria remain visibly incomplete with their owning carriers."
    state: not-met
    owner: core
    handoff: "FerroxLabs/wayland#1349"
    note: "NOT MET 2026-09-10 on the W16 memory result, which has no carrier. The measurements, verbatim from evidence/execution-progress.json: W16_mixed_soak run 2, source 3530199fb757f3fcefecf9e40ba820775b867d84, 8938 cycles, 7213.781675985083 s, RSS growth 5277444096 bytes against a 127151104 limit, state TERMINAL_FAILED_RSS_ACCEPTANCE, rounds_remaining 0. memory_confirmation2h, source 6b7da6a0f0b1bd19a8a9326c9da1c15d15d037c7, binary sha256 9fc61db03bb74b1ff71746f00e95fdc84b9d03ddce0dc9ccfe54fa99f4723ab3, 8897 cycles, 7205.694144072011 s, RSS growth 61534208 bytes against a 33554432 limit, state TERMINAL_FAILED_RSS_ACCEPTANCE, 0 model calls. memory_latency_disposition records candidate p95 6262.826 ms against an original limit of 6219.149 ms, 10.77 percent against a 10 percent limit, and its own field says the original result remains FAIL. memory_release_disposition records the user accepting documented residual memory growth as a release tradeoff with the original test failure preserved and scoped explicitly to no claim that flat memory or an unbounded leak is resolved. That is a decision to ship with a known failure; it is not a measured pass and nothing here converts it into one. THE GAP: both trackers were searched on 2026-09-10 for a carrier -- RSS growth soak, memory growth, two-hour soak, open and closed, across FerroxLabs/wayland and FerroxLabs/wayland-core -- and every arm returned zero, while the control query AppContainer open on wayland-core returned core#410 and core#368, so the search works and the empty result means absence. The largest unmet acceptance result in the programme therefore exists only inside execution-progress.json and its receipts, with no ticket, no owner and no milestone, so it cannot reach the release gate, which counts ledger files and nothing else. W16_mixed_soak.next says await a pending bounded memory repair decision and scope_decision_pending says repair versus retain blocker with no threshold change; that decision was never recorded as taken. Owed, one act: file a FerroxLabs/wayland-core defect carrying both soak results verbatim with source SHA, binary sha256 and receipt path, plus the 10.77 percent latency figure, record the user tradeoff as a disposition ON that ticket, and handoff this criterion's residual to it. Filing tickets was not this lane's to do unasked. The other external carriers ARE correctly recorded: wayland#1323 Desktop is OPEN at milestone 0.13.14, wayland-core#410 maintainer is OPEN at milestone 0.13.12, and wayland-core#368's ledger hands its five criteria to #410 by field."
---

Recorded from live #1324 on 2026-09-06; reconciled 2026-09-10 by the w14/recon
lane against `2fe7a70df`. Tracker is OPEN, area:core, milestone 0.13.14. The
package-by-package reconciliation and the memory-carrier search, including its
control query, are in `.planning/RECON-1269-1272-1324-2026-09-10.md`. This
entry records pending work, not a new implementation plan or release waiver.

Classification: the tracker describes a programme coordination handle, not an
individual defect. Package completion and external residuals remain separate,
and unmet criteria above remain unmet. The W16 memory acceptance FAILURE is
recorded above with its exact numbers and stays visible: the historical user
release tradeoff is a disposition beside that failure, never a measured pass.

Squash provenance reconciliation: PR #455 head `28634521b854d58672819843865f6ab7dd16d72c` and integrated
commit `1780b3164c6973279fd49d4dda17e8605f51d047` have identical tree
`90e028ae6de0537c60c1d86450ceb3083ae4610b`. Original anchor `3d2089b45`
is an ancestor of the PR head; squash integration removed that ancestry. This
repin preserves the recorded grading and does not establish new runtime or
release acceptance. Exact-head CI `34242741624` passed before that squash.
