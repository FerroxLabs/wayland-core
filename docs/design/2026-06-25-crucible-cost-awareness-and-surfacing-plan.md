# Crucible — Cost-Awareness + Surfacing (TUI / JSON-stream / Model-Mix) Plan

**Status:** brainstorm + plan for review (Sean, 2026-06-25). Council engine + `--auto`
gate are built, gated-green, pushed, and **live-validated** via Flux Router
(Claude Opus 4.8 + GPT-5 + Gemini 3.1 Pro fused by Opus). This plan covers the
four threads Sean raised after the live run.

> **UPDATE 2026-06-26 — AUTO-ASSEMBLY SHIPPED** (branch `feat/crucible-mop-slice1`,
> tip `48cfcea1`, full-workspace gate green 9309 tests). The deterministic
> Assembler (Stages 0–8 of `docs/superpowers/plans/2026-06-25-crucible-auto-assembly.md`)
> is built and merged on the branch: flux-pinned pricing stopgap, tail-latency
> cut, judge-inclusive pre-flight cap, stakes gate + member ladder, runtime
> `family()`, the pure `assemble()` (diversity + price-floor + **decoupled** strong
> judge + downshift ladder), CLI auto mode (`[crucible].assembly = "auto"` +
> `--council/--judge/--direct/--force-council/--deep/--deny`), and opt-in
> privacy-safe preference logging. Every stage was cross-audited + fixed; the
> capstone audit confirmed no over-cap council, decoupled judge, manual path
> byte-identical, no key leak. The CROSS-AUDIT REVISIONS below are all satisfied.
> Deferred fast-follows (non-blocking): flag-conflict warnings, R3 pool-narrowing
> note, `global_deadline_s < proposer_deadline_s` validation, `assembly="auto"`
> manual-config-drop doc/notice, absolute cross-pool price floor, BetaScorer
> learning (the logging is the day-one signal), and the authoritative Flux cost
> (FerroxLabs/wayland#319) that replaces Stage 2's markup stopgap.

## What the live run exposed

```
crucible: fused 3 proposal(s) from [flux-router, flux-router, flux-router]
crucible: spend = 6065 in + 1286 out tokens, ~$0.0000 across 4 member(s)
crucible:   flux-router (flux-pinned-claude-opus-4-8): 1499 in / 370 out → unpriced
crucible:   flux-router (flux-pinned-gpt-5):           710 in / 222 out → unpriced
crucible:   flux-router (flux-pinned-gemini-3-1-pro):  780 in / 269 out → unpriced
```

Two gaps: (1) **cost shows $0.0000** for a Flux council; (2) **provenance collapses**
to `flux-router ×3` because for a router the diversity lives in the *model*, not the
provider.

## Grounding (verified file:line)

- Engine cost path: `wcore_observability::cost::estimate_turn_cost(in,out,cr,cw,&ProviderCompat)`
  (`crates/wcore-observability/src/cost.rs:19`) multiplies tokens × `compat.cost_per_*_token`.
- For `flux-router`, `ProviderCompat.cost_per_input_token` / `..output..` etc. are
  **`Some(0.0)` sentinels** (`crates/wcore-config/src/compat.rs:470-473`), and the
  pricing catalog has **no `flux-router` key** → both paths yield $0.
- **No cost field is parsed from the HTTP response.** `TokenUsage`
  (`crates/wcore-types/src/message.rs:157`) is token-only; `openai.rs` usage parsing
  (`crates/wcore-providers/src/openai.rs:1468`) extracts tokens only; `flux_router.rs`
  is a thin wrapper.
- Session-level spend already exists: `ProtocolEvent::SessionCost { total_cost_usd,
  per_turn: Vec<TurnCost> }` (`crates/wcore-protocol/src/events.rs`), accumulated in
  the engine via `resolve_turn_cost_usd`.
- Surfacing precedent: **`workflow.rs:203-216`** attaches `AgentBus` + a
  `lifecycle_logger`; sub-agent events flow `AgentBus → ChannelSink → SubAgentRelay →
  ProtocolEvent::SubAgentEvent` (`spawner.rs:568`, `channel_sink.rs:49/103`,
  `spawn_tool.rs:351`), gated by `Capabilities.sub_agent_traces`
  (`events.rs:476`). The TUI renders these as `SubAgentView` rows via
  `protocol_bridge.rs:1539`. **The council attaches neither today** — `run_crucible`
  is a one-shot that prints at the end.

---

## Thread 1 — Flux / router cost visibility

**Why it matters (Sean):** Flux Router is ours; users must see what a run cost, and
cost-effectiveness *is* the value prop. `$0.0000` is unacceptable.

**Options**

- **A. Authoritative response cost (best, long-term).** Flux returns the real charge
  in the response (e.g. `usage.cost_usd` or an `x-flux-cost` header). We add a
  `cost_usd: Option<f64>` channel, parse it on the OpenAI-wire path, and have
  `CouncilSpend` / the engine prefer it. This is the *only* way to show Flux's TRUE
  (cost-effective) price — including whatever markup/flat-rate model Flux uses — and
  it's uniquely available because Flux is ours.
  - Cost: a `flux-router-app` server change (separate lane) **+** a small
    wcore-types/providers plumbing change.
- **B. Upstream-model estimate (stopgap, self-contained).** Map `flux-pinned-<x>` →
  the underlying catalog `(provider, model)` (e.g. `flux-pinned-claude-opus-4-8` →
  `anthropic/claude-opus-4-8`), price via the existing catalog, label it
  `~est (upstream list)`. Kills `$0.0000` immediately; lives entirely in the council
  spend module. **Caveat:** it shows the *upstream list price*, which Flux undercuts —
  so it's an **upper bound**, not the real charge. Honest if labeled, but it hides
  Flux's savings (shows more than billed).
- **C. Hybrid (recommended).** Cost-resolution precedence:
  `response-cost (A) → catalog → compat-heuristic → upstream-map estimate (B) →
  unpriced`. Show authoritative when present, a labeled estimate otherwise.

**Recommendation:** C, with A as the real fix and B as the immediate stopgap.

**Decision needed:**
1. Do you want Flux to return cost in the response (A)? If yes I'll spec the
   `flux-router-app` field + build the parse/plumbing here.
2. For the stopgap (B), is an upstream-list **upper bound** acceptable (labeled
   `~est`), or would you rather show *nothing* than an over-estimate?

---

## Thread 4 — Cost-aware model-mix (the "intelligent setup")

**Insight (Sean):** Don't default to frontier (Opus/GPT-5/Gemini-pro). Flux's edge is
**strong-but-cheap** models; a 4-model "balanced" council can cost less than one
frontier call and often match it. That's the headline: *Mixture-of-Providers cheaper
than one frontier model, often better.*

**Design — named roster presets + a cost tier the Conductor picks.** Proposed presets
(all via Flux, one key; picks drawn from the live `/v1/models` list):

| Tier | Proposers | Aggregator | Intent |
|------|-----------|-----------|--------|
| `frontier` | `flux-pinned-claude-opus-4-8`, `flux-pinned-gpt-5-5`, `flux-pinned-gemini-3-1-pro`, `flux-pinned-grok-4-3` | `flux-pinned-claude-opus-4-8` | max quality, $$$ |
| `balanced` (default) | `flux-pinned-deepseek-v4-pro`, `flux-pinned-glm-5-2`, `flux-pinned-grok-4-fast-reasoning`, `flux-pinned-gemini-3-5-flash` | `flux-pinned-deepseek-v4-pro` | strong, ~10-20% of frontier cost |
| `budget` | `flux-pinned-glm-4-5-air`, `flux-pinned-gemini-3-1-flash-lite`, `flux-pinned-qwen3-7-plus`, `flux-pinned-nova-lite` | `flux-pinned-glm-5-2` | cheap + fast |

**Conductor extension:** today `--auto` decides *council vs direct*. Extend to also pick
a **tier** by complexity + budget: trivial→direct; moderate→`budget`/`balanced`;
high-stakes→`balanced`/`frontier`. The existing `max_cost_usd` pre-flight cap already
enforces the ceiling.

**Buildable first slice:** ship the 3 presets as documented config + a
`crucible --tier <frontier|balanced|budget>` selector. Auto-tier-by-complexity is a
follow-up once Thread-1 cost numbers are real (the tier picker needs real $ to be smart).

**Decision needed:** bless the preset model picks (above) + the default `--auto` tier
(I recommend `balanced`).

---

## Threads 2 + 3 — Surfacing (TUI + JSON-stream), shared backbone

Both need `run_council` to **emit progress**, not print once at the end. Mirror
`workflow.rs`.

**Plan**

1. **Backbone (unblocks both).** Attach an `AgentBus` to the council spawner (one line,
   like `workflow.rs:209`) → each proposer's `Spawned/FirstMessage/Completed/Errored`
   flows for free. Add a dedicated **council-phase** signal — recommend a new
   `ProtocolEvent::CouncilEvent { phase, .. }` with phases
   `convening{members} → proposer_done{provider,model,latency_ms,tokens,cost} → fusing →
   spend{total,per_member} → done{chosen_from}` — for the council-level echo, while
   per-proposer lifecycle rides the existing `SubAgentEvent`.
2. **JSON-stream.** With the bus attached and `sub_agent_traces` on, `SubAgentEvent`
   emits automatically; add `CouncilEvent` emission in `run_council`. Desktop
   (`../wayland`) renders: *"Crucible convening 4 → opus done ($.01, 1.2s) → … → fused
   ($.06)."*
3. **TUI.** Move `crucible` from the one-shot early-return into the engine loop as a
   **`/crucible` slash command** (or later, a council *tool* the main agent can call), so
   the existing `SubAgentView` rows + `protocol_bridge` render proposers live, plus a
   small council summary panel (members, phase, running spend).
4. **CLI.** Keep the stderr summary (`render_provenance`, already done + tested). Fix the
   provenance collapse: show `provider:model` (or the spec) in the fused-from line so a
   router council reads `[deepseek-v4-pro, glm-5-2, gemini-3-5-flash]`, not
   `[flux-router ×3]`.

**Decisions needed:**
1. Dedicated `CouncilEvent` vs reuse `SubAgentEvent` payloads? (recommend dedicated.)
2. TUI surface = `/crucible` slash command first? (recommend yes; council-as-tool later.)
3. Desktop (`../wayland`) rendering — my scope, or hand to the desktop lane once the
   wcore-side events exist?

---

## Recommended sequencing

1. **Thread 1 stopgap (B/C)** — kill `$0.0000` with a labeled upstream estimate +
   provenance `provider:model` fix. Small, immediate, decision-light. *(Needs only the
   Thread-1 stopgap decision.)*
2. **Thread 4 presets + `--tier`** — high strategic value; config + Conductor.
3. **Backbone** — `AgentBus` + `CouncilEvent` on `run_council`.
4. **JSON-stream emission + desktop echo** — coordinate with desktop lane.
5. **TUI `/crucible`.**
6. **Thread 1 authoritative cost (A)** — needs the `flux-router-app` change (separate
   lane); plumbing on this side is ready after step 1.

## Cost evidence (from the live run + bundled catalog)

Using the **actual** live token counts and catalog list prices as a proxy (1 query):

| Member | Tokens (in/out) | Frontier model | ~$ | Balanced model | ~$ |
|--------|-----------------|----------------|----|----------------|----|
| p1 | 1499 / 370 | opus-4-8 ($5/$25) | 0.017 | deepseek-v4-pro ($0.55/$2.19) | 0.0016 |
| p2 | 710 / 222 | gpt-5 ($5/$15) | 0.007 | gemini-flash ($0.30/$2.50) | 0.0009 |
| p3 | 780 / 269 | gemini-pro ($1.25/$10) | 0.004 | glm-5-2 / grok-4-fast | (cheap) |
| agg | 3076 / 425 | opus-4-8 | 0.026 | deepseek-v4-pro | 0.0026 |
| **total** | | | **~$0.053** | | **~$0.006** |

**Headline:** a 4-model **balanced cross-provider council ≈ $0.006** vs a frontier
council ≈ $0.053 (**~8-9× cheaper**), and ~4× cheaper than a *single* Opus call
(~$0.026). That is the Flux value story in one number — and the case for `balanced`
as the default `--auto` tier.

**Catalog-lag finding (important for Thread 1):** the bundled catalog prices the
frontier upstreams (opus, gpt-5, gemini-2.5, deepseek-v4-pro, grok-3) but **not** the
newer Flux models the balanced/budget tiers lean on (`glm-5-2`, `gemini-3-1`,
`grok-4-fast`, `kimi-k2`, `nova`, `qwen3-7`). So the stopgap estimate (B) is
**partial** — exactly the models that make Flux cost-effective are the ones the catalog
doesn't know. This is a strong argument that **authoritative response-cost (A) is the
real fix**, because a static catalog will always lag Flux's roster.

## Model selection / assignment — how you compose a council

Four control surfaces, manual → automatic:

**1. Explicit (works today).** Name the members:
`proposers = ["flux-router:flux-pinned-deepseek-v4-pro", "flux-router:flux-pinned-glm-5-2", ...]`.
Full control; the floor everything else builds on.

**2. Named tiers + teams (next slice, curated).**
- *Tiers* by cost/quality: `frontier` / `balanced` / `budget` → `crucible --tier balanced`.
- *Teams* by purpose: define once, reuse — e.g.
  `[crucible.teams.security]`, `[crucible.teams.code]` → `crucible --team security "<task>"`.

**3. Auto (the Conductor picks the roster).** Selection runs along four axes:
- **Cost/tier** — complexity + budget → tier. Trivial→direct; moderate→budget/balanced;
  high-stakes→balanced/frontier. `max_cost_usd` (exists) hard-bounds it: pick the best
  roster that fits the budget.
- **Diversity (the core MoA lever)** — a council's value is *family* diversity (Anthropic
  vs OpenAI vs Google vs DeepSeek catch different errors). Selection maximizes distinct
  families rather than 3-of-one. Derivable **today** from the provider/model spec.
