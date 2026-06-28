# Cost & token-spend governance

Wayland Core ships three **independent** cost-control mechanisms. Each is
configured separately, each tracks something different, and each fails its own
way when a limit is hit. All are opt-in except the Crucible daily envelope,
which defaults on.

| Mechanism | Config block | Tracks | When a cap trips |
|-----------|--------------|--------|------------------|
| Execution-tree budget | `[budget]` | wall time, tool runtime, process count, agent depth, tokens, USD — across the whole session tree | Emits a single `BudgetExceeded` protocol event on the first cap to trip |
| Per-session spend tracker | `[session_cap]` | input + output tokens, USD cost per session | Warns at 80%, then blocks the **next** turn and terminates the run cleanly |
| Crucible council caps | `[crucible]` | per-run worst-case council cost, per-day aggregate, per-stakes-tier auto cost | Refuses to spawn the council (per-run / daily) or trims the roster (stakes tiers) |

These caps are **post-hoc** for the engine path: the provider has already billed
a turn by the time it is charged, so a cap blocks the turn *after* the one that
crossed it. It never un-spends the crossing turn.

> **Note on "#65 token-spend governance".** The 0.12.6 `#65` change is a
> *performance improvement* — routing-tier wiring, cheap-but-accurate
> compaction, bounded retries, and prompt-cache hygiene. It **reduces** spend; it
> is not a configurable cap. The configurable caps are the three blocks above.

---

## 1. Execution-tree budget (`[budget]`)

Use this to bound a whole run — including every sub-agent it spawns — across
multiple dimensions at once (not just money). Counters are **tree-shaped**:
when a child records tokens or cost, the counter rolls up to all of its
ancestors, so a stricter child cap never relaxes the parent.

When any cap is exceeded the engine emits a single `BudgetExceeded` protocol
event carrying the `reason`, the `observed` value, and the `limit`. Caps are
checked in a **fixed, deterministic order**, so the first one to trip is always
reported the same way:

```
max_wall_time → max_tool_runtime → max_processes → max_agent_depth
            → max_tokens_in → max_tokens_out → max_cost_usd
```

```toml
[budget]
max_wall_time_secs    = 600      # 10 minutes of wall clock
max_tool_runtime_secs = 300      # cumulative tool runtime
max_processes         = 8        # concurrent Bash/Script children
max_agent_depth       = 3        # sub-agent delegation depth
max_tokens_in         = 2_000_000
max_tokens_out        = 400_000
max_cost_usd          = 5.00
```

### Config reference

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `[budget].max_wall_time_secs` | `Option<u64>` | `None` | Wall-clock cap for the tree, in seconds |
| `[budget].max_tool_runtime_secs` | `Option<u64>` | `None` | Cumulative tool-runtime cap, in seconds |
| `[budget].max_processes` | `Option<usize>` | `None` | Max concurrent child processes (Bash/Script) |
| `[budget].max_agent_depth` | `Option<usize>` | `None` | Max sub-agent delegation depth |
| `[budget].max_tokens_in` | `Option<u64>` | `None` | Input-token cap for the tree |
| `[budget].max_tokens_out` | `Option<u64>` | `None` | Output-token cap for the tree |
| `[budget].max_cost_usd` | `Option<f64>` | `None` | USD cost cap for the tree (checked last) |

Every field is optional; leave one unset to leave that dimension uncapped.

---

## 2. Per-session spend tracker (`[session_cap]`)

Use this when you only want a **money / token spend** ceiling per session, with
an early warning. It is configured with the same shape as `[budget]`, but the
tracker only consumes the **token and cost** fields — see the gotcha below.

After every accepted LLM turn the engine charges the tracker:
`charge(session_id, tokens, cost)`. Cost is read from the `wcore-pricing`
catalog for the model **actually dispatched** (so a tier-swapped cheap turn is
billed cheap), falling back to a `ProviderCompat` heuristic on a catalog miss.

What happens as the total climbs:

- **At ≥ 80%** of the strictest configured cap, the tracker emits one `CapWarn`
  alongside the normal `Charge` event.
- **On the charge that would cross a cap**, the charge is **rejected and does not
  stick** — running totals are not incremented. The engine then repairs any
  orphaned `tool_use` blocks, emits a `BudgetExceeded` protocol event plus a
  user-visible `Run stopped: budget cap … exceeded` error, and finishes the run
  cleanly rather than starting another paid turn.

