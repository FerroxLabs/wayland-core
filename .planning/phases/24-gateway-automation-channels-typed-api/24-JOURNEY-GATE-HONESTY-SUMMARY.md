---
lane: journey-gate-honesty
branch: lane/journey-gate-honesty
base: 5013505e7caefa5561f0de40c75406afe1b42fc3
findings: [F24-GWP-H1]
status: COMPLETE — the gate now passes on a real Windows journey and still fails on a dishonest one
---

# 24-JOURNEY-GATE-HONESTY — the permanently-red gate, repaired in both directions

**Verdict: the lane's goal is ACHIEVED.** The Windows setup-to-recovery journey now has a
reachable pass state, and reached it — `JOURNEY COMPLETE`, rc=0, on real hardware. A planted
replay, an unjudgeable repeat, a loss, an unclassified repeat and a forged classification are
each still refused, by both gates, with the same verdict.

**I drove REAL Windows journeys, twice. Not a synthetic.** Quadrants 2 and 4 are
fixture-driven and I say why below.

---

## 1. The premise, re-verified before acting

Per LANE-BRIEF "your brief's measurements are probably stale". Both instrument controls run in
the same capture (`grep -c "fn " trigger.rs` → **26**, `grep -c zzzz_not_present_zzzz` → **0**),
so these are measurements rather than a dead grep returning zeros.

| Brief claim | At HEAD |
|---|---|
| `every:15` rate-floored to 60s, `trigger.rs:238`, applied `:366` | **HELD** — verbatim |
| delivery id is `cron:{job}:{scheduled_millis}`, `runner.rs:324-338` | **HELD** — `:327` / `:332` |
| Rust `verify_counts` refuses ANY `duplicates != 0` | **HELD** — `journey.rs:559-564` |
| the receipt carries `delivery_identity` | **HELD** — `f24-journey.mjs:1043` |
| the driver also refuses any `duplicates != 0` | **HELD, and in two places** — `:833` and `:1069` |
| 8 of 24 arrivals carry an `idempotency_key` | **HELD** — and re-measured live tonight |

**6 of 6 held.** One thing the brief did not have changed the shape of the fix.

### The find: the verifier could not see the field

`/usr/bin/grep -rln delivery_identity .` over the tree returns **two paths, neither of them
Rust**. `JourneyReceipt` had no such field and `serde` does not `deny_unknown_fields`, so the
previous lane's classification was **discarded at parse time**. The two gates were therefore not
a strict and a lax version of one rule — the verifier had no input on which it *could* agree.
Relaxing the driver alone would not merely have produced disagreement; it would have produced
disagreement with no way to fix it from the driver's side.

### Second find: step 13 waited on a count that cannot fall

`deliveryReconcile` polled `while (losses > 0 || duplicates > 0)`. `duplicates` is
`arrived - unique` over an **append-only** journal — monotonically non-decreasing — so once one
repeat landed the loop could not exit except by timeout, then held the gateway alive for the
full 180 s budget, crossing three more 60 s trigger periods and **manufacturing the repeats it
was waiting to see disappear.** Now waits on `losses` alone.

---

## 2. What changed

**One predicate, written in both languages in the same clause order.** A delivery leg is clean iff:

1. the arithmetic reconciles;
2. `losses == 0`;
3. every repeat is classified — `duplicates > 0` requires an identity block whose three buckets
   sum to exactly `duplicates`;
4. `replays == 0`;
5. `indeterminate == 0`.

`recurrences` is unconstrained. `indeterminate` and `unidentified` count **against** the run.

- `crates/wcore-eval-scenarios/src/journey.rs` — `DeliveryIdentity` is now a verified receipt
  field; `verify_counts(counts, identity)`; the blanket `DirtyReconciliation` is replaced by
  seven named refusals; the classification is **printed on the success line**.
- `scripts/f24-journey.mjs` — `classifyVerdict` implements the same five clauses; both driver
  gates use it; step 13's wait loop repaired; `assertFinalReconciliation` returns its report on
  the clean path so a **passing** run also states its verdict.
- `scripts/f24-journey-quadrants.mjs` (new) — generates the four quadrant receipts through the
  driver's own `receipt()` path and writes the driver's verdict to a **sidecar**.
- `--adapters` (new, opt-in) — see §5.

