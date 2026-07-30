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

## Still to establish

- [ ] `trigger.rs` and `runner.rs` line citations at HEAD.
- [ ] Rust: `delivery_identity` parsed, verified, printed.
- [ ] JS: same predicate, same verdict.
- [ ] Four quadrants x two gates = eight results, from unproxied tools.
- [ ] `docs/delivery-semantics.md` §5 reworded.
- [ ] Windows: real journey, or a faithful synthetic — SAY WHICH.
