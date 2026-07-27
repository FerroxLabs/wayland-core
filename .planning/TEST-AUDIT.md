# TEST-AUDIT — wayland-core test suite, one-shot read-only audit

**Scope**: `plan/f20-unified-audit-repair` @ `70ccd708` (code-identical to `01a5b0ae` — `git diff --stat 01a5b0ae 70ccd708 -- crates/ .github/ justfile .config/` is empty; `70ccd708` is a docs-only commit).
**Green baseline**: `.planning/phases/20-transactional-delegated-mutation/20-56-evidence/test-01a5b0ae-GREEN.log.gz` — `Summary [ 194.331s] 11519 tests run: 11519 passed (1 slow, 3 flaky), 48 skipped`, nextest profile `ci`, 469 binaries, Hetzner 96-core Linux.
**Mac cannot compile this workspace** — no cargo was run on the Mac. All timings come from the captured Linux log; all gate data is static source analysis.

---

## 1. NEVER-EXECUTING TESTS

### 1.0 What CI actually runs (the baseline every finding below is measured against)

| Workflow | Trigger | OS | Test command | Ignored tests? | Non-default features? |
|---|---|---|---|---|---|
| `ci.yml` job `ci` | `pull_request → main`, `push → main` | `macos-latest` + `[self-hosted, Windows, X64, msvc]` (`ci.yml:36-46`) | `vx just test-ci` → `cargo nextest run --workspace --profile ci --no-fail-fast` (`justfile:34-35`) | **No** | **No** |
| `ci.yml` job `ci-linux` | same | `ubuntu-latest` (containerized) | same nextest invocation (`ci.yml:278`) | **No** | **No** |
| `ci.yml` eval-gate step | same | matrix + dedicated Linux job | `nextest -p wcore-eval --features acceptance-gate ... --run-ignored only` (`ci.yml:169`, `ci.yml:340`) | **1 named test only** | `acceptance-gate` |
| `ci.yml` F01 driver gate | same | matrix + Linux | `cargo test -p wcore-eval-scenarios --features packaged-driver-gate` (`ci.yml:147`) | No | `packaged-driver-gate` |
| `ci.yml` browser-live | same | `ubuntu-latest` | `--features browser-live-tests --test chromium_live_test` (`ci.yml:482`) | No | `browser-live-tests` |
| `nightly-windows-soak.yml` job `soak` | `cron 30 5 * * *` (⇒ **`main` only**) | `windows-2022` | `scripts/wayland-e2e-windows-soak.ps1` → `cargo nextest run -p wcore-cron -p wcore-config -p wcore-providers -p wcore-tools -p wcore-swarm` (`scripts/wayland-e2e-windows-soak.ps1:146-151`) | No | No |
| `nightly-windows-soak.yml` jobs `f20-windows-candidate` / `f20-macos-candidate` | `workflow_dispatch` **only**, gated on `f20_candidate == 'true'` + a Sean-supplied nonce | self-hosted msvc / ephemeral macOS | `scripts/f20-native-windows-proof.ps1:168` and `scripts/f20-native-macos-proof.sh:259` — `nextest run --run-ignored all` over **11 hardcoded target selectors** | **Yes, 11 selectors** | `live-docker` (macOS side) |
| `e2e.yml`, `mutants-nightly.yml`, `bench-regression.yml`, `osv-scan.yml` | dispatch / cron | — | do not run the workspace suite | No | — |

`--run-ignored` appears in exactly three places repo-wide (`/usr/bin/grep -rn 'run-ignored' scripts/ .github/ justfile`): `ci.yml:169`, `ci.yml:340`, `justfile:160` (all the same single eval test), plus `--run-ignored all` in the two manual F20 proof scripts.

### 1.1 **This branch has never been through CI at all.**

```
gh run list -R FerroxLabs/wayland-core --branch plan/f20-unified-audit-repair --limit 20 --json ... → []
git ls-remote gh plan/f20-unified-audit-repair → 70ccd708… refs/heads/plan/f20-unified-audit-repair
```

The branch **is** pushed, but `ci.yml` only fires on `pull_request → main` / `push → main`, and no PR exists. The last `ci.yml` run of any kind was **2026-07-13**; the most recent is `action_required` on a different branch (2026-07-23). Therefore:

- The macOS CI job has **never** compiled or run this tree.
- The Windows self-hosted CI job has **never** compiled or run this tree.
- **All 155 Windows-only tests and all 23 macOS-only tests in this audit have zero execution evidence at `01a5b0ae`.** The 11,519 green results are a Linux-only artifact.

The Windows host `SeanD@seandesktop:C:\ferrox-win` is at `ce9a11a6` (`docs(20-75): record the native Windows closeout and its two blockers`) — a **different commit** from the audited tree, so it is not standing evidence for `01a5b0ae` either.

### 1.2 Gate census (static, whole workspace)

11,816 test functions found in `crates/**/*.rs` (attribute scan for `#[test]` / `#[tokio::test]` / `#[rstest]`; the Linux run executed 11,519 + 48 skipped = 11,567, the ~249 delta being the Windows/macOS-only bodies that do not compile on Linux).

| Gate | Test fns | Where it actually runs |
|---|---|---|
| `cfg(unix)` / `cfg(all(test, unix))` | 231 + 10 | Linux CI + macOS CI. Never Windows. |
| `cfg(target_os = "linux")` | 72 + 6 | Linux CI only. |
| `cfg(windows)` / `cfg(all(test, windows))` / appcontainer subtree | **155 total Windows-only** | Windows self-hosted CI job **only** — which has never run this branch. |
| `cfg(target_os = "macos")` | 23 | macOS CI only. |
| `#[ignore]` | **86** | 1 runs (eval gate); ~11 reachable only by manual dispatch; the rest run nowhere. |
| non-default, non-CI feature gates | **60** | **Nowhere.** Never compiled in any workflow. |

### 1.3 CRITICAL SUBSET — tests that run on NO platform in normal CI

Ranked by impact.

