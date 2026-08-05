# 23B-04 — the authorized clock policy for the multi-day journey

Task 1 of `23B-04-PLAN.md`. Measured first, cross-audited second, committed
third. The measurement removed two of the four options before the panel argued,
and the panel then split three ways on the two that remained plus escalate.

**This decision was NOT parked on a human.** It is a cost-versus-evidence
tradeoff, which is not on the reserved list.

---

## The machine-readable record

```
authorized_option=real-time-full
determination=system-clock-at-evaluation
determination_evidence=.planning/phases/23B-continuous-agency/evidence/23B-04-clock-probe-linux.log:experiment A (45s real gap across a real process death) exhausted the wall-time envelope while its control B (same durable state, no gap) did not, so the comparison consults the system clock read at evaluation time
decision_basis=minority-stronger-evidence
decision_basis_evidence=.planning/phases/23B-continuous-agency/evidence/23B-04-clock-probe-linux.log:thirteen candidate acceleration seams were attempted and every one was inert, and the shipped binary exposes zero clock/deadline/now flags, so F23_04_CLOCK_INJECTION_SEAM=none removes both accelerated options as mechanisms the product does not have
panel_codex_position=real-time-full
panel_codex_sha256=56fe28928e0c15b44405dda5b66131dd4b997e9403efa418f78de0a5c49d5b02
panel_gemini_position=escalate
panel_gemini_sha256=bd4f644699fee9df0d61a8b12e070628223a5e1dfc8b38d05e02b84c743b35e4
panel_kimi_position=accelerated-except-absolute-deadline
panel_kimi_sha256=a3cbc8ad5d199ea12c7f68c1a9a50b1fed6db2d94747cec5bb91407c2a3221dc
panel_internal_position=real-time-full
panel_internal_sha256=a7fb7a4d4076ca16b2b669d66067ba189e5ce1cf5142adaa5342432036e712d7
panel_gemini_dissent=Acceleration is impossible because no clock injection seam exists, and real-time-full is blocked because the macOS leg cannot run at all: Cargo is forbidden on the Mac by the phase's controlling instruction and scripts/f23-macos-binary.sh was never landed. You cannot execute a plan when the mechanisms it depends on are missing, so the decision should be recorded as open rather than closed on a shape that cannot be carried out.
panel_kimi_dissent=Only the absolute-deadline authority was measured to charge real downtime across process death; active-runtime measurably does not, so compressing an active-runtime leg's span loses nothing real. Option 3's standing objection was that its determination might be wrong; that determination is now measured fact on the authoritative host against the commit under test. real-time-full therefore spends three calendar days and three hosts to re-prove, for behaviours with no measured real-time dependency, what a 45-second real gap across a real process boundary already proved on the code path that matters.
linux_required_real_span_seconds=259200
macos_required_real_span_seconds=259200
windows_required_real_span_seconds=259200
```

No platform records a zero span, so no `weaker_claim_<platform>=acknowledged`
line appears: under `real-time-full` no leg may be accelerated, and a platform
that cannot run records OPEN with its blocker named rather than green on a
simulated span.

---

## Step 1 — the live determination, which ran before the panel was convened

`scripts/f23-clock-probe.sh` ran on `hetzner-dsm` against
`target/release/wayland-core` built at `9be07203bc65bb2cce87621a85df609b2da0ccaa`,
whose own `--build-info` reports `(source 9be07203...)`. Full capture:
`evidence/23B-04-clock-probe-linux.log`, nonce `64b35c212168149b`.

Each experiment armed durable budget authority in one real process, let that
process EXIT, and bound the authority again in a SECOND real process. Nothing
was injected and the host system clock was never moved.

| Experiment | Setup | Observed |
|---|---|---|
| **B (control)** | absolute deadline, 20s wall cap, restored with **no** gap | `exceeded=false` |
| **A** | identical durable state, restored after a **45s real gap** | `exceeded=true reason=max_wall_time` |
| **C** | active runtime, 20s wall cap, restored after a **45s real gap** | `exceeded=false` |
| **D** | 13 candidate env seams + the binary's own `--help` | every seam inert; `0` clock/deadline/now flags |

A and B differed on nothing but how much real time elapsed while no process
existed, so the experiment discriminated and the determination is
`system-clock-at-evaluation`. C is the counterpart: the active-runtime form does
**not** charge downtime, so a multi-day gap must not consume its envelope — which
is what the journey's `authority-envelope` invariant asserts on every resume.

```
F23_04_ABSDEADLINE_EVAL=system-clock-at-evaluation
F23_04_ABSDEADLINE_CHARGES_DOWNTIME=true
F23_04_ACTIVERUNTIME_CHARGES_DOWNTIME=false
F23_04_CLOCK_INJECTION_SEAM=none
F23_04_ACCEL_HONEST_FOR_ABSOLUTE_DEADLINE=false
F23_04_ABSDEADLINE_PRODUCT_CONSTRUCTION_SITES=0
F23_04_ABSDEADLINE_PRODUCT_REACHABLE=false
```

### Where the code and the probe disagreed with the plan, and the probe won

The plan assumed the absolute-deadline form could be exercised "against the
SHIPPED binary through 23B-01's session verbs". It cannot. Measured over the
tree under test, `BudgetWallClockAuthority::AbsoluteDeadline` has **zero**
production construction sites: `bootstrap.rs:194`, `engine.rs:22926`,
`recovery.rs:1509`, `spawner.rs:3152`, `spawner.rs:3227` and `tool_budget.rs:354`
each hardcode `ActiveRuntime`, and `BudgetConfig` (`wcore-budget/src/config.rs`)
exposes no deadline field at all. No CLI flag, config key or environment
variable reaches it.

