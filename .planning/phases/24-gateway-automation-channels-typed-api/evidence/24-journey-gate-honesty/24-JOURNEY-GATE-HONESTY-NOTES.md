# 24-JOURNEY-GATE-HONESTY — NOTES (append-only, committed early per LANE-BRIEF §6b-i)

Lane `journey-gate-honesty`. Base `5013505e7caefa5561f0de40c75406afe1b42fc3`
(asserted with `/usr/bin/git rev-parse HEAD` redirected to a file and read back
per §3b). Started 2026-07-30.

Target: the Windows setup-to-recovery journey gate is **permanently red** (§3b-iii).
Make it able to pass on an honest run, and still fail on a dishonest one.

---

## Minute 10 — the premise, verified at HEAD before acting (§"your brief is stale")

Every claim the brief carries, re-checked against this tree, not taken on trust:

| Brief claim | Verified at HEAD? | Where |
|---|---|---|
| `every:15` is rate-floored to 60s at `trigger.rs:238`, applied at `:366` | TO DO | — |
| delivery id is `cron:{job}:{scheduled_millis}` at `runner.rs:324-338` | TO DO | — |
| Rust `verify_counts` refuses ANY `duplicates != 0` | **YES** | `crates/wcore-eval-scenarios/src/journey.rs:559-564` — `DirtyReconciliation` |
| the receipt already carries `delivery_identity{replays,recurrences,indeterminate,unidentified}` | **YES** | `scripts/f24-journey.mjs:1043`, built by `classifyRepeats` at `:1147` |
| the driver ALSO refuses any `duplicates != 0` | **YES, and in two places** | `assertFinalReconciliation` `:1069`; and step 13 `deliveryReconcile` `:833` |
| `delivery_identity` reaches the Rust verifier | **NO — and this is the load-bearing find** | see below |

### The find the brief did not have: the Rust verifier cannot even SEE the field

`/usr/bin/grep -rln delivery_identity .` over the whole tree returns exactly two
paths — `scripts/f24-journey.mjs` and the previous lane's SUMMARY. **Zero Rust
files.** `JourneyReceipt` (journey.rs:218-253) has no such field, and `serde`
does not `deny_unknown_fields`, so the classification the previous lane computed
is **silently discarded at parse time**.

So the two gates are not merely stricter/looser versions of one rule. The driver
classifies and then throws anyway; the verifier never receives the classification
at all. Any fix that only relaxes one side produces the contradiction the brief
names. Both sides must be changed together, and the field has to become part of
the verified receipt schema rather than a decoration on it.

### Second find: step 13's wait loop cannot terminate on the state it waits for

`deliveryReconcile` (`:827`) polls
`while (Date.now() < deadline && (t.losses > 0 || t.duplicates > 0))`.
`duplicates` is `arrived - unique` over an append-only journal, so it is
**monotonically non-decreasing** — once one repeat lands the loop cannot exit
except by timeout, and it then burns the whole `ARRIVAL_BUDGET_MS` keeping the
gateway alive, which is the exact condition that manufactures the next
recurrence. That is a second self-inflicted source of the state being graded.

## Plan (to be proven, not asserted)

1. Make `delivery_identity` a first-class, verified receipt field in Rust.
2. Replace the blanket `duplicates != 0` refusal on BOTH sides with one shared
   predicate: clean iff `losses == 0 && replays == 0 && indeterminate == 0`,
   and the buckets must partition `duplicates`.
3. `duplicates > 0` with NO identity block = refusal, not a pass. An
   unclassified repeat is not a clean one.
4. Prove all four quadrants, on both sides, on the SAME receipt bytes.

## Minute 25 — remaining premise claims verified at HEAD

Both controls alive in the same capture (`/usr/bin/grep -c "fn " trigger.rs` = 26,
`/usr/bin/grep -c zzzz_not_present_zzzz` = 0), so these are measurements and not
a dead instrument returning zeros:

- `trigger.rs:238` — `Self::Interval { every_secs } => TriggerBound::new((*every_secs).max(60), 1)` **CONFIRMED**
- `trigger.rs:366` — `let earliest = after + Duration::seconds(bound.min_interval_secs.max(1) as i64)` **CONFIRMED**
- `runner.rs:327` / `:332` — `"cron:{}:{}"` and `"cron:{}:{}:{}"` **CONFIRMED**

**6 of 6 brief claims held.** The one thing the brief did not have — that the
Rust side never sees `delivery_identity` — is the reason the fix had to add a
verified field rather than relax a comparison.

## Minute 120 — both gates changed, four quadrants proven, both directions

Post-fix, at `2d231653` then `5014f070` on hetzner (`hz/journey-gate-honesty`,
SHA asserted after each checkout):

| suite | result |
|---|---|
| `--test journey_receipt_contract` | **39 passed, 0 failed, 0 ignored, 0 filtered out** |
| `--lib journey::` | **32 passed, 0 failed, 0 ignored, 247 filtered out** |
| `-p wcore-channels-registry --test delivery_semantics_declaration` | **8 passed, 0 failed** |
| `clippy --all-targets -- -D warnings` (both crates) | rc=0 |
| `node --test scripts/f24-journey.test.mjs` (Mac) | **36 passed, 0 failed, 0 skipped** |

