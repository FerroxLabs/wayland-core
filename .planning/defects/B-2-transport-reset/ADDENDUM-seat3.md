# B-2 seat-3 addendum — claim 3 re-run, the full mutation battery, Windows, Phase 3

Companion to `VERDICT-seat3.md`. Everything here was run after that file was committed.

## Claim 3 — re-run on the fix-branch binary. CONFIRMED

Instrument: private corpus snapshot `/root/b2seat3-canon` @ `dd1b227b`, runner
`/root/s3-corpus.sh`, binary `fc9ccd8e…` built from this branch (`cargo build --release`,
RC=0). The archived WA/WB were produced by a *different* binary, `943c2a23…` from the
adversarial worktree, so this is a genuine re-measurement rather than a re-read.

First attempt returned in seconds with `B-2 UNPROVEN`: the snapshot requires
`JOBCORPUS_PROVIDER_TOML`, which the shared `/dev/shm/jobcorpus/env.sh` does not set. That is
the concurrently-edited-harness hazard landing exactly as warned. Worth recording that the
harness **refused to grade** rather than reporting a false green — "the fixture could not be
stood up, so the product was never asked to do the job". Re-run with the fragment
(`/dev/shm/b2cred/provider.toml`, sha256 `6eee4332…`) that `/root/adv_b2.sh` uses.

| arm | shape | retries | budget consumed | wall | exit | fault case |
|-----|-------|---------|-----------------|------|------|------------|
| WB' | reset | 6 | ~15 s of 900 | 98.7 s | 0 | PASS |
| WA' | timeout | 6 | ~465 s of 900 | 619.6 s | 0 | PASS |

Both arms took the **window** path — every retry logged `Ns of outage budget left`, never
`attempt N/M`. That is the discriminating observation, and it reproduces.

The property is visible in the budget column, not the retry column: **the same six induced
faults consumed 15 s in one arm and 465 s in the other, a ~31x spread.** The retry counts are
equal because the fixture heals after `--fault-requests 6`, so the count is set by the fixture,
not by the bound — which is precisely why a count is the wrong unit. A count calibrated on the
fast arm carries no information about the slow one.

Two honest notes against over-claiming:

- My WA' **passed** where the archived WA failed. I am **not** claiming the fix caused that.
  The two runs induced outages of materially different severity (465 s vs ~811 s of budget
  consumed) because per-attempt provider latency differed between the nights. The comparison
  that is safe is within my own pair, above.
- Each invocation still grades **UNPROVEN overall**, because the harness correctly refuses to
  call an un-run fault shape survivable and each of these invocations ran only two of four.

## Full-shape run — B-2 is PASS, for the first time

`fault-503` had never been induced by any seat, in any arm. Every B-2 result on record — mine
above, and both archived adversarial runs — grades **UNPROVEN**, because each invocation ran
two of the four declared shapes and the harness rightly refuses to call an un-run shape
survivable. That is not a verdict on the product; it is a verdict on the invocation.

Run with all four shapes in one invocation (`JOBCORPUS_B2_CASES=control,fault-reset,fault-503,fault-timeout`),
same snapshot, same binary `fc9ccd8e…`:

```
verdict: PASS
  B-2.control        PASS
  B-2.fault-reset    PASS
  B-2.fault-503      PASS
  B-2.fault-timeout  PASS
```

| case | window retries | wall | exit |
|------|----------------|------|------|
| control | 0 | 45.4 s | 0 |
| fault-503 | 6 | 98.7 s | 0 |
| fault-reset | 6 | 191.6 s | 0 |
| fault-timeout | 6 | 624.7 s | 0 |

**B-2 is now genuinely PASS rather than UNPROVEN**, and `fault-503` is exercised for the first
time. Read alongside the new `http_529` unit fixture, both "overloaded" statuses now have
coverage where neither had any.

This table is also the cleanest statement of the property under test. Identical fault counts —
six in every faulted case, because that is what the fixture induces — and a wall clock spanning
98.7 s to 624.7 s, a 6.3x range. Every one survives, because the budget is sized in the unit
the outage actually varies in. The control's `window_retries=0` is the negative control: the
window machinery does not engage when nothing fails.

## The planned mutation battery — all of M1, M3–M8 now run