### `duplicates == 0` may still omit the block, and that is not a hole

A replayed delivery identity is by construction a second arrival of the same job text, so it
always shows up as a repeated **body** first. `duplicates == 0` therefore implies
`replays == 0` without the field. Silence is permitted exactly where it is vacuous;
`duplicates > 0` with no block is `UNCLASSIFIED-REPEATS`.

### Why agreement is a string equality, not a claim

Both gates emit exactly one `verdict=<TOKEN>` from a shared ten-token vocabulary. The driver's
verdict is written to a **sidecar file, never into the receipt** — a receipt carrying the
driver's answer would let the verifier grade the driver's *opinion* instead of its
*measurements*, and "the two agree" would be a tautology.

---

## 3. The four quadrants — results, and both gates on each

Fixtures are **real driver output** (`receipt()` over synthetic arrival journals), committed at
`crates/wcore-eval-scenarios/tests/fixtures/journey-quadrants/`. **q1, q2 and q3 carry a
byte-identical headline** — `submitted=12 arrived=24 unique=12 duplicates=12 losses=0` — so
nothing but the identity block can separate a correct slow-platform run from a
duplicate-delivering one.

| # | state | driver | verifier | agree | rc |
|---|---|---|---|---|---|
| 1 | proven recurrences | `RECURRENCE` | `RECURRENCE` | ✅ | **PASS** |
| 2 | planted true replay | `EXACTLY-ONCE-VIOLATED` | `EXACTLY-ONCE-VIOLATED` | ✅ | FAIL |
| 3 | `indeterminate > 0` | `NOT-PROVEN` | `NOT-PROVEN` | ✅ | FAIL |
| 4 | clean, no repeats | `NO-REPEATS` | `NO-REPEATS` | ✅ | **PASS** |

Agreement is asserted by `the_verifier_and_the_driver_return_the_same_verdict_on_the_same_receipt`,
which compares the token **and** the pass/fail it implies, and counts the pairs it ran (4, so a
loop that silently shortened cannot report a pass over nothing).

### Gate results, with executed counts read back (§3.2)

| suite | host | result |
|---|---|---|
| `--test journey_receipt_contract` | hetzner | **39 passed, 0 failed, 0 ignored, 0 filtered out** |
| `--lib journey::` | hetzner | **32 passed, 0 failed, 0 ignored, 247 filtered out** |
| `wcore-channels-registry --test delivery_semantics_declaration` | hetzner | **8 passed, 0 failed** |
| `clippy --all-targets -- -D warnings` (both crates) | hetzner | rc=0 |
| `node --test scripts/f24-journey.test.mjs` | Mac | **38 passed, 0 failed, 0 skipped** |
| `cargo fmt --all -- --check` | Mac | rc=0 |

### CAN IT FAIL? The pre-fix control

Blanket `duplicates != 0` refusal restored in `verify_counts` on hetzner, nothing else touched:

```
test result: FAILED. 34 passed; 5 failed
  quadrant_1_a_windows_run_of_proven_recurrences_verifies
  quadrant_2_a_planted_replay_is_refused
  quadrant_3_indeterminate_repeats_are_refused
  a_forged_classification_that_does_not_partition_the_repeats_is_refused
  the_verifier_and_the_driver_return_the_same_verdict_on_the_same_receipt
```

and the agreement test states the whole problem in one line:

> `q1-recurrence-passes: the driver recorded verdict=RECURRENCE and the verifier returned verdict=UNCLASSIFIED-REPEATS.`

**q4 stayed GREEN under the control**, so it is targeted at the change rather than blanket
breakage. Source restored (`git diff --stat` → 0 bytes) and 39/0 re-proven at the same commit.

---

## 4. REAL Windows journeys — and I say which is which

