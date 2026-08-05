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

## Minute 45 — M1 FIXED, both directions

`snapshot()` reads the journal once and projects it into headline and
breakdown. `receipt()` uses that snapshot for both; `this.counts` survives only
as the step-13 record inside `steps[]`.

- **Fix present:** 24/24 pass, 0 fail (`01-m1-tests-postfix.txt`). Executed
  count read back per §3.2 — not an exit status.
- **Fix absent** (headline reverted to the step-13 freeze, control run inside
  the repo so `driverCommit()` resolves): **1 fail — the M1 positive test,
  `12 !== 24`**, the literal Windows numbers (`02-…`, `03-…`).
- **Negative direction:** the clean-run test passes both before and after, so
  the new gate is not permanently red (§3b-iii).

**My first attempt at that control was INVALID and I am recording it.** I ran
the pre-fix copy from the scratchpad, where `driverCommit()` fails with
`not a git repository`. Three tests went red for a reason that had nothing to
do with the defect, and the summary line "3 failed" would have read as a
successful control. Re-run inside the repo it is 1 failure, in the one test
that measures the defect.

## Minute 70 — H1 ROOT CAUSE: it is NOT a duplicate. It is recurrence.

The brief, the finding and my own first hypothesis all assumed re-delivery.
**All three are wrong, and the product's own delivery ids say so.**

`04-h1-idempotency-keys.txt`, `05-h1-occurrence-cadence.txt`, both computed
from the sink's journal (`windows-arrivals.jsonl`) — the sink, never the
headline:

```
f24j-delivery-01  occurrences=2  distinct ids = 2 of 2 -> DIFFERENT
f24j-delivery-04  occurrences=2  distinct ids = 2 of 2 -> DIFFERENT
f24j-delivery-07  occurrences=2  distinct ids = 2 of 2 -> DIFFERENT
f24j-delivery-10  occurrences=2  distinct ids = 2 of 2 -> DIFFERENT
f24j-heartbeat    occurrences=3  distinct ids = 3 of 3 -> DIFFERENT
```

**Not one arrival in the whole run is a replay.** Every repeat carries a new
`scheduled_millis`, i.e. a new occurrence of a recurring job.

### Why the jobs recurred inside a 3-minute run

The journey submits all twelve deliveries AND the heartbeat with
`--trigger every:15`. `every:` is **seconds** (`trigger.rs:413`,
`next_after` → `after + Duration::seconds(every_secs)` at `:339`) — but the
rate is floored:

```rust
// trigger.rs:236-238
// A minute floor: the tick is 30s, so anything faster cannot be
// honoured evenly and would simply fire on every tick.
Self::Interval { every_secs } => TriggerBound::new((*every_secs).max(60), 1),
```

applied to the *result*, not only the parameters, at `trigger.rs:366`:
`earliest = after + bound.min_interval_secs`. So **`every:15` is a 60-second
recurring job.**

The heartbeat — the one job that was never in a kill window — measures that
floor directly: scheduled deltas **60068 ms and 64940 ms**. The delivery jobs
show 110s and 124s deltas because their second occurrence is the first tick
after the platform brought the runtime back.

### So the platform difference is a race, not a defect

A run that keeps the gateway alive for longer than ~60 s after submitting a
recurring job **will** see every body twice. That is the trigger doing its job.

- launchd and systemd restart in seconds, so those runs finish inside one
  60 s window and see each body once.
- Task Scheduler's minimum repetition interval is `PT1M`, so the Windows
  kill→recover leg alone costs ~60 s and the window is always crossed.

Windows is not delivering twice. **Windows is slow to restart, and the journey
asserts a property that is only true of short runs.**

### The second finding, and it is the more serious one

Of 24 delivery arrivals, **only 8 carry an `idempotency_key` at all; 16 are
null** (`04-h1-idempotency-keys.txt`). That is exactly one of the journey's
three adapters. Slack emits a delivery identity; whatsapp and sms emit none.

So for those two adapters the sink cannot tell a replay from a recurrence
**even in principle** — and the receipt reports `duplicates: 0` for them. That
is the M1 sin one level down: a property that is unmeasurable being reported as
a property that was measured and found clean.

## Verdict forming

H1 as written — "Windows re-delivers" — is **REFUTED with executable
evidence** (§5 permits this for a HIGH). The real defects are both in the
instrument and in what the receipt is willing to claim.

## Still to establish

- [ ] Repair the instrument in-lane (§6b-ii): classify replay vs recurrence,
      and refuse to report zero duplicates for arrivals that carry no identity.
- [ ] Both-directions control on that classifier.
- [ ] `cargo test -p wcore-cron` on hetzner for the 60 s floor, both directions.
- [ ] Declaration text, consistent with `lane/24c1-declaration`'s
      `docs/delivery-semantics.md`.
