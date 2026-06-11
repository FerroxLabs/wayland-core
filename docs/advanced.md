# Advanced Features

## Sub-Agent Spawning

The LLM can use the Spawn tool to create independent sub-agents that run tasks in parallel. Each sub-agent has its own conversation context and full tool set, but shares the parent agent's LLM provider (connection pool reuse).

### Use Cases

- "Search these 3 files simultaneously and summarize each"
- "Run tests and lint in parallel"
- "Search for X in the codebase while reading Y"

### Limits

| Setting | Default | Description |
|---------|---------|-------------|
| Max parallel sub-agents | 5 | Prevents resource exhaustion |
| Sub-agent max turns | 10 | Per sub-agent conversation turn limit |
| Sub-agent max tokens | 4096 | Per sub-agent response token limit |

### Behavior

- Sub-agents auto-approve all tool calls (no confirmation prompts)
- Sub-agents do not save sessions
- Sub-agents run silently (no stdout output)
- All results are merged and returned to the parent agent

---

## Hook System

Event-driven hooks execute shell commands at specific points in the tool lifecycle, enabling auto-formatting, linting, auditing, and more.

### Hook Types

| Type | Trigger | Behavior |
|------|---------|----------|
| `pre_tool_use` | Before tool execution | Non-zero exit blocks the tool |
| `post_tool_use` | After tool execution | Non-blocking; errors are logged |
| `stop` | When agent session ends | Non-blocking |

### Configuration

```toml
# Auto-format Rust files after modification
[[hooks.post_tool_use]]
name = "rustfmt"
tool_match = ["Write", "Edit"]
file_match = ["*.rs"]
command = "rustfmt ${TOOL_INPUT_FILE_PATH}"

# Auto-format TypeScript files after modification
[[hooks.post_tool_use]]
name = "prettier"
tool_match = ["Write", "Edit"]
file_match = ["*.ts", "*.tsx"]
command = "npx prettier --write ${TOOL_INPUT_FILE_PATH}"

# Audit Bash commands
[[hooks.post_tool_use]]
name = "audit-log"
tool_match = ["Bash"]
command = "echo \"$(date): ${TOOL_INPUT_COMMAND}\" >> .wayland-core/audit.log"

# Run lint on session end
[[hooks.stop]]
name = "final-lint"
command = "cargo clippy --quiet 2>&1 | tail -5"
```

### Environment Variables

Hook commands can reference these variables via `${VAR}` syntax:

| Variable | Description |
|----------|-------------|
| `TOOL_NAME` | Tool name |
| `TOOL_INPUT` | Full tool input JSON |
| `TOOL_INPUT_FILE_PATH` | File path (if the tool has a file_path parameter) |
| `TOOL_INPUT_COMMAND` | Command (if the tool has a command parameter) |
| `TOOL_INPUT_PATTERN` | Search pattern (if the tool has a pattern parameter) |
| `TOOL_OUTPUT` | Tool output (post_tool_use only) |

### Matching Rules

- `tool_match`: glob patterns matching tool names; empty = match all
- `file_match`: glob patterns matching file paths; empty = match all
- Default timeout: 30 seconds, configurable via `timeout_ms`

---

## Prompt Caching (Anthropic)

Prompt caching stores system prompts and tool definitions on Anthropic's servers, so subsequent requests only process the changed parts.

- **First request**: full input token cost + 25% write premium
- **Subsequent requests**: cached portion costs only 10%
- **Cache TTL**: 5 minutes (auto-renewed on each hit)

### Configuration

```toml
[providers.anthropic]
api_key = "sk-ant-xxx"
prompt_caching = true   # default true (Anthropic only)
```

### Token Stats

With caching enabled, stats show cache data:

```
[turns: 3 | tokens: 100 in (5000 cached) / 200 out | cache: 5000 created, 5000 read]
```

---

## VCR Recording & Replay

Record real API interactions and replay them in tests — no API key or network needed.

### Usage