| # | Test set | Evidence | Where it runs | Verdict |
|---|---|---|---|---|
| 1 | **105 Windows-only tests in `wcore-sandbox`** | `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/tests.rs` (41), `crates/wcore-sandbox/src/directory_authority_windows_tests.rs` (22, mounted `#[cfg(all(test, windows))]` at `crates/wcore-sandbox/src/directory_authority.rs:1269-1271`), `crates/wcore-sandbox/tests/live_fs_acl.rs` (12), `.../acl_lease/tests.rs` (8), `.../acl_lease/storage.rs` (6), `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` (6), `crates/wcore-sandbox/tests/live_integrity.rs` (5), `.../acl_lease/mutation_lock.rs` (2) | Windows CI job (never ran this branch) — and `wcore-sandbox` is **not in the nightly soak's 5-crate list**, so the recurring Windows automation never touches it either. 34 of them are additionally `#[ignore]`d. | **FIX** — this is the exact rot the prompt names. The retained-handle security proofs have no recurring execution path on any branch. Add `wcore-sandbox` to `wayland-e2e-windows-soak.ps1:146-151`. |
| 2 | **86 `#[ignore]` tests, of which 85 run in no automatic workflow** | full list §1.5. Only `acceptance_gate_meets_precision_recall_threshold` (`crates/wcore-eval/tests/acceptance_gate.rs:24`) is rescued, by `ci.yml:169`. | ~11 more are reachable only through the Sean-nonce `f20_candidate` dispatch. The other ~74 run only if a human types `--ignored` locally. | **FIX** for the security-bearing ones (§1.4); **KEEP** for the genuinely-live ones (real API keys, real Docker, real kubectl) which are correctly opt-out. |
| 3 | **60 tests behind non-default features never enabled anywhere in CI** | `live-docker` 17, `bge-local` 11, `live-openai` 9, `landlock` 4, `seccomp` 4, `live-anthropic` 2, `harness-failure-injection` 2, `voice` 2, `browserbase` 2, `chromium` 2, `live-gemini`/`network-tests`/`live-honcho`/`otlp`/`live-voyage` 1 each | Nothing in `.github/workflows/*.yml` passes `--all-features`; the only `--features` flags are `test-utils`, `packaged-driver-gate`, `acceptance-gate`, `browser-live-tests`. | **FIX** for `landlock` (4) + `seccomp` (4) — these gate Linux sandbox *hardening* on the one platform CI does run well, and are free to enable. **KEEP** for the `live-*` families (require credentials). |
| 4 | **`wcore-sandbox` `live_fs_acl.rs` — 12 Windows-only `#[ignore]`d ACL tests, only 2 named in any runner** | `crates/wcore-sandbox/tests/live_fs_acl.rs:40,167,192,257,310,358,407,450,497,543,585,637` | `scripts/f20-native-windows-proof.ps1:83-84` names exactly `one_execution_grant_never_leaks_to_another_identity` and `granted_path_is_readable_then_revoked`. **The other 10 run nowhere at all** — including `deny_ace_still_blocks_granted_read`, `normal_sid_only_grant_is_denied`, `unrelated_acl_survives_exact_sid_cleanup`. | **FIX** — these are the ACL-boundary proofs; triple-gated (`#![cfg(windows)]` + `#[ignore]` + not-in-selector). |
| 5 | **`hard_process_containment_windows.rs` — `native_containment_gate_marker`** | `crates/wcore-sandbox/tests/hard_process_containment_windows.rs:434` | The other 5 ignored tests in the file are named in `f20-native-windows-proof.ps1:85,87`; this marker is not. | **FIX** or **DELETE** — a gate marker that no gate selects is dead. |
| 6 | **`wcore-agent/tests/actor_acl_test.rs` — 5 `#[ignore]`d ACL policy tests with no platform gate** | `:170`, `:181`, `:191`, `:202`, `:213` (`root_actor_bypasses_deny_policy`, `sub_agent_with_deny_policy_short_circuits`, …) | Nowhere. Not platform-gated, not feature-gated, not in any `--run-ignored` selector. Named in the Linux SKIP list. | **FIX** — these are authorization tests (deny-policy short-circuit). An `#[ignore]` with no runner is an untested security control. |
| 7 | **`wcore-permissions/tests/threat_model_coverage.rs:26` `t1_plugin_manifest_cannot_claim_system_actor`** | single `#[ignore]`, no gate | Nowhere. | **FIX** — a threat-model coverage test that never runs provides zero coverage. |
| 8 | **41 Windows-only tests in `appcontainer/windows_impl/tests.rs`** | mounted under `#[cfg(windows)] mod windows_impl` (`crates/wcore-sandbox/src/backends/appcontainer.rs:416-417`), inner `#[cfg(test)]` at `:425` | Windows CI only; 2 of them (`echo_runs_live:377`, `large_output_survives_live:417`) are additionally `#[ignore]`d. | **FIX** — largest single Windows-only block; no recurring runner. |
| 9 | **22 Windows-only retained-handle tests in `directory_authority_windows_tests.rs`** | mounted `#[cfg(all(test, windows))]` `#[path=...]` at `directory_authority.rs:1269`; its unix twin `directory_authority_tests.rs` is `#[cfg(all(test, unix))]` at `:1260` | Windows CI only. The mutually-exclusive mount means **no single CI leg ever compiles both**, so a change to shared `directory_authority` internals is only half-proven per platform. | **FIX** — this mount pattern is structurally why the wcore-sandbox breakage stayed invisible. |
| 10 | **`wcore-memory` 9 + `wcore-skills` 7 + `wcore-mcp` 4 + `wcore-agent` 11 + `wcore-plugin-subprocess` 2 + `wcore-cli` 2 Windows-only tests** | e.g. `crates/wcore-skills/src/bundled/bundled_tests.rs:683,728,802,837,866,914,930`; `crates/wcore-plugin-subprocess/src/runner.rs:1255,1293`; `crates/wcore-agent/src/session_journal/snapshot.rs:1301,1314,1331,1341`; `crates/wcore-mcp/src/transport/stdio.rs:1328,1388` | Windows CI only — none of these crates are in the nightly soak's 5. | **FIX** — same structural gap, lower blast radius. |
| 11 | **`wcore-sandbox/tests/backend_integration.rs:303` `sandbox_exec_execute_echo_returns_exit_zero`, gated `#[cfg(not(target_os = "macos"))]` + `#[ignore]`** | `sandbox-exec` is *the macOS backend*. The test is gated **off** on macOS. | Nowhere (also `#[ignore]`d). | **DELETE or FIX** — this is a gate inversion: a macOS-backend smoke test excluded from macOS. It cannot prove anything where it is allowed to run. |
| 12 | **`wcore-agent/src/plugins/adapters/cua_adapter.rs:174` `reifies_real_cua_tool_on_non_linux`** | gated `#[cfg(any(target_os = "macos", target_os = "windows"))]` | macOS CI + Windows CI. | **KEEP** — name and gate agree; flagged only because the mechanical inversion scan surfaced it. |

### 1.4 Gate-inversion scan result

