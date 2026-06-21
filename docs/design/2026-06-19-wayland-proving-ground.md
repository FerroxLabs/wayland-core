# Wayland Proving Ground — Design Spec (v2, post cross-audit)

> **Status:** Design, revised after a 3-auditor cross-audit (architecture · oracle-integrity · solo-founder pragmatics). · **Date:** 2026-06-19
> **Failure mode this kills:** discovering your own product's bugs from GitHub issues after users hit them.

## Goal

A standing, automated user-simulation QA system that drives the **real** Wayland Core
binary (and later the desktop app) like dozens of different users, across configs,
terminals, and platforms — catching lived-flow, usability, and config bugs that unit
tests miss, **gating releases on deterministic invariants**, and getting smarter over
time without becoming a second full-time job.

## What the cross-audit changed (read this first)

The first draft had four serious flaws. This version fixes them:

1. **The gate would have missed the bugs it exists to catch.** A fixtures-only gate
   never exercises real provider logic, and `sk-flux-` was *pre-network detection
   logic* — no provider response involved. **Fix:** pre-network decision logic
   (detection, egress, alias/auth branching) becomes **network-free invariants** on the
   PR gate, driven by the product's own declared registries. This catches the
   `sk-flux-` class *before it is known*, deterministically, at $0.
2. **"Slice-first" was applied to the domain, not the machine.** P1 is now genuinely
   minimal: one driver behavior, a flat record, ~6 invariants, ~15 cells, $0, days.
3. **Standing AI automation is an abandonment trap.** Standing/CI automation emits
   **only deterministic invariant pass/fail**. The LLM judge and AI explorers are
   **on-demand tools**, never a nightly queue. Auto-fix-PR is **cut entirely**.
4. **The unified `Driver`/`Frame` was false across TUI and DOM.** `Frame` is now an
   enum; only the persona/intent library and the judge are genuinely cross-driver.

## Scope decision (author override of the audit)

The pragmatics auditor recommended deferring the app and Windows. The author chose to
**keep app + Windows in the architecture** — so the `Driver` trait is defined now and
P3/P4 are committed phases. This spec honors that **while** keeping the abstraction
honest (enum `Frame`, per-variant oracles) and recording the audit's stall-risk: P3
(Playwright↔embedded-terminal) and P4 (Windows ConPTY) are flagged research seams to
prototype before their breadth is committed, with **headless + json-stream** as the
locked Windows fallback if ConPTY TUI capture proves unreliable in CI.

## Non-Goals

- Replacing the existing unit/integration suite — the Proving Ground sits above it.
- Load/throughput benchmarking; security pen-testing (separate tracks).
- An autonomous bug-fixer. The system generates failing tests + locator hints, never
  speculative fix PRs.

---

## Architecture

### 1. Driver trait + honest `Frame` enum

```
trait Driver {
    fn start(session: &Session, term: TermShape) -> Self;
    fn send(&mut self, input: Input);
    fn snapshot(&self) -> Frame;
    fn stop(self) -> ExitRecord;
}

enum Frame {
    TextGrid(Vt100Snapshot),         // PTY / ConPTY: cells + style attrs
    Dom { ax_tree: AxTree, shot: PngBytes, embedded: Option<Vt100Snapshot> },
}
```

- **PtyDriver** (unix) — promote the existing `smoke_p0.rs` PTY harness. Concrete
  behavior lands first; it implements the trait.
- **ConPtyDriver** (Windows, P4) — same trait; **interactive-TUI capture is best-effort
  and NOT a release gate** (ConPTY headless capture is documented-unreliable in this
  repo). Windows gating runs on headless + json-stream + Windows invariants.
- **PlaywrightDriver** (app, P3) — produces `Frame::Dom`. The app embeds an xterm.js
  terminal, hence the `embedded` field.

**Honest reuse boundary:** invariants over `Frame::TextGrid` are TUI-only; most do not
auto-apply to `Frame::Dom`. The genuinely cross-driver layer is the **persona/intent
library** and the **LLM judge** (which reads a screenshot/AX-tree, not a typed grid).
The spec does not claim oracle reuse across drivers.

### 2. RunRecord — split into asserted core + observability sidecar

- **Deterministic core (asserted, replay-stable):** final normalized frame, the *set*
  of outgoing HTTP requests, filesystem delta, exit code, stderr, `.dirty-death`
  sentinel, the resulting `config.toml`/credential-store state (redacted).
