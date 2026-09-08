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

## Explicit evaluation model settings

`wayland-eval --provider openai --model gpt-6-astra --effort medium
--responses-api --max-tokens 1024` selects the model, reasoning effort, existing
ProviderCompat Responses route, and output-token cap. Omitted options preserve
existing defaults. Effort uses Core's existing `set_config` protocol before each
turn. This bounded surface supports `low`, `medium`, and `high` on OpenAI;
unsupported combinations and conflicting scenario model/effort commands fail
before execution. It does not import a developer profile into the isolated home.

`--max-tokens` caps output; it does not enlarge the scenario's time, step, or USD
budget. Each scenario retains its hard `max_total_cost_usd` and post-run cost
oracle. `--budget 0.25` remains the whole-invocation admission ceiling, not a
replacement for a scenario's declared budget. A task whose declared cost bound
exceeds the permitted trial budget requires a scope decision before execution.

For repeated trials, call the existing runner once per trial and give each call
its own `--report-dir "$evidence_root/trial-$trial_index"` and
`--output "$evidence_root/trial-$trial_index.status"`. Repeat `--scenario` only to
select distinct tasks: duplicate IDs are deduplicated, not repeated trials.
Always supply `--binary` and `--expected-source-commit` for the agreed candidate.
The loopback-only test
`packaged_explicit_model_effort_and_responses_reach_wire` checks the actual
request model, effort, route, and token cap; it does not establish live-provider
availability or benchmark readiness.

## Prepared paired task bridges (W16)

`--paired-task task.json` selects one of the eight `w16_*` family IDs from
the prepared task, preserving its prompts, seed, fixture files, whole-task cost
bound and deadline. It is exclusive with ordinary catalogue selectors. The
existing 36-scenario catalogue and default process-spawn behavior are unchanged.

```text
wayland-eval --paired-task task.json --list
wayland-eval --paired-task task.json --prepare-paired-peer peer
wayland-eval --paired-task task.json --verify-paired-artifacts peer/workspace --peer-final peer/final.txt
wayland-eval --paired-task task.json --serve-paired-effects effect-service
```

Preparation makes identical content-addressed inputs and prompt files for the
reference CLI. Artifact verification executes the same protected-file checks
and independent Python regression tests under evaluator containment. It is
explicitly an artifact result, not proof of complete peer execution or cost.
The effect service is needed only for skill discovery and interrupted recovery;
its `ready.json` gives the native MCP URL and supervisor-only durable-effect
barrier. Stop it with SIGINT after reaping the peer; its journal counts repeated
effects instead of hiding them behind idempotency.

Core execution uses the normal binary/source pin, provider, model, effort,
Responses, output cap, budget and report options with `--paired-task`. The task
itself is the receipt's fixture digest. Output retains each session result,
the task, final workspace and authoritative effect journal. Paid paired calls
remain refused until the separate shared spend-admission prerequisite is
verified; explicit known-free loopback fixture controls can run now.

A successful local diagnostic outcome does not imply authoritative cleanup.
Unavailable containment/orphan evidence remains unavailable and blocks receipt
certification. Actual runner cleanup failures remain failed outcomes with their
existing typed failure codes; recovery still requires authoritative cleanup
before resuming an interrupted session.

Memory drives a clean-home negative, store, and cold recall in three actual
processes. Their native session IDs must differ. Recovery creates a fixed
session ID, cuts its owned process tree only after the external effect journal
is synced, and passes that same ID to native `--resume`. A quarantine/refusal
is recorded as a failed/incomplete task, never automatically a success. Unknown
interrupted usage is charged conservatively at its admitted bound and labeled
as such, rather than quietly treating the cut as free.

Long-session tasks retain at least eight substantive prompts and a substantial
repository corpus within the existing 4-MiB fixture limit. Completion additionally
requires an actual `compact_offload` event with measured reclaimed tokens;
a small canary or a conversation without compaction cannot satisfy that family.
The configured model window and compaction thresholds are not reduced to force
an inexpensive pass. A declared task bound above the available trial budget is
refused by normal admission; the memory control's three-session bound must not
be evaded by dropping its negative control.
