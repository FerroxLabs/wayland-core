# Testing Patterns

**Analysis Date:** 2026-07-18

## Test Framework

**Runner:**
- Rust's built-in test harness executed by cargo-nextest; the repository does not pin a cargo-nextest version, while `justfile` routes execution through the toolchain manager configured in `vx.toml`.
- Config: `.config/nextest.toml`

**Assertion Library:**
- Use Rust's built-in `assert!`, `assert_eq!`, `assert_ne!`, `matches!`, and `panic!` macros throughout unit and integration tests, as in `crates/wcore-types/src/message.rs` and `crates/wcore-eval-scenarios/tests/fixture_manifest_contract.rs`.
- Use `rstest` 0.26 for table-driven cases where multiple inputs share one contract, as in `crates/wcore-protocol/tests/commands_test.rs`; the version is declared in `Cargo.toml`.

**Run Commands:**
```bash
vx just test                 # Run workspace unit and integration tests with the default nextest profile
vx just test-one NAME        # Filter to one test name
vx just test-verbose         # Run with captured output visible
vx just test-ci              # Run the CI profile without fail-fast
vx just coverage             # Generate lcov.info with cargo-llvm-cov and nextest
```
All recipes are defined in `justfile`; no watch-mode recipe is configured.

## Test File Organization

**Location:**
- Put private-implementation unit tests in an inline `#[cfg(test)] mod tests` beside the code, as in `crates/wcore-types/src/message.rs`, `crates/wcore-memory/src/error.rs`, and `crates/wcore-channel-discord/src/gateway.rs`.
- Put public API and cross-module integration tests under `crates/<crate>/tests/`, as in `crates/wcore-tools/tests/`, `crates/wcore-protocol/tests/`, and `crates/wcore-cli/tests/`.
- Put shared integration-test helpers under a local `tests/common/` or `tests/support/` module, as in `crates/wcore-agent/tests/common/mod.rs` and `crates/wcore-cli/tests/support/mod.rs`.
- Root-level compatibility and engine tests live under `tests/`, with shared utilities in `tests/common/mod.rs`.
- Put Criterion benchmarks under `crates/<crate>/benches/`, as in `crates/wcore-agent/benches/orchestration_graph_bench.rs` and `crates/wcore-providers/benches/parse_sse_chunk_bench.rs`.

**Naming:**
- Name test functions in `snake_case` after the observable behavior and expected outcome, such as `deserialized_manifest_rejects_tampered_identity_component_and_schema` in `crates/wcore-eval-scenarios/tests/fixture_manifest_contract.rs`.
- Use `_test.rs` for focused contracts, `_e2e.rs` for multi-component flows, and descriptive contract names without a suffix when clearer; examples include `crates/wcore-tools/tests/git_argv_injection_test.rs`, `crates/wcore-memory/tests/memory_concurrency_e2e.rs`, and `crates/wcore-protocol/tests/recovery_protocol.rs`.

**Structure:**
```
crates/<crate>/
├── src/<module>.rs              # implementation + #[cfg(test)] unit module
├── tests/
│   ├── common/mod.rs            # optional shared builders/fakes
│   ├── fixtures/                # optional checked deterministic data
│   └── <behavior>_test.rs       # public-surface integration target
└── benches/<benchmark>.rs       # Criterion benchmark target
```
Concrete examples are `crates/wcore-agent/src/engine.rs`, `crates/wcore-agent/tests/common/mod.rs`, `crates/wcore-evolve/tests/fixtures/`, and `crates/wcore-agent/benches/orchestration_graph_bench.rs`.

## Test Structure

**Suite Organization:**
```rust
#[tokio::test]
async fn test_engine_text_response_ends_turn() {
    let provider = Arc::new(MockLlmProvider::with_text_response("Hello, world!"));
    let mut engine = AgentEngine::new_with_provider(
        provider,
        test_config(),
        ToolRegistry::new(),
        silent_output(),
    );

    let result = engine.run("Hi", "").await.expect("engine should succeed");

    assert_eq!(result.text, "Hello, world!");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.turns, 1);
}
```
This arrange-act-assert shape is used in `tests/engine_test.rs` and the crate-local equivalent `crates/wcore-agent/tests/engine_test.rs`.

