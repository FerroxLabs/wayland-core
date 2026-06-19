# Getting Started

## Installation

**npm** (recommended — pulls the right prebuilt binary for your platform):

```bash
npm install -g @ferroxlabs/wayland-core
wayland-core --version

# or run it once with no install
npx @ferroxlabs/wayland-core "summarize the TODOs in this repo"
```

**Prebuilt signed binaries** for macOS (arm64/x64), Linux (arm64/x64), and
Windows (arm64/x64) are on the
[Releases](https://github.com/FerroxLabs/wayland-core/releases) page, each
verifiable against `wayland-core-checksums.txt`.

**From source** (Rust 1.95+):

```bash
cargo install --git https://github.com/FerroxLabs/wayland-core wcore-cli

# or build the workspace directly
cargo build --release
./target/release/wayland-core
```

## Command Format

```
wayland-core [OPTIONS] [PROMPT]...
```

- With `PROMPT`: single-shot mode — completes the task and exits
- Without `PROMPT`: enters interactive REPL mode

> For the full list of CLI parameters, run `wayland-core --help`.

### Key Parameters

| Parameter | Description |
|-----------|-------------|
| `--provider <name>` | Provider: `anthropic`, `openai`, `bedrock`, `vertex`, or a custom alias |
| `--model <id>`, `-m` | Model name |
| `--api-key <key>`, `-k` | API key (overrides config / env) |
| `--base-url <url>`, `-b` | Base URL for the API |
| `--profile <name>` | Named profile from config file |
| `--agent <name>` | Built-in agent persona to inherit (e.g. `architect`, `debugger`) |
| `--list-agents` | List built-in agent personas and exit |
| `--max-tokens <n>` | Max output tokens per response |
| `--max-turns <n>` | Max agent loop turns |
| `--system-prompt <text>` | Custom system prompt |
| `--force` | Approve every tool call without prompting (aliases: `--yolo`, `--dangerously-skip-permissions`) |
| `--auto-approve` | Skip all tool confirmations |
| `--project-dir <path>` | Directory to load the project `.wayland-core.toml` from (defaults to CWD) |
| `--continue`, `-c` | Resume the most-recent session |
| `--resume <id>` | Resume a previous session |
| `--session-id <id>` | Use a specific session ID instead of auto-generating one |
| `--list-sessions` | List saved sessions and exit |
| `--no-tui` | Fall back to the line-based REPL instead of the TUI |
| `--no-memory` | Run stateless — disable long-term memory for this run |
| `--json-stream` | JSON Lines mode for host integration |
| `--compaction <level>` | Output compaction: `off`, `safe` (default), `full` |
| `--toon` | Enable TOON tabular encoding (with `full` compaction) |
| `--init-config` | Generate a default config file and exit |
| `--config-path` | Print the config file path and exit |
| `--doctor` | Run the system-dependency / provider health doctor |

Diagnostic and replay flags (`--skills-audit`, `--replay`, `--memory-show`,
`--probe-mcp`, …) are intentionally omitted here — run `wayland-core --help`
for the full set.

---

## Subcommands

Beyond the flag-driven agent/REPL path, `wayland-core` exposes a set of
verb subcommands. The headline ones:

| Subcommand | Purpose |
|------------|---------|
| `auth` | Manage provider API keys and OAuth sign-in (see [Authentication](#authentication)) |
| `setup` | Re-run the interactive onboarding (connect / configure) flow |
| `self-update` | Update to the latest signed release (`--check-only` to just compare versions) |
| `models list` | Print known models from the bundled catalog (`--provider` to filter) |
| `mcp-serve` | Serve the engine's own tools as an MCP server (stdio / SSE) for other clients |

The rest:

| Subcommand | Purpose |
|------------|---------|
| `plugin` | Install / list / remove plugins |
| `swarm` | Dispatch a worktree-isolated worker swarm |
| `workflow` (alias `forgeflows`) | Validate / list / run saved `.ron` workflows |
| `project-context` | Print the resolved project context (WAYLAND.md / AGENTS.md / CLAUDE.md) |
| `init` | Scaffold `.wayland/config.toml` + `WAYLAND.md` in the current directory |
| `acp` | ACP server / client surface |
| `agent` | Manage user-defined agents (create / list / show / edit / delete) |
| `cron` | Manage scheduled cron jobs (add / list / remove / enable / disable) |

---

## Configuration

### Three-Level Cascading

```
<global config>                   (global, user-level; run `wayland-core --config-path` to find)
    ↓ overridden by
<project config>                  (project-level, working directory — see layouts below)
    ↓ overridden by
CLI parameters / env vars        (highest priority)
```

Each level is a `config.toml`-format file. A field set at a lower level is
overridden by the same field at a higher level; fields you do not set inherit
from the level below.

#### Global config

A single user-level `config.toml`. Run `wayland-core --config-path` to print
its location (it varies by OS). Created by `wayland-core --init-config`.

#### Project config (two accepted layouts)

The working directory may carry a project-level config in **either** of two
layouts:

| Layout | Path | Notes |
|--------|------|-------|
| File form | `./.wayland-core.toml` | The documented, canonical layout. |
| Directory form | `./.wayland-core/config.toml` | Also accepted (the eval-harness scaffold writes this form). |

The file form is canonical. If **both** files exist in the same directory,
the file form (`.wayland-core.toml`) wins and a precedence warning is printed
to stderr — remove one file to silence it. Keep only one project layout per
directory to avoid the warning.

#### Legacy YAML (auto-migrated)

A pre-TOML `~/.wayland/config.yaml` (honouring `WAYLAND_HOME` when set) is
detected on startup and migrated to the canonical TOML config automatically;
the migration is skipped once the canonical TOML exists. This path is for
upgrading older installs and is not a layer you author by hand.

### Generate Default Config

```bash
wayland-core --init-config
# Creates the global config file (run `wayland-core --config-path` to see the location)
```

### Config File Format

```toml
# Global config file (path varies by OS, use `wayland-core --config-path` to find)

[default]
provider = "anthropic"
# model = "claude-sonnet-4-20250514"
max_tokens = 8192
max_turns = 30

[providers.anthropic]
# api_key = "sk-ant-xxx"       # or env var ANTHROPIC_API_KEY
# base_url = "https://api.anthropic.com"

[providers.openai]
# api_key = "sk-xxx"           # or env var OPENAI_API_KEY
# base_url = "https://api.openai.com"

# Custom provider alias
[providers.my-service]
provider = "openai"
model = "custom-model-v1"
api_key = "sk-xxx"
base_url = "https://my-service.example.com/api/openai"

# Named profiles, switch with --profile <name>
[profiles.deepseek]
provider = "openai"
model = "deepseek-chat"
api_key = "sk-xxx"
base_url = "https://api.deepseek.com"

[profiles.ollama]
provider = "openai"
model = "qwen2.5:32b"
api_key = "ollama"
base_url = "http://localhost:11434"

[profiles.my-service]
provider = "my-service"

[tools]
auto_approve = false
allow_list = ["Read", "Grep", "Glob"]

[session]
enabled = true
directory = ".wayland-core/sessions"
max_sessions = 20

[compact]
compaction = "safe"   # off | safe | full
toon = false          # Enable TOON encoding for JSON arrays

[file_cache]
enabled = true
max_entries = 100

[plan]
enabled = true
plan_directory = ".wayland-core/plans"
```

### API Key Resolution Order

1. `--api-key` CLI parameter
2. Config file `providers.<name>.api_key`
3. Env var `API_KEY`
4. Env var `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` (depends on provider)

> **Note**: `bedrock` and `vertex` providers use their own cloud credentials and do not require a traditional API key. See [Providers & Auth](providers.md).

> **Sign in with ChatGPT**: instead of an OpenAI API key you can authenticate
> with your ChatGPT subscription via `wayland-core auth login chatgpt`, then run
> with `--provider openai-chatgpt`. See [Sign in with ChatGPT](providers.md#sign-in-with-chatgpt).

### Authentication

The `auth` subcommand manages provider credentials directly against the global
`config.toml` — the lightweight alternative to the full onboarding flow:

```bash
# List every configured provider with a masked key
wayland-core auth list

# Add (or replace) a key — validated live against the provider before it is
# written. Use `autodetect` to infer the provider from the key's prefix.
wayland-core auth add anthropic sk-ant-xxx
wayland-core auth add autodetect sk-or-v1-xxx
wayland-core auth add openai sk-xxx --no-validate   # skip the live check

# Remove a provider's key
wayland-core auth remove openai
```

**OAuth sign-in.** The only OAuth sign-in is ChatGPT — sign in with your
ChatGPT subscription instead of an OpenAI API key:

```bash
wayland-core auth login chatgpt            # opens a browser (loopback PKCE)
wayland-core auth login chatgpt --device   # headless device-code flow (SSH/remote)
wayland-core auth login chatgpt --import-codex  # import an existing Codex CLI login
wayland-core auth status                   # show signed-in provider, plan, token expiry
wayland-core auth logout chatgpt           # delete the stored OAuth token
```

After signing in, run with `--provider openai-chatgpt`.

> xAI / Grok is **not** an OAuth sign-in — it is an API-key provider. Add the
> key (`wayland-core auth add xai <key>`) and run with `--provider xai`; the
> engine refreshes any internal OAuth tokens on its own.

### Custom Provider Alias

If a backend is compatible with a built-in provider's protocol, you can declare an alias under `providers.<alias>`:

```toml
[default]
provider = "my-service"

[providers.my-service]
provider = "openai"
model = "custom-model-v1"
api_key = "sk-xxx"
base_url = "https://my-service.example.com/api/openai"
```

- Both `default.provider` and `profile.provider` accept alias names
- `providers.<alias>.provider` must declare the underlying type — currently one of `anthropic`, `openai`, `bedrock`, `vertex`
- The alias entry overrides the default configuration of the underlying provider

---

## Quick Start

### 1. Connect a Provider (paste-to-connect)

The fastest path is to just run `wayland-core` (or `wayland-core setup`) and
paste an API key when prompted. It fingerprints the provider from the key's
shape, validates it live against the provider's model endpoint (a confidence
ladder: Detected → CanListModels → Ready), stores it (OS keyring by default,
plaintext fallback), and makes it the default provider — with no restart. You
can also paste a key from inside the TUI at any time with `/connect`.

Prefer to edit the config by hand instead:

```bash
wayland-core --init-config
# Edit the config file (run `wayland-core --config-path` to find it), add your API key
```

### 2. Single-Shot Mode

```bash
wayland-core "Read and explain crates/wcore-agent/src/engine.rs"
```

### 3. Interactive REPL

```
$ wayland-core

> Read the file Cargo.toml
     1  [package]
     2  name = "wayland-core"
     ...
[turns: 1 | tokens: 1234 in / 567 out]

> Add serde_yaml to dependencies
[tool] Write({"file_path":"Cargo.toml","content":"..."})
Allow? [y]es / [n]o / [a]lways / [q]uit > y
[Write] OK
[turns: 2 | tokens: 2345 in / 890 out]

> /quit
```

REPL commands: `/quit`, `/exit`, or empty line to exit.

### 4. Switching Profiles

```bash
wayland-core --profile deepseek "Fix the bug in main.rs"
wayland-core --profile ollama "Analyze code quality"
```

### 5. Environment Variables

```bash
export ANTHROPIC_API_KEY=sk-ant-xxx
wayland-core "List all Rust files in this project"
```

---

## TUI commands

Inside the full-screen TUI, slash commands drive configuration and inspection.
Type `/` from any surface to open a command palette. The high-value ones:

| Command | What it does |
|---------|--------------|
| `/connect` | Paste an API key to fingerprint, live-validate, and connect a provider |
| `/config` | All settings — Essentials and Advanced editors |
| `/doctor` | Provider / key / MCP health, plus DISCOVERED rows for connectable providers |
| `/effective` | View the resolved config, with secrets redacted |
| `/model` | Switch model (arrow-key picker, cross-provider) |
| `/provider` | Switch provider (arrow-key picker) |
| `/mcp` | List MCP servers; `/mcp connect` to connect a discovered one |

---

## Tool Confirmation

Destructive tools (Write, Edit, Bash) prompt for confirmation before execution:

```
[tool] Write({"file_path": "/tmp/test.rs", "content": "..."})
Allow? [y]es / [n]o / [a]lways / [q]uit > y
```

| Option | Description |
|--------|-------------|
| `y` / `yes` / Enter | Allow this execution |
| `n` / `no` | Deny — LLM receives a "denied" error |
| `a` / `always` | Auto-approve this tool for the rest of the session |
| `q` / `quit` | Abort the entire agent run |

- Read-only tools (Read, Grep, Glob) are auto-approved by default
- `--auto-approve` skips all confirmations
- `tools.allow_list` in config customizes the whitelist

---

## Session Management

Sessions auto-save to `.wayland-core/sessions/`.

```bash
# List saved sessions
wayland-core --list-sessions

# Resume the most-recent session (shortcut for --resume <latest-id>)
wayland-core --continue          # or -c

# Resume the latest session
wayland-core --resume latest

# Resume a specific session
wayland-core --resume a1b2c3

# Create a session with a custom ID
wayland-core --session-id my-conv-123
```

- `--continue` / `-c` resumes the most-recent session; it is mutually exclusive with `--resume` and `--session-id`
- `--session-id` and `--resume` are mutually exclusive
- `--session-id` errors if the ID already exists
- Both flags work in interactive and `--json-stream` mode
- Auto-saves after each tool call turn
- Auto-cleans oldest sessions when exceeding `max_sessions`