`RESUME-b2r2.md` planned M1–M8 and recorded "nothing run yet". M2 was run by a previous seat
(it survived, and the test was honestly relabelled a regression guard). M1 is my M-A in
`VERDICT-seat3.md`. The rest were run here, via `/root/s3-m38.py`, which asserts each mutation
lands on **code** (rejects `//`, `///`, `*`, requires a unique exact match, and for M8 confines
the search to the target function's body) and `touch`es after both mutation and restore.

| id | mutation | landed | test | expected | got |
|----|----------|--------|------|----------|-----|
| M3 | `unserved_retry_backoff` flat 500 ms | engine.rs:1415 | `unserved_retry_backoff_doubles_…_holds_the_cap` | RED | **RED** |
| M4 | manufacture `Some(CooldownPermit::HalfOpen)` | resilient.rs:403 | `an_open_circuit_probe_is_single_flight_…` | RED | **RED** |
| M5 | drop `&& cooldown_is_momentary(primary_state)` | resilient.rs:401 | `open_circuit_without_fallback_refuses_a_rate_limited_request` | RED | **RED** |
| M6 | drop `&& !e.is_timeout()` | retry.rs:480 | `a_client_side_request_timeout_is_not_a_destroyed_socket` | RED | **RED** |
| M7 | broken-connection arm → `attempt < max_retries` | retry.rs:737 | `connection_reset_mid_request_is_retried` | RED | **RED** |
| M8 | `try_acquire_unprotected_probe` takes no lease | cooldown.rs:242 | `an_unprotected_probe_is_still_single_flight_while_cooling` | RED | **RED** |

Every restore was byte-identical to its backup (`restore_matches_backup=True` throughout), and
no run reported `running 0 tests`.

## `http_529` — the gap is closed

It had no fixture anywhere. The only thing asserting it was
`is_unserved_request_failure("http_529")` — a predicate agreeing with itself, which says nothing
about whether a real 529 ever reaches that predicate.

Added `an_http_529_outage_is_ridden_out_on_the_window`: a provider that answers every request
with `ProviderError::Api { status: 529 }`, asserting the turn gets the whole window
(exactly `sends_the_window_admits(0)` = 36 sends) rather than the 3-attempt served budget.

That the new test earns its place is itself measured. Mutation **M529** breaks the *wiring*
that carries the status to the classifier (`provider_failure_code` for `Api` returns a constant
`http_500`) while leaving the predicate untouched:

- `an_http_529_outage_is_ridden_out_on_the_window` → **RED**
- `only_unserved_requests_get_the_larger_retry_budget` (the old predicate test) → **GREEN**

The predicate test is blind to the break. The new one is not.

`fault-503` at corpus level is covered by the full-shape run above.

## Windows leg — run for the first time

Never started by either previous seat. Run on SeanDesktop (`D:\b2r2win`, D: only, nothing on
`C:` touched), `cargo 1.95.0`, at HEAD **`f738a560`** with a clean tree verified before and
after.

| target | tests | result |
|--------|-------|--------|
| `wcore-agent --lib an_unserved_outage_is_bounded_by_wall_clock` | 1 | ok |
| `wcore-agent --lib only_unserved_requests_get_the_larger_retry_budget` | 1 | ok |
| `wcore-agent --lib unserved_retry_backoff_doubles_…` | 1 | ok |
| `wcore-providers --test provider_transport_reset_test` | 4 | ok |
| `wcore-providers --test adv_b2_default_install_test` | 5 | ok |

`Get-Process wayland-core` was clean (0) before every measurement. No target reported
`running 0 tests` — and the zero-test trap **fired and was caught**: a helper named its
parameter `$args`, colliding with PowerShell's automatic variable, so cargo ran argument-less,
printed help, and exited 0. Six clean-looking `EXITCODE: 0` results with no `running` lines.
The table above is the corrected re-run. Exit code alone can never certify a cargo test run.

The 36/13 send counts were **not directly observable** on Windows — the test prints nothing on
the success path and instrumenting the verified checkout was declined. They are implied: the
test asserts `observed == sends_the_window_admits(delay)`, and that helper is pure arithmetic
over two constants with no clock read and no platform branch, so an exact-equality pass means
Windows observed 36 and 13. Sound, but it is an inference, not a printed number.

**Merge logistics, flagged:** `fix/b2-transport-reset` exists on **no remote**. 415 heads plus
tag and PR refs on `FerroxLabs/wayland-core` match neither the branch name nor `f738a56`. The
commit had to be moved to Windows as a verified thin bundle. It currently lives only on
hetzner and SeanDesktop. This is a process gate, not a code defect, but nothing can be merged
until the branch is pushed.

## Phase 3 — unserved re-sends are now disclosed

Implemented as decided: the window is not narrowed; the product's existing honesty is extended.

`AgentEngine::unserved_resends` counts every physical send classified unserved. It is reset and
disclosed in a wrapper around `run_inner`, which is the correct choke point — `run_with_content`
ends at line 6835 and the third `run_inner` call site is inside `resume_interrupted_turn`, a
separate public turn entry, so wrapping the outer function would have missed recovered turns.

On a turn that had any, exactly once, however the turn exits:

> `36 provider requests never returned a response. Each were dispatched, so the provider may
> have served and billed it; that spend is not included in any cost or token figure shown here.`

Gate, both directions, and both proven non-vacuous:

| mutation | `…reports_how_many_requests_never_returned_a_response` | `a_clean_turn_says_nothing_about_unreturned_requests` |
|----------|------|------|
| none (baseline) | GREEN | GREEN |
| P3A suppress the disclosure | **RED** | GREEN |
| P3B count off by one per send | **RED** | GREEN |
| P3C always emit (`if false`) | GREEN | **RED** |

P3C is the one the brief asked for specifically: it proves the message cannot silently become a
sentence emitted on every turn. P3B proves the count is checked for accuracy, not mere presence
— the test asserts the reported number equals the physical sends the provider actually saw.

### And it was proven live, not only in unit tests

Unit tests prove the logic; they do not prove a user ever sees the sentence. Release binary
`46b7750a…` built from this branch, run through the real corpus fixture against the real
provider and the real fault proxy:

| arm | window retries | product exit | disclosure lines |
|-----|----------------|--------------|------------------|
| control | 0 | 0 | **0** |
| fault-reset | 6 | 0 | **1** |

read verbatim out of the product's own stdout:

> `6 provider requests never returned a response. Each were dispatched, so the provider may
> have served and billed it; that spend is not included in any cost or token figure shown here.`

The count `6` matches the six window retries the same run logged, and both directions hold on
real hardware: absent on the clean arm, present and accurate on the faulted one.

The important detail is `exit=0`. The turn **succeeded** — `B-2.fault-reset` graded PASS — and
the disclosure still fired. That is precisely the case where the spend would otherwise be
invisible: the user gets their work, sees a normal successful result, and would never learn
that six billable requests were also sent. A disclosure that only appeared on failed turns
would have missed it.

Two honest notes. First, that capture shows **"Each were dispatched"** — a real copy defect I
introduced ("each" is singular whatever the count). Found by reading the live output rather
than the test, because the tests assert on the count and the keywords, not on grammar. Fixed in
`9beb8c70`; the capture above predates the fix and is left as-run rather than doctored.

Second, the first live attempt **failed** with `Session persistence authority unavailable: …
the configured store rejected this profile's recovery key`, before any provider call. It is a
transient environment fault, not a regression: the control arm of that same run opened the
vault and exited 0 with the same binary, and the second sample — same binary, same env, same
script — ran clean. I could not reproduce it a second time, so I cannot fully explain it, and
I am recording it rather than dismissing it.

## A separate defect found on the way: `wcore-agent --lib` is flaky under parallelism

Not part of the B-2 brief, found while checking my own change for regressions, and it needs an
owner. `cargo test -p wcore-agent --lib` does not pass cleanly on hetzner at **either** commit:

| sample | commit | result |
|--------|--------|--------|
| 1 | `713e68a8` (with my changes) | 2310 passed, **19 failed** |
| 2 | `f738a560` (before my changes) | 2308 passed, **18 failed** |
| 3 | `713e68a8` (same code as sample 1) | 2305 passed, **24 failed** |

It is flakiness, not a regression, and that is established rather than assumed — three ways,
the last of which is conclusive on its own:

1. A regression would make the baseline failure set a strict subset of mine. It is not: 6 tests
   fail only at the baseline and 7 only at HEAD, with 12 in common.
2. **Samples 1 and 3 are the same commit and disagree** — 19 failures vs 24, with 2 tests
   failing only in the first and 7 only in the third. Identical code, different verdict. No
   property of my change can explain that.
3. All 7 of the "only at HEAD" tests **pass in isolation** at HEAD:

`rejects_append_reopen_reduce_corruption`, `invalid_live_session_switch_leaves_existing_authority_untouched`,
`post_charge_budget_cap_recovers_terminal_without_running_tools`, `resumed_engine_holds_journal_lease_until_drop`,
`session_retirement_never_deletes_a_replacement_journal_or_collateral`,
`cancel_terminates_an_interrupted_turn_and_the_state_is_durable`,
`cleanup_error_attempts_all_artifacts_but_retains_index_authority` — 7/7 `ok` when run `--exact`.

The affected tests cluster in session, journal, lease and lifecycle code — shared-filesystem
and exclusive-lease territory, run 96-ways parallel on a box with other lanes active. That is
a plausible mechanism, but I did not isolate the true cause, so treat the mechanism as a
hypothesis and the flakiness itself as measured.

This predates the change and does not implicate it. It is still ours: a suite that fails ~18
tests per run cannot certify anything, and any future seat running it will burn time
re-deriving what this section already establishes.

## Verdict

**`safe_to_merge = true`**

Claims 1, 2, 4 and 5 confirmed by execution; claim 3 re-measured on this branch's own binary
and confirmed. The full planned mutation battery is run and every test died as designed. The
Windows leg is green. The two gaps I was asked about are closed rather than deferred.

Conditions, none of which is a defect in the change:

1. **The branch is on no remote.** It must be pushed before it can be merged.
2. The wall-clock control cannot defend the *value* 900 s, only its shape — proven, since
   moving `DEFAULT_FAILURE_THRESHOLD` 3→5 changes behaviour 55% and keeps the suite green. The
   900 s rests on the out-of-corpus probe, and the doc now says so plainly.
3. The corpus can never see the billing risk: its fault proxy returns before relaying upstream,
   so a faulted request never reaches the provider. Anything that widens the window needs
   `scripts/b2_transport_billing_probe.py`, not a corpus run. Phase 3 mitigates the visibility
   half of this; it does not add a spend ceiling, and there still is none.