**Patterns:**
- Build only the state needed by a test, perform one behavior, and assert the externally visible result; `crates/wcore-eval-scenarios/tests/fixture_manifest_contract.rs` is a compact reference.
- Use `#[test]` for synchronous logic and `#[tokio::test]` for async providers, tools, channels, and orchestration, as in `crates/wcore-protocol/tests/commands_test.rs` and `crates/wcore-agent/tests/engine_test.rs`.
- Use `rstest` `#[case]` inputs for repeated protocol shapes instead of duplicating test bodies, following `crates/wcore-protocol/tests/commands_test.rs`.
- Isolate filesystem state with `tempfile::tempdir` or `TempDir` and keep the guard alive for the full assertion scope, as in `crates/wcore-agent/tests/common/mod.rs` and `crates/wcore-cli/tests/harness_tui_flow.rs`.
- Use `serial_test` only for tests that mutate process-global environment, shared config roots, or singleton state; representative uses are in `crates/wcore-config/tests/config_resolution_provenance.rs` and `crates/wcore-agent/tests/bootstrap_test.rs`.
- Gate platform-specific behavior with `#[cfg(...)]`, following the Unix PTY suite in `crates/wcore-cli/tests/harness_tui_flow.rs` and platform branches in `crates/wcore-eval-scenarios/tests/fixture_manifest_contract.rs`.
- Include assertion messages for non-obvious invariants and diagnostics, as in `crates/wcore-types/src/message.rs` and `crates/wcore-cli/tests/harness_tui_flow.rs`.

## Mocking

**Framework:** `wiremock` 0.6 for local HTTP boundaries, `mockall` 0.14 for generated trait mocks, `mockito` 1 for focused CLI HTTP tests, plus hand-written trait fakes; dependencies are declared in `Cargo.toml`, `crates/wcore-agent/Cargo.toml`, and `crates/wcore-cli/Cargo.toml`.

**Patterns:**
```rust
pub async fn physical_attempt_server() -> wiremock::MockServer {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .mount(&server)
        .await;
    server
}
```
The local-server pattern appears in `crates/wcore-agent/tests/common/mod.rs`; provider-specific request and response assertions appear in `crates/wcore-providers/tests/provider_openai_test.rs` and `crates/wcore-providers/tests/provider_anthropic_test.rs`.

**What to Mock:**
- Replace provider streams with deterministic trait fakes that emit scripted `LlmEvent` sequences, following `MockLlmProvider` in `crates/wcore-agent/tests/common/mod.rs`.
- Replace external HTTP services with loopback `wiremock` servers and assert method, path, headers, body, retries, and status mapping, following `crates/wcore-providers/tests/provider_openai_test.rs` and `crates/wcore-browser/tests/policy_test.rs`.
- Use small fake tools, clocks, sinks, and handlers when the test targets orchestration rather than those dependencies, following `MockTool` in `crates/wcore-agent/tests/common/mod.rs` and channel fakes in `crates/wcore-channel-discord/src/lib.rs`.

**What NOT to Mock:**
- Do not mock pure serialization, validation, policy, or state-transition logic; exercise the real public type directly, as in `crates/wcore-protocol/tests/commands_test.rs`, `crates/wcore-permissions/tests/acl_test.rs`, and `crates/wcore-eval-scenarios/tests/fixture_manifest_contract.rs`.
- Do not substitute mocks at a packaged-boundary proof: release smoke, PTY, protocol corpus, and packaged evaluator tests drive real binaries or real generated contracts in `crates/wcore-cli/tests/release_binary_smoke.rs`, `crates/wcore-cli/tests/harness_tui_flow.rs`, `crates/wcore-protocol/tests/desktop_contract_corpus.rs`, and `crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs`.
- Keep default workspace tests offline; real provider, browser, model-download, Docker, and hardware tests remain feature-gated or ignored as configured in `crates/wcore-providers/Cargo.toml`, `crates/wcore-browser/Cargo.toml`, `crates/wcore-memory/tests/bge_local_real.rs`, and `crates/wcore-sandbox/Cargo.toml`.

## Fixtures and Factories

**Test Data:**
```rust
fn manifest(values: [char; 6]) -> CompositeFixtureManifest {
    let bytes = artifacts(values);
    CompositeFixtureManifest::from_artifacts(
        &bytes[0], &bytes[1], &bytes[2], &bytes[3], &bytes[4], &bytes[5],
    )
}
```
Use small deterministic factories to make the changed dimension obvious, following `crates/wcore-eval-scenarios/tests/fixture_manifest_contract.rs`.

