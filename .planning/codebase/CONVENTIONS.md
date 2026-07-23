# Coding Conventions

**Analysis Date:** 2026-07-23

## Naming Patterns

**Files:**
- One responsibility per `.rs` file; error types live in a dedicated `error.rs` per crate (e.g. `crates/wcore-sandbox/src/error.rs`, `crates/wcore-memory/src/error.rs`, `crates/wcore-plugin-api/src/error.rs`).
- Files kept under ~1000 lines by AGENTS.md mandate; when a module approaches the limit, extract sub-modules (e.g. `crates/wcore-agent/src/orchestration/workflow/` is split into `dsl.rs`, `error.rs`, `limits.rs`, `pipeline.rs`, `runner.rs` rather than one monolithic file).

**Crates:**
- `wcore-*` prefix for internal library crates; `wayland-*` prefix reserved for plugin crates that go through `wcore-plugin-api` (e.g. `wayland-browser`, `wayland-cua`, `wayland-ollama`, `wayland-ijfw`).

**Types/Errors:**
- Per-crate error enum named `<Crate>Error` (e.g. `SandboxError`) implementing `thiserror::Error`.

## Code Style

**Formatting:**
- `cargo fmt --all` — CI enforces zero diff. Run via `vx cargo fmt --all` (all cargo invocations route through `vx` per `vx.toml`).

**Linting:**
- `cargo clippy --workspace --all-targets -- -D warnings` (see `justfile:76`, target `lint`) — must pass with zero warnings, no exceptions.

## Error Handling

**Public API errors — `thiserror`:**
- Every crate with a public error surface defines a `thiserror::Error` enum in `src/error.rs`. Example pattern from `crates/wcore-sandbox/src/error.rs`:
  ```rust
  #[derive(Debug, Error)]
  pub enum SandboxError {
      #[error("unknown sandbox backend selection: {0}")]
      UnknownBackend(String),
      #[error("sandbox child output exceeded {limit_bytes} bytes")]
      OutputLimitExceeded { limit_bytes: usize },
      ...
  }
  ```
  Variants are matchable by callers; each carries a `#[error("...")]` message, often with a doc comment explaining the invariant it protects (e.g. `UnsafeBypassSource` documents why config/env can never disable containment).
- Both `anyhow` and `thiserror` are declared as workspace dependencies in most crates (`crates/wcore-config/Cargo.toml`, `crates/wcore-tools/Cargo.toml`) — `thiserror` for the crate's own public error type, `anyhow` for propagating errors internally/at application boundaries (per AGENTS.md §Code style).
- Never `unwrap()` in production code unless the invariant is proven and the comment explains why (AGENTS.md rule, workspace-wide).
- Never silently swallow errors.

## No Hardcoded Provider Quirks (ProviderCompat)

**The single most important architectural rule in this codebase.** Never branch on provider identity (`if base_url.contains("api.openai.com")`) inside provider or engine code. All provider-specific behavior routes through the `ProviderCompat` config struct.

```rust
// WRONG
if self.base_url.contains("api.openai.com") {
    body["max_completion_tokens"] = json!(max_tokens);
}

// CORRECT
let field = self.compat.max_tokens_field.as_deref().unwrap_or("max_tokens");
body[field] = json!(request.max_tokens);
```

To add new compat behavior:
1. Add an `Option<T>` field to `ProviderCompat` (in `wcore-config`).
2. Set its default in the relevant preset function (e.g. `openai_defaults()`).
3. Read it via `self.compat.field_name` at provider call sites — never re-derive it from URL/model-name sniffing.

All providers implement the `LlmProvider` trait (`wcore-providers`); the agent engine only ever sees provider-neutral types (`LlmRequest`, `LlmEvent`, `Message`, `ContentBlock`). Format conversion is isolated inside each provider's `build_messages()` / `build_request_body()`.

## Process Spawning — `wcore_config::shell`