```bash
# Record mode
VCR_MODE=record VCR_CASSETTE=tests/cassettes/my_test.json \
  wayland-core -k sk-ant-xxx "Read Cargo.toml"

# Replay mode (in tests)
VCR_MODE=replay VCR_CASSETTE=tests/cassettes/my_test.json \
  wayland-core "Read Cargo.toml"
```

### Features

- Auto-sanitization: sensitive headers (api-key, auth, token) are replaced with `[REDACTED]` during recording
- JSON-formatted cassette files, editable by hand
- Supports recording/replay of SSE streaming responses

---

## AGENTS.md Hierarchical Loading

AGENTS.md files provide project-specific instructions that are automatically injected into the system prompt. Files are discovered hierarchically and merged from remote to near:

1. **Global**: `<config_dir>/wayland-core/AGENTS.md` — user-level instructions for all projects
2. **Project hierarchy**: Walk up from cwd to the git root (or home directory), collecting every `AGENTS.md` found along the way

Files closer to the working directory appear later in the prompt and take precedence (via LLM recency bias). Each file is annotated with its absolute path for traceability.

### @include Directive

AGENTS.md files can include other files using `@` syntax:

- `@FILENAME` or `@./relative/path` — relative to the AGENTS.md file's directory
- `@~/path` — relative to home directory
- `@/absolute/path` — absolute path

Paths inside fenced code blocks are ignored. Includes are recursive (up to depth 5) with circular reference detection. Non-existent files and non-text files are silently skipped.

### Example

Given this structure:

```
my-workspace/
├── .git/
├── AGENTS.md          ← workspace rules
└── packages/
    └── server/
        └── AGENTS.md  ← server-specific rules
```

Running aion in `packages/server/` produces a system prompt containing both files, workspace first, then server.

---

## Memory System

Persistent, file-based memory that allows the agent to retain project-specific knowledge across sessions. Memory is automatically loaded into the system prompt at conversation start.

### Memory Types

| Type | Purpose |
|------|---------|
| `user` | User's role, goals, preferences, knowledge |
| `feedback` | Corrections and confirmations on work approach |
| `project` | Ongoing work context not derivable from code/git |
| `reference` | Pointers to external systems and resources |

### Storage

Memory files live in a per-project directory under the global config:

```
<config_dir>/wayland-core/projects/<sanitized-project-path>/memory/
├── MEMORY.md              # Index (auto-loaded into prompt, max 200 lines)
├── user_role.md
├── feedback_testing.md
└── project_auth_rewrite.md
```

Each memory file uses YAML frontmatter:

```markdown
---
name: auth rewrite
description: Auth middleware rewrite driven by compliance
type: project
---

Auth middleware rewrite is driven by legal/compliance requirements.
```

### Configuration

Memory is enabled by default with no configuration required. The memory directory is auto-resolved from the current working directory.

Override the base directory via environment variable:

```bash
export WCORE_MEMORY_DIR=/custom/path
```

The legacy `AIONRS_MEMORY_DIR` is still honored as a backward-compat alias —
useful if you're migrating from an older `aionrs` configuration. When both are
set, `WCORE_MEMORY_DIR` wins.

### How It Works

1. Agent starts → memory directory resolved from project path
2. `MEMORY.md` index loaded into system prompt (truncated at 200 lines / 25 KB)
3. Agent reads/writes memory files using standard Read/Write tools
4. Agent maintains the `MEMORY.md` index as memories are added or removed

---

## Plan Mode

A read-only exploration mode where the agent focuses on understanding the codebase and producing an implementation plan before making any changes.

### How It Works

1. Agent calls `EnterPlanMode` → tool access restricted to read-only (Read, Grep, Glob)
2. Agent explores code, designs approach, writes a structured plan in its response
3. Agent calls `ExitPlanMode` → full tool access restored, plan optionally saved to disk

### Configuration

```toml
[plan]
enabled = true                    # Register Plan Mode tools (default: true)
plan_directory = ".wayland-core/plans"  # Where plan files are saved
```

### Workflow Phases

When in plan mode, the agent follows a structured 4-phase process:

1. **Understand** — Explore the codebase with read-only tools
2. **Design** — Identify files to modify, code to reuse
3. **Write the plan** — Compose a clear, actionable implementation plan
4. **Submit** — Call `ExitPlanMode` to restore full tool access

---

## Context Compression

A three-tier automatic compaction strategy that prevents context window overflow during long conversations.

### Tiers

| Tier | Trigger | Method | LLM Call |
|------|---------|--------|----------|
| **Microcompact** | Tool result count exceeds threshold or time gap | Clears old tool result content, keeping the N most recent | No |
| **Autocompact** | Input tokens approach context limit | LLM summarizes the conversation | Yes |
| **Emergency** | Input tokens near absolute limit | Blocks further API calls, asks user to start fresh | No |

### How It Works

- **Microcompact** runs automatically: replaces old Read/Bash/Grep/Glob/Write/Edit results with `[Tool result cleared]`, keeping the 5 most recent results intact. Triggered by count (>10 compactable results) or time (>1 hour since last assistant message).

- **Autocompact** triggers when input tokens reach `context_window - output_reserve - autocompact_buffer` (default: 200,000 - 20,000 - 13,000 = 167,000 tokens). The agent calls the LLM to produce a conversation summary, then replaces history with a compact boundary marker. A circuit breaker stops retrying after 3 consecutive failures.

- **Emergency** is the last safety net at `context_window - emergency_buffer` (default: 197,000 tokens). Always active regardless of config. Blocks API calls and prompts the user to compact or start a new conversation.

### Configuration

```toml
[compact]
enabled = true              # Enable compaction system (default: true)
context_window = 200000     # Context window in tokens
output_reserve = 20000      # Reserved for output generation
autocompact_buffer = 13000  # Buffer before autocompact triggers
emergency_buffer = 3000     # Buffer before emergency block
max_failures = 3            # Circuit breaker threshold
micro_keep_recent = 5       # Keep N most recent tool results
```

---

## File State Cache

An LRU cache that tracks files the agent has recently accessed, enabling read deduplication and automatic cache updates on writes.

