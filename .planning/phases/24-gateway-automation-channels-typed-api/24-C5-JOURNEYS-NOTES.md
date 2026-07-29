# 24-C5-JOURNEYS — NOTES (live, appended as measured)

Lane: `lane/24-c5-journeys`. Base: `71acfd19258e0fc7484d80a0a95be3f29d0ee2b5`
(asserted against `git ls-remote gh plan/f20-unified-audit-repair`).

## T+0 — brief premise is under suspicion before any work

My brief states, of `24-C5`:

> "NOT MET, no evidence on any platform. Plan 24-04's four tasks were never started:
> no journey driver, no receipt schema, no receipt on any platform, no acceptance panel."

**First filesystem read contradicts every clause.** At base `71acfd19`:

- journey driver: `crates/wcore-eval-scenarios/bin/wayland-journey.rs` EXISTS
- journey lib:    `crates/wcore-eval-scenarios/src/journey.rs` EXISTS
- receipt schema: `crates/wcore-eval-scenarios/src/receipt.rs` EXISTS
- receipt contract test: `crates/wcore-eval-scenarios/tests/journey_receipt_contract.rs` EXISTS
- receipts on THREE platforms under `24-C5-finish-evidence/`:
  `linux-receipt-at-candidate.json`, `macos-receipt.json`, `windows-receipt.json`
- two prior lane summaries: `24-C5-JOURNEY-SUMMARY.md`, `24-C5-FINISH-SUMMARY.md`
- four prior commits: `73cba94c`, `fd64bd5c`, `4b904d01`, `e535c1a4`

So at least two lanes (`lane/24-journey`, `lane/24-c5-finish`) already did the work
the brief says was never started. The ledger row was measured "at `873cc389`" — the
tree has moved since.

**Next: establish what is actually short, rather than rebuilding what exists.**