The probe therefore exercised the form through `wcore-agent`'s public API from a
real two-process harness rather than through the CLI, and records that
distinction rather than blurring it. This is recorded as finding **23B-04-M1**
(severity MEDIUM, see the SUMMARY): a defended, reducer-guarded, fail-closed
safety mechanism that no user can currently reach. It harms nobody today —
which is why it is not HIGH — but it means the absolute-deadline half of this
plan's own premise is presently dead capability.

---

## Step 2 — the four-way cross-audit over that same measurement

One bundle (`evidence/23B-04-panel-bundle.md`, nonce `05028d2c95e3a675`, 7,049
bytes) went verbatim to all four members. Every reply is captured verbatim,
including hook noise, MCP truncation notices and repeated final blocks, and the
five-line run-time manifest verifies (`shasum -a 256 -c`).

| Member | Position | Core of the argument |
|---|---|---|
| codex (gpt-5.6-sol) | `real-time-full` | Only absolute-deadline inherently needs real time; the others could accelerate **if a trustworthy seam existed**. It does not — every attempted seam was inert. That removes both accelerated options; escalation leaves Criterion 5 open; so full real time is the executable choice. |
| gemini (3.1-pro-preview) | `escalate` | Same seam finding, opposite conclusion: with acceleration impossible and the macOS leg unable to run at all, no option is executable, so record the decision open. |
| kimi (K3) | `accelerated-except-absolute-deadline` | The probe converts option 3's one CON — "depends entirely on the determination being correct" — into measured fact. Active-runtime provably has no downtime dependency, so compressing it loses nothing; real-time-full buys three days of calendar for behaviours measured not to need it. |
| internal (adversarial) | `real-time-full` | Argued against kimi's option first, then gemini's, then steelmanned the objection to its own. |

**Three different positions from three external members. No option drew three of
four, so `decision_basis=majority` is arithmetically unavailable.**

---

## Step 3 — the commitment, and why the minority

`real-time-full` drew two of four. It is committed to on
`minority-stronger-evidence`, and the stronger evidence is a single measured
fact that the two dissenting positions each argue around rather than through.

**Against kimi, the strongest dissent.** Kimi's case rests on: "for
active-runtime legs you don't need to fake anything; downtime isn't charged, so
a compressed journey is honest by construction." That is correct about budget
arithmetic and does not reach Success Criterion 5, which is not a claim about
arithmetic. Three things a compressed span cannot buy:

1. **The wait condition becomes unfalsifiable.** The journey's wait is satisfied
   by real elapsed time. Compress the span and it completes on the first resume
   — the exact failure the plan names. There is then no observation that
   distinguishes a wait that correctly stayed pending from one that never could
   have.
2. **The environment has no clock seam either, and it is half the claim.** Three
   days exercise tmpfiles reaping the state directory, log rotation, a real
   reboot, and a stale writer-lease file whose recorded PID has since been
   REUSED — a genuine single-loop-owner hazard nothing in the budget model
   measures and a 45-second gap cannot produce.
3. **Option 3 is self-defeating on the same probe run kimi cites.** It reserves
   real time for "the absolute-deadline leg". `PRODUCT_CONSTRUCTION_SITES=0`
   means there is no product-reachable absolute-deadline leg to reserve it for.
   Option 3 therefore reduces to "accelerate everything, hold nothing real",
   with every span assertion trivially satisfiable. Kimi engaged the charging
   line and never the reachability line.

**Against gemini.** Its three premises are correct and its conclusion does not
follow: it conflates the POLICY question (which legs must use real time) with
the REACHABILITY question (which legs can run). Termination state 2 —
"complete with named open requirements", the shape Phase 20A closed in — exists
for precisely a blocked platform. Escalating would also leave the two
**reachable** platforms unexercised at a calendar saving of zero. And the plan
defines deadlock as no option drawing three **and** no minority carrying
stronger evidence; the second conjunct fails, so this is a split, and escalate
is not available for a split.

**The objection to the authorized option, stated rather than hidden.**
`real-time-full`'s text says "all three platforms" and macOS cannot run — the
controlling instruction forbids Cargo on the Mac and `scripts/f23-macos-binary.sh`
was never landed. So it is not literally executable either. The commitment is to
`real-time-full` as a **rule about how a leg must run** — no leg may be
accelerated and every span claim must be a real span — not as a prediction that
every leg will run. macOS records OPEN with its blocker named.

---

## The evidence cost accepted with this decision

- **Three real calendar days minimum**, from `2026-07-27T14:03:59Z` (Linux day
  one) to no earlier than **`2026-07-30T14:03:59Z`**. Phase 23B cannot close
  before then.
- **Two hosts occupied for that span** — `hetzner-dsm` and `SeanDesktop`. macOS
  is not occupied because its leg cannot start.
- **A defect found on day three costs another full three-day cycle**, because a
  span shorter than the authorized threshold fails the driver's own span gate
  rather than being re-described.
- **In exchange**: no platform's span assertion is trivially satisfiable, no leg
  is published as equivalent evidence when it is weaker, and the wait condition
  is falsifiable on every resume.

## What this policy binds

`scripts/f23-multi-day-journey.sh` and its PowerShell port read the three
`*_required_real_span_seconds=` lines above and refuse any journey whose span,
**recomputed from the run log's own first and last timestamps**, is shorter.
Proved able to fail: with the threshold set to `999999` the verify step exits
`72` emitting `F23_04_SPAN_MEETS_AUTHORIZED_POLICY=false`; with this file absent
it exits `70`; with an empty run log it exits `71`; with a mismatched `--sha` it
exits `68`.
