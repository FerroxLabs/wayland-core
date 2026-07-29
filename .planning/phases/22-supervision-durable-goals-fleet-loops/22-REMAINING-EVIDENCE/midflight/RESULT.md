# F05-TRUTH-2 (mid-flight monitor) — live measurement against the shipped binary

**Binary:** `wayland-core 0.12.25` release, built on `hetzner-dsm` at
`5457710e5bccd7c91a117f055ed42531bc2327bb` (`/root/wayland-22-remaining/target/release/wayland-core`).
**Date:** 2026-07-29. **Harness:** `../midflight-live.sh` + `../canned_provider.py`.

Everything below is read back out of **the product's own stdout**, not inferred from source
and not inferred from the environment (LANE-BRIEF §3b-ii — `/root/.wayland/.env` injects
`ANTHROPIC_API_KEY` regardless of the shell, so the provider identity is read back from the
product's own log line and from the canned endpoint's request log, both of which show the
turn was served by `http://127.0.0.1:18733`).

## Claim under test

COMPETITIVE-LEDGER `F05-TRUTH-2`, and the `GOAL-*` row that cites it as a checkable blocker:

> | 2 | Mid-flight monitor | Unavailable: runtime path unwired | None | GOAL-* | **Unchanged.** No adapter surface was built; 22-02 Task 3 unattempted |

Both columns are FALSE at this SHA.

## Result — positive run (`CANNED_TOOL_TURNS=6`)

`wl22r/stream.jsonl`, 27 `capability_activation` events. The mid-flight monitor's own chain,
verbatim and in order:

```
{"type":"capability_activation","capability":"mid_flight_monitor","stage":"declared"}
{"type":"capability_activation","capability":"mid_flight_monitor","stage":"configured"}
{"type":"capability_activation","capability":"mid_flight_monitor","stage":"constructed"}
{"type":"capability_activation","capability":"mid_flight_monitor","stage":"ready"}
{"type":"capability_activation","capability":"mid_flight_monitor","stage":"reached"}
{"type":"capability_activation","capability":"mid_flight_monitor","stage":"outcome_changed"}
{"type":"capability_activation","capability":"mid_flight_monitor","stage":"observed"}
```

and the runtime consult that produced the last three:

```
{"type":"mid_flight_monitor_decision","directive":"replan","reason":"repeated_error"}
```

- **Startup truth column:** `ready`, not `unavailable`. The product never emits
  `runtime_path_unwired` for this capability.
- **Runtime outcome proof column:** the `reached → outcome_changed → observed` triple,
  emitted only after a real side effect — five real `Read` dispatches, three of which
  returned the same root-cause signature.

## One-variable negative control (`CANNED_TOOL_TURNS=3`)

`wl22r-neg2/stream.jsonl`. Same binary, same config, same harness; **one variable**, the
number of canned tool turns, taking the identical-error count from 3 (≥ `REPEAT_THRESHOLD`)
to 2 (below it).

| | positive (6) | negative (3) |
|---|---|---|
| `tool_result` events | 5 | **2** |
| `mid_flight_monitor_decision` | **1** | **0** |
| `mid_flight_monitor … "stage":"reached"` | **1** | **0** |
| `mid_flight_monitor … "stage":"ready"` | 1 | 1 |

So the measurement is falsifiable at a point: real tool errors below the threshold produce
no decision and no occurrence, while `ready` is unaffected. A gate that only asserted exit
status would have read `PRODUCT_RC=0` in both runs and distinguished nothing.

## Instrument liveness

Every run prints, before driving the product:

```
PROBE_POSITIVE_HTTP=200          # canned endpoint answers
PROBE_NEGATIVE=000rc=7           # the adjacent dead port does not
```

## Two further stale rows found incidentally (NOT this lane's to fix)

The same activation stream also shows, at this SHA:

| F05 row | receipt says | the shipped binary emits |
|---|---|---|
| 1 Pricing refresher | `Unavailable: no production constructor` | `unavailable` / **`disabled_by_config`** |
| 3 Cooldown tracker | `Unavailable: no production constructor` | **`ready`** (declared→configured→constructed→ready) |

Both map to `CONT-*` (cache economics), not `GOAL-*`. Reported, not edited — see the
SUMMARY. `learned_policy` is the one row of the four that the binary still reports
`unavailable / runtime_path_unwired`, which is what this lane went on to wire.
