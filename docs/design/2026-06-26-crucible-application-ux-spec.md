# Crucible — Application & UX Spec (invocation, model selection, cost governance)

**Status:** READY for implementation plan (revised 2026-06-26 after Usability + DevOps/implementation council review).
**Date:** 2026-06-26
**Branch:** `feat/crucible-mop-slice1`
**Supersedes the forward-design framing in:** `docs/design/2026-06-25-crucible-cost-awareness-and-surfacing-plan.md`
**Review trail:** spec-review councils returned REVISE on the first draft (4 CRITICALs); this revision absorbs them. Raw findings: workflow `w8k34rmd8`.

---

## 0. What this spec is (and is not)

The **engine is built** — the deterministic Assembler, cross-provider council fan-out, decoupled judge, stakes gate, judge-inclusive pre-flight estimator (`estimate_preflight_microcents` / `certified_microcents`), tail-latency cut, and a fire-and-forget CLI subcommand all exist and are gate-green on `feat/crucible-mop-slice1`.

This spec covers the **application layer**: how a user *invokes* a Crucible on demand, how *model selection* is presented, how *cost is made surprise-proof*, and — critically — the **runtime cost-governance wiring that does not yet exist** and is the entire differentiator vs OpenRouter Fusion. The council review established that "hard cost governance" is currently aspirational: `council/run.rs` enforces only a single council's per-run pin and never charges the per-session/day envelope, and the cap gate uses a judge-*excluding* estimator. Closing that is **Stage 1** and gates everything else.

**Goal:** A user spins up a cross-provider council from Wayland — by slash command, natural language, or a suggestion — expresses as much or as little as they want, and always approves a single honest cost **ceiling** before anything spends; aggregate spend across a session/day is bounded.

---

## 1. Strategic frame (anchors every decision)

Crucible is **Mixture-of-Providers (MoP)** — distinct from the Mixture-of-Agents *within one family* that Claude Code and Codex already do (five Claudes with different roles).