A mechanical scan for "platform-specific behavior tested on the platform where it is least meaningful" (test name mentions one OS, gate names another) returned **zero** name/gate inversions in the Windows direction and **one** benign hit (#12 above). The inversions in this codebase are structural rather than nominal:

- **#11** (`sandbox_exec` excluded from macOS) is a real inversion — found by semantics, not by name matching.
- The known `#[cfg(unix)]`-on-the-heartbeat-status-probe case cited in the brief is of the same class: `crates/wcore-swarm/src/worktree_tests.rs` mounts `crates/wcore-swarm/src/worktree_tests/linux.rs` under `#[cfg(target_os = "linux")]`, leaving only 2 Windows-only tests in that crate.
- The larger, quieter inversion is **#9**: mutually-exclusive `#[cfg(all(test, unix))]` / `#[cfg(all(test, windows))]` test mounts over the same production module, so neither CI leg sees the whole proof surface.

Separately, ~30 `windows_*`-named tests in `wcore-config` run on **all** platforms (`crates/wcore-config/src/shell/executable_readiness_tests.rs:181-637`, `crates/wcore-config/src/shell.rs:298,305,322`, `crates/wcore-config/src/shell/mcp_stdio_launch_context.rs:356,389,402`). These are pure-logic PATHEXT/quoting resolvers driven from injected state, so testing them on Linux is legitimate — **KEEP**. Only `native_windows_command_shell_resolves_cmd_and_bat_from_effective_cwd` (`executable_readiness_tests.rs:704`) is `#[cfg(windows)]`, i.e. exactly one of that family touches the real OS.

### 1.5 The 48 skipped tests (named, with reason)

All 48 are `#[ignore]` on Linux. Grouped by why:

**Live external service (11) — KEEP, correctly opt-out:**
`wcore-agent tool_backends::exa_web::tests::live_exa_search_returns_results`, `…firecrawl_web…live_firecrawl_search_returns_results`, `…parallel_web…live_parallel_free_search_returns_results`, `wcore-agent::ollama_e2e_test ollama_live_smoke`, `wcore-agent::web_fetch_backend_test http_fetch_live_real_hostname_resolves_fast`, `wcore-cli::smoke_p0 live_real_key_first_prompt_round_trip`, `wcore-eval-scenarios judge::tests::judge_live_grades_honest_refusal`, `wcore-eval-scenarios::cross_session_live memory_recall_across_sessions`, `wcore-eval-scenarios::live_personas overnight_personas`, `wcore-tools kubectl_tool::tests::execute_version_against_live_kubectl`, `wcore-plugin-wasm::wasm_real_execute real_component_execute_round_trip`.

**Security / authorization tests with no runner (6) — FIX:**
`wcore-agent::actor_acl_test root_actor_bypasses_deny_policy`, `…sub_agent_ask_policy_falls_through_to_approval`, `…sub_agent_with_allow_policy_runs_tool`, `…sub_agent_with_deny_policy_short_circuits`, `…sub_agent_without_policy_runs_tool`, `wcore-permissions::threat_model_coverage t1_plugin_manifest_cannot_claim_system_actor`.

**PTY / TUI, ignored for terminal-dependence (7) — FIX (they are the TUI's only end-to-end proof):**
`wcore-cli tui::surfaces::tests::router_tab_accepts_at_candidate_does_not_switch_tabs_d042`, `wcore-cli::smoke_p0 pty::smoke_15_askuser_card_arrows_and_no_yan_footer`, `…pty::smoke_22_question_mark_shows_help_overlay`, `…pty::smoke_23_at_completion_tab_inserts_candidate`, `wcore-eval-scenarios pty_capture::tests::pty_process_tree_helper`, `wcore-eval-scenarios::pty_tui_smoke tui_boots_and_renders_workspace`, `wcore-skills watcher_tests::diag_basic_event_received` + `…diag_multi_thread_event_received` (fs-watcher timing).

**Named fixtures / helpers, not tests (13) — KEEP:** the `*_fixture` and `*_helper` family is a documented nextest-invoked child-process idiom, not assertions:
`wcore-swarm::dispatch_smoke {heartbeat_symlink,malformed_heartbeat,standalone_authority}_fixture` + `repository_replacement_must_not_execute`, `wcore-swarm::worker_runtime_limits {descendant,flood,many_entry,parent,sleeping}_worker_fixture` + `workspace_growth_fixture`, `wcore-swarm::workspace_authority capacity_registration_fixture`, `wcore-cli::f14_sigkill_recovery f14_seed_recoverable_turn_helper`.

**Perf / size baselines (2) — KEEP (machine-dependent, correctly opt-out):**
`wcore-memory::hybrid_retriever_perf_test hybrid_retriever_perf_p95_under_100ms`, `…binary_size_baseline`.

**Pre-built-binary / heavy-toolchain dependents (5) — KEEP:**
`wcore-agent::tool_token_bench_smoke scripted_run_writes_expected_markdown` (has a dedicated `ci.yml:114` pre-build + `nextest.toml` 180s override, yet still `#[ignore]`d — see note), `wcore-cli::bin/wayland-core tests::signal_shutdown_native_subprocess`, `wcore-cli::acp_engine_turn acp_turn_streams_text_then_done` + `a2a_on_message_routes_task_to_engine`, `wcore-honcho-adapter tests::selector_picks_honcho_when_configured`.

**Platform-backend smokes gated to the wrong/no platform (4) — FIX:**
`wcore-sandbox::backend_integration sandbox_exec_execute_echo_returns_exit_zero` (see #11), `wcore-sandbox::backend_integration appcontainer_execute_trivial_command_returns_exit_zero`, `wcore-skills watcher_tests::*` (listed above), `wcore-observability` OTLP (feature-gated, below).

> Note on `scripted_run_writes_expected_markdown`: `.config/nextest.toml` carries a 180s `[[profile.ci.overrides]]` and `retries = 0` for it, and `ci.yml:114` pre-builds its binary — substantial machinery maintained for a test that is `#[ignore]`d and therefore never selected by `just test-ci`. **FIX or DELETE** the dead override.

The remaining 38 of the 86 `#[ignore]`s do not appear in the Linux SKIP list at all because they are *also* `cfg`'d out on Linux (34 Windows, 3 macOS, 1 feature). Those are the double-gated set in §1.3 #1/#4/#5.

---

## 2. ASSERTION-FREE / TAUTOLOGICAL TESTS

Method: Rust-aware tokenizer (raw strings `r#"…"#`, byte/char literals, block comments handled) over every `#[test]`/`#[tokio::test]`/`#[rstest]`/`#[proptest]` body in `crates/`; a body is assertion-free if it contains none of `assert*`, `expect(`, `unwrap(`, `panic!`, `matches!`, `?;`, `.await?`, `unreachable!`, `.success()`, `.failure()`, and is not `#[should_panic]`. A naive brace matcher first reported 104; the tokenizer removed **20 false positives** where a JSON raw-string literal truncated the body. Numbers below are post-fix.

**11,784 test functions scanned → 84 assertion-free, 15 tautological.** A separate `/usr/bin/grep -rn -E 'assert!\(\s*(true|!false|1 == 1)\s*[,)]'` returned **zero** hits — there is no `assert!(true)` anywhere, and no "assert a value the same line just constructed" pattern.

### 2.1 Assertion-free — ranked by worthlessness

| # | Finding | Evidence | Verdict |
|---|---|---|---|
| 1 | **Group D — 8 self-documented hollow tests whose NAME claims a behavior the body never exercises.** The worst class: the name is load-bearing documentation and it is false. | `crates/wcore-agent/src/skill_tool.rs:819` `tc_11_4_case_sensitive_no_match` — builds a tool, never calls matching, comment says *"Verified via execute in tc_13.x"*. `crates/wcore-eval-scenarios/src/pty_capture.rs:871` `screenshot_png_is_an_honest_stub_not_a_panic` — takes a **function pointer** to `screenshot_png` and never calls it, so it cannot detect a panic. `crates/wcore-plugin-wasm/src/host_adapters/secrets.rs:91` `secret_exists_never_exposes_value` — only type-annotates the return `bool`; a leak elsewhere passes. `crates/wcore-cli/tests/engine_rebind.rs:122` `rebind_rebuilds_provider_from_onboarded_config` — comment: *"reaching this line at all proves…"*; `disk` is never compared to `provider`. `crates/wcore-skills/src/watcher_tests.rs:546` — comment: *"The key assertion: … does not panic"*, and there is no assertion. Plus `crates/wcore-plugin-api/tests/inventory_discovery.rs:74,256`, `crates/wcore-agent/src/output/mod.rs:685`. | **DELETE** — the names actively mislead a reader into believing the behavior is covered. `secrets.rs:91` is the most dangerous: a security-named test with no security assertion. |
| 2 | **Group A — 31 identical provider `constructs_*` no-ops.** Every one is `let p = XProvider::with_defaults(...); let _ = p;`. | `crates/wcore-providers/src/`: `cerebras.rs:105,115`, `deepseek.rs:98,111`, `fireworks.rs:105,125`, `flux_router.rs:103,113`, `groq.rs:98,108`, `http_client.rs:177,183`, `mistral.rs:97,108`, `moonshot.rs:108,118`, `nvidia.rs:101,111`, `openai_compatible.rs:121`, `openrouter.rs:148,159`, `perplexity.rs:103,113`, `qwen.rs:118,128`, `sakana.rs:94,104`, `together.rs:97,107`, `xai.rs:112,123`. `deepseek.rs:104-106` documents its own emptiness: *"We can't reach the private inner base_url; the next test exercises a custom URL"* — the next test is the same shape and also asserts nothing. | **DELETE** — 31 tests proving a constructor does not panic, cloned across 16 files. Under `ProviderCompat` (the codebase's central rule) the thing worth asserting is the resolved `base_url`/`max_tokens_field`, which none of them reach. |
| 3 | **Group B — 11 constructor/compile-only smokes.** Several are ≤36 characters of body. | `crates/wcore-agent/src/engine.rs:26717` (comment admits it *"silences an unused-import lint"*), `crates/wcore-skills/src/lib.rs:45,50` (`let _ = crate::draft::MODULE_NAME;`), `crates/wcore-protocol/src/writer.rs:75`, `crates/wcore-eval/tests/crate_exists.rs:3`, `crates/wcore-plugin-api/src/tool.rs:266` (tests `derive(Clone)`, never compares), `crates/wcore-cua/src/redact/apple_vision.rs:170` + `windows_ocr.rs:170`, `crates/wcore-agent/src/tool_backends/mod.rs:564`, `crates/wcore-tools/src/url_safety.rs:821`, `crates/wcore-sandbox/tests/docker_smoke.rs:143`. | **DELETE** — the compiler already proves these. `engine.rs:26717` should be an `#[allow]`, not a test. |
| 4 | **Group C — 26 "must not panic" render/smoke tests.** | `crates/wcore-cli/src/tui/surfaces/{marketplace.rs:1112,subagents.rs:697,workflows.rs:945,onboarding.rs:2711}`, `crates/wcore-cli/src/tui/widgets/{banner.rs:178,header.rs:317,317,323}`, `crates/wcore-cli/src/tui/mod.rs:981`, `terminal_guard.rs:50`, `crates/wcore-agent/src/output/{mod.rs:655,728,735, null_sink.rs:39, protocol_sink.rs:1343}`, `crates/wcore-agent/src/agents/{channel_sink.rs:282,320, observer.rs:259}`, `crates/wcore-egress/src/lib.rs:144`, `crates/wcore-sandbox/src/backends/bwrap.rs:571`, others. | **KEEP (weak)** — these route through ratatui `Terminal::draw`, which **panics on layout overflow**, so the panic *is* the assertion and they genuinely guard 1×1-area arithmetic. Exception: `crates/wcore-agent/src/output/mod.rs:735` is named `test_tool_lifecycle_truncates_long_input`, pushes a 5,000-char string through three formatters, and inspects nothing — **FIX** that one (assert the truncation). |
| 5 | **Confirmed FALSE POSITIVES — 6. Do not touch.** | `crates/wcore-agent/tests/actor_acl_test.rs:172,183,193,204,215` end in `expect_executed(&results)` / `expect_denied(&results)`; both helpers assert at `crates/wcore-agent/tests/actor_acl_test.rs:131-140` and `:147-156`. `crates/wcore-sandbox/tests/backend_integration.rs:305` is an intentionally empty `#[ignore]` placeholder, documented at `:300-304`. | **KEEP** — flagged by the mechanical scan only because the assertion lives in a helper. (Note the five ACL ones carry `#[ignore = "v0.8.1 U11: pre-filter removed, will re-enable when sub-agent ACL wired"` — dead-but-intentional, and they are §1.3 #6.) |

### 2.2 Tautological — all 15

| # | Finding | Evidence | Verdict |
|---|---|---|---|
| 1 | **6 genuine tautologies** — `assert_eq!` comparing two `PartialEq`-derived enum literals to themselves, i.e. testing `derive(PartialEq)`. | `crates/wcore-agent/src/auto_skill/recorder.rs:57` (`TurnOutcome::Success` vs itself); `crates/wcore-types/src/skill_types.rs:108,113`; `crates/wcore-types/tests/plan_mode_transition_test.rs:50,57` — the last two pairs are **the same test duplicated across unit and integration**. | **FIX** — delete the tautological line only. Each of these tests also carries a real `assert_ne!` on an adjacent line, so the test is not worthless, just the one assertion. Also de-duplicate `skill_types.rs` vs `plan_mode_transition_test.rs`. |
| 2 | **9 determinism checks** — call a pure fn twice, compare. | `crates/wcore-agent/src/channel_dispatch.rs:384` (`hashed_session_id`), `crates/wcore-agent/src/compact/micro.rs:1569` (`cache_anchor_index`), `crates/wcore-agent/src/file_history.rs:437` (`path_bucket`), `:464` (`byte_digest`), `crates/wcore-agent/tests/file_history_sha256_test.rs:41,42` (duplicate of `file_history.rs:464`), `crates/wcore-cli/src/tui/streaming/verbs.rs:324,325,326` (`pick_turn_verb`), `crates/wcore-memory/tests/paths_integration.rs:296` (`sanitize_path`). | **FIX** — defensible as purity checks but **none pins an output value**, so a change to the digest or bucketing algorithm passes silently. `byte_digest` in particular is SHA-256 over a known input; assert the hex. |

---

## 3. COST OUTLIERS

Parsed from the GREEN log: 11,519 timed results, **1,997.1 s total CPU-seconds** against **194.3 s wall** (469 binaries, 96 cores).

Distribution: median **0.013 s**, mean **0.173 s**. Only 305 tests exceed 1 s; 105 exceed 5 s; 32 exceed 10 s; **4** exceed 30 s.

- **Top 10 = 459.5 s = 23.0 % of total CPU.**
- Top 25 = 665.3 s = 33.3 %.
- Top 100 = 1,175.8 s = 58.9 %.

The suite is not bloated by cost — 98 % of tests are sub-second. The problem is concentrated in a handful.

| # | Test | Time | What it proves | Verdict |
|---|---|---|---|---|
| 1 | `wcore-cli::deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed` | **187.06 s** (9.4 % of all CPU-seconds; ~96 % of the 194 s wall) | Two packaged runs are byte-identical. `.config/nextest.toml` documents both child processes finishing in **under 2 s each**; the remaining ~185 s is in-process work. | **FIX** — the assertion is worth keeping, the cost is not. ~99 % of its runtime buys nothing the two <2 s runs don't already give. It alone sets the suite's wall-clock floor and its 120 s×4 override is the only reason CI passes. |
| 2 | `wcore-agent::workflow_limits_test fix1_dispatch_budget_aborts_with_partial_result` | **80.88 s** | A dispatch budget aborts and returns a partial result. | **FIX** — 80 s to prove a budget abort means the budget under test is measured in real wall-clock. Inject a clock. Second-largest single cost in the suite; 4 % of total CPU. |
| 3 | `wcore-agent tool_backends::image_gen::tests::gemini_imagen_send_error_omits_api_key` | **30.02 s** | An error string does not contain the API key. | **FIX** — the `30.0x s` figure is a network timeout being waited out to produce an error. A string-redaction assertion should cost microseconds. |
| 4 | `wcore-agent tool_backends::gemini_vision::tests::vision_send_error_message_omits_api_key` | **30.02 s** | same | **FIX** — same pattern. |
| 5 | `wcore-agent tool_backends::google_meet::tests::google_meet_handles_network_timeout` | **20.03 s** | A network timeout is handled. | **FIX** — proves the timeout constant, not our handling. Also a §4 candidate (asserts the reqwest/OS timeout fires). |
| 6 | `wcore-agent tool_backends::homeassistant::tests::hass_handles_network_timeout` | **15.02 s** | same | **FIX** — same. |
| 7 | `wcore-agent::engine_compact_test tc_2_6_context_overflow_sheds_tool_output_and_continues` | 28.25 s | Context overflow sheds tool output. | **KEEP** — genuinely exercises the compaction path over a large transcript. |
| 8 | `wcore-agent::streaming_backpressure_test channel_sink_drops_excess_when_consumer_is_slow` | 25.92 s | Backpressure drops excess. | **FIX** — "slow consumer" is simulated with real sleeps. Worth keeping, worth an injected clock. |
| 9 | `wcore-config credentials::tests::encrypted_file_write_then_read_via_backend` | 22.13 s | Encrypted credential round-trip. | **KEEP** — the cost is Argon2/KDF work, which is the point. |
| 10 | `wcore-cli::f14_sigkill_recovery packaged_host_continue_and_non_genesis_reconnect_are_exactly_once` | 19.05 s | Exactly-once semantics across SIGKILL. | **KEEP** — spawns real packaged processes; the cost is the proof. |
| 11-16 | `wcore-cli::f14_sigkill_recovery` × 5 more (`packaged_tui_restart_projection_matches_json_host` 15.54 s, `packaged_fresh_process_reopens_sealed_request_and_dispatches_once` 15.42 s, `recovered_approval_approve_executes_effect_once_and_continues_once` 15.38 s, `stop_during_active_host_continue_preserves_unknown_provider_authority` 14.62 s, `sigkill_during_model_stream_resumes_as_provider_reconciliation_without_redispatch` 10.61 s, `sigkill_during_tool_execution_requires_reconciliation_without_reexecution` 10.40 s) | ~82 s combined | Crash-recovery matrix. | **KEEP** — highest value-per-second block in the top 25. Note the whole file is `#![cfg(unix)]` (`crates/wcore-cli/tests/f14_sigkill_recovery.rs`), so **none of this crash-recovery proof runs on Windows.** |
| 17-21 | `wcore-config credentials::tests::*` × 5 more (`migrate_merges_without_clobbering_existing_vault_keys` 16.18 s, `encrypted_file_survives_fresh_store_instance` 13.91 s, `open_store_encrypted_file_migrates_plaintext_once` 12.48 s, `migrate_plaintext_into_vault_imports_verifies_and_removes` 12.47 s, `migrate_discards_orphaned_ciphertext_without_kdf` 12.46 s) | ~67 s combined | Vault migration matrix. | **KEEP** — but the cohort pays the KDF cost 6× over. A shared low-cost-KDF test parameter would recover ~80 s at no loss of coverage. |
| 22-25 | `wcore-cli::harness_tui_flow` × 4 (`esc_on_pending_approval_durably_cancels_turn_without_tool_execution` 14.20 s, `newly_wired_slash_commands_reach_real_handlers_not_the_llm` 12.24 s, `session_save_resume_threads_prior_history_into_the_next_request` 12.24 s, `agent_turn_streams_mock_assistant_text_into_the_transcript` 11.71 s) | ~50 s | TUI harness flows. | **KEEP** — file is `#![cfg(unix)]`; same Windows blind spot. |
| — | `wcore-cli::smoke_p0 pty::gap_d009_large_transcript_keeps_input_responsive` 14.04 s, `pty::gap_d010_huge_paste_is_capped_or_responsive` 10.77 s | 24.8 s | Responsiveness under load. | **KEEP** — responsiveness assertions legitimately need wall-clock. |
| — | `wcore-agent spawner::spawn_task_set_tests::parallel_spawn_caps_active_child_engines_across_shared_calls` 14.03 s; `wcore-agent::session_journal_crash_matrix_test crash_before_during_and_after_every_event_variant_preserves_only_committed_frames` 11.17 s; `wcore-agent channel_tools::tests::apply_posture_workspace_installs_jail` 10.55 s | 35.8 s | — | **KEEP**. |

**Bottom line on cost**: recovering items 1-6 and 8 (~365 CPU-seconds, 18 % of total) would cost nothing in coverage — every one of them asserts something that does not require the elapsed time it spends. Item 1 alone would take suite wall-clock from ~194 s to well under 100 s.

The 3 flaky tests, for the record: `wcore-agent::dangerous_lease_e2e_test dangerous_expiry_reaches_bootstrapped_spawn_child` (3/3), `wcore-swarm worktree::tests::linux::status_output_cap_kills_git_descendant` (2/3), `wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream` (2/3). The `ci` profile's `retries = 2` is what turns these green.

---

## 4. FALSE-ASSURANCE CANDIDATES

Mechanical counts over all 56 crates (err-assertion patterns + OS/FS/syscall vocabulary in body or preceding comment):

| Pattern | Count |
|---|---|
| Err-assertion (`expect_err`/`unwrap_err`/`.is_err()`/`assert!(matches!(…Err`) **with** OS/FS/syscall vocabulary | **92** |
| Tight subset: the comment/message **attributes the refusal to the OS/kernel/platform** (`Windows must…`, `OS-level`, `errno`/`EACCES`/`ESRCH`, `ERROR_*`, `SBPL`, `bwrap`, `landlock`, `Keychain`) | **26** |
| Tests that **silently `return`** on a missing env var / absent binary / unavailable backend (vacuous-pass class) | **43** |

### 4.1 HIGH risk — encodes a premise that may be flatly false, or asserts an OS behavior on a platform where it is never executed

| # | Finding | Evidence | Verdict |
|---|---|---|---|
| 1 | **`assert_rename_refused_by_open_descendant` — the third member of the family that already burned this project twice, and the worst.** On Windows the **entire test body is the OS assertion**; both callers test zero lines of our code on that platform. | `crates/wcore-swarm/tests/dispatch_smoke.rs:385` — `.expect_err("Windows must refuse renaming an ancestor of a swarm-held descendant")` then `assert_eq!(error.kind(), ErrorKind::PermissionDenied, "expected OS-level PermissionDenied…")`. Premise stated at `:369-378`: *any* open descendant handle, at any access, under **any share mode**, blocks renaming any ancestor. That is the same claim `FILE_SHARE_DELETE` falsifies. Callers `dispatch_rejects_different_head_repository_replacement:284→301` and `dispatch_rejects_same_head_repository_replacement:311→327`. The comment at `:423` admits the software-defense coverage was *"traded for an OS-behaviour assertion"* in commit `334f264d`. | **DELETE the OS assertion, FIX the test** — restore the software defense it was traded for. Never executed on Windows at this SHA (§1.1). |
| 2 | **`live_lsa_dependent_tool_fails_under_hardened_sandbox` — passes for a reason the file's own doc comment refutes.** | `crates/wcore-sandbox/tests/live_integrity.rs:41` — `assert_ne!(out.exit_code, 0, "whoami /groups SUCCEEDED under hardened AppContainer…")`. The file header at `:14-22` documents that binaries *"cannot load under the hardened sandbox: NTFS DACLs on `target\debug\` exclude the AppContainer SID… unable to resolve VCRUNTIME140.dll."* `whoami.exe` is an external binary subject to the identical loader failure, so `exit != 0` almost certainly proves image-load failure, **not** LSA ALPC denial. The matched positive control at `:91` runs `cmd /c echo` — a **cmd builtin**, which never exercises the external-binary loader, so the pair does not close the hole. | **FIX** — exact stated-rationale-vs-actual-cause mismatch. Positive control must be an external `.exe`. |
| 3 | **`secret_read_deny_case_a/b/d` — the non-vacuity guard is bwrap-only, so all three negative cases are fully vacuous on macOS**, despite the comment claiming cross-platform coverage. | `crates/wcore-sandbox/tests/secret_read_deny.rs:64,122,275`. Guard at `:103` is `assert!(!stderr.contains("bwrap: "), "…non-vacuous deny…")`; the comment at `:99-101` states *"The marker never appears on macOS sandbox-exec."* Assertion at `:109` is `assert!(!stdout.contains("SECRET_TOKEN"))`. **Exit code is never asserted.** On macOS, `cat` failing for any reason yields empty stdout and green. | **FIX** — add an exit-code / positive-control assertion for the macOS arm. |
| 4 | **`sandbox_exec_denies_read_of_secret_under_allowed_root` — a disjunction whose first disjunct is satisfied by any failure whatsoever.** | `crates/wcore-sandbox/src/backends/sandbox_exec.rs:409`, assertion at `:454`: `assert!(out.exit_code != 0 \|\| !stdout_str.contains("SECRET"))`. Missing `/bin/cat`, an SBPL compile error, `sandbox-exec` deprecation, or a spawn failure all satisfy it. Zero non-vacuity guard. Asserts macOS Seatbelt last-match-wins deny semantics, not our deny-list construction. | **FIX** — split the disjunction; assert the command ran. |
| 5 | **`failed_transaction_cleanup_remains_retryable` — direct sibling of the two proven cases, same file, same unverified premise declared as fact.** | `crates/wcore-swarm/src/worktree_tests.rs:176` (Windows arm at `:199`): `.expect_err("Windows must refuse renaming a swarm-held control root")` + `assert_eq!(error.kind(), ErrorKind::PermissionDenied, "expected OS-level PermissionDenied…")`. Comment `:188-195` declares the *"Measured Windows rename rule"* and then declares the software defense *"cannot arise here."* | **FIX** — same repair as #1. |
| 6 | **`bwrap_confines_filesystem_writes_outside_allowlist` — weak-negative that passes if `sh` never starts.** | `crates/wcore-sandbox/tests/backend_integration.rs:220` — `assert_ne!(out.exit_code, 0, "writing outside fs_write_allow must fail inside the sandbox")`. Under `SandboxManifest::default()` there is no `/bin/sh` bind, which is the *likely* reason for the non-zero exit. Asserts bubblewrap's mount-namespace behavior, not our manifest→argv translation. The companion `!forbidden.exists()` is the only load-bearing assertion. | **FIX** — assert the inner command ran. |
| 7 | **`process_is_alive_invalid_pid_is_dead` — verifies the Windows kernel, not our branch.** | `crates/wcore-cli/src/cron.rs:644` — `assert!(!super::process_is_alive(0))`. Comment: *"PID 0 is the System Idle Process; OpenProcess with PROCESS_QUERY_LIMITED_INFORMATION **will fail** → returns false."* Our production code returns `false` on **any** `OpenProcess` failure, so the test cannot distinguish the branch from the kernel. Windows-only, never run at this SHA. | **DELETE** — if the kernel premise flips on an elevated context or a future Windows build, a stale-PID-file bug ships silently and this test still passes. |

### 4.2 MED risk — asserts third-party/OS behavior we don't control; premise probably correct

| # | Finding | Evidence | Verdict |
|---|---|---|---|
| 8 | **`identity_drift_never_signals_foreign_process` — a race dressed as a proof.** `try_wait().is_none()` immediately after `signal(SIGKILL)` can return `None` even if the code *did* signal, because delivery and reaping are asynchronous. **The test can pass with the bug present.** | `crates/wcore-sandbox/src/backends/process_tree.rs:451` and the Linux twin at `:769`. | **FIX** — needs a bounded wait plus a positive control. |
| 9 | **`hung_scenario_does_not_leak_pid` — 50 ms sleep as a kernel-reaping guarantee, plus a PID-reuse hazard** (a recycled PID reads as "alive"). | `crates/wcore-eval-scenarios/tests/smoke.rs:91`, sleep at `:119`, `libc::kill(pid, 0)` probe. | **FIX** — both directions fail for reasons outside our control. |
| 10 | **`transaction_lease_is_mutually_exclusive` / `swarm_lock_is_mutually_exclusive` — correct only because of `fd_lock`'s current backend choice.** A second open **in the same process** observes contention under BSD `flock` but **not** under POSIX `fcntl` record locks (same-process locks merge silently). Unstated third-party dependency. | `crates/wcore-swarm/src/worktree_tests.rs:93` and `:128`; `ErrorKind::WouldBlock` asserted at `:159`. | **FIX** — document/pin the `fd_lock` backend assumption, or lock from a child process. |
| 11 | **`retained_reservation_rejects_same_inode_truncate_and_rewrite` — asserts `std::fs::write` preserves the inode** (i.e. that std uses `O_TRUNC`, not write-then-rename). If that ever flipped, the subsequent `expect_err` would still fire **for the wrong reason** and the test would stay green while losing its meaning. | `crates/wcore-swarm/src/worktree_tests.rs:63`. | **FIX** — precisely the `transaction_cleanup_preserves_same_path_replacement` shape. |
| 12 | **`read_only_authority_replay_subprocess` — vacuously green in every normal run.** Returns immediately when `WCORE_TEST_READ_ONLY_JOURNAL_PATH` is unset; the conditional `geteuid()==0` privilege drop is the only thing making it non-vacuous; hard-codes UID/GID `65534` existing. | `crates/wcore-agent/tests/session_journal_test/foundation_cases.rs:464`, driver at `:482`. | **FIX** — the 0o400/0o500 fixture is meaningless as root. |
| 13 | **`required_live_macos_retained_transport_rejects_path_replacement` — a 500 ms window against a container running `sleep 2`**, racing Docker Desktop image pull, daemon latency, and CI load. The attack window is timing-defined, not synchronised. | `crates/wcore-sandbox/src/backends/docker_tests.rs:302`. | **FIX** — synchronise the attack instead of timing it. |
| 14 | **Wall-clock bound as the entire descendant-reaping proof** (`elapsed < 20s`, sandwiched between a 45 s `sleep` and a 30 s manifest timeout). | `crates/wcore-sandbox/tests/hard_process_containment_macos.rs:65`; same shape at `hard_process_containment.rs:55`. | **FIX** — assert the process set, not the clock. |
| 15 | **`roundtrip_macos_keychain` / `api_key_roundtrip_macos` — round-trip through the developer's REAL macOS login Keychain**, and the `expect_err` verifies Apple's Security framework rather than our wrapper. Fails on a locked keychain, headless CI, or an SSH session; mutates real user state during `cargo test`. | `crates/wcore-config/src/keychain.rs:88`; `crates/wcore-acp/src/auth.rs:233`. | **FIX** — fake the backend; keep one opt-in `#[ignore]` live test. |
| 16 | **Symlink-creation tests that hard-panic without `SeCreateSymbolicLinkPrivilege` / Developer Mode.** | `crates/wcore-sandbox/src/directory_authority_windows_tests.rs:332`, `:348` — `.expect("native Windows proof requires file-symlink creation authority")`; also `crates/wcore-sandbox/src/backends/appcontainer/acl_lease/storage.rs:714`, `crates/wcore-agent/tests/session_journal_compaction_test.rs:185`. | **KEEP** — hard-failing is the correct choice for a "native proof"; noted so the Windows runner prerequisite is explicit. |
| 17 | **`workspace_posture_jails_filesystem_reads` — near-tautology: the expected value is produced by the same predicate the production code consults**, so it can never fail regardless of the posture logic. Its comment (`:168-171`) frames it as an OS-level guarantee, which is what makes it read as assurance. | `crates/wcore-agent/tests/channel_tool_posture_test.rs:136`; assertion at `:172` compares against `wcore_tools::bash::platform_enforces_read_deny()`. | **FIX** — hardcode the per-platform expectation. Belongs to §2 as much as §4. |
| 18 | **`nonexistent_path_is_path_denied` — asserts the `landlock` crate's error mapping, and the file header documents that `restrict_self()` is deliberately never invoked**, so the actual enforcement path is untested. Whole file is `#![cfg(all(target_os = "linux", feature = "landlock"))]` — one of the 60 never-compiled-in-CI tests (§1.3 #3). | `crates/wcore-sandbox/tests/landlock_ruleset_construction.rs:23`. | **FIX** — enable the feature in the Linux CI leg first (it is free), then strengthen. |
| 19 | **Timeout-budget tests with 100 ms of slack over a 500 ms budget**, dependent on external `sleep`/`ping -n 3` and cmd.exe argument parsing on Windows. | `crates/wcore-cli/src/tui/statusline/exec.rs:260`, `:277`. | **FIX** — scheduler-load dependent; widen or inject a clock. |
| 20 | **`binary_matches_repo_head` — silently `return`s if `git` is missing or the checkout is not a repo**, then asserts `git merge-base --is-ancestor` behavior. The invariant belongs to the git binary and the developer's tree, not our code. | `crates/wcore-cli/tests/build_provenance.rs:24`, skip at `:29-38`. | **DELETE or FIX** — trivially skippable and tests git. |
| 21 | **`descriptor_walk_rejects_concurrent_directory_swap` — `if geteuid() != 0 { return; }`, vacuously green off the privileged harness**; when it runs it asserts `chown`/`O_NOFOLLOW` kernel semantics and requires UID/GID `65532` to exist. | `crates/wcore-eval-scenarios/src/process_tree.rs:1718`. | **FIX** — make the skip loud (fail in CI, skip locally). |
| 22 | **Hard-link fixtures requiring filesystem hard-link support** (fails on FAT/exFAT temp dirs, some network mounts, Windows without privileges). | `crates/wcore-agent/tests/session_journal_compaction_test.rs:199`; `crates/wcore-agent/tests/child_transaction_store_test.rs:291`. | **KEEP** — premise usually true; noted only. |
| 23 | **`unix_non_executable_file_and_metadata_io_are_distinct` — a kernel-`ELOOP` premise dressed as an error-taxonomy test.** | `crates/wcore-config/src/shell/executable_readiness_tests.rs:300`. | **KEEP (weak)** — the taxonomy mapping is ours; the trigger is the kernel's. |

### 4.3 LOW risk

| # | Finding | Evidence | Verdict |
|---|---|---|---|
| 24 | `dead_pid()` helper spawns `true` / `cmd /C exit 0` and returns the PID as "guaranteed dead" — PID reuse can invalidate this between `wait()` and the assertion; also a bare-PATH `Command::new("true")`. | `crates/wcore-cli/src/crash_sentinel.rs:414`. | **KEEP** — low probability. |
| 25 | Both `return` early if `WAYLAND_WRITE_SAFE_ROOT` is set — an env-dependent silent skip on the **deny-list guard**. | `crates/wcore-tools/src/file_safety.rs:551`, `:574`. | **FIX** — an env var in a developer's shell silently disables a security test. |
| 26 | `cfg!(target_os = "macos")` branches the expectation, so only one arm is ever exercised per platform. | `crates/wcore-cli/tests/doctor_smoke.rs:117`. | **KEEP** — harmless by construction. |

### 4.4 Pattern notes

- **The `transaction_cleanup_*` / `assert_rename_refused_by_open_descendant` family is 3 tests + 1 shared helper**, all in `wcore-swarm`, all keyed on one unverified sentence ("the measured Windows rename rule"): `crates/wcore-swarm/src/worktree_tests.rs:176`, `:289`, `:355`, and `crates/wcore-swarm/tests/dispatch_smoke.rs:385`. **Fixing the premise once fixes all four.**
- **Explicitly cleared, despite matching the mechanical pattern**: `assert_eq!(error.kind(), PermissionDenied)` at `crates/wcore-sandbox/src/backends/process_tree.rs:478` and `:498` — those errors are constructed by **our own** code (`io::Error::new(ErrorKind::PermissionDenied, …)` at `:270`, `:321`, `:344`, `:358`). Not false assurance.
- **The single largest structural risk is the weak-negative shape** — `assert_ne!(exit_code, 0)` / `assert!(!stdout.contains(SECRET))` with no non-vacuity control — used across `wcore-sandbox`'s live tests (findings 2, 3, 4, 6). **None of them assert that the inner command actually ran.** One shared positive-control helper would close all four.

---

## Bottom line

**Of 11,816 test functions (11,519 executed green on Linux at `01a5b0ae`, 48 skipped), roughly 11,100–11,300 appear sound. The defensible defect count is ~380–420, and the single largest category is not waste — it is absence of execution.**

| Category | Count | Basis |
|---|---|---|
| **No execution evidence at this SHA in normal CI** (union, de-duplicated) | **283** | 155 Windows-only + 23 macOS-only + 86 `#[ignore]` + 60 non-default-feature-gated, minus 41 overlaps. |
| — of which Windows-only in crates the nightly soak never touches | 140 | soak covers only `wcore-cron`/`wcore-config`/`wcore-providers`/`wcore-tools`/`wcore-swarm`; `wcore-sandbox` alone contributes 105. |
| — of which run in **no** automatic workflow on **any** branch | ~145 | 85 unrescued `#[ignore]` + 60 never-compiled feature-gated. |
| **Assertion-free with effectively zero verification value** | **~52** | of 84 mechanical hits: 6 confirmed false positives, ~26 legitimate-but-weak ratatui no-panic guards. Dominated by 31 cloned provider `constructs_*` and 8 self-documented hollow tests. |
| **Tautological** | **6** genuine (of 15) | 9 are defensible determinism checks that pin no output value. Zero `assert!(true)` anywhere. |
| **False-assurance** | **7 HIGH, 16 MED, 3 LOW** (of 92 mechanical, 26 tight-subset) | 4 of the 7 HIGH are one family sharing one unverified premise. |
| **Cost outliers worth fixing** | **7** | ~365 CPU-seconds = 18 % of total, none of which buys coverage. |
| **Remainder — appear sound** | **~11,100–11,300** | executed green on Linux, not in any category above. |

**The honest headline is narrower and worse than "how many are useful":** the 11,519 green results are a **Linux-only** artifact of a manually-run Hetzner build. `gh run list --branch plan/f20-unified-audit-repair` returns `[]` — **CI has never run on this branch**, so neither the macOS nor the Windows leg has compiled this tree, and the last `ci.yml` run of any kind was 2026-07-13. The Windows host is on a different commit (`ce9a11a6`). The recurring Windows automation (`nightly-windows-soak`, cron, `main` only) runs 5 crates and **excludes `wcore-sandbox` entirely** — the crate holding 105 Windows-only tests including every retained-handle security proof. The 5 most recent `f20-native-uat-*` candidate dispatches (2026-07-24) all concluded `failure`.

### What I could NOT determine

- **Whether the 155 Windows-only and 23 macOS-only tests currently COMPILE at `01a5b0ae`.** The Mac cannot build this workspace, the Windows host is on a different commit, and CI has never run this branch. Compilation is the precondition for every claim about them, and I have no evidence either way. This is the same failure mode that hid the ~133 `wcore-sandbox` tests for two weeks.
- **Whether any of the 283 never-executed tests would PASS if run.** No verdict here is "this test fails" — only "this test has no execution evidence."
- **Per-test timings for anything outside the Linux run.** No macOS or Windows timing data exists at this SHA, so the `default`-profile cost picture on a developer laptop is unmeasured.
- **The true assertion-free count with full confidence.** The tokenizer removed 20 false positives and I hand-verified 15 more; the residual ~26 ratatui "panic is the assertion" tests are a judgment call I resolved as KEEP, and a reviewer could reasonably resolve ~10 of them the other way.
- **Whether the 3 flaky tests are flaky for a real reason.** `retries = 2` masks them; I did not investigate root causes.
- **Mutation-testing coverage.** `mutants-nightly.yml` runs on macOS against a per-crate shard list I did not cross-reference against the never-executing set.

---

## Not investigated

1. `crates/wcore-eval` / `wcore-evolve` scenario-scoring test quality — the eval lane has its own gate semantics and needs a domain read, not a mechanical one.
2. Whether the 469 test binaries contain duplicate coverage across unit vs integration (the `skill_types.rs` / `plan_mode_transition_test.rs` and `file_history.rs` / `file_history_sha256_test.rs` duplicates suggest more exist).
3. `bench-regression.yml` and `mutants-nightly.yml` shard lists vs the never-executing set.
4. The 43 "silently `return` on missing env/binary" vacuous-pass tests beyond the ~8 surfaced in §4 — a full enumeration would likely add findings.
5. Property/fuzz test coverage (`proptest`/`rstest` parameterization) — counted as single functions here, so real case counts are higher than 11,816.
6. Whether `packaged_f04_run_is_repeatable_and_content_addressed`'s 185 s is reducible without weakening the content-addressing proof — needs profiling on the host.
7. The `desktop_contract_corpus` drift failure visible in `/root/f20-56/test.log.gz` (the RED run) — belongs to contract regeneration, not test-suite quality.
8. Windows-host reality checks beyond the checkout SHA — running anything there risked mutating a checkout the brief put off-limits.

