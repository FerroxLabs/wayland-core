# Anvil — Native Gated-Forge Engine — Design Spec v1

**Status:** Draft for build (designed by Overwatch/Fable, 2026-07-12, Sean-directed) · **Lane:** core
**Sibling of:** `2026-06-25-crucible-mixture-of-providers-design.md` (Crucible MoP) — same ForgeFlow
family, opposite trigger condition.

> Source of truth for the mechanism: `~/dev/anvil/v2/` (engine.py 348 + judge.py 129 + agents.py 137
> lines — the benchmarked engine: 12-task / 165-check, solves every task at a fraction of frontier
> cost) and `~/dev/anvil/ANVIL-PRODUCT.md` (Sean's product doctrine, 2026-07-11). The flux-router
> `src/elevation/` port is the server-side sibling; this spec is the **native core flagship** of the
> same engine.

## 1. One law, two engines, zero user decisions

Ratchet's own architecture line is the routing rule:

> **Flux** routes. **Crucible** fuses — for work with **no checkable reward**. **Anvil** forges — for
> work that **has one**. **Ratchet** ships.

Core gets ONE elevation detector and TWO engines behind it. The user never picks an engine:

- Task has (or can derive) a **checkable gate** → **Anvil**: build → score → iterate → verify.
- Task is judgment/taste/blind-spot work with **no gate** → **Crucible council** (already native).
- Neither warranted (simple task, probe passes) → single cheap call, done. No ceremony.

Sean's canonical loop (2026-07-10): **build it → score it → iterate it → verify it** — a governed loop
that exits ONLY on verified success or an honest "blocked because X". This is the structural answer to
the reliability cluster (#746 watchdog, #774 recovery, #665 stop-lying-about-done): verification is not
a feature bolted on, it is the exit condition.

## 2. UX doctrine (Krug × Sutherland)

- **Don't make me think (Krug):** no new mode, no toggle, no configuration step. The detector engages
  Anvil when the work warrants it; the user sees a status verb while it runs ("Forging — check 3/14")
  and a receipt when it lands. Explicit invocation exists (`/forge`, like `/council`) but is the power
  path, not the door.
- **Show the win (Sutherland):** every Anvil result carries a **receipt chip**:
  `✓ Verified — 14/14 checks · 3 iterations · $0.07 (≈$0.61 saved vs frontier est.)`
  The win is visible, quantified, and credited to the product. Never buried in logs.
- **Honest escalation:** when the pool can't crack specific checks:
  `⚠ Needs escalation — 2/14 checks uncracked: <named checks>` with the concrete options (escalate
  those checks to frontier · show attempts · accept partial). A red receipt is a result; a silent
  plain answer is forbidden (A5 no-silent-degrade, same invariant as the flux port).
- **User-facing name is "Verified", never "Anvil".** Internal flow id `anvil`; chips, receipts, and
  docs say "verified" (explains the win; matches the `flux-verified` alias and stamp semantics).

## 3. Architecture — where it lives in core

New ForgeFlow topology, sibling of `council`:

```
crates/wcore-agent/src/orchestration/anvil/
  detector.rs    # engagement + anvil-vs-crucible routing (one detector, shared with council)
  gates.rs       # gate hierarchy: real / derived / self-generated; immutability fence
  climb.rs       # the governed loop: probe → ensemble → surgical climb → escalate → verify
  receipt.rs     # cost roll-up + receipt event (win/escalation/blocked)
```

- `GraphConfig::gated_forge` + `anvil` ForgeFlow (parse-validated, kill-switched — mirror council's
  config surface).
- **Reuses Crucible Slice-1 infrastructure wholesale:** the keyed-provider resolver (provider id →
  Config → `Arc<dyn LlmProvider>`, cached per run, BYO-key aware, absent key ⇒ skip), per-node
  provider+model threading, cost roll-up, JSON-stream provenance events. Anvil adds the loop, the
  gate machinery, and the receipt — not a second provider stack.
- Runs INSIDE the existing engine/runner (Sean's Anvil disposition: "layered on the existing
  workflow/engine, not a parallel runtime").

## 4. The detector (auto-engagement — the "smart" part)

Inputs, cheap and local (no model call to decide):
1. **Task signature** — does the turn produce a checkable artifact? (code in a repo with
   tests/build/lint; structured output with a schema; a claim set that cross-checks; a file edit with
   a compiler.)
2. **Situation** — a prior verify failed on this task; the watchdog recovered a stalled turn (#746);
   a "done" claim is about to be emitted with failing signals (#665). Failure re-engages HARDER
   (bigger pool, higher budget), not silently weaker.
3. **Stakes** — writes to files / long artifacts get verification bias; throwaway chat answers don't.
4. **Explicit** — `/forge [criteria…]` forces Anvil; `/council` forces Crucible (existing).

Decision table: gate exists or derivable → Anvil · no gate derivable but multi-perspective helps →
Crucible · else → plain single call. Config: `anvil.enabled` (kill switch, default per §8 rollout),
`anvil.auto = off | suggest | on`, spend/iteration caps. In a **trusted workspace / Cowork posture**
(#671, blessed 2026-07-12) `auto=on`; in plain Chat posture the default is `suggest` — a one-keystroke
chip ("can verify this — ⏎"), never a blocking dialog.

## 5. Gate hierarchy (test criteria: use → derive → generate → route away)

1. **Real gates** (highest trust): the repo's own tests / build / lint / typecheck; JSON-schema
   validation; a Ratchet-adopted repo's land-gate command. Run in the existing exec sandbox under the
   existing permission model — the trust axis (#671) already governs exec, so Anvil inherits, never
   bypasses.
2. **Derived gates — "propose the test criteria":** when the task is checkable in principle but no
   gate is provided, Anvil PROPOSES acceptance criteria before building: a short natural-language
   checklist compiled to executable asserts where possible (doctest-style snippets, schema fragments,
   grep-able invariants). Surfaced in one line — "Verifying against: builds clean · handles empty
   input · public API unchanged (edit)" — and proceeds without blocking. Criteria are visible,
   editable, and stamped into the receipt.
3. **Self-generated gates** (fallback): self-consistency (N-sample agreement), cross-model check,
   format/constraint conformance, held-out re-derivation. Weaker; receipt says so ("self-checked",
   not "verified") — the stamp vocabulary stays honest.
4. **No gate derivable** → not Anvil's job → detector routes to **Crucible**. This is the law from §1
   applied mechanically.

**Gate immutability fence (anti-gaming):** the gate is pinned at climb start. The builder lane may
NOT modify test files, schemas, or criteria mid-climb — any diff touching the gate aborts the
candidate (score = worst). Anvil climbs the gate; it never lowers it. (Ratchet's own rule, enforced
in `gates.rs`.)

## 6. The climb (faithful to v2 engine.py — the proven mechanism)

1. **Probe:** best cheap builder (merit-ordered pool — per-model prior from evidence, so the
   demonstrably-best builder goes first) does a full build. Score against the gate. **Pass → done.**
   A simple task costs one cheap call and still gets the receipt.
2. **Ensemble:** on failure, parallel full builds from a small DIVERSE cheap pool; keep best by
   (score, failing-set).
3. **Surgical climb:** per failing check, a minimal targeted fix (one check at a time), re-scored,
   accepted **only on strict improvement** (`better()`: score up, or equal score with fewer fails) —
   the monotone ratchet property. Plateau detection stops wasted spend.
4. **Escalate narrow:** frontier model ONLY on the specific checks the cheap pool cannot crack —
   surgical prompts, never a full frontier rebuild. This is where the cost win lives (benchmark: the
   9.7M→1.7M token gap).
5. **Exit:** verified (all checks green) · **escalation receipt** (named uncracked checks + options) ·
   honest-blocked ("blocked because X"). No fourth exit.

## 7. Provider awareness (Flux Auto native)

Resolution order in `detector.rs`/`climb.rs` via the shared keyed-provider resolver:
1. **Flux configured** → the pool is pinned cheap-model ids from the **Flux catalog** (known
   availability, real `cost_usd` per call returned by Flux — receipts use REAL dollars, no estimate);
   Flux Auto is the single-call path for probe-passes. Core knows exactly what's available because
   Flux tells it.
2. **BYO providers** → cheap tier per configured provider, absent key ⇒ skip (council semantics);
   costs estimated from published rates and FLAGGED as estimates in the receipt.
3. **Single provider only** → self-iteration mode: same loop, one model, gate still gates. The gate
   is most of the value even without ensemble diversity.

## 8. Safety rails & rollout

Rails (all config-validated at parse, mirror council): kill switch (`enabled=false` in Slice A1),
`max_iterations`, `max_calls`, per-check timeout, wallclock budget, spend ceiling with honest stop,
strict-improvement acceptance only, gate immutability fence, proposers read-only except the designated
build lane, gate output treated as untrusted data (injection-fenced), receipts stamped only by the
engine (client/agent cannot forge "verified" — same invariant class as the flux forgery seam #123).

Rollout slices (post profiles-cut, per Sean 2026-07-12):
- **A1 — engine:** `anvil` ForgeFlow + real gates + explicit `/forge` + receipt events. Kill-switched,
  nextest + golden climb transcripts. (~the v2 engine, in Rust, on council's plumbing.)
- **A2 — don't-make-me-think:** detector auto-engagement + derived-criteria proposals + desktop
  receipt chip (pairs with #671 Cowork posture; `auto=on` in trusted workspaces).
- **A3 — full doctrine:** Flux-catalog pool + real-cost receipts, self-generated gates, Crucible
  cross-routing, merit-prior persistence (evidence-driven pool ordering), #172 promptless-intent
  absorption (per its DEFER-into-Anvil ruling).

## 9. Tests (gate for the gate-builder)

Unit: `better()` monotonicity, detector routing table (anvil/crucible/plain), gate-hierarchy
selection, immutability fence (candidate touching gate = rejected), receipt honesty (no "verified"
stamp without all-green). Golden: recorded climb transcripts replayed deterministically. Adversarial:
gate-gaming attempts, injection via gate output, forged receipt attempts, budget-exhaustion honesty.
Cross-provider: pool diversity, absent-key skip, Flux/BYO/solo modes. E2E: proving-ground harness
(#53 re-cut) drives real climbs against real repo gates. Perf: probe-pass path adds <1 model call of
overhead vs plain turn.

## 10. Explicitly out of scope

New inference endpoints or compute (Flux IS the endpoint) · gate registry/community platform · local
runner fleets · per-verified-PR pricing · any user-facing "Anvil" branding · rebuilding council
plumbing Anvil can reuse. (The DROPPED list from ANVIL-PRODUCT.md is binding.)
