# 24-GWP-H1 — NOTES (append-only, committed early per LANE-BRIEF §6b-i)

Lane `gwp-h1-duplicates`. Base `5cd37f79`. Started 2026-07-30.

Targets: **F24-GWP-M1** (receipt headline reports `duplicates: 0` for a run that
duplicated — fix FIRST) and **F24-GWP-H1** (Windows re-delivers every live cron
job at the Task Scheduler `PT1M` repetition boundary).

---

## Minute 10 — M1 root cause, MEASURED, not inferred

`scripts/f24-journey.mjs` builds the receipt from **two different reads of the
same journal, taken minutes apart**:

- line 782 `this.counts = t` — the tally is **frozen at step 13**
  (`delivery-reconcile`).
- line 981 `adapter_coverage: this.adapterCoverage()` — the breakdown is
  **recomputed at receipt-write time**, after steps 14–17
  (upgrade, rollback, redaction canary, drain/uninstall).

Anything that arrives during those four steps is counted by the breakdown and is
structurally invisible to the headline. `duplicates = arrived - unique` is a
correct formula applied to a stale `arrived`.

Confirmed on the committed artifacts of the lane that found it
(`00-m1-two-snapshot-proof.txt`, produced with `/usr/bin/python3`, read back with
the Read tool per §3b):

```
windows-receipt-attempt3.json
  counts     : {'submitted': 12, 'arrived': 12, 'unique': 12, 'duplicates': 0, 'losses': 0}
  cov arrived: 24  unique: 12
  finished_at: 2026-07-30T02:19:04.611Z
```

The duplicate burst is timestamped `02:19:03.032Z–02:19:03.202Z`
(`windows-duplicate-arrival-timeline.txt`). It lands **between step 13 and
`finished_at`** — 61 s after the clean first pass. That is the whole of M1.

Both clean receipts (`FINAL-windows-receipt.json`, `FINAL-macos-receipt.json`)
have coverage summing to 12, matching their headline — so the disagreement is
not a units bug in the breakdown; it is a time-of-read bug in the headline.

### Consequence for the fix

The headline must be derived from the **same journal read** as the breakdown, at
receipt-write time. Making the two merely "equal today" is not enough — they must
be structurally incapable of disagreeing (one snapshot, two projections).

Recomputing at receipt time makes the attempt-3 style run emit
`duplicates: 12`, which `wcore_eval_scenarios::journey` already refuses as
`DirtyReconciliation`. That is the intended outcome: **the product stops being
able to claim exactly-once on a run that duplicated.**

## Minute 12 — H1 hypothesis to test (NOT yet established)

Brief's clue: the whole batch re-arrives in one burst at the `PT1M` Task
Scheduler repetition boundary; `Get-Process wayland-core` never exceeded 1, so
it is **not** two concurrent gateways. Suspect **cron state durability**: a
restarted runtime re-fires jobs whose last-fired mark was never durably
persisted. Next: find what marks a cron job fired and when it is flushed.

## Still to establish

- [ ] H1 root cause in source (cron last-fired persistence).
- [ ] Fix or declare.
- [ ] Duplicate ratio over N>=5 Windows runs that each cross a `PT1M` boundary.
- [ ] Both-directions control on the repaired receipt: plant a duplicate, prove
      it is reported; remove it, prove zero.