- **Task-fit (capability)** — code task → code-strong models, reasoning/math → reasoning
  models, etc. **This needs per-model capability metadata, which wayland-core does NOT
  have yet** (it has cost + context-window, no capability tags). Requires a models.dev
  source (per the "source live, don't hand-type" rule) — a dependency/follow-up.
- **Aggregator pick** — a strong, cost-matched synthesizer (reasoning-capable).

**4. Learned (long-term differentiator).** The Thompson `TemplateRouter`
(`wcore-dispatch`) already exists: track outcome quality per `(task-type, roster)` and
let the Crucible *learn* which mix wins for which task — the cross-provider analog of
Fugu's learned routing. Needs Flux cost (#319) + an outcome scorer.

**The pipeline:** `pool (live model list + metadata) → filter by budget → rank by
task-fit → maximize family diversity → pick aggregator → tier-gate by complexity →
(learned refinement)`.

**Buildable now vs blocked:**
- *Now:* explicit rosters (done), tier/team presets, diversity-aware pick, budget gating,
  cost-tier auto.
- *Needs models.dev metadata:* task-fit-by-capability.
- *Needs #319 cost + scoring:* learned selection.

**Recommended first slice:** tiers + named teams (curated, explicit) + `--tier`/`--team`,
and extend `--auto` to pick a tier by complexity+budget. Defer capability-fit (metadata
dep) and learning (cost+scoring dep).

