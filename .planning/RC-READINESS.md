# Release-candidate readiness — measured 2026-07-28, integration `8897f04b`

Tracks the **six release-blocking items** `CRITERIA-GAP-LEDGER.md` §3 identified this morning
(the ledger counts seven criteria because item 3 is two). Ranked there by
**customer promise × reachability × failure mode**, and that ranking is preserved here.

| # | Blocker | Status at `8897f04b` |
|---|---|---|
| 1 | `24-C2` — three advertised trigger kinds can never fire | **BLOCKING ELEMENT CLOSED** |
| 2 | `27-C2(a)` — browser remediation string sends users into an impossible loop | **CLOSED** |
| 3 | `24-C5` + `24-C1` — macOS/Windows setup-to-recovery journeys | ✅ **MET on all three platforms** (`5ed01866`) |
| 4 | `23A-C1` — `--skills-promote` advertised and always fails | **BLOCKING ELEMENT CLOSED** |
| 5 | `27-C2(b)` — capability flags lie to Desktop | **BLOCKING ELEMENT CLOSED**, Desktop train owed |
| 6 | `24-C3` — inbound channel matrix never driven end to end | **IN FLIGHT** — unblocked, the shared fixture now exists |

**Five of six closed. One remains: `24-C3`, in flight.**

### Item 3 CLOSED 2026-07-28 — and the Windows fix is a cautionary tale

`24-C5` is **MET on all three platforms**. Linux 17/17, Windows 17/17, macOS 17/17, each with a
verifier-accepted receipt and recovery **observed** rather than asserted.

**The obvious Windows fix does not work, and a gate would have certified it.** `<RestartOnFailure>`
registers, reads back correctly through Task Scheduler's own `/query /xml`, and leaves the runtime
**still down 3m20s after `taskkill /F`** against a `PT1M` interval. Any gate asserting the element
is present would have signed off a service that stays dead. What actually recovers it is a
`<TimeTrigger>` with a one-minute `<Repetition>` and `MultipleInstancesPolicy=IgnoreNew` — measured
end to end: killed pid 46164, no manual start, platform-started pid 9376 thirty-six seconds later.

Two further measurements each of which would have broken real installs: `encoding="UTF-8"` is
**rejected** while UTF-16-declared UTF-8 bytes are accepted; and `%USERDOMAIN%\%USERNAME%` is
**rejected on a workgroup machine**, so emitting a `<Principals>` block would have broken install on
**every non-domain-joined desktop**.

Open and reported rather than smoothed: `bind` refuses the three-platform trio because macOS ran at
an ancestor differing only in `.planning`, and its CI artifact sat queued 45 minutes. **A provenance
gap, not a coverage gap** — the lane declined to describe an unbound trio as bound.

### Item 3, measured 2026-07-28 evening — the honest state

`24-04` really had never been started, so the harness did not exist. It does now: a 17-step
ordered journey identical on every platform, an **independent sink as its own OS process**, and
a verifier that hashes the binary itself and **derives** duplicates/losses rather than trusting
a reported figure.

- **Linux — PASS, receipted, twice.** 17 steps, 12 submitted, 12 arrived, 0 duplicates, 0
  losses. **Recovery observed, not asserted:** `kill -9`, no manual start, `NRestarts=1`, new
  live pid. Upgrade and rollback both performed and observed — the first time either `24-C1`
  clause has been exercised anywhere.
- **Windows — RED at step 12 of 17.** Two HIGHs found and fixed live; **`F24-J-H3` remains
  open and is a genuine product defect**: `schtasks /sc onlogon` sets **no restart-on-failure
  policy**, so after a crash the platform does **not** bring the runtime back — task `Ready`,
  `Last Result: 1`, 120s, nothing. systemd carries `Restart=` and launchd `KeepAlive`; the
  Windows path carries neither. This was `24-01`'s own carried-forward risk, now measured.
- **macOS — NOT RUN**, and *not* the false "unobtainable binary" premise: CI publishes the
  artifact on every push, but concurrency cancelled the run on each of four re-pins.

**Graded NOT MET on one of three platforms, deliberately not narrowed to the one that worked.**

## Item 7, added 2026-07-29 — the headless remedy, found by Phase 30's peer trial

**`HEADLESS-KEYRING` — BLOCKING ELEMENT CLOSED. HIGH, unanimous three-way panel.**

Found sideways: 30-02 could not run `wayland-core` against the peer harnesses because it refuses to
start without an OS keyring, where **neither competitor needs an equivalent**.

The error text names a remedy. **The remedy is wrong in three independent ways at once**, each
measured live on a keyring-free host. `credentials` is not a section — it is `[storage.credentials]`
— so writing it literally makes the product log *"ignoring unknown or mis-sectioned config key"*
and re-emit the **identical** error. **That is the closed loop verbatim**, the same shape as
`27-C2(a)`. At the correct section the advertised value is rejected outright, so the config no
longer loads **at all** and following the advice is **strictly worse than ignoring it**. The
underscored spelling fails too — the variant is a struct and can never be a bare string. The
passphrase half names no mechanism: **0** hits under `docs/`, **0** in 13,921 bytes of `--help`,
**0** in any error message, and the one string that names it is `WAYLAND_HOME`-gated so it does not
print on a default install — suppressed exactly where it is needed.

**Why this was missed twice.** The symptom was graded LOW on two prior occasions, and `24-C3`
scoped it to "an isolated profile". The lane **tested that scoping rather than arguing with it**: a
plain default install with `WAYLAND_HOME` unset reproduces identically, and `session.enabled`
defaults `true` — so **the default config of a default install is the failing one on any container,
CI runner or minimal VM**. The earlier gradings were scoped too narrowly. Not CRITICAL: fails
closed, no data loss.

Every other lane was unaffected because hetzner runs a live secrets daemon. **The gate is
conditional and they were never in the condition** — which is exactly how it survived this long.

**Closed as a blocker** because both refusal strings now name what was measured to work, gated by
tests that re-parse the advertised values through the real parser. **Still open:** the `docs/` and
`--help` gap, and the fact that the default still needs one line two peers do not.

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
