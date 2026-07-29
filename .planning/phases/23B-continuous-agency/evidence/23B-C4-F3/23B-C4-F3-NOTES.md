# C4-F3 NOTES — `provider` on the cost surfaces is the compat profile, not the route

Lane: `lane/cost-provider`. Base merge: `632ad619` (integration
`gh/plan/f20-unified-audit-repair` merged in, fast-forward).

## M1 — where the wrong value is written (measured, unproxied grep)

Search run:
`/usr/bin/grep -rn "provider_type()" crates/ --include="*.rs"`

Three cost-relevant writers, all in `crates/wcore-agent/src/engine.rs`:

| site | line | what it feeds |
|---|---|---|
| cache/cost ledger | `engine.rs:13017` | `let provider = self.compat.provider_type().to_string();` → `TurnSample.provider` + `resolve_turn_cost` + `pricing_turn_cost_with_cache` |
| `TurnTrace` (no-tool-calls path) | `engine.rs:11331` | `provider: self.compat.provider_type().to_string()` |
| `TurnTrace` (tool-loop path) | `engine.rs:12087` | `provider: self.compat.provider_type().to_string()` |

`resolve_turn_cost` is ALSO called with the same string at `engine.rs:11316` and
`engine.rs:12074`, so the price lookup key is the compat profile too — this is
not only a label, it selects the rate card.

## M2 — why the compat profile is not the route (mechanism)

`bootstrap.rs:172` `PluginProviderRouter` — a `Fn(&str, &[Arc<dyn PluginProvider>])
-> Option<Arc<dyn LlmProvider>>`. Invoked at `bootstrap.rs:956` on
`self.config.model`. `wcore-cli/src/main.rs:151 make_plugin_provider_router`
claims any model with the `ollama:` prefix and returns `wayland-ollama`'s
provider. **Nothing on that path touches `config.compat`.** So a session
configured with `anthropic` compat that runs `ollama:smollm2:135m` keeps
`compat.provider_type() == "anthropic"` while the turn is served by Ollama.

Second, independent divergence: `bootstrap.rs` wraps the primary in
`ResilientProvider::new_with_policy(..., fallbacks, ...)`. A failover arm has its
OWN compat (`bootstrap.rs:4029` `fallback.compat.provider_type()`), so even with
no plugin route the serving arm can differ from `self.compat`.

## M3 — where the RIGHT value already lives (and why it was never selected)

`ProviderCompat::ollama_defaults()` (`compat.rs:831`) already carries
`provider_type: "ollama"`, all four cost rows at `0.0`, and
`cost_is_known_free: true`. It was never reachable in production.

Paired absence measurement (LANE-BRIEF §3b-i), unproxied `/usr/bin/grep`:

```
KNOWN-POSITIVE  grep -rn "anthropic_defaults" crates/ --include="*.rs" | grep -v "/tests/"  → 69 hits
TARGET          grep -rn "ollama_defaults"    crates/ --include="*.rs" | grep -v "/tests/"  → 4 hits
```

All four target hits are non-production: `compat.rs:831` (the definition),
`compat.rs:1617` + `compat.rs:1718` (inline `#[cfg(test)]`), and
`engine.rs:1920` (inside the `#[cfg(test)]` module that opens at `engine.rs:1797`).
`compat_defaults_for` (`config.rs:1929`) matches on `ProviderType`, which has
**no Ollama variant**, so it could never return it. So: `ollama_defaults()` had
ZERO production construction sites.

## M4 — the fix, and the in-repo precedent that dictates it

`config.rs:2229` already documents this exact defect class, twice:

> "D.2 (v0.6.3) — … Reusing `openai_defaults()` verbatim **mislabelled their cost
> attribution as `openai` and charged them GPT-class rates ($8/$32 per Mtok)** for
> cheap open-weight models."
> "A catalog provider … must NOT use `openai_defaults()` — that mislabels cost
> attribution … Derive the compat from the catalog entry so `provider_type`
> carries the real id."

The established repair is therefore **select the compat defaults from the ROUTE,
at `compat_defaults` in `Config::resolve`.** The local route was missed because
it is selected by the MODEL STRING (`make_plugin_provider_router`), not by
`ProviderType` and not by a catalog entry. Fix = one added arm, ordered ahead of
the catalog arm, keyed on the existing canonical predicate
`wcore_types::model_aliases::is_local_model` (already used two blocks below at
`config.rs:2188` for the credential exemption, and at `bootstrap.rs:999` for the
refusal). No `base_url` sniff, no provider conditional — AGENTS.md §"No
Hardcoded Provider Quirks" satisfied by construction.

One change fixes all four surfaces because `AgentEngine.compat` is
`config.compat.clone()` (`engine.rs:3095`, `engine.rs:3338`) and all four read
`compat.provider_type()`.

## M5 — status

- [x] wrong-value sites located
- [x] right-value source located (`ollama_defaults`, production-dead)
- [x] fix (`config.rs` compat_defaults local arm)
- [x] tests authored (wcore-config × 4, wcore-agent × 2)
- [ ] known-negative run (must actually fail on unfixed code) — hetzner
- [ ] hetzner fmt/check/tests