**Decision needed:** pull in models.dev capability metadata now (unlocks task-fit
selection), or ship curated tiers/teams first and add capability-fit later?

## DECISION — auto-assembling councils (council verdict, 2026-06-25)

Ran a 7-agent deliberation (research: Fugu / OpenRouter Fusion+MoA / Claude → DevOps,
Product, C-Suite councils → Decision Council). Verdict: **no named/locked tiers** — a
single deterministic, inspectable, flag-gated **Assembler** stage.

**Mechanism:** a pure `assemble(task, keyed_pool, pricing, gate_cfg, policy) -> AssemblyPlan`
between `classify_task` (gate.rs) and `run_council` (run.rs), gated by
`[crucible].assembly = "manual" | "auto"` (default `manual` = today's shipped path, zero
regression). Three **orthogonal** decisions, each on a deterministic signal:
- **Convene-or-not:** the gate's stakes output (Low→Direct, Med/High→council). Stays in
  deterministic engine logic — Anthropic's own research shows LLMs over/under-fan-out when
  asked freeform.
- **Count + which models:** complexity ladder (Low=0, Med=3, High=5, clamped to
  `max_proposers` + keyed-candidate count) × greedy **max provider-family diversity** then
  **cheapest-per-family** over the live keyed pool. Diversity-by-family is the v1 axis —
  zero capability metadata, validated by the live $0.006-vs-$0.053 finding.