**Location:**
- Keep reusable Rust builders and fakes in the owning suite's `tests/common/` or `tests/support/`, such as `crates/wcore-agent/tests/common/mod.rs` and `crates/wcore-cli/tests/support/mock_llm.rs`.
- Keep checked static artifacts beneath the owning crate's `tests/fixtures/`, such as `crates/wcore-evolve/tests/fixtures/`, `crates/wcore-repomap/tests/fixtures/`, and `crates/wcore-eval-scenarios/tests/fixtures/`.
- Resolve fixture paths from `env!("CARGO_MANIFEST_DIR")` or use `include_str!`/`include_bytes!` when compile-time embedding is part of the contract; representative uses are in `crates/wcore-pluginsrc/tests/conformance.rs` and `crates/wcore-eval/tests/corpus_load.rs`.
- Generate mutable artifacts inside a `tempfile` directory and never depend on a developer home or ambient credentials, following `crates/wcore-cli/tests/harness_tui_flow.rs` and `crates/wcore-config/tests/hermeticity_audit_test.rs`.

## Coverage

**Requirements:** No numeric line or branch coverage threshold is configured in `justfile` or `.github/workflows/ci.yml`. CI instead enforces formatting, Clippy, workspace nextest, protocol/eval gates, release-binary smoke, packaged-driver proof, and security audit in `.github/workflows/ci.yml`.

**View Coverage:**
```bash
vx just coverage             # Writes lcov.info via cargo llvm-cov nextest
```
The coverage recipe is defined in `justfile`. Mutation testing for selected crates is configured separately in `.github/workflows/mutants-nightly.yml`.

## Test Types

**Unit Tests:**
- Test private helpers, parsing, serialization, state transitions, and edge conditions in inline `#[cfg(test)]` modules, following `crates/wcore-types/src/message.rs`, `crates/wcore-memory/src/error.rs`, and `crates/wcore-safety/src/pii.rs`.
- Exercise boundaries and malformed inputs as well as happy paths; `crates/wcore-eval-scenarios/tests/fixture_manifest_contract.rs` covers tampering, traversal, symlinks, mutation, and round trips.

**Integration Tests:**
- Test each crate's public surface under `crates/<crate>/tests/`, including protocol contracts in `crates/wcore-protocol/tests/`, tool safety in `crates/wcore-tools/tests/`, and persistence/concurrency in `crates/wcore-memory/tests/`.
- Test cross-component engine flows with deterministic providers and tools in `crates/wcore-agent/tests/`, using common builders from `crates/wcore-agent/tests/common/mod.rs`.
- Test generated or golden wire artifacts byte-for-byte where compatibility is the contract, following `crates/wcore-protocol/tests/golden_v0_1_21.rs` and `crates/wcore-observability/tests/golden_trace_v1.rs`.

**E2E Tests:**
- Use Rust integration targets under `crates/wcore-agent/tests/e2e/`, selected by the explicit `e2e` target in `crates/wcore-agent/Cargo.toml` and the sequential, zero-retry profile in `.config/nextest.toml`.
- Drive CLI and TUI user flows through the compiled binary, PTY, and deterministic local servers in `crates/wcore-cli/tests/harness_cli_surface.rs`, `crates/wcore-cli/tests/harness_tui_flow.rs`, and `crates/wcore-cli/tests/support/mock_llm.rs`.
- Keep paid or network-dependent provider tests behind features and credential checks; orchestration is defined in `.github/workflows/e2e.yml` and feature declarations in `crates/wcore-agent/Cargo.toml`.
- Run deterministic acceptance and evaluation gates through dedicated recipes such as `eval-gate` and `desktop-contract-check` in `justfile`.

## Common Patterns

**Async Testing:**
```rust
#[tokio::test]
async fn test_openai_stream_text_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let events = collect_events(provider.stream(&make_request()).await.unwrap()).await;
    assert_eq!(events.len(), 3, "expected 3 events, got: {:?}", events);
}
```
Use `#[tokio::test]`, await setup and the behavior under test, and let owned guards perform teardown; this pattern is taken from `crates/wcore-providers/tests/provider_openai_test.rs`, with shared async helpers in `crates/wcore-agent/tests/common/mod.rs`.

**Error Testing:**
```rust
assert_eq!(
    tampered.verify(),
    Err(FixtureManifestError::DigestMismatch),
);

assert!(matches!(
    BoundCompositeFixtureManifest::from_artifacts(root.path(), traversal),
    Err(FixtureManifestError::InvalidArtifactPath { .. })
));
```
Prefer exact enum equality when the full error is stable and `matches!` when only the variant or selected fields matter, following `crates/wcore-eval-scenarios/tests/fixture_manifest_contract.rs`. Use `.expect("specific operation")` in test setup so failures identify the broken precondition, following `crates/wcore-agent/tests/common/mod.rs`.

---

*Testing analysis: 2026-07-18*
