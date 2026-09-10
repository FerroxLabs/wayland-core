---
issue: 1352
repo: FerroxLabs/wayland
kind: defect
title: "ACP serve cancels a keeping-up 16 MiB reader with 'protocol relay overloaded' under concurrency"
status: open
last_verified_commit: 827e69174
criteria:
  - id: c1
    text: "The cause of the cancellation is NAMED with the instrument that named it, and a red test reproduces it deterministically -- not only under soak contention."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10. OBSERVED, not inferred: the wayland#1349 c2 soak (run1, hetzner-dsm, fix binary sha256 d0819d10...cb0) aborted at 125.162 s when a fast reader of a 16,777,216-byte turn at concurrency 32 was cancelled after 3,932,160 bytes with -32003 `protocol relay overloaded; turn cancelled without truncating structured events`; the other 31 rows of that batch passed, every slow and disconnected reader included. The BASE 38f82b87f fails the same way (2 of 16 fast 16 MiB readers pass at concurrency 8). Evidence: .planning/evidence/w15-1349-leak2/soak-run1.txt and census-and-authz-base.txt. HYPOTHESIS ONLY, from a code reading, which this row says is not sufficient: crates/wcore-acp/src/bounded.rs bounds live delivery at LIVE_BYTES 1 MiB / LIVE_EVENTS 256 per channel plus AGGREGATE_BYTES 64 MiB, and the RelayTarget in crates/wcore-cli/src/acp_engine.rs carries a 1 s wait_budget; if the aggregate is process-wide, other sessions' slow or disconnected readers could hold it long enough to exhaust a keeping-up reader's wait budget."
  - id: c2
    text: "After the repair a reader that keeps up is never cancelled because of OTHER sessions' readers, shown by a base-vs-fix A/B through the real acp serve with fast 16 MiB readers at concurrency 8 and 32 mixed with slow and disconnected readers, n>=30 fast rows per arm, 0 relay cancellations on the fix -- and delivery stays BOUNDED, with peak and quiescent RSS recorded on both arms and no bound raised without a measured justification."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10. The bound exists to protect memory, and memory growth is exactly what wayland#1349 grades, so raising AGGREGATE_BYTES or removing the bound trades this failure for that one and does not satisfy this row. The soak oracle is not to be relaxed either: retrying fast rows or excusing the error would turn a red green by weakening the check."
  - id: c3
    text: "The 16 MiB concurrency-1 fast-turn latency (median ~21.7 s on this tree against ~2.6 s at 3530199fb) is measured on both sources at n>=10, then either repaired or explained by a named cause."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10. Recorded in soak-run1.txt under CYCLE COUNT. Whether it shares a cause with c1 is NOT established."
  - id: c4
    text: "wayland#1349 c2 names this issue as the carrier for its soak abort."
    state: met
    evidence: "file:.planning/ledger/wayland-1349.md:19:FerroxLabs/wayland#1352"
    owner: core
    note: "MET 2026-09-10 at filing: #1349 c2's note now names FerroxLabs/wayland#1352 as the carrier. Both trackers were searched open and closed for 'relay overloaded', 'overloaded' and 'relay in:title' before filing, with zero relevant hits; a control query ('single-flight') in the same session returned 3, so the zero means absence rather than a broken query."
---

Created 2026-09-10 at filing time, not retroactively -- the wayland#1272 c2
lesson. Found by the w15/leak2 lane when the fresh #1349 soak aborted on a
functional failure before it could reach an RSS verdict.
