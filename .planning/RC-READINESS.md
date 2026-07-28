# Release-candidate readiness — measured 2026-07-28, integration `8897f04b`

Tracks the **six release-blocking items** `CRITERIA-GAP-LEDGER.md` §3 identified this morning
(the ledger counts seven criteria because item 3 is two). Ranked there by
**customer promise × reachability × failure mode**, and that ranking is preserved here.

| # | Blocker | Status at `8897f04b` |
|---|---|---|
| 1 | `24-C2` — three advertised trigger kinds can never fire | **BLOCKING ELEMENT CLOSED** |
| 2 | `27-C2(a)` — browser remediation string sends users into an impossible loop | **CLOSED** |
| 3 | `24-C5` + `24-C1` — macOS/Windows setup-to-recovery journeys | **IN FLIGHT** |
| 4 | `23A-C1` — `--skills-promote` advertised and always fails | **BLOCKING ELEMENT CLOSED** |
| 5 | `27-C2(b)` — capability flags lie to Desktop | **BLOCKING ELEMENT CLOSED**, Desktop train owed |
| 6 | `24-C3` — inbound channel matrix never driven end to end | **NOT STARTED** |

**Four of six addressed in one day. Two remain: item 3 in flight, item 6 not started.**

## Read the "blocking element closed" rows precisely

Three rows above are closed **as release blockers** while their criteria stay **open**. That
distinction is the whole point of the ledger and must not be collapsed in either direction.

- **`24-C2`** — `event` is genuinely implemented on a durable queue. `webhook` and `poll` now
  **refuse at add** and are gone from `--help`; persisted jobs list as `WILL NEVER FIRE`. What
  made this the worst item in the ledger was **silent acceptance**, and silent acceptance is
  gone. The criterion remains PARTIAL because webhook and poll were not built, and the repair
  lane explicitly declined to record its work as closing it.
- **`23A-C1`** — the panel's split was accepted: the blocking element is the *advertisement*,
  not the implementation. `--skills-promote` is hidden and still exits 1 with a governed-promotion
  message. Real governed promotion (3–4 sessions) is **not** in this RC, and was measured, not
  assumed: `ProcedureStatus` has no `Revoked` variant, no generation store exists to roll back
  to, and no artifact-provenance binding exists.
- **`27-C2(b)`** — capability advertisement now narrows on a **probe that can fail** rather than
  on linkage, live-proved A/B on one binary. The full probe-based readiness contract still needs
  a **Desktop co-release**, per the panel's mechanism argument.

## Item 6 is sequenced behind item 3, deliberately

`24-C3` needs a hermetic fixture endpoint, and **that same endpoint closes `24-C1`'s remaining
tally** — the two share a prerequisite. Dispatching them concurrently would have two lanes
building one fixture. `24-C3` follows once the journey lane lands its fixture.

## What is NOT on this list, and must not be quietly added to it

The **11 can-ship-open criteria** (≈20–30 lane-sessions). Treating all of them as blocking is
precisely what turned Phase 20 into a 74-plan loop lasting two weeks. They are real work, they
are tracked, and they are not gating this RC.

## Still Sean's, and nothing is waiting on either

- **A release trust root + a signed manifest asset in `release.yml`.** Until both exist,
  `self-update` **installs nothing** — deliberate fail-closed behaviour, not a defect to route
  around. Commands in `SR-29-9`. This does not block cutting an RC; it blocks the RC being able
  to update itself.
- **Tag and publish**, and the **Desktop digest re-pin** which must ride the same train (see
  `CLASS-CONTRACT-01`; `observation.rs:329` makes a mismatch a hard error at `ready`).

## Defects found today that would have shipped in this RC

Recorded because they are the argument for having done this work rather than cutting on the
plan count.

1. **Remote command injection in the ssh exec backend**, root execution on the far end. Never
   shipped — no tag contains `d0fc5095`, instrument verified against 36 tags.
   `SECURITY-NOTE-SSH-INJECTION.md`.
2. **Data loss on interrupted migration** — a truncating write on the live quarantine index left
   331 payload directories orphaned and 0 profiles imported, reproducibly, on 5 of 35 kills.
3. **The cloud backend was broken, not merely unexercised** — two of its three defects would
   have produced a *false green* rather than a failure.
4. **`--trigger poll:` fired unconditionally without ever contacting its URL** — worse than the
   silence it was reported as.

Every one was found by driving the real product against real hardware. None was visible from
source review, and two were actively masked by gates that passed.