Each session has its own bucket; separate sessions never share a cap.

```toml
[session_cap]
max_tokens_in  = 1_000_000
max_tokens_out = 200_000
max_cost_usd   = 1.50
```

### Config reference

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `[session_cap]` (whole block) | `Option<BudgetConfig>` | `None` | Opt-in per-session tracker. Same 7-field shape as `[budget]`, but only the token + cost fields below are wired in |
| `[session_cap].max_tokens_in` + `max_tokens_out` | `Option<u64>` (each) | `None` | **Summed** (saturating) into one per-session token cap. Either or both may be set |
| `[session_cap].max_cost_usd` | `Option<f64>` | `None` | Per-session USD cap charged against each turn |

> **Gotcha — these are not interchangeable with `[budget]`.** Only the token and
> USD fields of `[session_cap]` are enforced. `max_wall_time_secs`,
> `max_tool_runtime_secs`, `max_processes`, and `max_agent_depth` are **ignored**
> inside `[session_cap]`. Put time / process / depth caps in `[budget]`, and
> token / USD spend caps in `[session_cap]` (or in `[budget]` if you also want
> them tree-rolled-up).

> **Not yet available — a generic per-user/day cap.** The tracker has an internal
> per-user daily USD bucket (keyed by user and UTC day), but it has **no TOML
> field** and the engine's per-turn path does not call it. The only
> config-wired daily cap that ships today is the Crucible `daily_cap_usd` below.

---

## 3. Crucible council caps (`[crucible]`)

