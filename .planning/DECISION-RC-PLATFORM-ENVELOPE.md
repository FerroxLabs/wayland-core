# Decision — the release candidate's supported-platform envelope

**Date:** 2026-07-28 · **Decided by:** orchestrator, under Sean's standing delegation
("I'll take your recommendations... do what you need to do") · **Status:** DECIDED, not parked.

## The question

`.github/workflows/release.yml:60-80` ships **six targets across three OS families** — Linux
(x86_64, aarch64), macOS (x86_64, aarch64), Windows (x86_64 MSVC, aarch64 MSVC). But `24-C5`
(setup-to-recovery journey) has evidence on **Linux only**; macOS and Windows have none.

So either the envelope narrows to match the evidence, or the evidence rises to match the
envelope. Absent a decision, `release.yml`'s existing promise makes `24-C5` block the RC by
default — which is a decision by inaction, and the worst of the three.

## Decision

**Keep all three OS families. Raise the evidence.** Close `24-C5` on macOS and Windows
(≈5–7 lane-sessions, shares a fixture prerequisite with `24-C1`'s remaining tally, so the two
are done together).

## Why, in order of weight

1. **Narrowing would ship to the wrong audience.** `AGENTS.md` §11 records the standing
   architecture position: *Wayland Desktop is the primary control plane over the bundled Core
   engine.* Desktop is an Electron app; its users are overwhelmingly on macOS and Windows. A
   Linux-only Core RC would be evidence-complete and commercially close to useless.
2. **The platforms are already certified at the layer that was actually hard.** Phase 28 ran
   **147/147 critical cells with 0 skipped**, the Windows soak passed **1000/1000**
   (`F28_SOAK_EXIT=0`), and macOS proved 8/8. Setup-to-recovery on those platforms is
   **incremental evidence over certified ground**, not new territory. That materially lowers
   the 5–7 session estimate's risk.
3. **Narrowing is a visible retraction.** Six targets are already promised in a committed
   workflow. Withdrawing two OS families is a louder, more expensive signal to a customer than
   taking a few sessions to finish the proof.
4. **The cost is affordable against the total.** 5–7 sessions inside a 13–17 session run to RC,
   with lanes running in parallel and hetzner at ~2.0 load and 751G free. This is not the long
   pole.

## What was rejected, and why

- **Narrow to Linux-only.** Rejected on reason 1 — it optimises the burndown at the expense of
  the customer.
- **Ship three families with an "unverified recovery on macOS/Windows" caveat.** Rejected as
  the worst option available: it is precisely the *silent-false-advertising* class this program
  spent today closing (`--trigger` accepting jobs that never fire, the `[browser]` hint that
  can never work). Shipping a recovery path we have not exercised, behind a footnote, is the
  same defect wearing a disclaimer. If we ship the platform, we prove the recovery.

## What this does NOT decide

Tagging, publishing and the release trust root remain Sean's. This decides only the **envelope
the RC is proved against**. `F29-03-01` still stands: `self-update` installs nothing until a
real trust root and a signed manifest asset exist, and that is deliberate fail-closed
behaviour, not a bug to route around.

## Falsifier

If a measurement shows Desktop does **not** target macOS/Windows, or that the customer base is
Linux-first, reason 1 collapses and this decision should be revisited immediately. It rests on
a documented architecture position, not on telemetry.
