# wcore-eval-scenarios

Scenario-level eval harness for `wayland-core`. Drives the real shipped binary against a real LLM API through a real tool chain and asserts the OUTCOME — not just that the tools ran.

Plan: [`.blackboard/EVAL-HARNESS-PLAN-2026-05-23.md`](../../.blackboard/EVAL-HARNESS-PLAN-2026-05-23.md) (v2, post-audit).

## Status

The crate ships the runner core, 36 standard single-session scenarios, and a canonical deterministic catalog. `wayland-eval --list`, exact `--scenario`, and substring `--filter` selection are live; scenario execution and complete reporting remain fail-closed until the later F01 slices.

What ships now:

- **Crate and catalog** — public API types (`Scenario`, `Turn`, `Assertion`, `TraceAssertion`, `ScenarioResult`, `Failure`, `ProviderChoice`, `Category`), workspace wiring, `[profile.eval]` nextest profile, and deterministic CLI catalog selection.
- **Runner core** — spawn `wayland-core --json-stream`, drive per-turn via `message` / `stream_end` events, capture stderr to a 50-line ring buffer, parse `session_cost` for USD totals, enforce wall-time with `kill_on_drop(true)` + explicit `start_kill()` on `Elapsed` (per cross-audit M-1).
- **`tempenv`** — hermetic per-scenario `TempDir` + seeded `<tempdir>/.wayland-core/config.toml` with an **absolute** `[session].directory` (per C-3 — relative defaults leak into cwd) and the per-provider API key.
- **`stderr_capture`** — ring-buffered stderr drain for failure dumps (per M-9 — D1 panic regressions need stderr or root cause is lost).
- **Smoke tests** — `tests/smoke.rs` exercises spawn plumbing + `kill_on_drop` hygiene without any API calls.

What is stubbed (types declared, behaviour in later waves — bodies return
honest sentinels, not `todo!()`, so the crate-level `#![deny(clippy::todo)]`
gate in `lib.rs` stays green and rules out silent-pass regressions):

- **T3** — `assertions.rs` + `trace.rs` (`Assertion::check` / `TraceAssertion::check` / `ToolTrace::parse_session`).
- **T4** — `providers::resolve(ProviderChoice)` matrix + strict-mode SKIP/FAIL.
- **F01/F03** — real scenario execution plus complete console, Markdown, and versioned machine-readable reports.
- **F04** — deterministic provider/MCP fixtures and PTY harness reuse.

## Quickstart (T2-era — runner is callable; assertions don't fire yet)

Pre-build the binary the runner discovers (needed unless `WCORE_EVAL_BIN` is set):

```bash
cargo build -p wcore-cli
```

Then run the scaffold's unit + smoke tests:

```bash
cargo build   -p wcore-eval-scenarios
cargo clippy  -p wcore-eval-scenarios --all-targets --no-deps -- -D warnings
cargo fmt     -p wcore-eval-scenarios -- --check
cargo test    -p wcore-eval-scenarios
```

All four gates green. **No API calls** are made — the smoke tests only exercise process plumbing.

## Cost notes (full harness — T5+)

Per the plan §4.2 (audit H-9 refresh):

| Mode | Scope | Estimate |
|---|---|---|
| `just eval-fast` | 36 scenarios × DeepSeek only | ~$0.30 |
| `just eval` | 36 scenarios × current default | ~$0.30 (DS) or ~$8 (Claude) |
| `just eval-matrix` | 36 × 3 providers × `--strict` | ~$25-40 |

Per-scenario hard ceiling enforced by the engine's `[budget] max_cost_usd` block (seeded into the per-run `config.toml` by `tempenv`).

## Provider setup (T4+)

Env vars consumed at runtime:

| Provider | Env var | Default model |
|---|---|---|
| DeepSeek | `DEEPSEEK_API_KEY` | `deepseek-chat` |
| Anthropic | `ANTHROPIC_API_KEY` | `claude-sonnet-4-6` |
| OpenAI | `OPENAI_API_KEY` | `gpt-4o` |

The engine's `default_model_for(DeepSeek)` returns the empty string — so the runner ALWAYS passes `--model` explicitly (per cross-audit H-5). Don't rely on engine defaults.

## `--strict` semantics (T5)

Default (lenient): a scenario whose required provider has no API key is **SKIP**ed — fine for local iteration.

`--strict`: missing API keys become **FAIL** — required by `just eval-matrix` so tag-time runs cannot silently skip the Claude or OpenAI safety net.

## How to add a scenario (T3+)

```rust
use std::time::Duration;
use wcore_eval_scenarios::{Scenario, Turn, Category, Assertion, TraceAssertion};

#[tokio::test]
async fn s11_github_trending() {
    Scenario::new("s11_github_trending", Category::Research)
        .turn(
            Turn::new("What are the top 10 trending GitHub repos this week?")
                .max_time(Duration::from_secs(60))
                .max_steps(8)
                .expect_tool("WebFetch")
                .forbid_tool("Browser") // H-7 — the 35-min hang regression test
                .assert(Assertion::Contains("github.com/"))
                .trace(TraceAssertion::NoErrorsOnTool("WebFetch")),
        )
        .max_total_time(Duration::from_secs(90))
        .max_total_cost_usd(0.10)
        .run_with(&provider_default())
        .await
        .unwrap();
}
```

## Wire-format note

The plan referenced `{"type":"user_message","text":"..."}` for sending user input. That is wrong — the actual `ProtocolCommand::Message` variant is `{"type":"message","msg_id":"...","content":"..."}` (per `crates/wcore-protocol/src/commands.rs`). The runner uses the correct shape.
