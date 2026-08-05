# C4-F3 — EVIDENCE

Lane `lane/cost-provider`. Fix commit `bce323a2`. Build host `hetzner-dsm`,
worktree `/root/wayland-cost-provider`, branch `hz/cost-provider` at `bce323a2`.

Every count below is read back verbatim from an unproxied `cargo` invoked over
ssh, so the `0 ignored` / `0 filtered out` fields the anti-vacuity rule depends
on are present (LANE-BRIEF §3b strips them locally).

---

## §1 — the defect, located

Wrong value written (all three are the same expression):

| surface | file:line |
|---|---|
| cache/cost ledger `TurnSample.provider` | `crates/wcore-agent/src/engine.rs:13017` |
| `TurnTrace.provider`, no-tool-calls path | `crates/wcore-agent/src/engine.rs:11331` |
| `TurnTrace.provider`, tool-loop path | `crates/wcore-agent/src/engine.rs:12087` |
| budget reservation `reservation_provider` | `crates/wcore-agent/src/engine.rs:9950` |
| journalled provider-attempt identity | `crates/wcore-agent/src/engine.rs:13226` → `journal_provider.rs:191` |

The same string is also the **pricing key**, not merely a label:
`resolve_turn_cost(provider, …)` at `engine.rs:11316`, `engine.rs:12074`,
`engine.rs:13018` and `pricing_turn_cost_with_cache(&provider, …)` at
`engine.rs:13032` all take it as the catalog lookup key.

Right value: `ProviderCompat::ollama_defaults()` — `crates/wcore-config/src/compat.rs:831`
(`provider_type: "ollama"`, all four cost rows `0.0`, `cost_is_known_free: true`).

Fix: `crates/wcore-config/src/config.rs:2246-2267` — one added arm at the
`compat_defaults` seam, ordered ahead of the catalog arm.

---

## §2 — paired absence measurement (LANE-BRIEF §3b-i)

Claim under test: *`ollama_defaults()` has no production construction site.*
An absence, so the instrument is proved alive on a known-positive in the same
form of invocation. Unproxied `/usr/bin/grep`, globs quoted.

```
KNOWN-POSITIVE
  /usr/bin/grep -rn "anthropic_defaults" "crates/" --include="*.rs" | /usr/bin/grep -v "/tests/" | wc -l
  → 69

TARGET
  /usr/bin/grep -rn "ollama_defaults" "crates/" --include="*.rs" | /usr/bin/grep -v "/tests/"
  → crates/wcore-agent/src/engine.rs:1920
    crates/wcore-config/src/compat.rs:831
    crates/wcore-config/src/compat.rs:1617
    crates/wcore-config/src/compat.rs:1718
```

All four are non-production: `compat.rs:831` is the definition;
`compat.rs:1617` / `compat.rs:1718` are inline `#[cfg(test)]`; `engine.rs:1920`
sits inside the `#[cfg(test)]` module opening at `engine.rs:1797` (verified:
`/usr/bin/grep -n "^#\[cfg(test)\]" crates/wcore-agent/src/engine.rs | awk -F: '$1<1920' | tail -1`
→ `1797`).

Corroborating structural fact, not a grep: `compat_defaults_for`
(`config.rs:1929`) matches exhaustively on `ProviderType`, and
`/usr/bin/grep -n "pub enum ProviderType" -A 40 crates/wcore-config/src/config.rs | /usr/bin/grep -i ollama`
returns nothing — **there is no Ollama variant**, so that function could never
have returned the preset.

---

## §3 — the three-assertion self-test (LANE-BRIEF §6b-ii)

### (a) known-positive — FIXED code, both suites green

```
cargo test -p wcore-config --test local_model_cost_attribution_test
running 4 tests
test a_near_miss_prefix_is_not_treated_as_local ... ok
test local_model_carries_the_free_cost_rows_not_the_cloud_family_rate ... ok
test local_model_is_attributed_to_the_local_route_not_the_configured_profile ... ok
test remote_model_still_carries_its_own_provider_and_real_rates ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

cargo test -p wcore-agent --test local_route_cost_attribution_test
running 2 tests
test a_local_turn_is_recorded_under_the_route_that_served_it ... ok
test a_remote_turn_is_still_recorded_under_its_own_provider_and_costs_money ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
```

### (b) known-negative — GENUINELY FAILS

Controlled: one worktree, the fix's condition disabled in place
(`if wcore_types::model_aliases::is_local_model(&model)` →
`if false && wcore_types::model_aliases::is_local_model(&model)`), rebuilt in
place, then restored in place and re-run. Verbatim:

