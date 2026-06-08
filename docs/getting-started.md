# Getting Started

## Installation

```bash
# Build from source
cargo build --release

# Binary location
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
| `--model <id>` | Model name |
| `--profile <name>` | Named profile from config file |
| `--compaction <level>` | Output compaction: `off`, `safe` (default), `full` |
| `--toon` | Enable TOON tabular encoding (with `full` compaction) |
| `--auto-approve` | Skip all tool confirmations |
| `--json-stream` | JSON Lines mode for host integration |
| `--resume <id>` | Resume a previous session |

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

### 1. Initialize and Configure

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

# Resume the latest session
wayland-core --resume latest

# Resume a specific session
wayland-core --resume a1b2c3

# Create a session with a custom ID
wayland-core --session-id my-conv-123
```

- `--session-id` and `--resume` are mutually exclusive
- `--session-id` errors if the ID already exists
- Both flags work in interactive and `--json-stream` mode
- Auto-saves after each tool call turn
- Auto-cleans oldest sessions when exceeding `max_sessions`
