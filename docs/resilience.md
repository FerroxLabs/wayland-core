# Endurance & Resilience Trial

*An ongoing experiment in long-horizon, unattended autonomous operation.*

## What this is

We run a **pinned build of the agent doing gated, autonomous maintenance on a fork of its
own source code** — unattended, on a single idle server, with faults injected on purpose.
The point isn't "an AI that completes tasks." Task-completion demos are easy and prove little.
The point is **operational endurance and integrity**: can an autonomous agent run for a long
time, on its own codebase, surviving crashes and faults, without ever drifting, corrupting its
state, or silently failing — and can every claim about that be independently verified?

This is framed as a **duration ladder**: smoke test → hours → a full day → a week → and,
ultimately, a month. We build only what each rung requires, and we let *measured* behavior set
the reachable duration rather than asserting it up front. The numbers below are from one
completed rung (a continuous 12-hour run); the experiment is ongoing toward the longer horizons.

## What we've measured so far (a representative 12-hour run)

A single continuous, unattended 12-hour run produced:

- **322** maintenance iterations attempted; **229** changes accepted and committed (~**71%**),
  each one passing a real **compile + lint gate** before it was allowed into history.
- **Survived an injected `SIGKILL` mid-run.** The process was killed without warning while it
  was mid-build; it restarted automatically and resumed — with **zero duplicate commits, zero
  lost commits, and a clean working tree**.
- **No degradation over the window.** Acceptance settled into a stable equilibrium (~68–71%)
  rather than collapsing; cache-hit rate, disk usage, and memory stayed flat — no leak, no
  drift, no slow rot.
- **Low, independently-measured cost** (single-digit USD for the 12 hours), measured from the
  model provider's *raw usage records*, not from any layer's self-reported number.

Separately, a dedicated fault-injection suite kills the process inside every sensitive window of
the commit path (before commit, after commit/before merge, after merge): **80/80 windows
recovered with zero duplicate commits.**

## How it works (the mechanisms)

The properties above come from a few deliberate design choices, each of which is what makes the
corresponding claim *checkable* rather than asserted:

- **Git is the single source of truth.** State is reconstructed from commit history on every
  restart, not from a mutable side-file that a crash could leave inconsistent. This is what makes
  "no duplicate or lost commits across a crash" a structural guarantee, not a hope.
- **Every change must pass a real gate.** A change is only committed if a genuine build + lint
  pass against the actual source. Failed attempts are discarded, not papered over. (Full test-
  suite gating is being hardened as a separate, stricter rung — see *What we don't claim*.)
- **Disposable per-iteration sandboxes.** Each iteration works in an isolated checkout with a
  hermetic environment, so a bad change can't corrupt the canonical repository — the worst case
  is a discarded attempt.
- **Provenance on every accepted change.** Each committed change carries a unique iteration
  identifier. Integrity is a one-line check anyone can run: no duplicate identifiers, clean tree,
  commit count equal to accepted count.
- **Chaos is injected, not awaited.** We schedule process kills *during* runs and verify
  automatic recovery, rather than passively reporting uptime.
- **Cost is measured out-of-band.** Spend is computed from the provider's raw usage stream,
  upstream of the agent — so the agent cannot influence or game its own reported cost.

## How we keep ourselves honest (and what we don't claim)

We'd rather under-claim than get caught over-claiming. Explicitly:

- **This is not "recursive self-improvement" and the model is not getting smarter.** It's a
  fixed, pinned build performing maintenance. Nothing here learns or rewrites itself.
- **Uptime is not the metric.** "It stayed up for N hours" is a vanity number; a process can be
  alive and doing nothing. We count **gate-passing committed changes**, which is why liveness and
  progress are reported separately.
- **The work surface is finite.** A codebase has a bounded amount of legitimate maintenance
  work. We don't pretend it's endless — we measure the depletion curve and let it inform how long
  a run can stay productive.
- **One clean run is not a proof of a week or a month.** The longer-horizon claims are
  *goals*, not results. We label what is measured versus what is targeted, every time.
- **We don't trust any single layer's self-report** — not for cost, not for cache, not for
  success. Where a number matters, it's cross-checked against an independent source.

## Roadmap

Smoke ✔ → 1 hour ✔ → 12 hours ✔ → multi-node + heavier chaos (in progress) → a continuous week
→ a month. Each rung is gated on the previous one passing cleanly, with the same integrity and
honesty checks applied at every scale.