- **Observability sidecar (captured, never asserted byte-equal):** ordered intermediate
  frames, per-frame + input→first-paint timing. Replay-confirm compares **invariant
  verdicts**, never the raw record (timing is non-deterministic by construction —
  the existing harness already polls-with-deadline for exactly this reason).
- Secrets are **redacted at capture**, before persistence.

### 3. Hermeticity model — Session, not per-run

The headline onboarding bug is a *relaunch* bug, so isolation is per **Session**, not
per run:

- A **Session** owns one temp `WAYLAND_HOME`. A run *within* a session may relaunch the
  binary against the *same* home — that is how connect→quit→relaunch persistence is
  tested. Sessions are isolated from each other.
- `harden_child_env` (existing) strips the developer's real provider keys/config.
- **Non-`WAYLAND_HOME` globals are enumerated and individually neutralized** (the
  "absolute hermeticity" claim is downgraded to this concrete list):
  - **Forge discovery file** — resolves via `dirs::config_dir()` *by design*
    (`forge_discovery.rs:81`), NOT `WAYLAND_HOME`. **Prerequisite task:** add a
    `WAYLAND_FORGE_DISCOVERY_PATH` env override so the session can redirect it; until
    then the Forge config cell is **excluded from P1** (it is P2 work anyway).
  - **Forge loopback port 3456** — a fixed singleton; parallel Forge sims would
    collide. Make the port configurable per session before any Forge cell runs.
  - **OS keychain** (macOS login keychain) — the keyring credential cell must force a
    session-local plaintext store unless explicitly testing the keychain path serially.

### 4. The input space

- **Config matrix:** no keys · env-keys-only · pasted-key · OAuth · Ollama ·
  multi-provider · each `ProviderType` · bad/expired key · `sk-flux-` key · fresh ·
  existing · partial · corrupt config · keyring-vs-plaintext · egress on/off ·
  `windows_shell`. (Forge cell deferred per §3.)
- **Terminal matrix:** `rows×cols` incl. a deliberately **short height** (forces the
  `/doctor` overflow), `TERM`, color support. **Interaction cells are forced, not
  sampled** — `/doctor × short-height` is a mandatory cell, not a lucky draw.
- **Persona × Intent library** (§4a).

#### 4a. Personas & Intents — partitioned by execution layer

Personas: first-timer · power user · **env-keys dev (the stuck case)** · pasted-key ·
Windows · returning user (expects persistence) · confused user (garbage input) ·
**impatient user who interrupts** (Ctrl-C, resize mid-op).

Intents are tagged by which layer they can run in:
- **Deterministic-gate intents** (fixed trajectory): connect a provider · paste a key
  for detection · scroll/read `/doctor` · edit config via `/config` · switch model ·
  resume a session · run a *specific* tool · recover from a bad key.
- **Generative/on-demand intents** (need a live model, non-deterministic, judged):
  **build a small project** (multi-turn write+edit+bash) · open-ended exploration.
  These are **never** deterministic-gate cells (a fixture can't produce a correct build
  without hand-authoring the whole trajectory, which tests nothing).

### 5. Provider fidelity — three distinct mechanisms

The first draft's fatal conflation (fixtures "cover" detection) is split apart:

1. **Network-free decision oracles (PR gate, $0).** Drive the engine's *pre-socket*
   logic directly from its **own declared registries**: every prefix in the detection
   table, every provider alias, every egress rule. Assert positively (each declared
   prefix → exactly one provider) **and negatively** (garbage / look-alike → zero
   candidates with a non-silent error). A supported-but-undetected prefix is
   automatically a finding — this is what catches `sk-flux-` *before* it's known.
2. **Response fixtures (PR gate, stable surface only).** Recorded real responses for
   *wire* behavior — model lists + happy-path connect handshake in P1. Volatile shapes
   (errors, auth challenges, streams) stay in live-smoke, not the gate.
3. **Nightly live-smoke (off the gate, $-capped).** Hits real APIs to catch drift.
   **Drift is a RED, human-classified finding — never auto-regenerates a fixture**
   (auto-regen would bake the regression in as the expected answer). A
   **fixture-freshness invariant** fails any fixture whose last live confirmation is
   stale or diverged.

### 6. Oracle suite