```
running 4 tests
test a_near_miss_prefix_is_not_treated_as_local ... ok
test local_model_carries_the_free_cost_rows_not_the_cloud_family_rate ... FAILED
test local_model_is_attributed_to_the_local_route_not_the_configured_profile ... FAILED
test remote_model_still_carries_its_own_provider_and_real_rates ... ok

failures:

---- local_model_carries_the_free_cost_rows_not_the_cloud_family_rate stdout ----

thread 'local_model_carries_the_free_cost_rows_not_the_cloud_family_rate' (3244866) panicked at crates/wcore-config/tests/local_model_cost_attribution_test.rs:93:5:
assertion `left == right` failed: a local model must not inherit the cloud provider's input rate
  left: Some(1.5e-5)
 right: Some(0.0)
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- local_model_is_attributed_to_the_local_route_not_the_configured_profile stdout ----

thread 'local_model_is_attributed_to_the_local_route_not_the_configured_profile' (3244867) panicked at crates/wcore-config/tests/local_model_cost_attribution_test.rs:71:5:
assertion `left == right` failed: the `ollama:` route serves this turn, so every cost surface keyed on compat.provider_type() must say so. Got `anthropic` — that is the configured compatibility profile, not the provider that ran the turn.
  left: "anthropic"
 right: "ollama"


failures:
    local_model_carries_the_free_cost_rows_not_the_cloud_family_rate
    local_model_is_attributed_to_the_local_route_not_the_configured_profile

test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

error: test failed, to rerun pass `-p wcore-config --test local_model_cost_attribution_test`
```

`Some(1.5e-5)` is **$15 per Mtok — Anthropic's input rate, applied to a model
running on local hardware for nothing.** That is the money, in the failure text.

And end to end through a real `AgentEngine::run()` to the ledger JSON on disk:

```
running 2 tests
test a_remote_turn_is_still_recorded_under_its_own_provider_and_costs_money ... ok
test a_local_turn_is_recorded_under_the_route_that_served_it ... FAILED

failures:

---- a_local_turn_is_recorded_under_the_route_that_served_it stdout ----
* done
thread 'a_local_turn_is_recorded_under_the_route_that_served_it' (3260572) panicked at crates/wcore-agent/tests/local_route_cost_attribution_test.rs:158:5:
assertion `left == right` failed: the ledger row must name the route that served the turn. `anthropic` is the configured compatibility profile — the operator reading this ledger would attribute local inference to a cloud vendor, and the budget path reads the same value.
  left: "anthropic"
 right: "ollama"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    a_local_turn_is_recorded_under_the_route_that_served_it

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s

error: test failed, to rerun pass `-p wcore-agent --test local_route_cost_attribution_test`
```

**Both controls stayed green in both runs.** A build that simply stamped
`ollama` (or zero rates) everywhere would have failed
`remote_model_still_carries_its_own_provider_and_real_rates` and
`a_remote_turn_is_still_recorded_under_its_own_provider_and_costs_money`. The
suite can fail in both directions.

### (c) the OLD SHAPE would have missed this — run at the same unfixed commit

Not argued, executed. With the fix disabled:

```
--- wcore-config local_model_no_credential_test ---
running 3 tests
test only_the_exact_prefix_counts_as_local ... ok
test local_model_resolves_without_any_credential ... ok
test remote_model_without_credential_still_refuses ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

--- wcore-observability cost_estimate ---
running 9 tests
test cost_anthropic_preset_smoke ... ok
test cost_bedrock_preset_smoke ... ok
test cost_includes_cache_when_set ... ok
test cost_ollama_preset_is_zero ... ok
test cost_openai_preset_is_zero ... ok
test cost_partial_rows_charge_cache_at_the_input_rate ... ok
test cost_uses_input_and_output_rows ... ok
test cost_vertex_preset_smoke ... ok
test cost_zero_when_compat_has_no_rows ... ok
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Twelve green tests over the broken code, and the two nearest misses are exact:

* `local_model_resolves_without_any_credential` resolves the **same
  `ollama:`-prefixed `Config`** that carried the wrong label, and asserts on
  `cfg.model` and `cfg.api_key` — never on `cfg.compat`.
* `cost_ollama_preset_is_zero` proves `ollama_defaults()` prices to zero — while
  constructing that preset **by hand**, which is the only way it was ever
  constructed. It guarded a preset nothing selected.

That is why the new assertions had to be made against a `Config` that
`Config::resolve` actually produced.

---

## §4 — LIVE: the real binary, a real local model, the operator's own surface

`hetzner-dsm` runs a live ollama at `localhost:11434` carrying **`smollm2:135m`
— the exact model the original C4 measurement billed $0.0756.** So the revert
arm of this A/B costs nothing to run, and was run.

Harness: `live/run.sh` (retained). One worktree, one condition flipped, rebuilt
in place each time, `WAYLAND_HOME` per-arm and lane-namespaced under
`/root/lane-cost-provider-live` (LANE-BRIEF §6a-ii — `/tmp` is shared and was
not used). Config pins `provider = "anthropic"` **deliberately**: that is the
configured profile the ledger used to record, and hetzner injects a real
`ANTHROPIC_API_KEY` anyway (§3b-ii), so nothing but the router forces the local
route. Every figure below is read from `wayland-core cache show` /
`cache report` — the operator-reachable surface — not from an internal probe.

| run | `BIN_SHA` (sha256, first 16) | ledger `provider` | `cost_usd` | `cache verify` |
|---|---|---|---|---|
| FIXED | `43379f732ece342c` | **ollama** | **0.000000** | 7 |
| REVERT | `ad0171983f03c184` | **anthropic** | **0.018840** | 7 |
| RESTORE | `43379f732ece342c` | **ollama** | **0.000000** | 7 |

**The binary identity is measured, not assumed.** RESTORE's binary is
byte-identical to FIXED's and REVERT's differs — the control that never moves.
The C4-LIVE lane's incident (two unverified binaries, both deleted before anyone
could check them) is the reason this column exists.

Verbatim ledger rows:

```
FIXED    F23_CACHE=turn round_trip=1 turn=0 provider=ollama    model=ollama:smollm2:135m … cost_usd=0.000000 cost_source=provider_defaults
REVERT   F23_CACHE=turn round_trip=1 turn=0 provider=anthropic model=ollama:smollm2:135m … cost_usd=0.018840 cost_source=provider_defaults
RESTORE  F23_CACHE=turn round_trip=1 turn=0 provider=ollama    model=ollama:smollm2:135m … cost_usd=0.000000 cost_source=provider_defaults
```

`REVERT-report.txt`, in full on the cost line:

```
F23_CACHE=cost usd=0.018840 uncached_equivalent_usd=0.018840 saving_usd=0.000000 saving_ratio=0.0000 cost_truth=estimated catalog_priced_round_trips=0 estimated_round_trips=1 unpriced_round_trips=0
F23_CACHE=cost_warning text=usd_is_a_family_rate_estimate_not_spend cost_truth=estimated
```

**$0.018840 charged for one 1126-token turn that ran on this machine's own GPU
for nothing.** The C4-F1 money bug, reproduced live at the current tip, and
closed by the fix in the adjacent arm.

### §4a — provider read back from the product's own output (§3b-ii), and a dead instrument caught

The engine's own `W7: wcore-pricing model is unresolvable` line prints the
pricing key it used. First extraction attempt returned **zero matches on a file
that visibly contained the lines** — the tracing output interleaves ANSI escapes
between `provider` and `=`, so `grep 'provider="ollama"'` cannot match. That is
an absence produced by a broken instrument (§3b-i); it was caught only because a
known-positive was available. Repaired by stripping escapes first, then proved on
the known-positive before being trusted:

```
instrument check:  grep -c 'provider="ollama"' FIXED-session.err   → 0   (DEAD)
repaired:          sed 's/\x1b\[[0-9;]*m//g' … | grep -o 'provider="[a-z-]*" model="[^"]*"'
  FIXED   →  8  provider="ollama"    model="ollama:smollm2:135m"
  REVERT  →  8  provider="anthropic" model="ollama:smollm2:135m"
```

Both the engine's internal pricing key and the operator-facing ledger agree, on
both arms. The selection was not inferred from what was exported.

---

## §5 — gates, at commit `bce323a2` on `hetzner-dsm`

Every count read back with `0 ignored` / `0 filtered out` present.

```
cargo test -p wcore-config                                       567 passed; 0 failed; 0 ignored; 0 filtered out  (+13 further binaries, all ok)
cargo test -p wcore-config --test local_model_cost_attribution_test   4 passed; 0 failed; 0 ignored; 0 filtered out
cargo test -p wcore-agent  --test local_route_cost_attribution_test   2 passed; 0 failed; 0 ignored; 0 filtered out
cargo test -p wcore-agent  --test cache_ledger_engine_test            6 passed; 0 failed; 0 ignored; 0 filtered out
cargo test -p wcore-agent  --test turn_trace_shape                    3 passed; 0 failed; 0 ignored; 0 filtered out
cargo test -p wcore-agent  --test ollama_e2e_test                     4 passed; 0 failed; 1 ignored; 0 filtered out   (the 1 ignored is pre-existing)
cargo test -p wcore-cli    --test cache_ledger_cli                   13 passed; 0 failed; 0 ignored; 0 filtered out
cargo test -p wcore-observability --test cost_estimate                9 passed; 0 failed; 0 ignored; 0 filtered out
cargo check --workspace --all-targets                            Finished dev profile in 1m 25s; 0 lines matching ^error
cargo fmt --all -- --check                                       clean (run on the Mac, which is permitted)
```

`cargo clippy -p wcore-config -p wcore-agent --all-targets` → **one warning, and
it is not mine**: `needless_update` at
`crates/wcore-agent/tests/cache_ledger_engine_test.rs:82`, a file this lane did
not touch (working tree was `git status --porcelain`-empty at the time of the
run). Named, not fixed — LANE-BRIEF §6 / AGENTS.md §3 scope discipline.
