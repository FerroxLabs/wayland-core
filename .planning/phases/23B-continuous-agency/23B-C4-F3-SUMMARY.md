# C4-F3 — LANE SUMMARY (`lane/cost-provider`)

**Branch:** `lane/cost-provider` · **Base:** `plan/f20-unified-audit-repair`, merged
forward to `632ad619` (merge, never rebase — LANE-BRIEF §0)
**Fix commit:** `bce323a2` · **Date:** 2026-07-29
**Evidence:** `evidence/23B-C4-F3/23B-C4-F3-EVIDENCE.md`, `…-NOTES.md`, live captures in
`evidence/23B-C4-F3/live/`

## VERDICT: FIXED, and live-proved on the operator's own surface at zero cost

| | |
|---|---|
| Defect real? | **Yes** — reproduced live at the current tip |
| Fixed? | **Yes**, at the `ProviderCompat` layer, 22 lines of which 4 are code |
| Known-negative fails? | **Yes** — captured verbatim, unit and live |
| Reaches `TurnTrace` / budget? | **Yes — five surfaces, all one field, all fixed together** |

---

## 1. The mechanism — the orchestrator's description was right, and incomplete in three ways

The brief said: *the ledger records `provider` as the ProviderCompat profile name, not
the route actually taken.* That is correct. `crates/wcore-agent/src/engine.rs:13017`:

```rust
let provider = self.compat.provider_type().to_string();
```

Three corrections, all material:

**(a) It is not only a label — it is the pricing key.** That same string is passed to
`resolve_turn_cost` (`engine.rs:11316`, `12074`, `13018`) and to
`pricing_turn_cost_with_cache` (`engine.rs:13032`), which look up the `wcore-pricing`
catalog by provider×model. So the wrong id does not merely mis-title the row; **it
selects the rate card.**

**(b) There are five surfaces, not three.** Beyond the ledger, `TurnTrace.provider`
(`engine.rs:11331`, `12087`) and the budget reservation (`engine.rs:9950`), the same
value stamps the **journalled physical-attempt identity** —
`JournaledLlmProvider::new(…, self.compat.provider_type(), …)` at `engine.rs:13226`,
consumed as `ProviderAttemptContext.provider` (`journal_provider.rs:191`). A durable
record of which provider a session physically dispatched to is also wrong.

**(c) The root is one layer lower than "the ledger reads the wrong variable".** The
ledger reads the only variable that exists. The defect is that **`config.compat` is
never derived from the route**: `make_plugin_provider_router` (`wcore-cli/src/main.rs:151`)
claims every `ollama:`-prefixed model and serves it from `wayland-ollama`, but
`ProviderType` has **no Ollama variant**, so `compat_defaults_for` (`config.rs:1929`)
could never return the local profile.

**And `ProviderCompat::ollama_defaults()` — carrying `provider_type: "ollama"`, all four
cost rows at `0.0`, and `cost_is_known_free: true` — had ZERO production construction
sites** (`compat.rs:831`; paired absence measurement in EVIDENCE §2, known-positive
`anthropic_defaults` → 69 non-test hits, target → 4, all of them the definition or
`#[cfg(test)]`). The compat layer already held the right answer and nothing selected it.

## 2. Where the right value lives, and why the fix goes there

`crates/wcore-config/src/config.rs:2229` already documents this exact defect class,
fixed twice before:

> "D.2 (v0.6.3) — … Reusing `openai_defaults()` verbatim **mislabelled their cost
> attribution as `openai` and charged them GPT-class rates ($8/$32 per Mtok)** for cheap
> open-weight models."

The established repair is *select the compat defaults from the route, at the
`compat_defaults` seam*. The local route was the one case missed, because it is selected
by the **model string**, not by `ProviderType` and not by a catalog entry. So:

```rust
let compat_defaults = if wcore_types::model_aliases::is_local_model(&model) {
    ProviderCompat::ollama_defaults()
} else if let Some(entry) = catalog_entry.as_ref() { … } else { compat_defaults_for(provider) };
```

