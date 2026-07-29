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

## T+25 — the brief's four clauses, measured at base `71acfd19`

| Brief clause (24-C5) | Measured | Evidence |
|---|---|---|
| "no journey driver" | **FALSE** | `crates/wcore-eval-scenarios/bin/wayland-journey.rs` (208 L), `src/journey.rs` (753 L) |
| "no receipt schema" | **FALSE** | `JourneyReceipt` + `RECEIPT_SCHEMA = "wayland.journey.receipt/1"`, `journey.rs:119` |
| "no receipt on any platform" | **FALSE** | receipts for linux, macos, windows under `24-C5-finish-evidence/` |
| "no acceptance panel" | not re-measured; moot | two lanes already graded C5 MET |
| ledger row "NOT MET, no evidence, any platform" | **STALE** | measured at `873cc389`; closed at `e535c1a4` |

`24-04-SUMMARY.md` does record non-execution — but `lane/24-journey` (`73cba94c`) and
`lane/24-c5-finish` (`fd64bd5c`, `4b904d01`, `e535c1a4`) executed the work afterwards under
different plan names. **Reading only 24-04-SUMMARY.md is what produced the stale premise.**

## T+35 — the REAL gap, and it is the exact trap the brief names

The brief warns: *"24-C1's 12-of-12 tally was graded entirely through Slack, the one adapter of
ten implementing the property under test."*

**That trap RECURRED in 24-C5's own receipts, and the receipt schema makes it structural.**

Measured — every one of the three platform receipts:

```
linux-receipt-at-candidate.json -> arrival endpoint: chat.postMessage
macos-receipt.json              -> arrival endpoint: chat.postMessage
windows-receipt.json            -> arrival endpoint: chat.postMessage
```

`chat.postMessage` is Slack. All three platforms' `12 submitted / 12 arrived / 12 unique /
0 duplicates / 0 losses` was carried by **one adapter of ten**.

Concept search over the whole receipt surface (schema + verifier + contract tests), with a
live known-positive in the same invocation:

```
KNOWN-POSITIVE  'platform' in journey.rs                       -> 28 hits (instrument alive)
CONCEPT SEARCH  adapter|channel|transport|coverage|matrix      -> 1 hit
                and that one hit is journey.rs:7, PROSE in a doc comment
```

So: **zero implementation.** The receipt has `platform`, `service_family`, `arrival_source` and
five counts — and no field naming which adapter carried them. A receipt reading `12/12/12/0/0`
is *byte-identical* whether one adapter ran or ten. `verify` cannot tell, `bind` cannot tell, and
no reader can tell. The three-platform matrix is real; the adapter matrix is a single
configuration standing for ten, exactly as in 24-C1.

## T+40 — `bind` has never been observed green (§3b-iii candidate)

```
linux    candidate=978f49d778ce  driver=978f49d778ce
macos    candidate=eba6e9d7b75d  driver=978f49d778ce
windows  candidate=978f49d778ce  driver=978f49d778ce
```

Driver commits agree; macOS's candidate does not. `bind` is rc=1 in the only recorded run.
Must run the §3b-iii control in BOTH directions: prove `bind` can fail AND can pass.