All process spawning in the workspace goes through `crates/wcore-config/src/shell.rs`. Two distinct modes, chosen deliberately per call site — never `Command::new("sh"/"bash"/"cmd")` directly:

**Argv mode — `shell_command_argv(program, &[args])`:**
- The OS resolves `program` against `PATH`/`PATHEXT`; each arg is a separate argv entry; no shell interpreter is involved.
- Shell metacharacters (`;`, `&&`, `|`, `$()`, backticks, redirection, glob) reach the child as literal bytes — never interpreted.
- **Mandatory for any command whose arguments include LLM-supplied data.**
- Example: `GitTool` uses argv mode for every git operation, with `.current_dir(cwd)` for the working directory.

**Shell-string mode — `shell_command(str)` / `shell_command_builder(str)`:**
- Runs `sh -c <str>` on Unix, `cmd /C <str>` on Windows — metacharacters ARE interpreted.
- Use only where shell semantics are the actual contract:
  - `BashTool` — the shell-tool surface itself (chaining/piping/redirection is the point).
  - MCP stdio transport's program-launch path (needs PATHEXT shim resolution for `.cmd`/`.bat` on Windows).
  - Skill `!shell:` directives.
- **Never `format!`-interpolate LLM-supplied data into a shell-string command** — every such site is a shell-injection vector (closed workspace-wide during "Wave SA"; see `SECURITY-v0.2.0.md` BLOCKER #1 if present in repo history).

External CLI tools that differ per platform (`grep` vs `findstr`) select via `cfg!(windows)` or equivalent — never hardcoded assuming Unix tools exist.

## Cross-Platform Discipline

CI runs macOS, Linux, and Windows (`.github/workflows/ci.yml`). Local dev on this Mac worktree can only exercise the current platform's `#[cfg(...)]` branches; other-platform branches are verified by CI only — **do not run cargo/clippy/nextest locally on this Mac** (build is Hetzner/self-hosted-only per project convention; see TESTING.md).

**Paths:**
- Never hardcode platform paths (`/tmp/...`, `C:\...`) in production code — use `Path::join()`, `dirs::config_dir()`, `tempfile::tempdir()`.
- Hardcoded Unix paths (`Path::new("/foo/...")`) are acceptable in tests only for pure string ops (join/display) or nonexistent-path error handling; add `#[cfg(unix)]`/`#[cfg(windows)]` variants when the path is passed to `is_absolute()`, `validate_memory_path()`, or similar platform-sensitive checks.
- Use `std::path::Component::Normal` (not raw byte length) when checking path depth — prefix/root components differ across platforms.

**Platform differences centralization:**
- Any platform-specific behavior (paths, permissions, shell commands, line endings) is wrapped in one centralized function; every call site uses that function. Never scatter raw `cfg!(windows)` / `cfg(target_os = ...)` detection across multiple crates or modules for the same concern.

## Duplicate Code

- If multiple crates need the same functionality, it is extracted to the lowest crate in the dependency graph where it semantically belongs (see crate map in `AGENTS.md`) — never copy-pasted or reimplemented per-crate.
- New crates are never created for a single shared function.

## Module Design

**Crate dependency direction:** strictly downward (bottom: `wcore-types`, `wcore-compact` → mid: `wcore-config`, `wcore-protocol`, `wcore-providers`, `wcore-tools`, etc. → top: `wcore-agent`, `wcore-cli`). Circular or upward references are forbidden; `cargo metadata` is used to verify dependency-graph changes fit.

**Plugin isolation:** `wcore-plugin-api` has zero internal deps beyond `wcore-types`/`wcore-protocol`; enforced by a `build.rs` lint. Plugin crates (`wayland-browser`, `wayland-cua`) mirror tool specs through `wcore-plugin-api` types and never depend directly on `wcore-browser`/`wcore-cua` (audit finding F2).

**File organization:** organize by domain responsibility, not by type (no generic `utils.rs` grab-bags spanning unrelated concerns).

---

*Convention analysis: 2026-07-23*
</content>