`is_local_model` is the existing canonical predicate, already used two blocks below
(`config.rs:2188`, the credential exemption) and in `bootstrap.rs:999` (the refusal to
fall through to a remote provider). **No `base_url` sniff, no provider conditional** —
AGENTS.md "No Hardcoded Provider Quirks" satisfied by construction. User
`[provider.compat]` overrides still merge on top. Ordered ahead of the catalog arm
because the router claims `ollama:` unconditionally and bootstrap refuses any remote
fallback for a local model, so the local route is the one that runs.

One change fixes all five surfaces because `AgentEngine.compat` is
`config.compat.clone()` (`engine.rs:3095`, `3338`).

## 3. Proof

**Three-assertion self-test** (LANE-BRIEF §6b-ii) — full transcripts in EVIDENCE §3.

* **Known-positive:** `wcore-config` 4 passed / 0 ignored / 0 filtered out;
  `wcore-agent` 2 passed / 0 ignored / 0 filtered out.
* **Known-negative — genuinely fails.** Controlled revert in place, rebuilt, restored:

  ```
  assertion `left == right` failed: the `ollama:` route serves this turn, so every cost
  surface keyed on compat.provider_type() must say so. Got `anthropic` …
    left: "anthropic"   right: "ollama"

  assertion `left == right` failed: a local model must not inherit the cloud provider's input rate
    left: Some(1.5e-5)  right: Some(0.0)          ← $15/Mtok, on free local hardware

  … and end-to-end through a real AgentEngine::run() to the ledger JSON on disk:
  assertion `left == right` failed: the ledger row must name the route that served the turn …
    left: "anthropic"   right: "ollama"
  ```

  **Both controls stayed green in both runs** — a build that stamped `ollama` (or $0)
  onto everything fails `remote_model_still_carries_its_own_provider_and_real_rates`.
  The suite can fail in both directions.
* **The old shape would have missed it — executed, not argued.** At the same unfixed
  commit, `local_model_no_credential_test` 3/3 ok and `cost_estimate` 9/9 ok. The two
  nearest misses are exact: the first resolves the *same* `ollama:` `Config` and asserts
  on `model` and `api_key`, never `compat`; the second proves `ollama_defaults()` prices
  to zero **while constructing that preset by hand**, which was the only way it was ever
  constructed.

**LIVE (LANE-BRIEF §3.1)** — `hetzner-dsm` runs a live ollama carrying `smollm2:135m`,
**the exact model of the original $0.0756 measurement**, so the revert arm cost nothing
and was run. Read off `wayland-core cache show`, the operator surface:

| run | `BIN_SHA` | ledger `provider` | `cost_usd` |
|---|---|---|---|
| FIXED | `43379f732ece342c` | **ollama** | **0.000000** |
| REVERT | `ad0171983f03c184` | **anthropic** | **0.018840** |
| RESTORE | `43379f732ece342c` | **ollama** | **0.000000** |

RESTORE's binary is **byte-identical** to FIXED's and REVERT's differs — binary identity
measured, not assumed. `$0.018840 charged for one 1126-token turn that ran on this
machine's own hardware for nothing`, then closed. Provider read back from the engine's
own log on both arms (§3b-ii): 8× `provider="ollama"` vs 8× `provider="anthropic"`.

**A dead instrument was caught and repaired mid-run** (EVIDENCE §4a): the first readback
grep returned **0 matches on a file that visibly contained the lines**, because tracing
interleaves ANSI escapes between `provider` and `=`. Repaired by stripping escapes, then
proved on the known-positive before being trusted. Repaired in-lane, not merely noted —
§6b-ii.

**Gates** at `bce323a2` on hetzner, every count with `0 ignored` / `0 filtered out` read
back: `wcore-config` **567 passed** (+13 further binaries ok), `cache_ledger_engine_test`
6, `turn_trace_shape` 3, `ollama_e2e_test` 4 (1 ignored, pre-existing),
`cache_ledger_cli` 13, `cost_estimate` 9. `cargo check --workspace --all-targets`
finished with **0** `^error` lines. `cargo fmt --all -- --check` clean.

