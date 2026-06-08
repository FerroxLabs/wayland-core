# AGENTS.md

Drop-in operating instructions for coding agents. Read this file before every task.

**Working code only. Finish the job. Plausibility is not correctness.**

This file follows the [AGENTS.md](https://agents.md) open standard. Claude Code, Codex, Cursor, Windsurf, Copilot, Aider, Devin, Amp read it natively. `CLAUDE.md` imports this file via `@AGENTS.md`; `GEMINI.md` is a symlink (or copy) of it.

---

## 0. Non-negotiables

These rules override everything else in this file when in conflict:

1. **No flattery, no filler.** Skip openers like "Great question", "You're absolutely right", "Excellent idea", "I'd be happy to". Start with the answer or the action.
2. **Disagree when you disagree.** If the user's premise is wrong, say so before doing the work. Agreeing with false premises to be polite is the single worst failure mode in coding agents.
3. **Never fabricate.** Not file paths, not commit hashes, not API names, not test results, not library functions. If you don't know, read the file, run the command, or say "I don't know, let me check."
4. **Stop when confused.** If the task has two plausible interpretations, ask. Do not pick silently and proceed.
5. **Touch only what you must.** Every changed line must trace directly to the user's request. No drive-by refactors, reformatting, or "while I was in there" cleanups.

---

## 1. Before writing code

**Goal: understand the problem and the codebase before producing a diff.**

- State your plan in one or two sentences before editing. For anything non-trivial, produce a numbered list of steps with a verification check for each.
- Read the files you will touch. Read the files that call the files you will touch. Claude Code: use subagents for exploration so the main context stays clean.
- Match existing patterns in the codebase. If the project uses pattern X, use pattern X, even if you'd do it differently in a greenfield repo.
- Surface assumptions out loud: "I'm assuming you want X, Y, Z. If that's wrong, say so." Do not bury assumptions inside the implementation.
- If two approaches exist, present both with tradeoffs. Do not pick one silently. Exception: trivial tasks (typo, rename, log line) where the diff fits in one sentence.

---

## 2. Writing code: simplicity first

**Goal: the minimum code that solves the stated problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code. No configurability, flexibility, or hooks that were not requested.
- No error handling for impossible scenarios. Handle the failures that can actually happen.
- If the solution runs 200 lines and could be 50, rewrite it before showing it.
- If you find yourself adding "for future extensibility", stop. Future extensibility is a future decision.
- Bias toward deleting code over adding code. Shipping less is almost always better.

The test: would a senior engineer reading the diff call this overcomplicated? If yes, simplify.

---

## 3. Surgical changes

**Goal: clean, reviewable diffs. Change only what the request requires.**

- Do not "improve" adjacent code, comments, formatting, or imports that are not part of the task.
- Do not refactor code that works just because you are in the file.
- Do not delete pre-existing dead code unless asked. If you notice it, mention it in the summary.
- Do clean up orphans created by your own changes (unused imports, variables, functions your edit made obsolete).
- Match the project's existing style exactly: indentation, quotes, naming, file layout.

The test: every changed line traces directly to the user's request. If a line fails that test, revert it.

---

## 4. Goal-driven execution

**Goal: define success as something you can verify, then loop until verified.**

Rewrite vague asks into verifiable goals before starting:

- "Add validation" becomes "Write tests for invalid inputs (empty, malformed, oversized), then make them pass."
- "Fix the bug" becomes "Write a failing test that reproduces the reported symptom, then make it pass."
- "Refactor X" becomes "Ensure the existing test suite passes before and after, and no public API changes."
- "Make it faster" becomes "Benchmark the current hot path, identify the bottleneck with profiling, change it, show the benchmark is faster."

For every task:

1. State the success criteria before writing code.
2. Write the verification (test, script, benchmark, screenshot diff) where practical.
3. Run the verification. Read the output. Do not claim success without checking.
4. If the verification fails, fix the cause, not the test.

---

## 5. Tool use and verification

- Prefer running the code to guessing about the code. If a test suite exists, run it. If a linter exists, run it. If a type checker exists, run it.
- Never report "done" based on a plausible-looking diff alone. Plausibility is not correctness.
- When debugging, address root causes, not symptoms. Suppressing the error is not fixing the error.
- For UI changes, verify visually: screenshot before, screenshot after, describe the diff.
- Use CLI tools (`gh`, `aws`, `gcloud`, `kubectl`) when they exist. They are more context-efficient than reading docs or hitting APIs unauthenticated.
- When reading logs, errors, or stack traces, read the whole thing. Half-read traces produce wrong fixes.

---

## 6. Session hygiene

- Context is the constraint. Long sessions with accumulated failed attempts perform worse than fresh sessions with a better prompt.
- After two failed corrections on the same issue, stop. Summarize what you learned and ask the user to reset the session with a sharper prompt.
- Use subagents (Claude Code: "use subagents to investigate X") for exploration tasks that would otherwise pollute the main context with dozens of file reads.
- When committing, write descriptive commit messages (subject under 72 chars, body explains the why). No "update file" or "fix bug" commits. Follow this repo's existing commit-style; do not add `Co-Authored-By:` unless the existing history uses it.

---

## 7. Communication style

- Direct, not diplomatic. "This won't scale because X" beats "That's an interesting approach, but have you considered...".
- Concise by default. Two or three short paragraphs unless the user asks for depth. No padding, no restating the question, no ceremonial closings.
- When a question has a clear answer, give it. When it does not, say so and give your best read on the tradeoffs.
- Celebrate only what matters: shipping, solving genuinely hard problems, metrics that moved. Not feature ideas, not scope creep, not "wouldn't it be cool if".
- No excessive bullet points, no unprompted headers, no emoji. Prose is usually clearer than structure for short answers.

---

## 8. When to ask, when to proceed

**Ask before proceeding when:**
- The request has two plausible interpretations and the choice materially affects the output.
- The change touches something you've been told is load-bearing, versioned, or has a migration path.
- You need a credential, a secret, or a production resource you don't have access to.
- The user's stated goal and the literal request appear to conflict.

**Proceed without asking when:**
- The task is trivial and reversible (typo, rename a local variable, add a log line).
- The ambiguity can be resolved by reading the code or running a command.
- The user has already answered the question once in this session.

---

## 9. Self-improvement loop

**This file is living. Keep it short by keeping it honest.**

After every session where the agent did something wrong:

1. Ask: was the mistake because this file lacks a rule, or because the agent ignored a rule?
2. If lacking: add the rule under "Project Learnings" below, written as concretely as possible ("Always use X for Y" not "be careful with Y").
3. If ignored: the rule may be too long, too vague, or buried. Tighten it or move it up.
4. Every few weeks, prune. For each line, ask: "Would removing this cause the agent to make a mistake?" If no, delete. Bloated AGENTS.md files get ignored wholesale.

---

## 10. Project context — wayland-core

wayland-core is a **multi-provider AI agent CLI** written in Rust. It connects to
LLM providers (Anthropic, OpenAI, AWS Bedrock, Google Vertex AI), orchestrates
built-in tools (Read, Write, Edit, Bash, Grep, Glob, Spawn), supports MCP
servers, skills, hooks, and long-term memory. It also exposes a JSON stream
protocol for host integration (e.g. Electron-based AionUI).

Tech stack: Rust 2021 edition, stable toolchain, Cargo workspace under `crates/`.

### Crate Map

Dependencies flow **downward** — never introduce circular or upward references.

| Layer | Crate | Responsibility |
|-------|-------|----------------|
| Bottom | `wcore-types` | Shared provider-neutral data types (LLM, message, tool) — zero internal deps |
| Bottom | `wcore-compact` | Context compression algorithms (folding, sanitization, tokenization) |
| Mid | `wcore-config` | Configuration, ProviderCompat, auth, hooks, **cross-platform shell helpers** |
| Mid | `wcore-protocol` | JSON stream protocol (events, commands, approval manager) for host integration |
| Mid | `wcore-plugin-api` | Plugin trait, PluginContext, scoped registries — zero internal deps beyond wcore-types/wcore-protocol; isolation boundary enforced by build.rs lint |
| Mid | `wcore-providers` | LLM provider implementations (Anthropic, OpenAI, Bedrock, Vertex) |
| Mid | `wcore-tools` | Built-in agent tools (Read, Write, Edit, Bash, Grep, Glob, Spawn) |
| Mid | `wcore-mcp` | MCP (Model Context Protocol) client |
| Mid | `wcore-skills` | Skills system (prompt snippets, hooks, permissions, shell expansion) |
| Mid | `wcore-memory` | Long-term cross-session memory (user prefs, feedback, project context) |
| Mid | `wcore-observability` | Trace schema, span sinks, OTLP exporter, prompt-cache discipline — sits between `wcore-types`/`wcore-config` and `wcore-agent`; protocol crate stays decoupled via opaque `serde_json::Value` payloads |
| Mid | `wcore-repomap` | Aider-style light symbol extractor + codebase index. Deliberately isolated — NO internal `wcore-*` deps |
| Mid | `wcore-browser` | Multi-backend browser tool family (Camoufox primary, chromiumoxide fallback, Browserbase cloud); ARIA-tree-first surface; BrowserPolicy network boundary; BrowserSupervisor lifecycle |
| Mid | `wcore-cua` | Multi-platform computer use (macOS / Linux X11 / Linux Wayland / Windows); background-mode invariant; CuaPolicy gating |
| Mid | `wcore-eval` | Acceptance / evaluation gate runner (precision/recall thresholds, eval-gate justfile target) |
| Mid | `wcore-evolve` | W10B GEPA evolution loop — child generation, scoring, retention. Paraphrase mutator backed by real `LlmParaphraseProvider` in production; fixture-replay in tests |
| Mid | `wcore-sandbox` | Platform-specific shell sandbox: bwrap (Linux), sandbox-exec (macOS), AppContainer + Job Object (Windows). Probes real spawn — never trust shallow API checks. |
| Top | `wcore-agent` | Agent engine, session management, orchestration. Hosts `HostBrowserRegistrar` + `HostCuaRegistrar` so plugin-side `BrowserToolSpec`/`CuaToolSpec` mirrors are bound to real `wcore-browser`/`wcore-cua` backends without violating audit F2 (plugins still have NO `wcore-browser`/`wcore-cua` dep). `src/orchestration/workflow/` hosts the **ForgeFlows (Dynamic Workflows)** engine: declarative RON lowers to the existing `GraphConfig` IR and executes via `WorkflowRunner` over the `AgentSpawner`/FleetDispatcher spawner path (NOT the per-turn `ExecutionGraph` walker) — see `docs/workflows.md` |
| Top | `wcore-cli` | CLI binary entry point |
| Plugin | `wayland-ollama` | Ollama local-inference provider (registers via `register_providers` only) |
| Plugin | `wayland-browser` | Plugin packaging of `wcore-browser` — `BrowserToolSpec` mirror through `wcore-plugin-api` (no `wcore-browser` dep, per audit F2) |
| Plugin | `wayland-cua` | Plugin packaging of `wcore-cua` — `CuaToolSpec` mirror through `wcore-plugin-api` (no `wcore-cua` dep, per audit F2) |
| Plugin | `wayland-ijfw` | IJFW anchor plugin — exercises every `register_*` surface (tools + hooks + agents + skills + rules + MCP server) through `wcore-plugin-api` mirror types |

When adding new functionality, place it in the **lowest crate where it
semantically belongs**. Don't create a new crate just for one shared function.
Run `cargo metadata` to verify dependency changes fit the graph.

### Commands

```bash
cargo build            # Build
cargo test             # Run all tests
cargo nextest run      # Run via nextest (preferred for matrix/per-test detail)
cargo clippy           # Lint
cargo fmt --all        # Format (CI enforces this)
just push              # Lint-fix → fmt → auto-commit-fixes → test → git push
```

**Pushing code: always use `just push` instead of `git push`.** It runs lint-fix → fmt → auto-commit-fixes → test → `git push`, preventing CI failures and silently fixing trivial drift before the push. Supports the same arguments as `git push`.

#### One-time setup: install `vx`

The `justfile` and CI workflows route every tool invocation through `vx` ([loonghao/vx](https://github.com/loonghao/vx)) so the Rust + `just` versions pinned in `vx.toml` are used deterministically across local dev and CI. **`just push` won't work without it.**

```bash
# macOS arm64 — adjust the asset name for your platform
# (https://github.com/loonghao/vx/releases/latest for other targets)
curl -sL https://github.com/loonghao/vx/releases/download/v0.8.36/vx-0.8.36-aarch64-apple-darwin.tar.gz \
  | tar -xz -C /tmp \
  && install -m 755 /tmp/vx-aarch64-apple-darwin/vx ~/.local/bin/vx

vx --version          # should print "vx 0.8.36"
vx just --list        # auto-installs rust + just on first run
```

`vx.toml` only tracks Rust + just. **`just push` additionally needs `cargo-nextest`** — install once with `cargo install cargo-nextest --locked`. CI installs `cargo-nextest` and `cargo-audit` via `taiki-e/install-action` in the workflows.

### Code style

- `cargo clippy` must pass without warnings; `cargo fmt` must pass without diffs.
- Comments in English, commit messages in English.
- Errors: `thiserror` for public API error types (structured, matchable); `anyhow` for internal/application-level propagation. Never silently swallow errors; never `unwrap()` in production code unless the invariant is proven and commented.

### File organization

- Each module (`.rs` file) follows the **single responsibility principle** — one clear purpose per file.
- Keep files under 1000 lines; extract sub-modules when approaching the limit.
- Organize by domain responsibility, not by type.

### Architecture principles

#### No Hardcoded Provider Quirks

**This is the single most important rule for this codebase.**

Handle provider differences through the **`ProviderCompat` configuration layer**, not through hardcoded conditionals.

```rust
// WRONG: hardcoded provider detection
if self.base_url.contains("api.openai.com") {
    body["max_completion_tokens"] = json!(max_tokens);
}

// CORRECT: read from compat config
let field = self.compat.max_tokens_field.as_deref().unwrap_or("max_tokens");
body[field] = json!(request.max_tokens);
```

If you need a new compat behavior:
1. Add an `Option<T>` field to `ProviderCompat`
2. Set its default in the appropriate preset function (e.g. `openai_defaults()`)
3. Use it in provider code via `self.compat.field_name`

All providers implement the `LlmProvider` trait. The engine sees only provider-neutral types (`LlmRequest`, `LlmEvent`, `Message`, `ContentBlock`). Format conversion happens inside each provider's `build_messages()` / `build_request_body()`.

> **Deep dive:** see [docs/providers.md](docs/providers.md) for provider setup, auth, aliases, and profile inheritance.

#### Centralize Platform Differences

Any platform-specific behavior (paths, permissions, shell commands, line endings, etc.) must be wrapped in a single centralized function. All call sites use that function — never scatter raw platform detection across multiple crates or modules. See [Cross-Platform](#cross-platform) for concrete rules.

#### No Duplicate Code Across Crates

If multiple crates need the same functionality, extract it to the appropriate existing crate in the dependency graph — don't copy-paste or reimplement. Choose the extraction target based on where it semantically belongs and where it minimizes dependency changes.

### Cross-platform

CI runs on macOS, Linux, **and Windows**. Local dev can only test the current platform's `#[cfg(...)]` code — other platform branches are verified by CI alone.

#### Paths

- Never hardcode platform paths (`/tmp/...`, `C:\...`) in production code. Use `Path::join()`, `dirs::config_dir()`, `tempfile::tempdir()`, etc.
- In tests, hardcoded Unix paths (`Path::new("/foo/...")`) are fine for pure string operations (join, display) or nonexistent-path error handling. Only add `#[cfg(unix)]` / `#[cfg(windows)]` variants when the path is passed to `is_absolute()`, `validate_memory_path()`, or similar platform-sensitive checks.
- Use `std::path::Component::Normal` (not byte length) when checking path depth — prefix/root components differ across platforms.

#### Shell Execution

All process spawning goes through `wcore_config::shell`. Two modes:

- **Argv mode — `shell_command_argv(program, &[args])`. Use this for any command whose arguments include LLM-supplied data.** The OS resolves `program` against `PATH` (and `PATHEXT` on Windows), each `arg` is a separate argv entry, and NO shell interpreter is involved. Shell metacharacters in arguments (`;`, `&&`, `|`, `$()`, backticks, redirection, glob expansion) are never interpreted — they reach the child program as literal bytes. This is the only safe mode for attacker-controlled input. Example: `GitTool` uses argv mode for every op, with `.current_dir(cwd)` setting the working directory.

- **Shell-string mode — `shell_command(str)` / `shell_command_builder(str)`.** Runs `sh -c <str>` on Unix and `cmd /C <str>` on Windows. Shell metacharacters in the string ARE interpreted. Use ONLY when the semantics genuinely require a shell — for example:
  - `BashTool` (the shell-tool surface — chaining via `&&`, pipes, redirection is the contract).
  - MCP stdio transport's program-launch path (needs PATHEXT shim resolution for `.cmd`/`.bat` wrappers on Windows).
  - Skill `!shell:` directives.

  In shell-string mode, **never `format!`-interpolate LLM-supplied data** into the command string. Every such site is a shell injection (closed during Wave SA — see SECURITY-v0.2.0.md BLOCKER #1).

- Never call `Command::new("sh")`, `Command::new("bash")`, or `Command::new("cmd")` directly — these are platform-specific and bypass the central helper.

- External CLI tools that differ across platforms (e.g. `grep` vs `findstr`) must use `cfg!(windows)` branches or equivalent platform-aware selection.

### Test organization

| Location | What goes there |
|----------|----------------|
| Inline `#[cfg(test)]` in each `.rs` file | Unit tests for that module's internals |
| `crates/<crate>/tests/` | Integration tests for that crate |

Unit tests target internal logic and code paths. Integration tests target functional requirements and public API — write them from the spec, not from reading the implementation.

Every test must verify a meaningful behavior or edge case. No trivial tests that just assert the happy path without checking boundaries, error conditions, or non-obvious logic.

### Documentation

Key references in `docs/` (don't duplicate their content here):

| Document | Covers |
|----------|--------|
| [getting-started.md](docs/getting-started.md) | Installation, CLI usage, config format and cascading precedence |
| [providers.md](docs/providers.md) | Provider setup, auth, ProviderCompat, custom aliases, profiles |
| [tools.md](docs/tools.md) | Built-in tool reference and execution flow |
| [skills.md](docs/skills.md) | Writing skills, front matter, shell expansion, conditional activation |
| [mcp.md](docs/mcp.md) | MCP server integration, transport types, deferred loading |
| [advanced.md](docs/advanced.md) | Sub-agents, hooks, memory, plan mode, context compression |
| [json-stream-protocol.md](docs/json-stream-protocol.md) | JSON Lines protocol spec for host integration (e.g. AionUI) |
| [troubleshooting.md](docs/troubleshooting.md) | Common errors and solutions |

### Forbidden

- Hardcoded provider quirks (use `ProviderCompat` — see above).
- Raw `Command::new("sh"/"bash"/"cmd")` (use `wcore_config::shell` helpers).
- Shell-string interpolation of LLM-supplied data (use argv mode).
- New crates created for a single shared function (extract to the lowest existing crate).

---

## 11. Project Learnings

**Accumulated corrections. This section is for the agent to maintain, not just the human.**

When the user corrects your approach, append a one-line rule here before ending the session. Write it concretely ("Always use X for Y"), never abstractly ("be careful with Y"). If an existing line already covers the correction, tighten it instead of adding a new one. Remove lines when the underlying issue goes away (model upgrades, refactors, process changes).

- (empty)
