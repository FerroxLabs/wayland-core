# Coding Conventions

**Analysis Date:** 2026-07-18

## Naming Patterns

**Files:**
- Use lowercase `snake_case.rs` names for Rust modules, including compound domains such as `crates/wcore-providers/src/openai_responses.rs`, `crates/wcore-tools/src/tool_result_storage.rs`, and `crates/wcore-agent/src/provider_recovery.rs`.
- Name crate roots `src/lib.rs` and binaries `src/main.rs` or explicit `src/bin/<name>.rs`; examples are `crates/wcore-types/src/lib.rs`, `crates/wcore-cli/src/main.rs`, and `crates/wcore-evolve/src/bin/wcore-evolve.rs`.
- Name integration tests for the behavior or contract they protect, commonly with `_test.rs`, `_e2e.rs`, or `_contract.rs`; examples are `crates/wcore-tools/tests/git_argv_injection_test.rs`, `crates/wcore-cli/tests/composer_attachments_e2e.rs`, and `crates/wcore-protocol/tests/approval_resume_contract.rs`.

**Functions:**
- Use `snake_case` for free functions, methods, async functions, and test functions, as in `Message::total_input_tokens` in `crates/wcore-types/src/message.rs` and `run_git` in `crates/wcore-tools/src/git.rs`.
- Give tests behavior-focused names that state the invariant, such as `artifact_mutation_after_manifest_creation_fails_verification` in `crates/wcore-eval-scenarios/tests/fixture_manifest_contract.rs`.
- Use `new`, `default`, `from_*`, `with_*`, and `build_*` consistently for constructors and configured variants, as shown by `Message::new` and `Message::now` in `crates/wcore-types/src/message.rs` and the test builders in `crates/wcore-agent/tests/common/mod.rs`.

**Variables:**
- Use `snake_case` for locals, fields, parameters, and modules; representative fields include `cache_creation_tokens` and `cache_read_tokens` in `crates/wcore-types/src/message.rs`.
- Prefix intentionally unused bindings with `_`, especially retained guards and callback arguments, as in `_log_guard` in `crates/wcore-cli/src/main.rs` and `_request` in `crates/wcore-agent/tests/common/mod.rs`.
- Use `SCREAMING_SNAKE_CASE` for constants and static configuration, such as `MAX_SSE_BUFFER_BYTES` in `crates/wcore-providers/src/openai.rs` and `RECOVERY_TEST_KEY` in `crates/wcore-agent/tests/common/mod.rs`.

**Types:**
- Use `UpperCamelCase` for structs, enums, traits, type aliases, and enum variants, as in `ContentBlock`, `FinishReason`, and `TokenUsage` in `crates/wcore-types/src/message.rs`.
- Name public error enums with an `Error` suffix and expose a local `Result<T>` alias when a module has a dominant error type, as in `MemoryError` and `Result<T>` in `crates/wcore-memory/src/error.rs`.
- Name behavioral interfaces as domain traits without an `I` prefix, such as `LlmProvider` in `crates/wcore-providers/src/lib.rs` and `Tool` in `crates/wcore-tools/src/lib.rs`.

## Code Style

**Formatting:**
- Format all Rust with the Rust 1.95 rustfmt component pinned by `rust-toolchain.toml`; the repository has no separate rustfmt settings file, so standard rustfmt layout is authoritative.
- Run formatting through `vx just fmt`; CI checks the same output with `vx just fmt-check` as defined in `justfile` and `.github/workflows/ci.yml`.
- Let rustfmt control four-space indentation, multiline argument layout, trailing commas, and brace placement; representative formatted code appears in `crates/wcore-types/src/message.rs` and `crates/wcore-tools/src/git.rs`.

**Linting:**
- Run `vx just lint`, which executes Clippy over the workspace and all targets with `-D warnings`; the command is defined in `justfile` and enforced in `.github/workflows/ci.yml`.
- Route outbound HTTP construction through `wcore_egress::EgressClient`; `clippy.toml` rejects raw `reqwest::Client::new`, `reqwest::Client::builder`, `reqwest::ClientBuilder::new`, and `reqwest::get`.
- Preserve crate-specific lint boundaries where they exist: `crates/wcore-evolve/src/lib.rs` denies unwrap, expect, panic, and indexing/slicing; `crates/wcore-permissions/src/lib.rs` forbids unsafe code; `crates/wcore-repomap/src/lib.rs` denies unsafe code and warns on missing docs.
- Keep any local `#[allow(...)]` narrow and explain the invariant at the use site, following the sanctioned exception pattern documented in `clippy.toml` and focused test allowances in `crates/wcore-cli/tests/plugin_install_smoke.rs`.

## Import Organization

**Order:**
1. Import `std` modules first, grouped together, as in `crates/wcore-providers/src/openai.rs` and `crates/wcore-repomap/src/lib.rs`.
2. Import third-party crates next, with related items combined in braces, as in the `async_trait`, `reqwest`, `serde_json`, and `tokio` group in `crates/wcore-providers/src/openai.rs`.
3. Import workspace crates after a blank line, followed by `crate::...` or `super::...` local modules, as in `crates/wcore-tools/src/git.rs` and `crates/wcore-agent/tests/common/mod.rs`.