1. **Invariants (deterministic, CI gate).** Each is falsifiable with a concrete check
   over the RunRecord core. The set (expanded from audit gaps):
   - no panic / no `.dirty-death` / exit code intended-only
   - **config persists**: connect → real process relaunch (same session home) → lands
     on Workspace, `config.toml` reflects the provider *(catches onboarding bug)*
   - **onboarding atomicity/reversibility**: Ctrl-C mid-onboarding leaves config
     *unchanged*, never half-written *(deepest form of the onboarding bug)*
   - **content reachable** within the *canonical* reveal set (arrows, PgUp/PgDn, wheel,
     `/`-search); reachable only via an obscure binding is itself a finding *(catches
     `/doctor` scroll)*
   - **paste-prefix → exactly one candidate** (network-free, positive + negative)
     *(catches `sk-flux-`)*
   - loopback/localhost egress never blocked
   - **build provenance**: the binary embeds its source hash; assert it equals the repo
     HEAD under test *(catches the Forge stale-build class)*
   - **MCP lifecycle**: advertised → discovered → granted → connected → tool-callable;
     a stale discovery file is detected as stale
   - **no secret in ANY RunRecord channel** (frame, config, fs delta, request log,
     stderr) except the redacted credential slot
   - **error legibility**: a bad/expired key yields a specific, actionable error — never
     a silent hang or generic failure
   - **responsiveness** (thresholded, **box-only — not a CI gate**, to avoid runner
     flake): input→first-paint p95 < 150ms even with a background op in flight; any op
     > 500ms emits ≥1 progress frame
   - idempotency: connecting/onboarding twice never duplicates or corrupts state
   - **`/command` → correct surface** (not merely *a* valid surface), and the surface
     isn't empty/errored
   - a model claimed available actually completes a turn *(the MiniMax/Moonshot 401
     class)*
2. **Functional outcome oracles** (for fixed-trajectory tool intents only): file
   written with correct bytes, edit applied, bash exit/stdout matched, **abort
   atomicity** (an interrupted write leaves the file fully-old or fully-new, never
   partial).
3. **LLM judge — ON-DEMAND ONLY.** Reads a **screenshot/AX-tree** (vision-capable, not
   text frames) + the persona's intent, flags broken/confusing/dead-end/undiscoverable.
   Never in the gate, never a nightly queue. Calibrated against the author's own
   "this baffled me" episodes; judge↔human agreement is reported, not assumed.

### 7. Execution modes

- **Deterministic regression matrix** — fixed-input cells vs decision-oracles +
  response-fixtures, invariants + functional oracles assert, **gates every PR**, $0.
- **Generative explorers — on-demand, box-only, $-capped.** LLM-as-user drives a live
  driver; findings replay-confirmed before reporting. **Intermittent findings are
  reported with a `confirmed_rate`, not discarded** (a race that reproduces 2/10 is a
  real bug, not noise). Judge classifications require **independent** confirmation
  (different model or a deterministic oracle), never the same judge re-run.

### 8. Fleet runner

Parallel hermetic *sessions* (a natural Workflow fan-out), each port-isolated and
global-neutralized per §3. Aggregates deduped, severity-ranked findings, each carrying
its replayable record. **Not in P1** (P1 runs serially).

### 9. Triage flywheel (no auto-fix)

Confirmed finding → **auto-generate a failing deterministic regression cell + a
one-line locator hint** (surface + most-likely module). The cell joins the gate
permanently; coverage compounds. **No auto-PR** — generating a correct engine fix is
neither safe nor faster-to-review than a one-line locator.

### 10. Coverage map — honest, not "comprehensive"

Enumerates commands/surfaces/config-keys/providers/tools. A cell is reported as
**"covered under {conditions}, untested under {conditions}"** — never a bare "covered,"
because every known bug is an *interaction* bug (works at 120×40, breaks at 80×24). The
sampling strategy and its blind spots (e.g. "pairwise: 2-way covered, 3-way not") are
named inline. The word "comprehensive" is **forbidden** over a sampled matrix.
**Not in P1.** Auto-discovery of new surfaces, if built later, is a passive column —
never a gate-red — so it never couples feature velocity to harness coverage.

---

## Build target — the overnight sweep (≈24–48h to first run, then sharpen nightly)

One push, not a phased crawl. The spine (deterministic, trustworthy) and the sweep
(generative, finds unknowns) land **together** so it can run overnight and surface bugs
nobody scripted from night one.

1. **Promote** the `Pty`, `MockLlm`, `harden_child_env`, `STRIPPED_PROVIDER_ENV` assets
   out of `smoke_p0.rs`/`harness_tui_flow.rs` into shared test-support, re-pointing the
   existing tests at it and keeping them green (non-lossy; inherits the `vt100` pin +
   input-delivery-race constraints). Define the `Driver` trait + `Frame` enum;
   `PtyDriver` is the first impl. `RunRecord` per §2; Session hermeticity per §3.