The [Crucible](../README.md#crucible--a-mixture-of-providers-council) council fans a task out across multiple
providers, so it is the most expensive thing the engine can do — and it has the
most cost machinery. Before spawning anything, the council certifies a
**judge-inclusive worst-case estimate** (the aggregator/judge is the dominant
cost and scales with proposer count) and refuses to run if that estimate
violates a hard cap or cannot be priced.

Pricing is catalog-driven (`wcore-pricing` `DEFAULT_CATALOG`), accounted in
integer **microcents** (no float drift). A catalog miss contributes `0` cost but
flags the roster `priced = false`, so the council can tell *free* from
*unpriced* and never silently passes a cap on an undercount.

There are three distinct caps:

- **`max_cost_usd` — strict per-run ceiling.** If set and the certified
  worst-case estimate exceeds it, the council refuses with `OverBudget`. An
  unpriceable roster is refused with `UnpriceableRoster` rather than run against
  a `$0` undercount. Unset by default.
- **`daily_cap_usd` — soft per-day envelope.** Defaults to **`$20`**. Binds
  *only* when the roster is fully priceable: if prior daily spend plus this
  council's certified ceiling exceeds the cap, the council refuses before spawn
  with `DailyBudgetExhausted`. An unpriced roster is never hard-refused here.
- **`cap_low_usd` / `cap_med_usd` / `cap_high_usd` — auto-path stakes caps.** In
  auto mode (`--auto`) a difficulty classifier picks a stakes tier and the
  assembler trims the roster to fit that tier's cap. Defaults: `$0.02` / `$0.05`
  / `$0.15`. `--deep` forces the High tier.

### Config reference

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `[crucible].enabled` | `bool` | `false` | Council kill-switch; no fan-out unless `true` |
| `[crucible].max_cost_usd` | `Option<f64>` | `None` | **Strict** per-run hard ceiling; refuses `OverBudget` / `UnpriceableRoster` |
| `[crucible].daily_cap_usd` | `Option<f64>` | `Some(20.0)` | **Soft** per-user/day envelope; binds only on a fully-priceable roster |
| `[crucible].cap_low_usd` | `f64` | `0.02` | Auto-path spend cap for a Low-stakes council |
| `[crucible].cap_med_usd` | `f64` | `0.05` | Auto-path spend cap for a Med-stakes council |
| `[crucible].cap_high_usd` | `f64` | `0.15` | Auto-path spend cap for a High-stakes council (used by `--deep`) |
| `[crucible].max_proposers` | `usize` | `5` | Upper bound on roster size — a blast-radius cap at roster validation |
| `[crucible].proposer_max_turns` | `usize` | `4` | Per-proposer turn budget; feeds the worst-case estimate |
| `[crucible].flux_markup` | `f64` | `1.0` | Multiplier on the native-SKU price when pricing a `flux-pinned-*` model (stopgap until Flux emits authoritative cost) |
| `[crucible].crucible_auto_spend` | `bool` | `false` | In a non-TTY invocation, auto-approve the plan instead of failing closed. Default `false` = headless/piped runs never spend without explicit human approval |

The certified worst-case also uses an internal per-proposer output budget
constant (`DEFAULT_PROPOSER_MAX_TOKENS = 4096`) as the single source of truth, so
the estimate cannot drift from actual spend.

### Minimal council config

```toml
[crucible]
enabled    = true
proposers  = ["anthropic:claude-…", "openai:gpt-…"]
aggregator = "anthropic:claude-…"
# defaults apply: daily_cap_usd = $20, no per-run cap,
# max_proposers = 5, tiers $0.02 / $0.05 / $0.15
```

### Raising caps interactively

In an interactive (TTY) crucible run the approval loop can raise caps in place:

- `ApprovePremium { ceiling_usd }` and `Edit { budget_usd }` raise all three
  stakes-tier caps and re-assemble a stronger roster.
- A `--judge` override re-prices the roster against the **actual** pinned judge
  and re-checks the (possibly-raised) cap. If it still exceeds, the council
  appends a `WARNING: pinned judge est … exceeds the … cap` note and proceeds —
  it never silently overspends or mis-reports.

### Crucible CLI flags

```bash
wayland-core crucible "<task>"      # run the council, subject to [crucible] caps
```

| Flag | Effect |
|------|--------|
| `--auto` | Gate a manual roster behind a cheap difficulty classifier (trivial → single direct call; high-stakes → full council) |
| `--deep` | Treat the task as **High** stakes — widest roster + strongest judge, governed by `cap_high_usd` |
| `--council <specs>` | Pin the auto candidate pool to exactly these `provider:model` specs (forces auto mode) |
| `--judge <spec>` | Pin the aggregator; re-prices the roster and re-checks the stakes cap |
| `--direct` | Force a single direct answer instead of convening a council |
| `--force-council` | Convene regardless of the gate |
| `--deny <families>` | Exclude provider families from an auto roster |
| `--advisor` / `--terminal` | Choose the synthesis sink — inject into the trusted loop vs print-and-stop (mutually exclusive) |

> **Gotcha — the daily envelope is in-process only.** The Crucible `daily_cap_usd`
> aggregates spend **within a single CLI process**, not across separate
> invocations. Cross-process daily persistence is not yet shipped. The CLI
> council identity is the `WAYLAND_USER_ID` env var (default `default`).

---

## Worked examples

**Per-session USD cap blocks the next turn.** With `[session_cap] max_cost_usd =
1.50`, once cumulative turn cost crosses `$1.50` the engine emits
`BudgetExceeded` and stops. The crossing turn is still billed — caps are
post-hoc.

**A token cap does not stick on overrun.** With a per-session token cap of
`1500`: a `1000`-token charge is accepted; a following `600`-token charge is
rejected (`CapExceeded`); the session total stays at `1000`, not `1600`.

**Separate sessions, separate buckets.** With `max_cost_usd = 0.10`: session
`s1` charges `$0.09` (ok) and session `s2` charges `$0.09` (ok — a fresh
bucket); then `s1 + $0.05` is rejected.

**The 80% warning fires once.** With `max_cost_usd = 0.10`, a single `$0.09`
charge (90%) emits exactly one `CapWarn` alongside its `Charge`.

**Crucible refuses an unpriceable roster.** With `[crucible] max_cost_usd = 5.0`
and a member the catalog can't price → `UnpriceableRoster`; the council never
runs.

**Crucible refuses before spawn when the day is exhausted.** A small
`daily_cap_usd` where prior spend plus the certified ceiling exceeds the cap →
`DailyBudgetExhausted` before any provider call.

**An auto-path stakes cap trims the roster.** A small `cap_med_usd` makes the
assembler trim to a council that fits; set it below the cost of any viable
council (e.g. `0.0001`) and there is no viable roster.

---

## See also

- [Crucible](../README.md#crucible--a-mixture-of-providers-council) — the council itself, roster selection, and
  synthesis modes.
- [Providers & Authentication](providers.md) — provider pricing and model
  selection that feed the cost catalog.