Two full journeys on `SeanD@seandesktop`, **17/17 steps each**, real Task Scheduler
install/recover, real hard kill, real independent sink, driver at lane commit `2a4751b2`
(asserted after checkout), against a shipped `wayland-core.exe` (`0.12.25`, source
`9c4d2612…`, sha256 `b3b235fc…`, digest re-verified after two network hops). Work under
`D:\lane-jgh\`; `C:\actions-runner-*` untouched.

**Run 1, default three adapters** — `replays=0 recurrences=4 indeterminate=8 unidentified=16`
at `twilio.messages,whatsapp.messages`, `verdict=NOT-PROVEN`, rc=1.
**This reproduces the 2026-07-30 measurement exactly**, which is independent confirmation that
the committed `q3` fixture is faithful rather than a tidied-up version of the real shape.

**Run 2, `--adapters slack`** —
`submitted=12 arrived=36 unique=12 duplicates=24 losses=0`, `replays=0 recurrences=24
indeterminate=0`, `verdict=RECURRENCE`, **`JOURNEY COMPLETE platform=windows`**, rc=0.

Cross-gate on those real receipts — `wayland-journey` built on hetzner at the same lane commit,
run against the receipts Windows produced and the binary Windows drove:

| run | driver (Windows) | verifier (hetzner) | rc |
|---|---|---|---|
| 2 | `verdict=RECURRENCE` | `verdict=RECURRENCE` | **0** |
| 1 | `verdict=NOT-PROVEN` | `verdict=NOT-PROVEN` | **1** |

```
JOURNEY VERIFIED platform=windows commit=9c4d2612… steps=17 submitted=12 arrived=36
unique=12 duplicates=24 losses=0 adapters=1/10 exercised=slack verdict=RECURRENCE
repeats=24 (replays=0 recurrences=24 indeterminate=0 unidentified=0)
```

Both runs' step 13 read `arrived=12 duplicates=0` — the repeats landed after it, the live
F24-GWP-M1 shape, and the receipt records the true final counts.

**Quadrants 2 and 4 remain fixture-driven, and that is a limit, not an omission.** A true replay
cannot be planted at a real destination without making the product misbehave, and a Windows
kill-and-recover leg cannot be made to finish inside the 60 s window — `PT1M` exceeds it by
construction. Both are proven on committed driver-produced bytes through the compiled verifier.

---

## 5. The finding this produced, and it is not comfortable

**A real Windows run of the DEFAULT adapter set cannot reach the passing verdict, and should
not.** `twilio.messages` and `whatsapp.messages` emit no delivery identity, so on any platform
slow enough to cross a trigger period 8 of the 12 repeats are unjudgeable **in principle** and
the run is `NOT-PROVEN` however well the product behaves. Measured live tonight, not argued.

That is an honest refusal about a real outbound-idempotency gap — `docs/delivery-semantics.md`
§6 explains why those seven adapters cannot simply declare `true` — and it is **distinct from**
the permanently-red gate this lane removed. The old gate refused every Windows run for the wrong
reason; the new one refuses this adapter mix for the right one, and passes a keyed mix.

So `--adapters` is opt-in and **defaults to all three**. Defaulting it to the keyed set would
quietly trade adapter coverage — a separate criterion, and the one Phase 24 was previously caught
overstating — for a greener verdict. Narrowing must be typed out, it lands in the receipt's
`adapter_coverage`, the success line reads `adapters=1/10`, and `verify --min-adapters N` still
refuses a narrow run whenever the claim is a matrix.

---

## 6. `docs/delivery-semantics.md` §5 — reworded

It said: *"On Windows, a gateway whose runtime restarts across the Task Scheduler `PT1M`
repetition boundary re-fires cron jobs that have already fired."* **Both halves are wrong**, and
the run it cited is the evidence against them:

- *"re-fires jobs that have already fired"* — the second delivery is a **different scheduled
  occurrence**, not a re-fire. Every repeat carried a different delivery id: 5 of 5 keyed jobs,
  zero replays.
- *"On Windows"* — platform-neutral. Windows crosses the window **reliably**, not exclusively.

The section now leads with the measurement, keeps the histogram, adds the **heartbeat control**
(60068 ms / 64940 ms in the same run, three occurrences nobody called duplicates), states the
run-duration prediction that makes it **deterministic rather than intermittent** (67.7 s → 12
repeats, 0.3 s → 0), answers "do the exactly-once rows still hold on Windows?" (**yes,
unchanged** — more delivery ids each delivered once is a stronger measurement), names the real
gap (8 of 24 keyed), and carries the grading table. §2's summary row and §4's cross-reference
follow. Per-cell citations and the machine-readable block untouched.

`.planning/CRITERIA-STATUS.md` `24-C1` said the finding *"defeats"* exactly-once on all three
exactly-once adapters. Corrected in place.

### The drift test was repaired, not merely noted (§6b-ii)

`the_windows_duplicate_finding_is_still_disclosed` asserted the document keeps saying Windows can
duplicate, on the premise that F24-GWP-H1 was "measured, open and unfixed". **It would have gone
on PASSING over the corrected section**, because both strings it grepped for survive the
correction — a gate still green after the claim beneath it was inverted. Rewritten as
`the_recurrence_section_keeps_its_measurement_and_its_correction`: it asserts the evidence
(`{2: 12, 3: 1}`, `60068`, `trigger.rs:238`, `5 of 5 keyed jobs`), that the id stays findable,
that the correction is stated, and that the refuted sentence appears **exactly once** — inside
the quotation — with a **known-positive for that search in the same test**, because a negative
assertion is free on a dead instrument.

Both directions proven: mutation A (delete the correction) and mutation B (re-assert the refuted
sentence) each give `7 passed; 1 failed`, the other seven unaffected, document restored to 0
bytes of diff.

---

## 7. Instrument defects found and repaired in-lane

1. **The wait-loop matcher** `/while \(Date\.now\(\) < deadline[^)]*\)/` stopped at the first
   `)` and returned the identical string for pre-fix and post-fix source — it **reported the
   repair present before the repair existed**. Repaired, and given the third assertion §6b-ii
   requires: the matcher must SEE the defect in the old source.
2. **That same matcher searched the whole file** and found the *recovery* poll rather than step
   13 — a confident answer about code nobody asked after. Scoped to `deliveryReconcile()`.
3. **`verdictFor` spoke a second grammar** (`VERDICT: <TOKEN>` vs `verdict=<TOKEN>`). Two
   spellings mean two extractors and the one that fails to match fails silently. One grammar now.
4. **`grep -c X || echo 0` returned `"0\n0"`** as the brief warns; switched to `grep -q`.
5. **`ssh … | grep` hung indefinitely** — a test binary left a child holding the pipe open, so
   the pipeline never saw EOF and the run looked like a stall. Switched to the brief's
   write-to-file / read-back-separately pattern for every remote measurement.
6. **A pre-existing hang** in `wcore-eval-scenarios --lib`
   (`process_tree::linux::tests::pre_exchange_failure_retains_private_materialization_for_recovery`,
   >60 s) is unrelated to this lane — different module, present before it. Not fixed; recorded
   here so the next lane does not spend the same twenty minutes on it. Journey suites were run
   scoped, with the filtered-out count read back (`247 filtered out`) so the filter is not
   silently matching nothing.

Every number in this document came from an unproxied tool redirected to a file and read with the
Read tool, per §3b.

---

## 8. What I did NOT do

- **Did not touch `trigger.rs`'s 60 s rate floor.** Explicitly forbidden, and other lanes depend
  on those receipts.
- **Did not change the journey's default adapter set.**
- **Did not run `wcore-contract generate`**, open a PR, merge, tag, or close an issue.
- **Did not fix the outbound-idempotency gap** in `twilio.messages` / `whatsapp.messages`. It is
  a product decision (`docs/delivery-semantics.md` §6), it is the reason a default-set Windows
  run is `NOT-PROVEN`, and it is now measurable rather than invisible.
- **Did not run the full `wcore-eval-scenarios` suite to completion** — a pre-existing unrelated
  hang blocks it (§7.6). Ran the journey suites scoped, counts read back.
- **Did not fix the pre-existing hang.** Out of scope, and guessing at `process_tree::linux`
  under five concurrent lanes is how a false regression gets reported.

## 9. For the orchestrator to serialise

- **`crates/wcore-eval-scenarios/src/journey.rs` is a public-API change**: `verify_counts` takes
  a second argument, `JourneyError::DirtyReconciliation` is gone, `JourneyReceipt` gains
  `delivery_identity`. `wayland-journey` and the contract test are the only callers in-tree
  (`/usr/bin/grep -rn verify_counts --include="*.rs"` → 2 hits, both in that crate).
- **No edits to `crates/wcore-cli/src/lib.rs` or `main.rs`.** The shared-file fence is untouched.
- `.planning/CRITERIA-STATUS.md` has a one-row edit (`24-C1`).