**Path Aliases:**
- Rust package names declared under `[workspace.dependencies]` in `Cargo.toml` are imported directly as `wcore_*`; no source-path alias layer is used.
- Use `crate::` for same-crate modules and `super::*` primarily inside inline test modules, following `crates/wcore-tools/src/git.rs` and `crates/wcore-memory/src/error.rs`.
- Use explicit `as` aliases only to disambiguate or shorten a concrete symbol, such as `Dispatcher as SlashDispatcher` in `crates/wcore-cli/src/main.rs`.

## Error Handling

**Patterns:**
- Define public, structured, matchable errors with `thiserror`, including `#[from]` and `#[source]` where the source is meaningful; `crates/wcore-memory/src/error.rs` is the reference pattern.
- Use `anyhow::Result` and `anyhow::bail!` at application and binary orchestration boundaries, as in credential-file validation in `crates/wcore-cli/src/main.rs`.
- Propagate recoverable failures with `?`, add operation-specific context in the mapped message, and use early returns for invalid input; examples appear in `crates/wcore-cli/src/main.rs` and `crates/wcore-tools/src/git.rs`.
- Return domain results rather than panicking in production paths; tool failures become `ToolResult { content, is_error }` in `crates/wcore-tools/src/git.rs`, while typed subsystems use error enums such as `MemoryError` in `crates/wcore-memory/src/error.rs`.
- Reserve `expect` or `unwrap` for tests or locally proven invariants with an explanatory message, matching the project rule in `AGENTS.md` and the strict lint boundary in `crates/wcore-evolve/src/lib.rs`.

## Logging

**Framework:** `tracing`, configured with `tracing_subscriber` in `crates/wcore-cli/src/main.rs`.

**Patterns:**
- Emit structured fields with `%` formatting and an explicit target when stable filtering is useful, as in `crates/wcore-cron/src/runner.rs`, `crates/wcore-tools/src/web_fetch.rs`, and `crates/wcore-protocol/src/reader.rs`.
- Use `debug!` and `trace!` for lifecycle or diagnostic detail, `info!` for operator-visible state changes, `warn!` for recoverable degradation, and `error!` for failed required operations; examples span `crates/wcore-channel-signal/src/subprocess.rs` and `crates/wcore-sandbox/src/lib.rs`.
- Keep user-facing protocol or terminal output on the output-sink surfaces rather than treating logs as product output; the separation is visible in `crates/wcore-cli/src/main.rs` and `crates/wcore-agent/src/output/`.
- Avoid logging secrets and prefer identifiers, paths, status, or error metadata; credential-facing implementations are isolated in `crates/wcore-config/src/credentials.rs` and `crates/wcore-config/src/confidential_blob.rs`.

## Comments

**When to Comment:**
- Use `//!` at module roots to state purpose, boundaries, and security invariants, following `crates/wcore-tools/src/git.rs`, `crates/wcore-permissions/src/lib.rs`, and `crates/wcore-repomap/src/lib.rs`.
- Use inline comments to explain why a non-obvious choice exists, especially protocol compatibility, cross-platform behavior, concurrency, security, or durability; examples appear in `crates/wcore-types/src/message.rs`, `.cargo/config.toml`, and `crates/wcore-cli/src/main.rs`.
- Keep ordinary control flow self-explanatory; comments should capture invariants and consequences rather than restate statements, following the focused helper implementation in `crates/wcore-tools/src/git.rs`.

**JSDoc/TSDoc:**
- Not applicable to primary Rust code. Use Rustdoc `///` on public types, fields, traits, and methods, with intra-doc links where useful; see `crates/wcore-types/src/message.rs` and `crates/wcore-memory/src/error.rs`.
- Document wire shapes, safety constraints, and caller obligations on public APIs, as shown by `MessageCacheHint` in `crates/wcore-types/src/message.rs` and `GitOp` in `crates/wcore-tools/src/git.rs`.

## Function Design

**Size:** Keep helpers single-purpose and extract named operations when a branch encodes a reusable invariant; examples include `run_git` in `crates/wcore-tools/src/git.rs` and `physical_attempt_server` in `crates/wcore-agent/tests/common/mod.rs`. The repository's explicit size and single-responsibility rules are in `AGENTS.md`.

**Parameters:** Borrow with `&str`, `&Path`, and slices when ownership is unnecessary; accept owned values at serialization, async-task, or trait boundaries. Examples include `run_git(cwd: &str, args: &[&str])` in `crates/wcore-tools/src/git.rs` and `BoundCompositeFixtureManifest::from_artifacts` usage in `crates/wcore-eval-scenarios/tests/fixture_manifest_contract.rs`.

**Return Values:** Return `Result<T, E>` for fallible operations, `Option<T>` for absence, domain enums for state transitions, and small structs for multi-field outcomes; examples are `MemoryError` in `crates/wcore-memory/src/error.rs`, `SlashOrRun` in `crates/wcore-cli/src/main.rs`, and `ToolResult` in `crates/wcore-tools/src/git.rs`.

## Module Design

**Exports:** Declare domain modules in each crate's `src/lib.rs` and re-export only the intended public facade. `crates/wcore-types/src/lib.rs`, `crates/wcore-permissions/src/lib.rs`, and `crates/wcore-cli/src/lib.rs` show this pattern.

**Barrel Files:** Rust crate roots act as controlled barrels; nested domains use `mod.rs` only where the directory itself is a domain, such as `crates/wcore-agent/src/orchestration/mod.rs`. Keep implementation paths private unless downstream crates need them, and place shared behavior in the lowest existing crate allowed by the dependency graph documented in `AGENTS.md` and `Cargo.toml`.

---

*Convention analysis: 2026-07-18*
