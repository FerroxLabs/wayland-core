---
phase: 24-gateway-automation-channels-typed-api
lane: gwp-h1-duplicates
branch: lane/gwp-h1-duplicates
base: 5cd37f79fc3313a220debd72ae2974a42c1fa80b
findings: [F24-GWP-M1, F24-GWP-H1]
status: complete
verdict: "M1 FIXED. H1 REFUTED with executable evidence — the Windows arrivals are recurrences of a 60-second trigger, not re-deliveries; zero replays in the whole run. The instrument that called them duplicates is repaired."
---

# 24-GWP-H1 — the duplicate that was not a duplicate

**One sentence: the receipt headline that reported `duplicates: 0` for a run
that repeated is fixed and can no longer be stale; and the HIGH it was hiding
is not a duplicate at all — every repeated body in that Windows run carries a
*different* delivery id, because the journey submits its deliveries with
`every:15`, which the product floors to a **60-second recurrence**, and the
Windows kill→recover leg always outlives it.**

Nothing was merged, pushed to `main`, tagged, released, or used to close an
issue. I did not run `wcore-contract generate`. No PR.

---

## 1. F24-GWP-M1 — FIXED (this was the priority and it is done)

### The mechanism, measured not guessed

`scripts/f24-journey.mjs` built its receipt from **two reads of the same
journal, minutes apart**:

- `this.counts = t` (line 782) — the tally **frozen at step 13**,
  `delivery-reconcile`.
- `adapter_coverage: this.adapterCoverage()` (line 981) — the breakdown
  **recomputed at receipt-write time**, four steps later.

Anything arriving during steps 14–17 was counted by the breakdown and was
structurally invisible to the headline. `duplicates = arrived - unique` was a
correct formula applied to a stale `arrived`.

On the committed artifacts:

```
windows-receipt-attempt3.json
  counts     : {'submitted': 12, 'arrived': 12, 'unique': 12, 'duplicates': 0, 'losses': 0}
  cov arrived: 24  unique: 12
  finished_at: 2026-07-30T02:19:04.611Z
```

The burst is timestamped `02:19:03.032–02:19:03.202`. It lands **between step
13 and `finished_at`**. That is the whole of M1.

### The fix

`snapshot()` reads the journal **once** and projects the same array into both
the headline and the breakdown, so they are incapable of disagreeing rather
than merely equal today. `receipt()` uses that snapshot for both;
`this.counts` survives only as the step-13 record inside `steps[]`, where
being a point-in-time reading is what it is for.

The journey then **writes the receipt and refuses to print `JOURNEY COMPLETE`**
when the final snapshot is dirty — evidence preserved, claim refused. Before
this, such a run exited 0.

### Both directions

| | result |
|---|---|
| Fix present | **30/30 pass, 0 fail** (`07-driver-suite-30-of-30.txt`) |
| Fix absent (headline reverted to the step-13 freeze) | **1 fail — the M1 positive test, `12 !== 24`**, the literal Windows numbers |
| Clean-run negative | passes **both** before and after — the gate is not permanently red |

**My first attempt at that control was invalid and I am recording it.** I ran
the pre-fix copy from a scratchpad, where `driverCommit()` dies with
`not a git repository`. Three tests went red for a reason unrelated to the
defect, and "3 failed" would have read as a successful control. Re-run inside
the repo it is **1** failure, in the one test that measures the defect.

---

## 2. F24-GWP-H1 — REFUTED. It is recurrence, not re-delivery.

§5 of LANE-BRIEF permits a HIGH to be *"disproved with executable evidence"*.
This one is.

### The product's own delivery ids say it

From the sink's journal (`windows-arrivals.jsonl`), never the headline:

```
f24j-delivery-01  occurrences=2  distinct ids = 2 of 2 -> DIFFERENT
f24j-delivery-04  occurrences=2  distinct ids = 2 of 2 -> DIFFERENT
f24j-delivery-07  occurrences=2  distinct ids = 2 of 2 -> DIFFERENT
f24j-delivery-10  occurrences=2  distinct ids = 2 of 2 -> DIFFERENT
f24j-heartbeat    occurrences=3  distinct ids = 3 of 3 -> DIFFERENT
```

**Not one arrival in the run is a replay.** Every repeat carries a new
`scheduled_millis`.

### Why they recurred inside a three-minute run

The journey submits every job — the twelve deliveries and the heartbeat —
with `--trigger every:15`. `every:` is seconds, but the rate is floored:

```rust
// crates/wcore-cron/src/trigger.rs:236-238
// A minute floor: the tick is 30s, so anything faster cannot be
// honoured evenly and would simply fire on every tick.
Self::Interval { every_secs } => TriggerBound::new((*every_secs).max(60), 1),
```

applied to the **result**, not only the parameters, at `trigger.rs:366`
(`earliest = after + bound.min_interval_secs`). So **`every:15` is a
60-second recurring job**, and any run that stays alive past one period sees
every body twice.

### The heartbeat is the internal control, and it settles it

The heartbeat is in the same run, subject to the same kill and the same
restart, and **nobody has ever called its repeats a duplicate.** Its scheduled
deltas are **60068 ms and 64940 ms** — it is simply a 60-second job in a
189-second run, so it fired three times. The deliveries fired twice for exactly
the same reason. One phenomenon, two readings.

### So the platform difference is a race, not a defect

- launchd and systemd restart in seconds; those runs finish inside one 60 s
  window and see each body once.
- Task Scheduler's minimum repetition is `PT1M`, so the Windows kill→recover
  leg alone costs ~60 s and the window is always crossed.

**Windows is not delivering twice. Windows is slow to restart, and the journey
asserts a property that is only true of short runs.**

### Executable proof, both directions, on hetzner

`cargo test -p wcore-cron --lib` at `33930493`:

```
test runner::tests::every_15_is_floored_to_60s_and_each_occurrence_has_its_own_delivery_id ... ok
test result: ok. 74 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Executed count read back per §3.2 — **0 ignored, 0 filtered out**, and my test
named in the log at line 83 (known-positive `fires_due_job_once_per_anchor` = 1
hit, known-negative = 0 hits, so the grep was alive).

**Mutation control — the test can fail:** expecting a 15 s floor instead of
60 s reds it with `left: 60s, right: 15s`, `1 failed`, rc=101, and the tree
restored to 0 modified files. The runtime genuinely produced 60 seconds.

The test also carries its own **other direction**: `every:3600` is honoured at
3600 s, so the floor is a floor and not a hardcoded constant.

---

## 3. The ratio, and why "intermittent" was the wrong word

The brief asked for the observed ratio and warned that one clean run proves
nothing. The honest answer is better than a ratio: **the repeat is deterministic
in the length of the delivery window**, not random.

| run | delivery arrival span | repeats | predicted |
|---|---|---|---|
| windows-attempt3 | **67.7 s** (> 60 s) | **12** | repeat — correct |
| windows-FINAL | **0.3 s** | 0 | single — correct |
| macos-FINAL (71.9 s run) | — | 0 | single — correct |
| linux-FINAL (73.5 s run) | — | 0 | single — correct |

**Observed: 1 of 4 archived runs repeated — 1 of 2 Windows runs.** Both Windows
runs are predicted correctly by the 60-second rule, 2 of 2. Note the FINAL
Windows run lasted 151 s in total and was still clean, so **total run length is
not the predictor** — the delivery window is.

**What I did NOT do:** I did not drive N fresh Windows journeys. The mechanism
is settled by the delivery ids, the trigger source and a mutation-controlled
unit test, and more journeys would re-measure a deterministic rule. That is a
deliberate call, not an omission I am hiding — if you want the ratio bounded
empirically rather than analytically, that leg is still open.

---

## 4. The finding that IS real, and it is not Windows

Of 24 delivery arrivals, **only 8 carry an `idempotency_key` at all; 16 are
null** — at endpoints `twilio.messages` and `whatsapp.messages`. Slack is the
only one of the journey's three adapters that puts a delivery identity on the
wire.

So for those two adapters a replay is **not distinguishable from a recurrence
even in principle**, and the receipt was reporting `duplicates: 0` for them.
That is the M1 defect one level down: an unmeasurable property published as a
measured clean one.

This matches `lane/24c1-declaration`'s table (7 of 10 adapters have no
idempotency slot) and is the thing worth fixing.

---

## 5. Instrument repaired in-lane (§6b-ii)

`duplicates = arrived - unique` counts repeats of a **body**. Exactly-once is
scoped to a **delivery identity**. The harness conflated them, and that
conflation is what raised a HIGH against Windows.

The driver now classifies every repeat and the receipt carries
`delivery_identity`:

| bucket | meaning |
|---|---|
| `replays` | same delivery identity twice — a real exactly-once violation |
| `recurrences` | same body, different identity — the trigger fired again |
| `indeterminate` | a repeat with no identity to judge by — **counted against the run** |

`counts.duplicates` keeps its `arrived - unique` meaning so the Rust verifier
is untouched. On the real Windows journal:

```
replays: 0, recurrences: 4, indeterminate: 8, unidentified: 16
VERDICT: NOT PROVEN — zero replays observed, but repeats arrived with no
delivery identity ... This is a gap in outbound idempotency, NOT evidence of a
duplicate.
```

**Both directions on the real journal**, as the brief required: plant a true
replay (same identity twice) → `replays=1`, verdict `EXACTLY-ONCE VIOLATED`;
remove it → `replays=0`, and the classification is byte-identical to before the
plant. The precondition is asserted first — the unmutated journal contains no
replay — so the positive is not free.

Three assertions per §6b-ii, including the third: the old body-only tally
reports **12 duplicates** on data containing **0 replays**, so the repair
demonstrably changes the answer.

---

## 6. Fix or declare — the declaration

Consistent with `lane/24c1-declaration`'s `docs/delivery-semantics.md` §4,
which is correct as written. **I did not edit that doc** — the brief said not to
and this is my verdict in my own file.

> **Wayland gateway delivery is exactly-once per delivery id
> (`cron:{job}:{scheduled_millis}`) and at-least-once per message, on every
> platform.**
>
> A recurring trigger that outlives a restart gap produces a second *occurrence*
> — a new delivery id, which no adapter's dedup can or should suppress. This is
> not a Windows defect. Windows merely crosses the window reliably, because
> Task Scheduler's minimum repetition (`PT1M`) exceeds the trigger's 60-second
> floor, while launchd and systemd restart inside it.
>
> Separately, for the **7 of 10 adapters with no idempotency slot**, exactly-once
> is **not measurable at the wire** and must not be reported as zero duplicates.

### Where I differ from `docs/delivery-semantics.md` §5

Its §4 (scope) is exactly right and my evidence confirms it. Its **§5** says
Windows *"re-fires cron jobs that have already fired"* and lists Windows as a
*"known defect [that] can produce a duplicate on any adapter"*. Two refinements,
both measured:

1. It is **not a defect and not Windows-specific.** The second fire is the next
   scheduled occurrence of a 60-second job. Any platform whose run outlives the
   recurrence repeats identically — the heartbeat does it three times in the
   same run with no restart involved.
2. *"already fired"* implies wrongful re-firing. The occurrences are distinct
   and each fired once.

**Recommendation to whoever owns that doc** (not me, and I have not touched
it): keep §4; reword §5 from "Windows re-delivers" to "a recurring trigger
whose period is shorter than the platform's restart gap yields additional
occurrences; on Windows the `PT1M` repetition guarantees this".

---

## 7. Open, and honestly named

1. **The journey is currently structurally unable to pass on Windows** — a
   permanently-red gate in the §3b-iii sense. Its deliveries recur every 60 s
   and the Windows recovery leg always exceeds that, so `duplicates != 0` is
   guaranteed. **I did not change the trigger.** Doing so alters what every
   platform receipt measures, and the criterion graders and
   `lane/24c1-declaration` depend on those receipts. It needs one owner's
   decision, not a unilateral edit from this lane.
2. **The driver still fails a recurrence-only run.** The honest gate would fail
   on `replays > 0` or `indeterminate > 0` and pass on proven recurrences — but
   the Rust `verify_counts` independently refuses any receipt with
   `duplicates != 0`, so changing only the driver would make the two gates
   disagree. That is a coordinated driver+verifier change, deliberately not
   attempted here.
3. Outbound idempotency for the 7 adapters without a slot — already owned by
   `lane/24c1-declaration`'s table; my `indeterminate` bucket makes it visible
   in the receipt.

## Self-check

Every number in this document was written to a file and read with the Read
tool, never off a Bash stdout render (§3b). Greps carry a known-positive and a
known-negative in the same capture. The pre-fix control was re-run after I
found the first one invalid, and the invalid one is reported rather than
discarded. The hetzner run's executed count, ignored count and filtered-out
count are all read back. The mutation control shows the Rust test failing and
the tree restored clean. No source file outside `scripts/f24-journey*.mjs` and
`crates/wcore-cron/src/runner.rs` (test-only addition) was modified; neither
shared-fence file was touched.