## 4. `TurnTrace` and the budget path — checked, and the answer has a caveat

All five surfaces read the identical `self.compat` field, so the fix covers them. But the
strength of the claim differs by surface and I am not going to flatten it:

* **Ledger** — observed directly, live, on disk (§3 above).
* **Budget reservation** (`engine.rs:9950`) — code-level: reads the same field and feeds
  `resolve_conservative_reservation_cost`. Consequence beyond mislabelling: under a
  strict monetary cap, free local inference was *reserving* Anthropic-priced dollars
  against the session envelope. Not exercised live (needs a configured cap).
* **`TurnTrace`** — a **two-link argument**, not one end-to-end observation:
  `Config::resolve` → `compat.provider_type() == "ollama"` (my test) and
  `TurnTrace.provider == compat.provider_type()` (the pre-existing, passing
  `turn_trace_shape` test). Sound, but stated as what it is.
* **Journalled attempt identity** — code-level only.

## 5. Out of scope but real — named, not fixed

1. **A configured failover arm still mislabels the ledger and `TurnTrace`.**
   `ConfiguredFallbackBudgetState.current_provider` (`engine.rs:616`, updated at `10222`)
   correctly tracks the arm that served the turn for the budget **settle** path
   (`engine.rs:10678`) — but the ledger (`13017`) and both `TurnTrace` sites
   (`11331`/`12087`) still read `self.compat`. When a configured fallback serves the
   turn, the ledger names the primary. **Distinct mechanism** from C4-F3 (mid-turn
   failover vs. boot-time route) and a larger change: `current_attempt_provider` is
   scoped inside the stream loop while the trace is emitted outside it.
2. **A proved-free route is graded as an estimate.** `cost_is_known_free` short-circuits
   the cost helper to a *proved* `Some(0.0)` (`engine.rs:745`), but `CostSource` has no
   `KnownFree` grade, so the local run lands in `ProviderDefaults` →
   `cost_truth=estimated` → `cache verify` **exit 7**, `cost_warning
   text=usd_is_a_family_rate_estimate_not_spend`. Observed live on all three arms. The
   number is now right; the label calls a certainty an estimate.
3. **Two different keys for one turn.** `engine.rs:9950` reads the raw field with
   `unwrap_or("")` while every other surface uses the `provider_type()` accessor, which
   defaults to `"unknown"`. A compat with no `provider_type` keys the budget on `""` and
   the ledger on `"unknown"`.
4. **`config.provider_label` still reads `anthropic` for a local run** — it feeds
   `ResilientProvider`'s `primary_name`, session records and circuit reports. Not a cost
   surface, but a second, still-wrong identity for the same turn.
5. **The council-derived path (`config.rs:~2723`) was deliberately NOT changed.** It has
   no plugin router and no `is_local_model` guard, so stamping the local compat there
   would price a real remote member at $0 — worse than the bug being fixed. The flip side
   is that a council member spec'd `ollama:*` has no local route at all.
6. **Stale comment** in the F-088 block (`config.rs`): "`model` is resolved below" —
   `model` is bound at `config.rs:2111`, above it. The block recomputes an
   `effective_model` it does not need.
7. **Pre-existing clippy warning**, not mine: `needless_update` at
   `crates/wcore-agent/tests/cache_ledger_engine_test.rs:82` (a file this lane did not
   touch; working tree was porcelain-empty at the time of the run).

## 6. What I did NOT do

No merge to `main`, no PR, no tag, no release, no issue closed, no
`wcore-contract generate`, no `git rebase`, no `git clean`/`reset`/`stash`/`checkout`, no
`git add -A`. Nothing pushed to `plan/f20-unified-audit-repair` — only to
`gh lane/cost-provider`. Neither shared-fence file (`wcore-cli/src/lib.rs`,
`wcore-cli/src/main.rs`) was modified. **No credential was used, printed or transmitted**
— the live leg ran entirely on a local model, which is why the revert arm was affordable.
No test was weakened, ignored, re-gated or deleted; no timeout raised.