- **Error decorrelation is the mechanism.** A council beats a single frontier model only because its members have *uncorrelated* blind spots, so cross-checking cancels errors instead of reinforcing them. Errors decorrelate across **vendor families**, not within one. Cross-family diversity is therefore load-bearing for "cheap models reach frontier quality" ("Fable-level without the price" — Fugu's positioning) and is the one thing same-family MoA structurally cannot deliver.
- **Inference runs on Flux.** Wayland owns Flux Router; Crucible is a demand engine for our own gateway. One Flux key exposes ~50 models / ~12 vendor families, so the **default** user gets cross-vendor diversity with no multi-key management and we earn the inference margin. BYO direct keys (incl. OpenRouter) is the power-user/sovereignty **opt-out**.
- **Agent-level + local compute beats router-level.** Orchestration, judge, and fusion run locally — transparent, deterministic, no router orchestration-margin — while inference points at Flux. OpenRouter Fusion and Sakana Fugu run this on their compute, their margin, their black box, with no access to local context.

**Competitors:** OpenRouter Fusion (2026-06-13: parallel panel + temp-0 judge, structured JSON, ~4–5× single-call cost, **no budget cap**, their rail) and Sakana Fugu (owns a fixed roster). Crucible's edge: MoP **at the agent level, on Flux, with hard cost governance, in the user's own environment.**

---

## 2. Invocation model

### 2.1 The specificity gradient

The user expresses intent at any level of detail; the system extracts **`{tier, roster, budget, focus}`** and fills the rest with defaults. Task text is everything not consumed by those fields.

| Example | tier | roster | budget | focus |
|---|---|---|---|---|
| "set up a crucible to analyze this decision, use DeepSeek, GLM, Opus and ChatGPT 5.5, max $5" | — | **explicit** | **$5** | — |
| "run this idea through a cost-effective crucible with a C-suite focus to help me decide" | **cost-eff** | auto | default | **C-suite** |
| "run this through a crucible" | default | auto | default | — |

- **tier** — `cost-effective` (default) or `premium` (explicit signal only).
- **roster** — explicit list, or `auto` (deterministic Assembler picks for diversity + competence under budget).
- **budget** — explicit per-run cap, or the configured default.
- **focus** — optional persona/lens applied as a prompt framing layer to proposers + judge.

### 2.2 Three surfaces, one driver

1. **Slash command `/crucible <task>`** — deterministic, zero false-trigger, interactive-only. The unambiguous front door.
2. **Natural-language `CrucibleTool`** — the model invokes on *intent to convene* ("cross-check this with a panel", "have a council review"), **never** on the noun "crucible" appearing in task content. Misfire is harmless: invocation produces a pure $0 proposal that always echoes for confirmation before spend.
3. **Gate suggestion** — on a detected hard/high-stakes prompt (reuses the built `classify_task`, `gate.rs:180`), the assistant *offers* to convene with a real assemble()-derived figure (never a heuristic dollar amount). Never auto-spends. Ships in v1 (Stage 4) as the only discovery surface for users who don't know the word; includes a dismiss-and-suppress affordance.

**Implementation reality (council finding):** none of these are "registry adds." The current `crucible.rs` is a batch subcommand that prints text, emits no `ApprovalRequired`, and has no `Tool` impl. All three surfaces sit on top of **one shared `assemble → emit ApprovalRequired → await resume → run → stream` driver** (Stage 3); the slash command and NL tool are thin front-ends that construct the identical `CrucibleTool` invocation (tested byte-identical).

---

## 3. The proposal card — the no-surprises contract

`assemble()` is **pure** (no IO/clock/RNG), so a plan costs nothing to produce. The card is the universal gate: **nothing spends until it is cleared.**

### 3.1 Typed payload (not prose)

The card is carried by a **typed `CruciblePlan`** (serde, in `wcore-types`/`wcore-protocol`), NOT regex'd out of `context` strings — otherwise the dollar numbers drift between TUI and Electron. It is a typed optional field on `ApprovalRequired` (with `context` retained for human text) and also rides `ToolRequest.tool.args`:

```
CruciblePlan {
  roster: Vec<CouncilMember{ name, vendor, role: Proposer|Judge, tier, est_usd }>,
  ceiling_usd,                 // the certified judge-inclusive number (Stage 1)
  single_model_baseline_usd,   // free third estimator call over a 1-member roster
  day_spent_usd, day_cap_usd,  // rendered ONLY when the envelope genuinely aggregates (Stage 1)
  focus, degraded: Option<{k,n}>, judge_independent: bool,
}
```

The decision rides back as a **typed `CrucibleDecision { Approve | ApprovePremium{ceiling_usd} | Edit{roster,budget} | Cancel }`** through `ApprovalOutcome.modifications` — so `[p]` and `[e]` are not crammed onto the binary approve/deny rail.

### 3.2 Card (TUI)

```
CRUCIBLE — analyze this decision                         focus: C-suite
  ● DeepSeek V4        cheap · contrarian lens · different maker
  ● GLM-5.2            cheap · different maker
  ● Opus 4.8           premium anchor
  ⚖ ChatGPT 5.5        judge — different maker than every member
  why these: three different makers cross-check each other to catch errors one alone would miss.
  You're approving up to $2.10.   One model alone ≈ $0.45.
  today: $6.20 / $20.00
  [Enter] run (cap $2.10)   [e] edit budget   [p] premium (~$4.50)   [Esc] cancel — no charge
```

### 3.3 Hard rules

- **One honest number: the certified ceiling.** No cosmetic "est." The displayed "up to $X" **equals** the number the cap enforces (`certified_microcents()`, judge-inclusive). If it cannot be certified (any unpriced member), the council **aborts** as `UnpriceableRoster` — it never silently passes a $0-soft estimate.
- **Lead with the number being approved**, bold, with the cap on the action (`[Enter] run (cap $2.10)`).
- **Always show the single-model baseline** so the user sees what diversity costs.
- **Day-spend line renders only when real** — omit it until Stage 1's envelope genuinely aggregates and persists cross-process; a hardcoded/zero number is worse than nothing.
- **Judge visibly independent** — marked, vendor ≠ every proposer's vendor (enforced on the *final* plan, post-downshift).
- **Provenance reflects reality** — a degraded run states "K of N makers contributed" from numeric fields, never a claimed full panel.
- **`[Esc]` is consumed by the card** — single Esc resolves `approved=false` and tears the card down *before* the generic Esc ladder runs (so it never kills the parent turn).

---

## 4. Adaptive confirmation

Two things get confirmed, with different weight: **interpretation** (did I understand you — matters most when little was said and much inferred) and **spend** (approve the cost — scales with the number).

**Rule: confirmation weight scales with `(how much was inferred) × (how much it costs)`.**

| Situation | Confirmation |
|---|---|
| Named roster + budget, cheap | Light — card, one keypress. Don't re-ask. |
| Intent only, cheap | One-line echo of auto-choices, confirm. |
| Intent only, **expensive** | Full itemized echo: names the premium models, the ~$X, "that's premium. Confirm?" |
| Bare "run this through a crucible" | Always echoes — everything was inferred. |

This subsumes the "express lane" (§9 Fork 3): trivially-cheap + fully-specified is the low-friction corner of the same scale. **One confirm artifact per surface** — in the TUI the card *is* the echo (the `CrucibleTool` emits its own `ApprovalRequired`; `engine_bridge` `with_dedupe` suppresses the generic "Allow CrucibleTool?" card — tested as exactly one approval per invocation). On cardless surfaces the prose echo *is* the confirm. The echo doubles as the false-trigger guard: a misfired NL invocation stops at $0.

---

## 5. Model selection, base of operations, empty-state

- **Auto (default):** the user does not pick from a blank menu — the Assembler proposes; the user confirms or tweaks. "Lead with a recommendation," applied to the product.
- **Manual (power path, Stage 6):** a net-new `CouncilPicker` surface (NOT a fork of the single-select `model_picker.rs`) with multi-select + a `family()`-based judge-exclusion validator sharing the runtime predicate; outputs a roster into the same driver, inheriting card + governance for free.
- **Base of operations:** the registry resolves each logical model to the cheapest available route across the user's keys (route-dedup). **Flux key (default):** one key satisfies cross-vendor diversity and is monetized. **BYO direct keys (opt-out):** OpenAI/Anthropic/Google/OpenRouter/etc. widen the base; OpenRouter is a user-brought source, never our default rail.
- **Empty-state — self-bootstrap (LOCKED).** On-demand `/crucible` and the NL path are **decoupled from `[crucible].enabled`** (that kill-switch gates only the implicit per-turn auto-council). When `proposers` is empty **and** a Flux key resolves, invocation forces `assembly=Auto` and feeds the Flux candidate pool to `assemble()`, so a default-config user gets a quorate cross-vendor roster on first try. Tested against literal `CrucibleConfig::default`.
- **Catalog is not a maintenance burden:** Flux's catalog is ours; `models.dev` is a neutral enricher; a BYO OpenRouter key is per-user reference only. We maintain only route-dedup + `family()`.

---

## 6. Cost governance (Stage 1 — the moat, currently unwired)

- **Wire the envelope into `council/run.rs`.** Thread `Arc<Mutex<BudgetTracker>>` via `AgentSpawner` (like `provider_resolver`); pre-check `min(per_run, remaining_daily)` against the certified ceiling **before** the spawn loop; `charge_for_user` per proposer + judge completion so aggregate spend across multiple councils is bounded. Add `per_session_usd`/`per_user_daily_usd` to `BudgetConfig` + extend `From<&BudgetConfig> for BudgetCap` (today it never maps `per_user_daily`).
- **Certified ceiling, not the worst-case-minus-judge estimator.** Replace the `run.rs` cap gate's `estimate_worst_case_microcents` (judge-excluding, `$0`-soft, input-undercounting) with `estimate_preflight_microcents(...).certified_microcents()`; `None` → abort `UnpriceableRoster`. Fix `spend.rs` `in_worst` to scale with `max_turns × max_tokens` so agentic tool-loops are bounded, and add live per-turn charge enforcement so a tool-looping proposer is hard-stopped mid-run, not only pre-checked.
- **Default-ON caps at the Crucible layer.** $2/run, $20/day, configurable, applied at the Crucible layer (NOT global `BudgetConfig` defaults — that would hit unrelated call sites). Named budget overrides per-run for that call.
- **Auto-downshift to fit, with a diversity floor.** Exceeding a cap downshifts to fit and labels it; a single bounded pass that holds **≥2 proposer families + an independent judge**, and **refuses-and-surfaces** rather than silently collapsing diversity. Overage is a typed `ApprovePremium{ceiling_usd}` outcome ([p]), shown as the certified number.
- **`$0` price-parser fix (both sites).** `wcore-pricing/refresh.rs:215-224`: replace `.parse().ok().unwrap_or(0.0)` at *both* prompt and completion with `Option<f64>` (None on missing/unparseable/≤0) and `continue` (matching the existing `None => continue`), so an unpriced model becomes *unpriced*, never *free* — the selector never seats it as cheapest and `certified_microcents` refuses to certify.
- **Per-credential concurrency.** `HashMap<CredId, Arc<Semaphore>>` keyed on resolved route (Flux key vs each BYO key), default ~4 permits, configurable — stops the unbounded fan-out from thundering-herding the single Flux key. **No auto-retry** on errored proposers (retries multiply spend under a fixed ceiling); a 429 is a degraded member feeding backoff.
- **Judge independence** enforced on the *final* assembled plan (post-downshift), with `family()` canonicalizing an `openrouter:` route to its upstream vendor first (else an OpenRouter-routed Claude judge vs OpenRouter-routed GPT proposer both read as "openrouter").

---

## 7. Headless / TTL / fail-closed (write-it-down, Stage 3)

- **No-approver = fail-closed, never hang.** Detect no interactive approver up front (capability discovery) and resolve immediately: either downshift to a single sub-cap Direct call, or return a typed `no_approver` error. **Never** reach `Suspend` (which silently hangs voice/CI), never wait out the reaper, never auto-spend.
- **Long/no-expire approval TTL.** Route the proposal card through `ApprovalBridge::with_ttl()` with a long/no-expire TTL so a careful user reading a 5-vendor card past the default 300s still executes on `Enter`. On reaper-cancel, the TUI tears the card down and shows "proposal expired — re-run `/crucible`" (distinguished from user-cancel).
- **The only auto-spend bypass** is `crucible_auto_spend` (config, default **OFF**), which *still* enforces per-run + per-session/day caps and *still* emits a $0 echo so a false trigger spends nothing.

---

## 8. Verified engine-fix scorecard (folded into Stage 1)

Source-verified 2026-06-26: a prior adversarial council over-claimed; corrected list — **confirmed:** the `$0` parser (both sites); the OpenRouter-route `family()` gap; the runtime governance gaps in §6. **Already handled (do not re-fix):** the council quorum (`run.rs:234` → `InsufficientProposals`; not a silent one-model fusion — the real work is K-of-N *provenance*, §9); `family()` flux-vs-direct dedup (tested). 

---

## 9. Cross-surface rendering (Stages 4–6)

- **Single source of truth:** TUI card, Electron card, and prose/voice echo all render from the one `CruciblePlan`. Cross-surface golden test: one fixture → identical ceiling/baseline/day-spend/judge-vendor strings on all surfaces.
- **TUI card:** add a `Crucible => CrucibleComponent` arm in `tui/permission/dispatch.rs` (else it renders as the flat fallback); render the §3.2 card as `Vec<Line>` inside the one inline-card surface.
- **Live council view:** do **not** unfreeze `SubAgentView`. Add a sibling `CouncilMemberView { name, vendor, role, cost_usd, status, feed }` + `CouncilView`, mirroring the `WorkflowView`/`WorkflowNodeView` precedent, fed via the opaque `SubAgentEvent.inner`; render role+vendor from the same `CruciblePlan` as the card, with a sticky judge (⚖) row and per-vendor running cost in a `CrucibleRunSurface` keyed by run id.
- **Cancel-mid-council:** `[Esc] stop council` reuses the turn-cancel path, aborts remaining proposers + judge; partial charges already decremented the envelope (honest accounting); post-run shows "cancelled — K of N ran, $X spent, judge skipped." Approval-timeout stragglers are distinguished from model-error stragglers in the errored-`Proposal` reason enum.
- **Discovery:** a `/crucible` line in `onboarding.rs` (zero mention today) + the command-palette/help description; the gate-suggestion ships in Stage 4, not deferred.
- **Voice/cardless:** a deterministic spoken-echo template generated from the same `CruciblePlan`, mapping spoken run/edit/premium/cancel to the same outcomes as the TUI keys; premium is taught in the echo ("say 'premium' for top-tier, ~$4.50") so it is discoverable without a hotkey.
- **Cross-process daily-spend (Stage 6):** persist `per_user_daily_usd` to a shared on-disk store keyed user+UTC-date that both the TUI and Electron read/write — the envelope is the single accumulator. If shared persistence slips, hide the "today" line on the desktop rather than show a per-process resetting number.

---

## 10. Build sequencing (corrected — governance first)

| # | Stage | Effort/Risk | Depends on |
|---|---|---|---|
| 1 | **Engine + governance foundation** — `$0` parser (both sites → None); `family()` OpenRouter canonicalization; swap cap gate to certified judge-inclusive estimator + fix `in_worst`; wire `BudgetTracker` into `run.rs` (charge per member, pre-check `min(per_run, daily)`); `BudgetConfig` fields + `From` mapping; judge-independence + downshift diversity floor on the final plan | L / high | none |
| 2 | **Typed `CruciblePlan` + approval rail** — serde struct on `ApprovalRequired`; `CrucibleDecision` outcome through `modifications`; baseline via a 1-member estimator call | M / med | 1 (certified ceiling must exist) |
| 3 | **Shared driver + headless fail-closed + TTL** — one `assemble→approve→run` fn behind `/crucible` and `CrucibleTool::execute`; round-trip resolves before the spawn loop; long/no-expire TTL; no-approver fail-closed; per-credential semaphore; `crucible_auto_spend` OFF | L / high | 2 |
| 4 | **TUI card + empty-state self-bootstrap + discovery** — `CrucibleComponent`; self-bootstrap on one Flux key; onboarding + palette entry; gate-suggestion pulled forward | L / med | 3 |
| 5 | **Live council view + provenance** — sibling `CouncilMemberView`/`CouncilView`; sticky judge + per-vendor cost; K/N fields; cancel accounting | L / med | 4 |
| 6 | **Multi-select picker + cross-process daily-spend + voice/premium discoverability + NL trigger discipline** | L / med | 4 |

Each stage is independently testable and ships working software. **No user-facing spending surface ships before Stage 1.**

---

## 11. Success criteria / verification

- Bare "run this through a crucible" with one Flux key (default config, `enabled=false`) self-bootstraps a quorate cross-vendor roster, echoes the plan, and waits for approval before any spend.
- **Aggregate bound:** a second council in a session is blocked once the per-session/day envelope is exhausted (not just the per-run pin).
- **Ceiling honesty:** the displayed "up to $X" equals `certified_microcents()`; an unpriceable roster aborts rather than running; a tool-looping proposer is hard-stopped at the ceiling mid-run.
- **`$0` price:** a null/`-1`/missing-priced catalog row yields no `$0` `ModelPrice` (parser test) AND is never chosen as cheapest (selector test).
- **Judge independence** holds on the final plan, including an OpenRouter-routed roster.
- A misfired NL invocation costs **$0**; with no approver and `crucible_auto_spend=false`, total spend is exactly **$0**.
- A degraded panel reports "K of N makers contributed" from numeric fields.
- A card left open past 5 minutes still executes correctly on `Enter`.
- `[Esc]` on the card resolves `approved=false`, tears the card down, and leaves the parent turn alive (render-assertion test through the real router).
- Exactly one `ApprovalRequired` per invocation on both bridges; `/crucible` and the NL tool produce a byte-identical `CruciblePlan`.

---

## 12. Non-goals

- Redesigning the engine / Assembler (built).
- Re-litigating build-vs-wrap (the engine exists; inference is ours via Flux).
- The quality-per-dollar eval vs frontier — a separate effort that *proves* the value this UX *delivers*; referenced, not specified here.

---

## Appendix — locked decisions

- **Fork 1:** cost-effective default; premium on explicit signal (named frontier model / "premium" / `[p]`); the stakes gate may *suggest* premium for high-stakes but never auto-spends it; premium discoverable on cardless/voice surfaces.
- **Fork 2:** default-ON $2/run, $20/day caps at the Crucible layer; named budget overrides; `[p]` overage is a typed `ApprovePremium` outcome at the certified number; downshift holds a ≥2-family + independent-judge floor.
- **Fork 3:** express lane is a real branch, default **OFF** (`crucible_auto_spend`), deferred to Stage 6, dependent on Stages 1+3; when enabled it still enforces all caps and still emits a $0 echo on a false trigger.
- **Empty-state:** self-bootstrap (one Flux key makes `/crucible` work; decoupled from the `enabled` kill-switch).
- **Card number:** drop cosmetic "est"; show only the certified judge-inclusive ceiling, led with "you're approving up to $X."