**PRE-FIX CONTROL** — blanket `duplicates != 0` refusal restored in
`verify_counts` on hetzner, nothing else touched:

```
test result: FAILED. 34 passed; 5 failed
  quadrant_1_a_windows_run_of_proven_recurrences_verifies
  quadrant_2_a_planted_replay_is_refused
  quadrant_3_indeterminate_repeats_are_refused
  a_forged_classification_that_does_not_partition_the_repeats_is_refused
  the_verifier_and_the_driver_return_the_same_verdict_on_the_same_receipt
```

and the agreement test's own message is the finding stated in one line:

```
q1-recurrence-passes: the driver recorded verdict=RECURRENCE and the verifier
returned verdict=UNCLASSIFIED-REPEATS.
```

**q4 stayed GREEN under the control** — so the control is targeted at the change,
not a blanket breakage. Source restored (`git diff --stat` = 0 bytes) and 39/0
re-proven at the same commit.

**Drift-test control, both directions.** Mutation A (delete the correction
sentence) and mutation B (re-assert the refuted sentence a second time) each
turn `the_recurrence_section_keeps_its_measurement_and_its_correction` red —
`7 passed; 1 failed` both times, the other seven unaffected — and the document
restores to 0 bytes of diff.

## Minute 175 — REAL Windows journeys. Not a synthetic.

Two full setup-to-recovery journeys driven on `SeanD@seandesktop`, **17/17 steps
each**, real Task Scheduler install/recover, real hard kill, real independent
sink, against a real shipped `wayland-core.exe`
(`C:\f28h2-target\release\wayland-core.exe`, `wayland-core 0.12.25`, source
`9c4d2612157851abc6d810fc75d153410372dafa`, sha256 `b3b235fcd4039403…`, digest
re-verified after two hops). Driver at lane commit `2a4751b2`, asserted after
checkout. Work under `D:\lane-jgh\`; `C:\actions-runner-*` untouched.

### Run 1 — the default three adapters

```
submitted=12 arrived=36->24 unique=12 duplicates=12 losses=0
replays=0  recurrences=4  indeterminate=8  unidentified=16
  at endpoint(s) twilio.messages,whatsapp.messages
verdict=NOT-PROVEN     driver rc=1
```

**This reproduces the 2026-07-30 measurement exactly** — `recurrences=4`,
`indeterminate=8`, `unidentified=16`, same two endpoints — which is independent
confirmation that the committed `q3` fixture is faithful to the real shape
rather than a tidied-up version of it.

It is also the honest limit of the default adapter set: `twilio.messages` and
`whatsapp.messages` emit no delivery identity, so 8 of the 12 repeats are
**unjudgeable in principle** and a Windows run of that set is correctly graded
NOT-PROVEN however well the product behaves. That is a real outbound-idempotency
gap, not a stuck gate — and it is why `--adapters` had to become explicit before
the passing state could be demonstrated on the real platform.

### Run 2 — keyed adapters only (`--adapters slack`)

```
submitted=12 arrived=36 unique=12 duplicates=24 losses=0
replays=0  recurrences=24  indeterminate=0  unidentified=0
verdict=RECURRENCE
JOURNEY COMPLETE platform=windows receipt=D:\lane-jgh\run2\windows-receipt.json
driver rc=0
```

**`JOURNEY COMPLETE` on Windows.** Twenty-four genuine recurrences across 36
arrivals, every one under a distinct delivery identity. Before this lane that
receipt could not exist: the driver and the verifier each refused any
`duplicates != 0`, and a Windows kill-and-recover leg crosses the 60 s period
every time.

Both runs' step 13 read `arrived=12 duplicates=0` — the repeats landed after it,
which is the live F24-GWP-M1 shape, and the receipt records the true final
counts because the headline and the breakdown now come from one snapshot.

### Cross-gate agreement, on the real receipts

`wayland-journey verify` built on hetzner at the same lane commit, run against
the receipts Windows produced and the binary Windows drove:

| run | driver (on Windows) | verifier (on hetzner) | rc |
|---|---|---|---|
| run 2 | `verdict=RECURRENCE` | `verdict=RECURRENCE` | **0** |
| run 1 | `verdict=NOT-PROVEN` | `verdict=NOT-PROVEN` | **1** |

```
JOURNEY VERIFIED platform=windows commit=9c4d2612… steps=17 submitted=12
arrived=36 unique=12 duplicates=24 losses=0 adapters=1/10 exercised=slack
verdict=RECURRENCE repeats=24 (replays=0 recurrences=24 indeterminate=0 unidentified=0)
```

Note `adapters=1/10` on the success line: run 2 passes, and it is impossible to
read as a matrix.

## Still to establish

- [x] `trigger.rs` and `runner.rs` line citations at HEAD.
- [x] Rust: `delivery_identity` parsed, verified, printed.
- [x] JS: same predicate, same verdict.
- [x] Four quadrants x two gates, both directions.
- [x] `docs/delivery-semantics.md` §5 reworded.
- [x] Windows: **REAL journeys, two of them, both graded and both agreed on.**
      Quadrants 2 and 4 remain fixture-driven — a true replay cannot be planted
      at a real destination without the product misbehaving, and a Windows run
      that misses the 60 s window is not something a kill-and-recover leg can be
      made to do.
