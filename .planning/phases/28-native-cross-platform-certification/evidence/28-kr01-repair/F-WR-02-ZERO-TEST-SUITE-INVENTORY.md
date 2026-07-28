# F-WR-02 — inventory of suites that can report success on zero tests

## The detector for this class had this class's disease

Recorded first, because it is the strongest argument for the finding. The first version
of this generator reported `live_integrity.rs` as **6 / 6 ignored** when its guard test is
deliberately NOT ignored. Cause: it tested `'#[ignore' in attrs` against a block that
included **doc-comment prose**, and that doc comment *describes* the defect using the
literal text `` `#[ignore]`d ``. So the instrument built to detect coverage-from-nothing
was itself reporting coverage **from prose rather than from code**.

Fixed by anchoring attribute matching on `^\s*#\[` and never collecting comment lines
into the attribute block. The numbers below come from the corrected parser.

## How the two cases were told apart

A suite with **some** `#[ignore]`d tests is normal and is NOT counted — the runner still
executes the rest, so it cannot report `ok` on zero work. Only a suite where **every**
test carries `#[ignore]` is the live defect: `cargo test --test <name>` then executes
nothing and still exits 0 printing `test result: ok`.

- suites with SOME ignored tests (normal, excluded): **12**
- suites where EVERY test is ignored (**the defect**): **15**

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
| 1 / 1 | `crates/wcore-sandbox/tests/live_integrity_macos.rs` |

**15 binaries can report success on zero executed tests.**

## Flavour B — env-gated early `return` (STRICTLY WORSE)

The test runs, executes nothing, and reports **`passed`** — an affirmative green, rather
than a visible `ignored` count a reader might question. This was `live_integrity.rs`,
which printed `5 passed` for zero work; it is the suite `KR-01` lives in, and this lane
converted all 5 to honest `#[ignore]` plus an asserting gate.

Remaining candidates of the same shape elsewhere (reported, NOT fixed — each needs
checking against its own gate before being called a defect):

| env-gated bodies | test binary |
|---|---|
| 1 | `crates/wcore-cli/tests/smoke_p0.rs` |
| 1 | `crates/wcore-eval-scenarios/tests/cross_session_live.rs` |
| 1 | `crates/wcore-eval-scenarios/tests/live_personas.rs` |
| 1 | `crates/wcore-sandbox/tests/backend_integration.rs` |
| 1 | `crates/wcore-sandbox/tests/hard_process_containment_macos.rs` |
| 1 | `crates/wcore-sandbox/tests/live_integrity_macos.rs` |

## Call-site exposure, checked rather than assumed

No CI workflow, justfile target or script invokes a fully-ignored suite unsafely.
`scripts/f20-native-windows-proof.ps1` and `scripts/f20-native-macos-proof.sh` already
pass `--run-ignored all --no-tests=fail`, which cannot report success on zero tests. The
real exposure is a human or agent typing the obvious command — unbounded, which is why
the guard belongs in the suite rather than at call sites.

## Status

FIXED: `crates/wcore-sandbox/tests/live_integrity.rs`. Guard demonstrated falsifiable on
hardware: env set + no `--ignored` -> `FAILED. 0 passed; 1 failed; 5 ignored`;
env unset -> `ok. 1 passed; 0 failed; 5 ignored`. It printed `5 passed` before.

NOT converted: all 15 Flavour-A binaries listed above (`live_integrity.rs` is no longer
among them — its always-running guard means it cannot report success on zero executed
tests). The guard is a
~20-line pattern. Claiming they were fixed would be false.