- **Read dedup**: When the agent reads a file it has already seen (and the file hasn't changed), the cache provides the content without re-reading from disk.
- **Write/Edit auto-update**: After Write or Edit operations, the cache is updated immediately with the new content.
- **Dual eviction**: Entries are evicted when either the entry count limit or the total byte size limit is reached.

### Configuration

```toml
[file_cache]
enabled = true                # Enable file state caching (default: true)
max_entries = 100             # Maximum cached files
max_size_bytes = 26214400     # Max total cache size (25 MB)
```

---

## Output Compaction

Post-processes tool output to reduce token usage. Three levels from lightest to heaviest:

| Level | Transformations |
|-------|----------------|
| `off` | No transformation |
| `safe` (default) | Strip ANSI escape codes, merge consecutive blank lines, collapse carriage-return progress bars |
| `full` | Everything in `safe`, plus: fold repeated lines, compact JSON indentation |

### TOON Encoding

When enabled alongside `full` compaction, TOON (Token-Oriented Object Notation) encodes uniform JSON arrays as compact tables:

```
[2]{id,name,role}:
  1,Alice,admin
  2,Bob,user
```

This is equivalent to:

```json
[{"id":1,"name":"Alice","role":"admin"},{"id":2,"name":"Bob","role":"user"}]
```

TOON instructions are injected into the system prompt so the LLM understands the format.

### Configuration

```toml
[compact]
compaction = "safe"   # off | safe | full (default: safe)
toon = false          # Enable TOON encoding (default: false)
```

### Runtime Control

In `--json-stream` mode, the compaction level can be changed at runtime via `set_config`:

```json
{"type": "set_config", "compaction": "full"}
```

### Native Bash Output Compaction

On top of the generic levels above, the engine compacts verbose **Bash** tool
output per-command before it enters the model's transcript. It parses the
output grammar of common dev commands and reconstructs a compact,
signal-preserving form:

| Command | What is kept |
|---------|--------------|
| `cargo build/check/clippy/test/nextest` | each `error[E…]`/`warning` **with its full code-frame block** (`-->` location, code frame, `= help`), the `could not compile` verdict, and `test result:` + `FAILED` names + panic blocks. Drops `Compiling …` spam and passing `... ok` lines. |
| `git status` / `diff` / `log` | status grouped by change-type with counts + capped path lists; diff structure (`@@` hunks, `+/-`); log collapsed to one line per commit (drops `Author:`/`Date:`). |
| `pytest` / `jest` / `vitest` / `go test` | the pass/fail summary, failing test names, and each failure's full traceback/assertion block. Drops passing lines. |
| `grep` / `rg` / `find` | total match count + unique-file count + the first paths. |
| anything else (fallback) | a generic shape classifier keeps error/warn/fail lines, else a head+tail with an omission marker. |

Properties:

- **Default on.** Resolved from `ProviderCompat::compact_bash()`.
- **Fail-open.** A parser that can't confidently parse falls through to the
  classifier, then to the raw output — the error signal is never dropped, and
  a non-trivial compaction always appends the last raw lines as a guaranteed
  tail. Output below ~40 lines / 8 KB is passed through verbatim.
- **Human sees full output.** Only the model's transcript copy is compacted;
  the host/terminal still receives the complete result via the live stream.
- **Telemetry.** Per-call `(raw_bytes, compacted_bytes)` is recorded on the
  tool-call trace (`compaction_bytes`) and a `wcore_agent::compaction` debug
  line is emitted, feeding a savings ("gain") report.

Disable it per provider/profile in `ProviderCompat`:

```toml
[providers.<name>.compat]
compact_bash = false   # default: true
```

## Skills lifecycle (W9)

When `observability.skills_lifecycle = true` in `wcore.toml`, three subsystems become available to the engine:

- **F10 autonomous skill creation.** A `PatternDetector` over recent `TurnTrace` history flags repeated tool-call sequences (length ≥ 5, repeats ≥ 3, stable input-key shape). Matches are written via `DraftWriter::stage` as P4 procedures with `status = Staged`. Staged skills are visible to `wcore skills audit` and operator promotion paths but never auto-load into the model's context. When `structured_traces` is also on, a `TraceEvent` with payload `{ "kind": "skill_drafted", "name": ..., ... }` is emitted on each stage.
- **F11 curator.** Runs on `on_session_end`. Reads active P4 procedures, scores them as `success_ratio · ln(1 + use_count)`, dedupes overlapping descriptions (Levenshtein ≤ 5 keeps the higher-scored entry), and archives entries with `use_count ≥ 5` and `success_ratio < 0.20`. `Pinned` procedures are never touched.
- **PUM (P5 user model; design §4.16).** A session-end inference pass derives four keys from `TurnTrace` history:
  - `preferences.tool_order` — top-5 tools by raw frequency (ties broken by first-seen turn index).
  - `tool_habits.recent_top5` — recency-weighted (last turn × 3, previous × 2, all others × 1).
  - `language.primary` — `"en"` stub in W9; future revisions sample user messages.
  - `working_hours.local_tz_window` — 24h window stub in W9; tightens once W6 adds wall-clock timestamps to `TurnTrace`.

  Writes go through `MemoryAccessGate` with `AccessToken::System` (P5 is system-only-write per W5 L4).

### Enabling

The `skills_lifecycle` flag is **default-off**. Turn it on per project in `.wayland-core.toml`:

```toml
[observability]
skills_lifecycle = true
```

The flag is also valid in the global config file under the same `[observability]` section; the project value ORs with the global value, mirroring `structured_traces`.

### Status in this release

W9 lands the four module-level pipelines (`PatternDetector`, `DraftWriter`, `Curator`, `UserModelInferencer`) and an end-to-end acceptance test (`wcore-skills/tests/w9_acceptance.rs`) that drives them against an in-memory `MemoryApi`. The engine bootstrap exposes the operator gate (`observability.skills_lifecycle`) and a real `MemoryApi` handle. As of W9.1, `PatternDetector` + `DraftWriter` are invoked from `AgentEngine`'s per-turn loop when the gate is on (see `crates/wcore-agent/src/engine.rs` — `detector` / `writer` construction inside the turn loop). Session-end invocation of `Curator` (skills curation) and `UserModelInferencer` (PUM) is still routed through host-side hooks rather than fired directly from `fire_on_session_end`.

F12 GEPA (the eval-driven mutation loop that evolves skill bodies once drafts have accumulated win/loss data) ships under W10B, with a real `LlmParaphraseProvider` for the Paraphrase mutator in production (Phase 3 PA); test runs continue to use the fixture-replay provider for strict determinism.

## Browser tool family (W8c.1)

`wcore-browser` is a multi-backend browser tool family. The three
backends share an ARIA-tree-first surface (`BrowserOp::Navigate /
Snapshot / Click / Type / NewTab / Download`) so prompt budgets stay
under control regardless of which provider actually drives the
session:

- **Camoufox** — primary. Stealth-fingerprinted Firefox; the default
  choice for any production session.
- **chromiumoxide** — local-Chromium fallback when Camoufox is
  unavailable.
- **Browserbase** — cloud-hosted browser sessions, opt-in via
  `BrowserPolicy.allow_cloud`.

`BrowserPolicy` is the network boundary: a deny-by-default origin
allow-list that gates every `Navigate`. A blocked op fires a
`browser_policy_denied` protocol event alongside the error
`tool_result` so hosts render the block as a distinct notification.

`BrowserSupervisor` owns process lifecycle, orphan-reaping, and
binary-management; per-platform binaries are downloaded on demand
via `BrowserBinaryManager`. There is deliberately NO `Evaluate` op
in v1 — arbitrary JS injection is too easy a security regression
for the first wave.

The plugin shell is `wayland-browser` (registers a `BrowserToolSpec`
through `wcore-plugin-api`; **no direct `wcore-browser` dep**, per
audit F2). Capability advertised on the wire via
`capabilities.browser_suite` whenever the plugin loads.

## Computer use (W8c.2)

`wcore-cua` is the multi-platform computer-use tool family —
synthesized mouse, keyboard, and screenshot ops across macOS, Linux
X11, Linux Wayland, and Windows backends.

Background-mode invariant: every CUA op MUST be performable without
stealing focus from the user's foreground app. `CuaPolicy` enforces
this at the type level and additionally gates:

- per-app first-touch approval (default ON);
- forbidden-app and forbidden-keycombo lists;
- optional screenshot redaction.

On restricted Linux Wayland compositors the host adapter refuses
registration at boot rather than silently degrading — operators
explicitly opt out by flipping `register_tools = false` in
`plugins.toml` for `wayland-cua`.

The plugin shell is `wayland-cua` (registers a `CuaToolSpec`
through `wcore-plugin-api`; no direct `wcore-cua` dep). Capability
advertised via `capabilities.computer_use`.

## Plugins (W2.5 / W8a / W8c)

Plugins are compiled-in Rust crates discovered through
`inventory::submit!`. Every plugin declares a `plugin.toml` manifest
that gates which `register_*` surface(s) it may touch (tools, hooks,
agents, skills, rules, MCP server, providers).

Currently shipped:

| Plugin | Surface(s) | Role |
|---|---|---|
| `wayland-ollama` | `register_providers` | Reference provider-only plugin — local Ollama inference |
| `wayland-browser` | `register_tools` | Packaging of `wcore-browser` via `BrowserToolSpec` mirror |
| `wayland-cua` | `register_tools` | Packaging of `wcore-cua` via `CuaToolSpec` mirror |
| `wayland-ijfw` | every surface | Anchor plugin — exercises tools, hooks, agents, skills, rules, and MCP server end-to-end |

The wire-side capability flags (`Capabilities.plugins`,
`.browser_suite`, `.computer_use`) flip when the matching plugin is
present, so the host UI can adapt without out-of-band coordination.
See `docs/json-stream-protocol.md` §§1.N+6 through 1.N+10 for the
per-event variants.
