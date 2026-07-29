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

## M3 — status

- [x] wrong-value sites located
- [ ] right-value source chosen
- [ ] fix
- [ ] known-negative test (must actually fail on unfixed code)
- [ ] hetzner fmt/check/tests