- **Aggregator (correctness invariant):** decoupled from proposer cost, scaled to
  complexity only, always strong. A weak judge makes a council *worse* than one model
  (selection-bottleneck) — so it's a hard invariant in code, not a knob.

**Budget = downshift ladder** (drop priciest proposer preserving spread → narrow N →
Direct), never a dead-end hard-fail; treat unpriced members as fail-closed cap-warnings,
not silent $0. **Transparency-by-default** (the wedge vs Fugu's opacity): emit an
inspectable `AssemblyPlan{members, aggregator, est_cost, reason}` pre-flight, fully
overridable (`--council`/`--judge`/`--direct`/`--deny <family>`) and pinnable into
`[crucible]`. "It picks, but you can always see why and pin it."

**Capability metadata deferred** (needs models.dev; fabricating is forbidden) → later
tie-breaker *after* diversity, never a gate. **Learning later:** re-key the existing
`BetaScorer` (wcore-dispatch Thompson) to `(task_kind, stakes) → family-mix`, on top of
the deterministic floor — and **log the preference tuple from day one** (the
cross-provider dataset is the moat no single-vendor incumbent can replicate).

**Research wedges (Fugu's top complaints → our strengths):** latency, cost-
unpredictability, opaque routing, over-orchestration. Found a real bug: `proposer_deadline_s`
exists but `run.rs`'s `join_all` never enforces it — one hung provider stalls the council.

**NOW build steps (additive, flag-gated, zero-regression):** (1) `assembly` flag; (2)
`CouncilProviderResolver::keyed_specs()` + pure `family(spec)`; (3) extend `CouncilDecision`
with stakes{Low,Med,High} + count ladder; (4) pure `assemble()` (snapshot-tested); (5)
budget downshift + unpriced fail-closed; (6) pre-flight `AssemblyPlan` line.
**NEXT:** (7) enforce per-member deadline; (8) AgentBus lifecycle events; (9) overrides +
copyable plan; (10) log preference tuple. **LATER:** (11) BetaScorer learning; (12)
models.dev capability tie-breaker + bounded recursion.

**Open questions for Sean (with my recs):**
1. Default `auto` budget cap — rec **$0.05/run** with downshift (cheap council lands ~$0.006, rarely binds).
2. Does `auto` eventually become the **default** (manual = power-user escape) or stay opt-in?
3. Interactive latency — cap **N=3 inline**, reserve N=5/High for an explicit `--deep`/async flag.
4. Family taxonomy — **one family per flux-pinned vendor** (anthropic/openai/deepseek/google/moonshot/xai/zhipu) for v1.
5. Sequencing — **diversity-floor first**, add models.dev capability later (rec), or pull capability in sooner?

## CROSS-AUDIT REVISIONS (4 adversarial lenses → synthesis, 2026-06-25)

Verdict: **framing sound, specifics not build-ready.** All 5 recs → REVISE (none held as-is,
none dropped). The audit verified claims against shipped code and refuted three premises.

**3 CRITICAL ship-blockers for the `auto` path (code-grounded):**
1. **Budget cap is POST-HOC, not pre-flight.** `wcore-budget/src/execution.rs:295` checks cost
   *after* it accrues. A real pre-flight gate must estimate from a **conservative output ceiling**
   (each member at `max_tokens_out`) and refuse to convene if it exceeds the cap; enforce
   per-member `max_tokens_out`; keep the post-hoc check as backstop.
2. **The estimator can't price the real pool.** `wcore-pricing/pricing.toml` has **zero**
   flux-pinned entries — grok-4-fast / glm-5-2 / gemini-flash / kimi-k2 absent. So the entire
   "$0.006 under $0.05" rationale is *uncomputable* today. Fix: a `flux-pinned-<vendor>` →
   native-SKU map + a Flux markup multiplier (stopgap, doesn't block on #319); unpriced members are
   **not-eligible for auto-selection** (exclude + surface "awaiting price"), hard-fail only if the
   priced pool is empty; `est_cost` renders `UNKNOWN`, never $0.
3. **No tail-latency control.** `wcore-tools/src/moa.rs:230` (and the council's `run.rs` `join_all`)
   drain *all* proposers — no timeout/quorum. `proposer_deadline_s` exists but is never enforced.
   Fix: per-member `tokio::time::timeout` + quorum + global soft-deadline (~20-25s inline) that fires
   the aggregator once quorum returns or the deadline elapses, cancelling stragglers.

**Other HIGH must-fixes:**
- **"Cheapest-per-family" is a degenerate-roster generator** — it picks flash/mini/distill SKUs,
  maximizing diversity while *minimizing* capability (the Fugu "Ultra loses to base" trap). Fix:
  **cheapest-COMPETENT-per-family** (price-tier floor; exclude bottom tercile / require ≥ fraction of
  family flagship). Uses cost data already in-repo. High-stakes requires ≥1 strong *proposer*, not
  just a strong judge; let the aggregator **abstain** → fall back to strong Direct.
- **Downshift ladder is quality-perverse** — it strips proposers (the decision-makers) while
  preserving the judge (the dominant, N-scaled cost). Fix: make the **judge a downshift rung**
  (Opus→Gemini Pro→DeepSeek Pro) first; swap a proposer down before dropping it; if it'd reduce to
  "1 proposer + judge" or a degenerate 2-cheap council → fall through to a single **strong Direct**.
- **Convene gate mis-routes** — keyword in pasted code/logs/attached docs auto-escalates; word-count
  convenes on a verbose stack-trace. Fix: classify from the **user-instruction span** (not file
  bodies), drop raw word-count, bias ambiguous → **lower** tier, require a *strong* signal for High.
- **Privacy** — don't log raw task text; log only stakes-class + family-mix + cost, local, with a
  `[crucible]` opt-out.
- **"Deterministic floor" is false** — `wcore-dispatch/src/scorer.rs:55` BetaScorer uses
  `from_os_rng()` in production. Fix: keep the default path purely deterministic (BetaScorer
  offline/opt-in only), or persist+log the RNG seed per run for reproducibility.

**Revised recommendations:**
- **R1 → tiered, judge-inclusive cap:** Low **$0.02** / Med **$0.05** / High **$0.15**, derived from a
  conservative judge-inclusive worst-case; wire into the existing `per_session_usd` /
  `per_user_daily_usd` caps; post-run actual-vs-estimate variance warning (>1.5× → recalibrate).
- **R2 → manual stays default indefinitely** until a *pre-registered* bar clears (real pre-flight gate
  + fully-priced pool + reproducibility + P95 cost under cap + a win-rate study auto ≥ best single
  strong-Direct with no High-stakes regression + P95 latency under the soft-deadline).
- **R3 → bind on a wall-clock global soft-deadline (~20-25s inline), not N.** Keep N=3 inline default;
  reserve top-tier judge + N=5 for `--deep`; **never silently** downgrade High-stakes → surface the
  N=5 plan and auto-suggest `--deep` or confirm.
- **R4 → vendor family key, but runtime-derived** from the `flux-pinned-<vendor>` prefix (a hardcoded
  7-vendor enum violates No-Hardcoded-Provider-Quirks); paired with the price-floor; flag shared-lineage
  vendors so diversity isn't over-rewarded for correlated models.
- **R5 → sequencing holds, but v1 ships "diversity + price-tier floor"** (capability earlier than "tie-
  breaker"); naked diversity-only does NOT ship. v1.5 = models.dev per-domain scores; v2 = BetaScorer
  error-decorrelation.

**Net:** manual-default Crucible ships now (already did); `auto` stays opt-in behind the 3 critical
fixes; the R2 default-flip is gated on the measurable bar. **#319 (Flux cost) is now on the auto
critical path** — but a `flux-pinned→native-SKU + markup` stopgap unblocks the estimator without
waiting on the Flux server change.

## Decisions to unblock me (summary)

1. **Flux cost:** authoritative response-cost (A) — want it, and should I plumb it? And
   is the labeled upstream **upper-bound** estimate (B) acceptable as the stopgap?
2. **Model-mix:** bless the preset rosters + default `--auto` tier (`balanced`)?
3. **Surfacing:** dedicated `CouncilEvent`? TUI via `/crucible` slash command? Desktop
   rendering = my scope or the desktop lane's?

Once you pick #1's stopgap direction I can build step 1 immediately; the rest follows
the sequence above.