2. **Invariant spine** — the deterministic oracles (§6.1). These are the trustworthy
   pass/fail and the morning's "works ok" signal. They catch all four known bugs by
   construction.
3. **Generative explorer (the sweep)** — AI-as-user drives the real `PtyDriver` across
   personas × intents × configs × terminal sizes, the invariants check every step, and
   every candidate finding is **replay-confirmed** before it's reported. This is what
   finds the bugs you didn't think to script.
4. **Overnight usability judge** — a vision/AX pass over the explorer's records, flagging
   confusing/dead-end/undiscoverable flows; classifications **independently confirmed**
   (different model or a deterministic oracle), reported with a confidence, never raw.
5. **`just proving-ground --overnight`** — runs the explorer fleet unattended on the box
   and emits **one triaged, deduped, severity-ranked report** with repros: either "spine
   green, no new findings" or the N findings each with a replayable record + locator
   hint.
6. **Replay-determinism test** so the spine's verdicts (and therefore the report) are
   trustworthy.

**Quality guards (non-negotiable — they are what make the overnight report trustworthy
instead of a wall of noise):** deterministic invariants as the spine; replay-confirm +
confidence gating on every generative finding; a hard **$-cap + kill-switch** on the
explorer/judge spend; secret redaction at capture; Session hermeticity. The deterministic
spine + the MockLlm-driven cells are $0; the generative explorer's overnight spend is
budget-capped and runs on the box.

## Breadth after the first overnight run
The sweep machine exists after the build above; everything else is *more cells, more
invariants, more drivers* feeding the same machine — not new infrastructure:
- **Engine breadth** — more invariants/cells; the response-fixture wire tier; the
  flywheel cell-generator; the blind-fault-injection recall harness (criterion 4);
  the Forge env-override + port-config prerequisites; tools → MCP → skills.
- **Desktop app** — `PlaywrightDriver` (`Frame::Dom`); prototype the embedded-terminal
  seam; reuse personas + judge.
- **Windows** — headless + json-stream gate + Windows invariants; ConPTY TUI capture
  best-effort, not gated.
- **Nightly live-smoke** — real-provider drift, $-capped with a kill-switch.

## Error handling & resilience

- Driver `send`/`snapshot` are deadline-bounded; a hung binary becomes an
  "unresponsive" finding, never a hung run.
- Generative runs are token-budget-bounded with a **hard dollar cap + kill-switch**;
  P1 is provably $0. Nightly live-smoke is per-provider-call-capped.
- Hermeticity failure (a run touching real state) is itself a test failure.

## Testing the tester

Keep the **replay-determinism test** in P1 (it makes findings trustworthy). Golden
known-good/known-broken records and a deliberately-broken fixture binary land with the
gate (P1.5), not before there's a gate.

## Success criteria

The north star: **`just proving-ground --overnight` runs unattended on the box and, by
morning, produces a trustworthy verdict — "spine green, nothing new" or a triaged bug
list with repros.**

1. **It runs overnight unattended** and emits one deduped, severity-ranked report with a
   replayable record per finding.
2. **It catches the four known bugs** (each a failing→passing cell across the fix).
3. **It surfaces ≥1 real bug nobody scripted**, replay-confirmed — proving the sweep
   finds *unknowns*, not just re-confirms knowns.
4. **The verdict is trustworthy:** the deterministic spine is binary green/red (and goes
   red on a seeded regression); generative findings carry a `confirmed_rate` + repro;
   the overnight run stays under its $-cap.
5. **Detection-power, measured:** blind fault-injection — seed K defects in the
   connect/config/scroll/detection paths *not* targeted by any cell, and **report recall
   as a number.** The honest measure of how much it actually catches.

## Risks & open questions

- **ConPTY fidelity (P4)** — likely unreliable in CI; the plan *is* the headless
  fallback, not the contingency. Interactive-TUI capture is never a gate.
- **Playwright ↔ embedded-terminal (P3)** — prototype the dual DOM+xterm read before
  committing P3 breadth.
- **Judge calibration** — an uncalibrated judge verdict is an unverified claim; measure
  judge↔human agreement before trusting "usable."
- **Fixture rot** — the volatile provider surface stays in live-smoke; only stable
  shapes are fixtured, and drift is human-classified.
- **Matrix explosion** — sampled for breadth, but interaction cells (`/doctor ×
  short-height`, etc.) are forced; coverage is reported honestly, never "comprehensive."
