# B-2 transport reset — independent verdict (seat 3)

Fresh seat. Verdict formed by execution before any change was made to the branch.

- Branch under test: `fix/b2-transport-reset` @ `f738a560`
- Worktree: `/root/wt-b2reset` (hetzner-dsm)
- Corpus: private snapshot `/root/b2seat3-canon` copied from `/root/jc-canon` @ `dd1b227b`,
  taken because a harness-repair lane is concurrently editing the canonical tree. Runner
  `/root/run-b2seat3.sh` (identical to `/root/run-canon.sh` except it points at the snapshot).
- Toolchain: cargo 1.95.0 (f2d3ce0bd 2026-03-21)

Every claim below was confirmed or refuted by running something. No claim is graded from the
diff.

## Claim 1 — the constants are derived, not literals. CONFIRMED

Instrument: `grep` over the B-2 surface, plus a cross-crate derivation probe
(`/root/s3-deriv.sh`).

Static: zero bare `from_secs(900)` anywhere in `crates/wcore-providers/src/`,
`crates/wcore-agent/src/engine.rs`, `crates/wcore-config/src/config.rs`. The only literal `900`
hits in the B-2 surface are prose in the doc comment and unrelated test fixtures.

Arithmetic, against the real sources:

- `READ_TIMEOUT = 300s` — `crates/wcore-providers/src/http_client.rs:86`
- `DEFAULT_FAILURE_THRESHOLD = 3` — `crates/wcore-config/src/config.rs:708`
- `UNSERVED_OUTAGE_BUDGET = 300 * 3 = 900s` — `crates/wcore-agent/src/engine.rs:1393`
- `UNSERVED_RETRY_BACKOFF_CAP = DEFAULT_RECOVERY_TIMEOUT_SECS = 30s` — `config.rs:714`

Static agreement is not proof the derivation is live, so it was driven. Changing
`DEFAULT_FAILURE_THRESHOLD` from 3 to 5 — one constant, in a *different crate* — moved the
engine's observed behaviour to values predicted in advance by hand from the backoff schedule:

| arm | threshold 3 (observed) | threshold 5 (predicted) | threshold 5 (observed) |
|-----|-----|-----|-----|
| budget | 900 s | 1500 s | 1500 s |
| fast sends (0 s/attempt) | 36 | 56 | 56 |
| slow sends (60 s/attempt) | 13 | 20 | 20 |

Both mutated-arm predictions were computed before the run and matched exactly. The constant is
genuinely bound to its sources. Restored clean.

## Claim 2 — `sends_the_window_admits` is non-vacuous. CONFIRMED

Instrument: `/root/s3-mutate.sh`, which asserts the mutated line is CODE (rejects `//`, `///`,
`*` prefixes and requires an exact unique match) and `touch`es the file after **both** the
mutation and the restore, so cargo cannot skip the rebuild and measure the wrong binary.

Baseline: `running 1 test` … `test result: ok`. Not zero tests.

| id | mutation | landed on | result |
|----|----------|-----------|--------|
| M-A | engine admission `Instant::now() < deadline` → `stream_attempt < 6` | engine.rs:11681, code | **RED** |
| M-B | predictor `sends += 1` → `sends += 2` | engine.rs:24604, code | **RED** |
| M-B2 | predictor `return sends` → `+1` on the slow arm only | engine.rs:24601, code | **RED** |

M-A is the decisive one: `stream_attempt < 6` is *exactly* the round-1 overfit constant
(`MAX_UNSERVED_STREAM_RETRIES = 6`, which equalled the fixture's `--fault-requests 6`). The
test catches that reversion.

M-B and M-B2 also read out the true observed values, which is how the doc's claim was checked
rather than trusted: **fast = 36, slow = 13**, both exactly as the doc states, and both matching
an independent hand-walk of the backoff schedule done before reading the numbers. The old
`fast_sends > slow_sends * 3` bound really would be false (36/13 = 2.77), so the doc's reason
for deleting it is correct.

All three restored `RESTORE_CLEAN`.

### Honest limitation of this control

The predictor calls the engine's own `unserved_retry_backoff` and the same two constants. So a
change that moves the constants moves both sides together and the test stays GREEN — proven,
not assumed: the threshold 3→5 probe above kept the test passing while the behaviour changed by
55%. This test proves the bound is *time-shaped* and pins the schedule; it does **not** and
cannot defend the choice of 900 s. That number rests on the out-of-corpus probe, and the doc now
says so.

## Claim 4 — fmt and clippy. CONFIRMED

- `cargo fmt --all --check` → `FMT_RC=0`
- `cargo clippy --workspace --all-targets -- -D warnings` → `CLIPPY_RC=0`

Only output is a future-incompat note for the third-party `imap-proto v0.10.2`, which is not a
warning in our code and does not fail the gate.

## Claim 5 — no zero-test false green

Every test invocation in this verdict reported `running 1 test`. No `running 0 tests` result was
accepted anywhere.

## Claim 3 — the wall-clock property at corpus level

Re-analysis of the adversarial seat's primary evidence (`/root/advp-WA`, `/root/advp-WB`,
both from binary `943c2a23`, the `adv/b2-round2-probe` build) **corrects the summary I was
handed**. The pair is not "same shape, different outage length"; the two runs used different
fixture fault shapes (`fault-reset` vs `fault-timeout`). I initially read that as fatal, because
`timeout` is explicitly excluded from `is_unserved_request_failure`. It is not fatal: the
product logs show both shapes were classified by the engine as `connection`, and **both took the
window path**. The fixture's shape name is not the engine's failure code.

What the logs actually show, which is the discriminating measurement:

| run | shape | retries | budget consumed | wall | exit | B-2 |
|-----|-------|---------|-----------------|------|------|-----|
| WB | reset | 12 | ~181 s of 900 | 274.0 s | 0 | control PASS, fault-reset PASS |
| WA | timeout | 9 | ~811 s of 900 | 1045.2 s | 1 | fault-timeout FAIL |

Same code, same budget: the fast-failing outage fits *more* sends into *less* wall clock and the
job completes; the slow-failing outage fits *fewer* sends, exhausts the window, and fails safely
with exit 1. A count bound would have produced the same send count in both. The property holds.

Two corrections to the record while I am here: WB's overall row status is **UNPROVEN**, not
PASS — `fault-503` and `fault-timeout` were not run in that invocation, and the harness
correctly refuses to call an unrun shape survivable. And WA's overall FAIL is carried by the
`fault-timeout` arm only. Neither changes the conclusion, but "WB passed" overstates what that
artifact says.

Status of my own re-run against the fix-branch binary: see the addendum committed after this
file. The analysis above is of archived evidence produced by a *different* binary
(`943c2a23`, adversarial branch) than the one under test (`f738a560`).

## Verdict

`safe_to_merge` — recorded in the addendum, after the fix-branch corpus re-run reports.
Claims 1, 2, 4 and 5 are confirmed by execution and none of them blocks.
