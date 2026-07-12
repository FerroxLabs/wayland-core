# Anvil — Native Gated-Forge Engine — Design Spec v2

**Status:** v2, post tri-model adversarial cross-audit (Fable code-grounded + GPT-5.6-Sol + Grok-4.5,
2026-07-12, 95 findings reconciled) · **Lane:** core · **Supersedes:** v1 (same day)
**Sibling of:** `2026-06-25-crucible-mixture-of-providers-design.md` (Crucible MoP) — same engine
family, opposite trigger condition.

> Source of truth for the mechanism: `~/dev/anvil/v2/` (614-line benchmarked engine: merit-ordered
> cheap pool, gate scoring, surgical per-check climb, strict-improvement acceptance, narrow frontier
> escalation; 12-task/165-check benchmark) and `~/dev/anvil/ANVIL-PRODUCT.md` (product doctrine).
> flux-router `src/elevation/` is the server-side sibling; this spec is the native core flagship.
>
> **v2 corrections are normative.** The audit refuted three v1 premises: (1) the council "ForgeFlow"
> rail is vestigial — production council is a bespoke driver (`drive_council`, run.rs), so Anvil
> builds on the DRIVER rail, not GraphConfig; (2) spawned children have no approval channel
> (spawner.rs:634-639), so A1 is **trusted/auto-approve posture only**; (3) no receipt/provenance
> protocol event exists (council provenance is CLI stderr) — the receipt event is **net-new protocol
> work**, specified here.

## 1. One law, two engines, zero user decisions

> **Flux** routes. **Crucible** fuses — for work with **no checkable reward**. **Anvil** forges — for
> work that **has one**. **Ratchet** ships.

- Task has a **real executable gate** (or user-confirmed derived criteria) → **Anvil**.
- Judgment/taste/blind-spot work, no gate → **Crucible council**.
- Simple task, probe passes → plain cheap call. No ceremony.
- **Hybrid tasks are decomposed**: Anvil stamps ONLY the checkable slice; the judgment slice routes
  to Crucible or stays plain. A partial gate never stamps the whole turn (audit: consensus HIGH).
- **One elevation decision per task-id**: Anvil and Crucible are mutually exclusive per task with a
  **shared elevation spend envelope** — the two systems can never stack spend on one goal.

Sean's canonical loop: **build → score → iterate → verify**, exiting on verified success or an honest
"blocked because X" — plus the explicit terminal states in §6.5. Answer to the reliability cluster
(#746, #774, #665).

## 2. UX doctrine (Krug × Sutherland) — with the honesty vocabulary fixed

- **Don't make me think:** no new mode. Detector auto-engages (per §4 rollout: shadow → suggest →
  on); `/forge` is the explicit verb. **Precedence lattice (normative):** kill-switch > budget/spend
  policy > workspace trust posture > `/forge` > detector. `/forge` can never override disabled,
  untrusted, or budget-exhausted states.
