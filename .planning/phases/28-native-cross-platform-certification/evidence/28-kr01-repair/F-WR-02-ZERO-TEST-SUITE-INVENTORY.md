# F-WR-02 — inventory of suites that can report success on zero tests

Generated from source. **Counts corrected**: the first generator walked test attributes
back across blank lines, so a preceding test's `#[ignore]` bled onto the next item and
inflated the ignored count — it wrongly reported `live_integrity.rs` as 6/6 ignored when
its guard is deliberately NOT ignored. Recorded because an inventory that miscounts is
the same defect class this finding is about.

## Flavour A — every test `#[ignore]`d

`cargo test --test <name>` runs ZERO tests and exits 0 printing `test result: ok`.

| ignored / total | test binary |
|---|---|
| 5 / 5 | `crates/wcore-agent/tests/actor_acl_test.rs` |
| 1 / 1 | `crates/wcore-agent/tests/tool_token_bench_smoke.rs` |
| 2 / 2 | `crates/wcore-cli/tests/acp_engine_turn.rs` |
| 1 / 1 | `crates/wcore-eval/tests/acceptance_gate.rs` |
| 1 / 1 | `crates/wcore-eval-scenarios/tests/cross_session_live.rs` |
| 1 / 1 | `crates/wcore-eval-scenarios/tests/live_personas.rs` |
| 1 / 1 | `crates/wcore-eval-scenarios/tests/pty_tui_smoke.rs` |
| 1 / 1 | `crates/wcore-exec-backend/tests/live_equivalence.rs` |
| 2 / 2 | `crates/wcore-memory/tests/bge_local_real.rs` |
| 2 / 2 | `crates/wcore-memory/tests/hybrid_retriever_perf_test.rs` |
| 1 / 1 | `crates/wcore-observability/tests/otlp_local_test.rs` |
| 1 / 1 | `crates/wcore-sandbox/tests/hard_process_containment_macos.rs` |
| 6 / 6 | `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` |
| 12 / 12 | `crates/wcore-sandbox/tests/live_fs_acl.rs` |
| 6 / 6 | `crates/wcore-sandbox/tests/live_integrity.rs` |
| 1 / 1 | `crates/wcore-sandbox/tests/live_integrity_macos.rs` |

**16 binaries.**

## Flavour B — env-gated early `return` (STRICTLY WORSE)

The test runs, executes nothing, and reports **`passed`** — an affirmative green, rather
than a visible `ignored` count a reader might question.

| tests | test binary |
|---|---|
| 1 | `crates/wayland-honcho/tests/live_test.rs` |
| 1 | `crates/wcore-cli/tests/smoke_p0.rs` |
| 1 | `crates/wcore-eval-scenarios/tests/cross_session_live.rs` |
| 1 | `crates/wcore-eval-scenarios/tests/live_personas.rs` |
| 2 | `crates/wcore-eval-scenarios/tests/runner_contracts.rs` |
| 1 | `crates/wcore-sandbox/tests/backend_integration.rs` |
| 1 | `crates/wcore-sandbox/tests/hard_process_containment_macos.rs` |
| 1 | `crates/wcore-sandbox/tests/live_integrity_macos.rs` |