- **Show the win — receipt chip:** `Forged — verified · 14/14 checks · 3 iterations · $0.07`.
  - **Stamp vocabulary is tiered and brand-separated** (audit: HIGH, all three): `verified` ONLY for
    real executable gates or user-confirmed derived criteria · `criteria-checked (proposed)` for
    unconfirmed derived gates · `self-checked` / `format-validated` / `consensus-only` for
    self-generated gates — visually distinct (not green-parity), no savings line.
  - **Savings line** appears ONLY when actual and counterfactual are metered by the same methodology
    (Flux real `cost_usd` both sides); otherwise omitted or `est.`-labeled with methodology version.
    Unpriced spend renders as **"unpriced"**, never $0 (blocks on the `priced` flag already in
    spend.rs; authoritative Flux cost = wayland#319, a named A3 dependency).
  - Receipt lists **coverage scope** (which checks ran / didn't) — a green suite that doesn't cover
    the change-set says so ("suite-passed; 2 files outside exercised tests").
- **Honest escalation:** `Needs escalation — 2/14 uncracked: <check ids>` with options (escalate to
  frontier · show attempts · accept partial). "Accept partial" produces a **structured incomplete
  result that blocks done-claims** (#665) listing residual fails.
- **Chip requirements (A2):** i18n via structured message ids (ICU plurals/currency), a11y (text+icon
  dual-encoding, ARIA live region, keyboard-accessible actions), suggest-chip bound to an explicit
  control (never bare Enter), debounced until idle, with persistent dismissal.

## 3. Architecture — the DRIVER rail, not ForgeFlow (v2 correction)

Production council is a bespoke driver (`wcore-cli/src/crucible.rs` → `drive_council` → `run_council`);
`GraphConfig::mixture_of_providers` is test-only scaffolding. **Anvil A1 is a driver-style module,
`drive_climb`, beside `drive_council`** — reusing the plumbing that actually exists:

- `CouncilProviderResolver` (resolver.rs — keyed provider→`Arc<dyn LlmProvider>`, BYO-key skip,
  `family()` diversity) — REAL, reused.
- Per-spawn provider/model pins (spawner.rs Crucible T2/T4) — REAL, reused.
- `CouncilSpend` roll-up (spend.rs, incl. the `priced` flag) — REAL, reused (extended per §7).
- **Net-new (named A1 work units, not assumed):** the climb loop; gate machinery (§5); per-candidate
  **git-worktree isolation** (parallel builders NEVER share a checkout; gate runs are serialized if
  isolation is unavailable); the **climb journal** (§6.5); the **receipt protocol event** (§8);
  atomic **budget ledger** (§7).

```
crates/wcore-agent/src/orchestration/anvil/
  detector.rs   # engagement + anvil/crucible routing (versioned feature matrix; shadow-mode first)
  gates.rs      # gate closure pinning + hierarchy + flake quarantine
  climb.rs      # drive_climb: probe → ensemble → surgical → escalate; journal; terminal states
  ledger.rs     # atomic budget reservation/settlement (tokens + exec wallclock)
  receipt.rs    # signed receipt event construction (engine-only emission)
```

ForgeFlow/GraphConfig migration is explicitly OUT of scope until the rail carries council in
production (tracked separately; do not anchor estimates to it).

## 4. The detector — shadow-first, telemetry-fed (v2 correction)

The v1 detector inputs (verify-failed history, watchdog recovery, done-claim signals) **do not exist
in core today** — the shipped classifier is a keyword list (gate.rs `classify_task`). Therefore:

- **A2 prerequisite — verification telemetry:** per-task gate-outcome protocol event + persistent
  failure-history store. Without it there is no smart detector, only keywords.
- **Detector ships in SHADOW MODE first** (decision logged, never acted on — precedent:
  spawner.rs:626-633 shadow workflow-detection). Thresholds calibrated from shadow data + the
  telemetry of §9 before `auto=suggest`, and cohort-staged before `auto=on` (Cowork/trusted only).
- **Deterministic, versioned feature matrix** with precedence rules, confidence thresholds, and an
  abstention state (abstain = plain path + shadow log). Inputs: task signature, change scope/stakes,
  gate cost probe (§5), budget state, prior outcomes (once telemetry exists). Engagement re-evaluates
  on material state change (first file write, first test discovery) — not only at turn open.
- **Engagement is value-based, not artifact-based**: expected verification value (risk × change scope
  × failure impact) against gate cost. A typo fix with an 8-minute suite fails the value test →
  suggest, not auto. Gate selection is **change-scoped** (affected tests) with a cost/time probe.
- **Failure re-engagement is bounded**: cumulative per-task-lineage budget (§7); re-engagement never
  auto-enlarges pool/budget past the ledger; known-flaky baseline failures are quarantined, not
  treated as "engage harder" signals.

## 5. Gates — closure pinning, tiered trust, flake quarantine (v2 correction)

**Tier 1 — real gates** (only tier that can stamp `verified` without user confirmation): repo tests /
build / lint / typecheck / schema / Ratchet land-gate.
- **Gate closure pinning** (replaces v1's path fence — audit consensus HIGH): pin and hash the FULL
  invocation closure — command + argv + env allowlist + config files + fixtures/helpers/goldens/
  conftest/package scripts (transitive inputs). ANY closure drift during the climb aborts the
  candidate. Candidates run against an **isolated trusted snapshot** of the gate, not the mutable
  worktree copy.
- **Clean-baseline requirement:** before pinning, record baseline results + provenance of
  pre-existing modifications; a dirty/weakened suite is surfaced, not silently pinned.
- **Gates are untrusted code** (audit HIGH): they run in the exec sandbox with network-deny by
  default, minimized env (no ambient secrets), and — in A1 — only under **trusted/auto-approve
  posture** (children have no approval channel, spawner.rs:634-639; the child-approval path is the
  named prerequisite for any broader posture). **Pre-climb gate probe:** run the gate ONCE on the
  baseline before spawning any builder; if the gate itself cannot execute (permissions, missing
  toolchain), refuse the climb immediately — never burn ensemble budget discovering it.
- **Flake quarantine:** a check that flips on identical code is flaky → quarantined out of the gate;
  `verified` requires N-of-M stability on the final candidate; receipt says "N checks quarantined as
  flaky". Plateau detection is defined with confidence bounds (k non-improving accepted steps, min
  Δscore, repeated measurement on noisy gates), with mandatory escalate-narrow before a blocked exit.
- **Test-authoring carve-out:** tasks whose deliverable IS a gate change (add tests, fix a flaky
  test) split the gate: the evaluation copy = parent suite minus the files under edit; the authored
  gate changes are user-visible deliverables verified separately — never auto-worst-scored.

**Tier 2 — derived gates** ("propose the test criteria"): criteria authored by a **provider family
disjoint from the build pool** (resolver.rs `family()` machinery), with a requirement→gate coverage
matrix; uncovered requirements block the `verified` word. Criteria are shown, editable, and stamped
into the receipt. **Unconfirmed derived criteria cap at `criteria-checked (proposed)`**; user
confirmation (or Tier-1 strength) is required for `verified`. High-impact operations (migrations,
deletions) REQUIRE confirmation.

**Tier 3 — self-generated** (self-consistency, cross-model, format): correlated-consensus evidence,
not truth (audit HIGH). Stamped `self-checked`/`consensus-only`, visually quarantined, no savings
claim. Verifier isolation is specified: disjoint family, independent context framing, no shared
artifacts beyond the candidate itself.

**Tier 4 — none derivable** → route to Crucible.

**Injection fencing (concrete, not asserted):** models receive typed bounded fields only (check id,
pass/fail, sanitized first-N-bytes message) — raw gate stdout NEVER interpolates into prompts. UI
rendering strips control sequences, normalizes Unicode, bounds lengths (terminal-escape/spoof
defense).

## 6. The climb (v2: acceptance discipline + terminal states)

1. **Probe:** merit-ordered best cheap builder, full build, gate score. Pass (with §5 flake stability)
   → done.
2. **Ensemble:** parallel full builds, each in an **isolated worktree**; best by a **deterministic
   total order**: (score, |fails|, severity-weighted fail vector, fail-id hash, cost).
3. **Surgical climb:** per failing check, minimal fix. **Acceptance = fail-set discipline** (audit
   consensus HIGH): accept iff new fail-set ⊆ old fail-set, or Pareto improvement on the severity
   vector — a safety-class regression is never tradeable for two cosmetic passes. Rejected trade-ups
   are logged. **Coupled-failure clustering:** checks that co-flip are repaired jointly, and a
   bounded multi-step transaction (evaluated at checkpoints, beam width ≥2 from ensemble seeds) is
   allowed through prerequisite regressions (e.g. mid-migration compile breaks) — global best is
   always retained.
4. **Escalate narrow:** frontier only on uncracked checks, with bounded dependency context (not a
   bare assert), full-suite re-validation after every escalated fix, and residual ledger budget only
   (§7) — the user never pays twice unknowingly.
5. **Promotion is atomic:** the winning candidate merges into the user workspace once, with
   merge-time re-verification if the workspace moved (external-dirty → abort/rebaseline). A
   per-workspace climb lease prevents two climbs (or climb + user edits) interleaving.

**6.5 Terminal states (complete enum — audit HIGH):** `verified` · `criteria-checked` ·
`self-checked` · `needs-escalation` · `blocked(reason)` · `cancelled` · `timed-out` ·
`permission-denied` · `crashed(recovered)` · `superseded`. Each defines artifact + receipt
publication. **Cancellation is structured:** process-group termination, provider-call cancellation,
write barrier, red receipt (`stopped — k/n · $spent`). **Crash recovery:** an append-only **climb
journal** (gate closure digests, candidates, scores, spend, idempotency keys per provider call)
persists each iteration; resume re-verifies digests (gate/dependency/toolchain drift → restart
verification), replays nothing paid, and can always emit the honest terminal receipt. No silent
fourth exit.

## 7. Cost governance (v2: one ledger, atomic reservation)

- **Single per-task-lineage ledger** (`ledger.rs`): every provider call and gate execution reserves
  capacity ATOMICALLY before dispatch (no check-then-launch races across parallel builders); actuals
  reconcile after; exhausted reservation cancels undispatched work. The ledger covers **tokens AND
  exec wallclock** (test suites are a real cost v1 ignored), spans retries/re-engagement/escalation
  (residual budget carries into user-chosen escalation), and is shared with Crucible per §1.
- Unknown-cost providers: conservative configured upper bound or paid escalation disabled. Receipt
  cost accounting includes retries, losing ensemble candidates, and abandoned calls — the $0.07 is
  the whole climb, reserved-vs-settled shown.

## 8. Receipt integrity (v2: a real protocol event + trust boundary)

- **Net-new `ProtocolEvent::AnvilReceipt`** (wcore-protocol events.rs) — emitted ONLY from the
  engine's climb exit path, carrying: terminal state, check counts + coverage scope, iterations,
  settled cost (+priced flags), gate-closure digest, artifact digest, session/task ids, engine
  version, monotonic sequence.
- **Host trust rule (normative):** receipt chips render ONLY from this top-level engine variant.
  Receipt-shaped content arriving via `SubAgentEvent.inner`, `PluginEvent.payload`, or text deltas
  is inert (adversarial test required: sub-agent emits a forged receipt → no chip). Same invariant
  class as ratchet 00364cf (previewed fragment cannot forge the Approve/Reject verdict).
- **Staleness:** the receipt binds to the artifact digest; any post-receipt mutation of the verified
  files invalidates the chip (re-hash at publication; `superseded` on later relevant writes).
  Receipts are revocable — a false-green discovered later emits a counter-event (trust repair).
- Named failing checks are redacted for secrets/PII before display or persistence.

## 9. Telemetry, rollout, safety rails

- **Telemetry before auto-on** (audit HIGH): detector decisions + overrides (shadow mode), gate tier
  mix, flake rate, spend/overshoot, time-to-receipt, escaped-defect labels, receipt-state
  distribution, cancellation. Kill-switch dashboard. A/B holdout to prove verified-ness improves
  outcomes (defect rate, correction rate) — not just "more green chips".
- **Rollout:** per-component flags (detector / derived-gates / self-gates / chip), staged cohorts,
  rapid rollback, receipt schema versioning.
- Rails recap: kill switch (default off in A1) · precedence lattice (§2) · ledger (§7) · closure
  pinning + clean baseline + flake quarantine (§5) · worktree isolation + climb lease (§6) · journal
  + idempotency (§6.5) · engine-only signed receipts (§8) · trusted-posture-only exec in A1 (§5).

## 10. Slices (rescoped post-audit)

- **A1 — engine on the driver rail** (kill-switched, trusted/auto-approve posture only): `drive_climb`
  + gates.rs closure pinning + pre-climb probe + flake quarantine + fail-set acceptance + worktree
  isolation + ledger + journal + terminal states + `/forge` + `AnvilReceipt` event + host trust rule.
  Real (Tier-1) gates ONLY. Golden climb transcripts + the adversarial tests of §5/§8.
- **A2 — don't-make-me-think, honestly:** verification-telemetry work unit → detector in SHADOW →
  `auto=suggest`; Tier-2 derived gates (disjoint author, coverage matrix, confirm-for-verified);
  desktop chip (a11y, i18n, dismissal semantics); staged cohorts + dashboards.
- **A3 — full doctrine:** `auto=on` in Cowork after shadow calibration; Flux catalog pool + real
  cost_usd (**dependency: wayland#319**) + savings line; Tier-3 self-gates (quarantined vocabulary);
  Crucible cross-routing + shared elevation envelope; child-approval channel → non-trusted postures;
  merit-prior persistence (exploration floor, poisoning defense, domain-conditioned); #172
  absorption.

## 11. Out of scope

New inference endpoints/compute (Flux IS the endpoint) · gate registry/community platform · local
runner fleets · per-verified-PR pricing · user-facing "Anvil" branding · ForgeFlow rail migration
(until council itself rides it in production) · rebuilding council plumbing that exists on the
driver rail. (ANVIL-PRODUCT.md DROPPED list is binding.)
