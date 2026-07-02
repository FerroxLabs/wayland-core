# wayland-core JSON Stream Protocol Spec

> This protocol defines the communication between wayland-core (Rust CLI) and a host client (e.g., the Wayland desktop Electron app) via stdin/stdout JSON Lines.

## Overview

```
┌──────────────┐   stdin (JSON Lines)    ┌──────────────────┐
│              │ ◄─────────────────────── │                  │
│ wayland-core│                          │   Host Client    │
│  (Rust CLI)  │ ──────────────────────► │  (Wayland app)   │
│              │   stdout (JSON Lines)    │                  │
└──────────────┘                          └──────────────────┘
     stderr → diagnostic logs (not part of protocol)
```

- **Transport**: stdin/stdout, one JSON object per line (JSON Lines / NDJSON)
- **Encoding**: UTF-8
- **Activation**: `wayland-core --json-stream [other flags]`
- **Lifecycle**: One process per conversation; process stays alive for multi-turn

## 1. Agent → Client Events (stdout)

Every line is a JSON object with a `type` field.

### 1.1 `ready`

Emitted once after initialization completes. Client MUST wait for this before sending messages.

```json
{
  "type": "ready",
  "version": "0.2.0",
  "session_id": "a1b2c3",
  "capabilities": {
    "tool_approval": true,
    "thinking": true,
    "effort": false,
    "effort_levels": [],
    "modes": ["default", "auto_edit", "yolo"],
    "current_mode": "default",
    "mcp": true
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `version` | string | Protocol version (semver) |
| `session_id` | string? | Session ID (omitted when sessions are disabled in config) |
| `capabilities.tool_approval` | bool | Whether agent supports pause-and-wait tool approval |
| `capabilities.thinking` | bool | Whether current provider supports extended thinking |
| `capabilities.effort` | bool | Whether current provider supports reasoning_effort |
| `capabilities.effort_levels` | string[] | Valid effort values (e.g., `["low", "medium", "high"]`). Empty when effort is false |
| `capabilities.modes` | string[] | Available approval modes for `set_mode` command |
| `capabilities.current_mode` | string | Currently active approval mode |
| `capabilities.mcp` | bool | Whether MCP tools are available |
| `capabilities.streaming_tools` | bool (W0) | Engine will emit `tool_chunk` events for streaming tool results (W7) |
| `capabilities.sub_agent_traces` | bool (W0) | Engine will emit `sub_agent_event` with `parent_call_id` (W7) |
| `capabilities.cost_attribution` | bool (W0) | Engine will emit per-turn/session `cost` events (W6) |
| `capabilities.hitl_suspend` | bool (W0) | Engine will emit `suspend` / `approval_required` events (W7) |
| `capabilities.non_destructive_compact` | bool (W0) | Engine will emit `compact_offload` instead of destructive compaction (W5) |
| `capabilities.structured_traces` | bool (W0) | Engine will emit `trace_event` per the F9 schema (W1) |
| `capabilities.rpc_tool_script` | bool (W0) | Engine supports the `Script` tool with trace expansion (W4) |
| `capabilities.browser_suite` | bool (W0) | Engine will emit browser tool events (W8) |
| `capabilities.computer_use` | bool (W0) | Engine will emit computer-use events (W8) |
| `capabilities.plugins` | bool (W0) | Plugin-registered tools/hooks/agents are visible to the host (W2.5/W8) |
| `capabilities.gepa_enabled` | bool (W0) | Engine will emit `evolution_event` during a `wcore-evolve` GEPA run (W10B) |

**Note on W0 flags.** Setting a flag to `true` is **engine advertisement**, not host
permission. The engine is announcing "I will emit these new event variants this
session." Hosts that don't know about a flag MUST still tolerate the corresponding
event types per the Host Decoder Contract section. New event variants added in
W6/W7/W8 stay disabled by `wcore-config` until an explicit release flips them on.
Default-off W0 flags are **omitted** from the serialized `capabilities` object
(`#[serde(skip_serializing_if = "is_false")]`), so v0.1.21 hosts see the original
seven-field shape unchanged.

### 1.2 `stream_start`

A new response turn has started.

```json
{
  "type": "stream_start",
  "msg_id": "abc-123"
}
```

### 1.3 `text_delta`

Incremental text output (streaming).

```json
{
  "type": "text_delta",
  "text": "Hello, ",
  "msg_id": "abc-123"
}
```

### 1.4 `thinking`

Model's internal reasoning (if extended thinking is enabled).

```json
{
  "type": "thinking",
  "text": "Let me analyze the code structure...",
  "msg_id": "abc-123"
}
```

### 1.5 `tool_request`

Agent wants to invoke a tool and needs client approval. Agent PAUSES execution until it receives `tool_approve` or `tool_deny`.

```json
{
  "type": "tool_request",
  "msg_id": "abc-123",
  "call_id": "tool-call-001",
  "tool": {
    "name": "Write",
    "category": "edit",
    "args": {
      "file_path": "/src/main.rs",
      "content": "fn main() { ... }"
    },
    "description": "Write to /src/main.rs"
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `call_id` | string | Unique ID for this tool invocation |
| `tool.name` | string | Tool name: `Read`, `Write`, `Edit`, `Bash`, `Glob`, `Grep`, `Spawn`, or MCP tool name |
| `tool.category` | string | `"info"` (read-only), `"edit"` (file mutation), `"exec"` (shell), `"mcp"` (MCP tool) |
| `tool.args` | object | Tool arguments |
| `tool.description` | string | Human-readable one-line description |

**Category mapping for built-in tools:**

| Tool | Category | Rationale |
|------|----------|-----------|
| `Read` | `info` | Read-only file access |
| `Glob` | `info` | Read-only file search |
| `Grep` | `info` | Read-only content search |
| `Write` | `edit` | Creates or overwrites files |
| `Edit` | `edit` | Modifies file content |
| `Bash` | `exec` | Executes shell commands |
| `Spawn` | `exec` | Spawns sub-agent |
| MCP tools | `mcp` | External MCP server tools |

> **Note**: When `auto_approve = true` (yolo mode) or when a tool is in the `allow_list`, the agent executes immediately and emits `tool_running` directly, skipping `tool_request`.

### 1.6 `tool_running`

Tool execution has started (after approval or auto-approve).

```json
{
  "type": "tool_running",
  "msg_id": "abc-123",
  "call_id": "tool-call-001",
  "tool_name": "Write"
}
```

### 1.7 `tool_result`

Tool execution completed.

```json
{
  "type": "tool_result",
  "msg_id": "abc-123",
  "call_id": "tool-call-001",
  "tool_name": "Write",
  "status": "success",
  "output": "File written successfully",
  "output_type": "text"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | `"success"` or `"error"` |
| `output` | string | Tool output (truncated if exceeds limit) |
| `output_type` | string | `"text"` (default), `"diff"` (for Edit tool), `"image"` (base64) |

**Special output for Edit tool** (`output_type: "diff"`):

```json
{
  "type": "tool_result",
  "msg_id": "abc-123",
  "call_id": "tool-call-002",
  "tool_name": "Edit",
  "status": "success",
  "output": "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,3 @@\n-old line\n+new line",
  "output_type": "diff",
  "metadata": {
    "file_path": "/src/main.rs"
  }
}
```

### 1.8 `tool_cancelled`

Tool was denied by client or cancelled.

```json
{
  "type": "tool_cancelled",
  "msg_id": "abc-123",
  "call_id": "tool-call-001",
  "reason": "User denied"
}
```

### 1.9 `stream_end`

Current response turn finished.

```json
{
  "type": "stream_end",
  "msg_id": "abc-123",
  "finish_reason": "stop",
  "usage": {
    "input_tokens": 1500,
    "output_tokens": 320,
    "cache_read_tokens": 800,
    "cache_write_tokens": 200
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `msg_id` | string | Message ID this turn belongs to |
| `finish_reason` | `"stop" \| "length" \| "error"` | Why the turn ended. `stop`: model finished normally. `length`: hit max_tokens. `error`: provider/runtime error. |
| `usage` | object? | Token counts (optional; omitted when provider does not report usage) |

### 1.10 `error`

An error occurred. The agent may or may not continue depending on severity.

```json
{
  "type": "error",
  "msg_id": "abc-123",
  "error": {
    "code": "provider_error",
    "message": "Rate limit exceeded",
    "retryable": true
  }
}
```

| Error Code | Description |
|------------|-------------|
| `provider_error` | LLM API error (rate limit, etc.) |
| `auth_required` | Provider rejected the credential (HTTP 401). Refreshable — the host should re-auth / refresh the OAuth token and re-send the turn. `retryable` is left as the engine set it (typically `false`, since re-sending the same credential just burns budget), so hosts drive retry off the **code**, not the flag. |
| `auth_invalid` | Provider denied access (HTTP 403). Hard failure — do not retry. |
| `tool_error` | Built-in tool execution error |
| `config_error` | Configuration or initialization error |
| `protocol_error` | Invalid command from client |
| `internal_error` | Unexpected internal error |
| `engine_error` | Fallback code for any error not matched to a more specific code above. |

> Hosts should branch on `error.code`, not parse `error.message`.

### 1.11 `info`

Informational message (non-critical, for display only).

```json
{
  "type": "info",
  "msg_id": "abc-123",
  "message": "Stream interrupted, retrying... (1/2)"
}
```

### 1.12 `config_changed`

Emitted after a `set_config` command is processed. Contains the updated capabilities snapshot reflecting the current provider/model configuration.

```json
{
  "type": "config_changed",
  "capabilities": {
    "tool_approval": true,
    "thinking": false,
    "effort": true,
    "effort_levels": ["low", "medium", "high"],
    "modes": ["default", "auto_edit", "yolo"],
    "current_mode": "default",
    "mcp": true
  }
}
```

Clients should update their UI controls (e.g., enable/disable thinking toggle, populate effort dropdown) based on the new capabilities.

### 1.13 `mcp_ready`

Emitted after a dynamically injected MCP server has connected and its tools are registered.

```json
{
  "type": "mcp_ready",
  "name": "my-tools",
  "tools": ["tool_a", "tool_b"]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Server name (as provided in `add_mcp_server`) |
| `tools` | string[] | List of tool names registered from this server |

### 1.14 `pong`

Response to a `ping` command from the client. Used for heartbeat/liveness detection.

```json
{
  "type": "pong"
}
```

No additional fields. The agent emits `pong` immediately upon receiving a `ping` command, regardless of whether a message turn is active.

## 2. Client → Agent Commands (stdin)

Every line is a JSON object with a `type` field.

### 2.1 `message`

Send a user message. Agent responds with a stream of events.

```json
{
  "type": "message",
  "msg_id": "abc-123",
  "content": "Read the file src/main.rs and explain the code",
  "files": ["/path/to/attached/file.png"]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `msg_id` | string | yes | Client-generated unique message ID |
| `content` | string | yes | User's message text |
| `files` | string[] | no | Attached file paths (images, documents) |

### 2.2 `stop`

Abort the current response stream.

```json
{
  "type": "stop"
}
```

Agent MUST:
1. Cancel any in-flight LLM request
2. Cancel any running tool (if possible)
3. Emit `stream_end` for the current msg_id

### 2.3 `tool_approve`

Approve a pending tool execution.

```json
{
  "type": "tool_approve",
  "call_id": "tool-call-001",
  "scope": "once"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `call_id` | string | Must match a pending `tool_request` |
| `scope` | string | `"once"` = this call only; `"always"` = auto-approve this tool+category for the session |

When `scope = "always"`, the agent adds the tool's category to the session allow-list, so future calls of the same category skip approval.

### 2.4 `tool_deny`

Deny a pending tool execution.

```json
{
  "type": "tool_deny",
  "call_id": "tool-call-001",
  "reason": "Not allowed to write this file"
}
```

Agent MUST:
1. Emit `tool_cancelled` event
2. Feed the denial reason back to the LLM as tool result
3. Continue the conversation (LLM decides next action)

### 2.5 `init_history`

Inject prior conversation context (for conversation resume).

```json
{
  "type": "init_history",
  "text": "Previous conversation summary:\nUser asked about X...\nAssistant replied with Y..."
}
```

Must be sent BEFORE the first `message` command. Agent incorporates this as conversation context.

### 2.6 `set_mode`

Change the agent's approval mode for the session.

```json
{
  "type": "set_mode",
  "mode": "yolo"
}
```

| Mode | Behavior |
|------|----------|
| `"default"` | All tools need approval (except allow-listed) |
| `"auto_edit"` | `info` and `edit` auto-approved; `exec` and `mcp` need approval |
| `"yolo"` | All tools auto-approved |

### 2.7 `set_config`

Update model, thinking, or effort configuration at runtime.

```json
{
  "type": "set_config",
  "model": "claude-opus-4",
  "thinking": "enabled",
  "thinking_budget": 16000,
  "effort": "high",
  "compaction": "safe"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `model` | string | no | Switch to a different model |
| `thinking` | string | no | `"enabled"` or `"disabled"` |
| `thinking_budget` | number | no | Token budget for thinking (default: 10000) |
| `effort` | string | no | Reasoning effort level (e.g., `"low"`, `"medium"`, `"high"`) |
| `compaction` | string | no | Output compaction level: `"off"`, `"safe"`, `"full"` |

All fields are optional. Only provided fields are updated.

> **Validation**: The agent validates `thinking` and `effort` values against the current provider's capabilities. If the provider does not support a feature, the change is rejected with a descriptive message in the `info` event. After processing, a `config_changed` event is always emitted with the updated capabilities.

### 2.8 `add_mcp_server`

Dynamically inject an MCP server before the conversation starts. This command is only accepted during the **pre-message phase** — after the `ready` event and before the first `message` command. Any `add_mcp_server` sent after the first `message` is rejected with an error.

```json
{
  "type": "add_mcp_server",
  "name": "my-tools",
  "transport": "stdio",
  "command": "node",
  "args": ["bridge.js", "--port", "9000"],
  "env": {"TOKEN": "abc123"}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Unique server name |
| `transport` | string | yes | `"stdio"`, `"sse"`, or `"streamable-http"` |
| `command` | string | stdio only | Executable to launch |
| `args` | string[] | no | Command arguments |
| `env` | object | no | Environment variables for the subprocess |
| `url` | string | sse/http only | Server URL |
| `headers` | object | no | HTTP headers (for sse/http) |

**Lifecycle:**

```
Agent  → stdout: {"type":"ready",...}
Client → stdin:  {"type":"add_mcp_server","name":"tools","transport":"stdio","command":"node","args":["bridge.js"]}
Agent  → stdout: {"type":"mcp_ready","name":"tools","tools":["tool_a","tool_b"]}
Client → stdin:  {"type":"message","msg_id":"m1","content":"Hello"}
                  ↑ first message ends the injection window
```

### 2.9 `ping`

Heartbeat probe. The agent responds immediately with a `pong` event.

```json
{
  "type": "ping"
}
```

Can be sent at any time — during idle, during message processing, or during tool execution. The agent always responds with `{"type":"pong"}`.

After the first `message`, any further `add_mcp_server` commands are rejected:

```json
{
  "type": "error",
  "error": {
    "code": "protocol_error",
    "message": "AddMcpServer 'name': rejected — only allowed before first Message",
    "retryable": false
  }
}
```

### 2.10 `approval_resume` (W7)

Resolve a pending HITL approval. Sent by the host in response to an
`approval_required` / `suspend` event pair (§1.N+4). The engine routes
the decision via `resume_token` to the parked `ApprovalBridge`, then
emits an `approval_resume` event as confirmation and either proceeds
with the original operation or fails it with a deny reason.

```json
{
  "type": "approval_resume",
  "resume_token": "rt-9b3c",
  "approved": true,
  "modifications": null
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `resume_token` | string | yes | Echoed verbatim from the `approval_required` event. Routes the decision to the right pending bridge. |
| `approved` | bool | yes | `true` to approve and proceed, `false` to deny. |
| `modifications` | object \| null | no | Reserved for forward-compat: host-side edits to the pending operation (e.g. an edited tool input). Engine currently ignores; future waves may wire this through. |

> **Capability gating.** Only meaningful when `capabilities.hitl_suspend`
> is advertised on the Ready event. If sent without a matching pending
> approval (unknown / stale `resume_token`), the engine logs and ignores
> the command.

## 3. Lifecycle

### 3.1 Startup

```
Client spawns:
  wayland-core --json-stream \
    --provider anthropic \
    --model claude-sonnet-4-20250514 \
    --max-tokens 8192 \
    --max-turns 30

Environment variables set by client:
  ANTHROPIC_API_KEY=sk-...
  # or OPENAI_API_KEY, AWS_REGION, etc.

Agent initializes → stdout: {"type":"ready","session_id":"a1b2c3",...}
```

**Pre-message phase (optional):**

Between receiving `ready` and sending the first `message`, the client may inject MCP servers via `add_mcp_server` commands. The agent connects each server and emits `mcp_ready` when ready. This phase ends when the first `message` is sent.

**Session lifecycle flags** (mutually exclusive):

| Flag | Description |
|------|-------------|
| `--session-id <ID>` | Use a specific session ID instead of auto-generating one. Errors if the ID already exists. |
| `--resume <ID>` | Resume a previous session (loads conversation history). Use `latest` to resume the most recent. |

```bash
# New session with a custom ID
wayland-core --json-stream --session-id my-conv-123 --provider openai --model gpt-4o

# Resume an existing session
wayland-core --json-stream --resume my-conv-123 --provider openai --model gpt-4o
```

### 3.2 Message Turn

```
Client → stdin:  {"type":"message","msg_id":"m1","content":"Hello"}
Agent  → stdout: {"type":"stream_start","msg_id":"m1"}
Agent  → stdout: {"type":"text_delta","text":"Hi! ","msg_id":"m1"}
Agent  → stdout: {"type":"text_delta","text":"How can I help?","msg_id":"m1"}
Agent  → stdout: {"type":"stream_end","msg_id":"m1","usage":{...}}
```

### 3.3 Tool Approval Flow

```
Client → stdin:  {"type":"message","msg_id":"m2","content":"Create a hello.rs file"}
Agent  → stdout: {"type":"stream_start","msg_id":"m2"}
Agent  → stdout: {"type":"text_delta","text":"I'll create the file.","msg_id":"m2"}
Agent  → stdout: {"type":"tool_request","msg_id":"m2","call_id":"t1","tool":{"name":"Write","category":"edit",...}}
  ← Agent PAUSES here, waiting for approval →
Client → stdin:  {"type":"tool_approve","call_id":"t1","scope":"once"}
Agent  → stdout: {"type":"tool_running","msg_id":"m2","call_id":"t1","tool_name":"Write"}
Agent  → stdout: {"type":"tool_result","msg_id":"m2","call_id":"t1","status":"success",...}
Agent  → stdout: {"type":"text_delta","text":"File created successfully.","msg_id":"m2"}
Agent  → stdout: {"type":"stream_end","msg_id":"m2","usage":{...}}
```

### 3.4 Multi-Tool Parallel Execution

When the LLM requests multiple tools in one turn, agent emits multiple `tool_request` events. Client can approve/deny them independently.

```
Agent  → stdout: {"type":"tool_request","call_id":"t1","tool":{"name":"Read","category":"info",...}}
Agent  → stdout: {"type":"tool_request","call_id":"t2","tool":{"name":"Read","category":"info",...}}
Client → stdin:  {"type":"tool_approve","call_id":"t1","scope":"once"}
Client → stdin:  {"type":"tool_approve","call_id":"t2","scope":"once"}
Agent  → stdout: {"type":"tool_running","call_id":"t1",...}
Agent  → stdout: {"type":"tool_running","call_id":"t2",...}
Agent  → stdout: {"type":"tool_result","call_id":"t1",...}
Agent  → stdout: {"type":"tool_result","call_id":"t2",...}
```

### 3.5 Shutdown

Client closes stdin (EOF) or sends SIGTERM. Agent cleans up and exits.

## 4. Error Handling

### 4.1 Invalid Command

If client sends malformed JSON or unknown command type:

```json
{
  "type": "error",
  "msg_id": null,
  "error": {
    "code": "protocol_error",
    "message": "Unknown command type: foo",
    "retryable": false
  }
}
```

### 4.2 Provider Errors

Agent should emit error and let the conversation continue if possible:

```json
{
  "type": "error",
  "msg_id": "m3",
  "error": {
    "code": "provider_error",
    "message": "Rate limit exceeded. Retry after 30s.",
    "retryable": true
  }
}
```

**Auth failures** carry a distinct code so the host can branch without parsing
the message. A `401` becomes `auth_required` — refreshable: the host should
re-auth (or refresh the OAuth token) and re-send the turn. A `403` becomes
`auth_invalid` — a hard failure the host must not retry. For both, the engine
leaves `retryable` as-is (typically `false`, since re-sending the same
credential just burns budget); hosts drive retry off `error.code`, not the flag.
Any error not matched to a specific code falls back to `engine_error`.

```json
{
  "type": "error",
  "msg_id": "m3",
  "error": {
    "code": "auth_required",
    "message": "API error 401: invalid x-api-key",
    "retryable": false
  }
}
```

### 4.3 Fatal Errors

For unrecoverable errors, agent emits error and exits with non-zero status:

```json
{
  "type": "error",
  "msg_id": null,
  "error": {
    "code": "config_error",
    "message": "ANTHROPIC_API_KEY not set",
    "retryable": false
  }
}
```

## 5. Configuration via CLI Flags

When spawned in `--json-stream` mode, all configuration is passed via CLI flags and environment variables:

```bash
wayland-core --json-stream \
  --provider <anthropic|openai|bedrock|vertex> \
  --model <model-id> \
  --max-tokens <N> \
  --max-turns <N> \
  --base-url <URL> \
  --system-prompt <TEXT> \
  --auto-approve          # Start in yolo mode
  --workspace <PATH>      # Working directory for file operations
```

**Environment variables** (set by client before spawn):

| Provider | Variables |
|----------|-----------|
| Anthropic | `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL` |
| OpenAI | `OPENAI_API_KEY`, `OPENAI_BASE_URL` |
| Bedrock | `AWS_REGION`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_PROFILE` |
| Vertex AI | `GOOGLE_APPLICATION_CREDENTIALS`, `VERTEX_PROJECT_ID`, `VERTEX_REGION` |

## 6. Protocol Versioning

The `ready` event includes a `version` field. Clients should check version compatibility.

- **Minor version bump**: New optional event types or fields added (backward compatible)
- **Major version bump**: Breaking changes to existing events/commands

Current version: `0.2.0`

---

## Host Decoder Contract (W0)

> **Production conformance gap.** This contract is enforced for the
> reference decoder in `crates/wcore-protocol/tests/host_decoder_contract.rs`.
> The production Wayland Desktop decoder at
> `app/src/process/agent/wcore/index.ts` is the actual consumer. Whether
> that production decoder honours every clause of this contract is a
> follow-up audit, not covered by W0. If you are modifying the Electron
> host's wcore decoder, **read this section first**; it is the
> authoritative spec.

The JSON event stream evolves additively across wcore versions. To stay
compatible without per-release host updates, the Wayland Desktop host
decoder MUST honour this contract:

### Rules

1. **Parse to a generic value first.** Decode each line into a
   `serde_json::Value` (or the host-language equivalent) before any
   type-specific interpretation. Do not derive directly into a closed
   enum.

2. **Distinguish three outcomes per line:**
   - **Known event type**: the `type` string is in the host's known set;
     render normally.
   - **Unknown event type**: the `type` string is NOT in the host's known
     set. **Drop silently** — no error log, no exception, no surfaced
     warning. This is the forward-compatibility path; new wcore versions
     emit variants the host hasn't learned about.
   - **Malformed**: input wasn't decodable JSON, OR had no `type` field,
     OR the `type` value wasn't a string. **Log or count with rate
     limiting** — this indicates protocol corruption (framing bugs,
     truncation, injection) and is observable evidence of a problem,
     distinct from normal version skew.

3. **Tolerate unknown fields on known variants.** Read only the fields
   the host expects. Unknown fields on a known event must be ignored;
   they appear when wcore adds new optional fields in future versions.

4. **Use `capabilities` advisory, not permissive.** The `Ready` event's
   `capabilities` block advertises which event families this wcore
   session will emit. The host CAN read the flags to decide whether to
   add relevant `type` strings to its known set. The host MUST NOT
   require any `capabilities` flag to be true to render a known event
   — unknown `capabilities` keys must also be ignored.

### Authoritative test

The behaviour above is enforced in
`crates/wcore-protocol/tests/host_decoder_contract.rs`. That file
contains a reference `host_decode` implementation that satisfies this
contract; use it as the spec when porting the Electron host's decoder
to match. The production host code lives at
`app/src/process/agent/wcore/index.ts` — conformance there is a
follow-up audit owned by the Wayland Desktop side, not by W0.

### Flag → event-type mapping

The new W0 `capabilities` flags gate the following future event `type`
strings. A host that wants to render any of these adds the listed
`type` strings to its known set.

| Capability flag | Wave | Gated event types |
|---|---|---|
| `streaming_tools` | W7 | `tool_chunk` |
| `sub_agent_traces` | W7 | `sub_agent_event` |
| `cost_attribution` | W6 | `cost` (per turn, per session) |
| `hitl_suspend` | W7 | `suspend`, `approval_required` |
| `non_destructive_compact` | W5 | `compact_offload` |
| `structured_traces` | W1 | `trace_event` |
| `rpc_tool_script` | W4 | (none new; expands `tool_result.metadata` shape for Script results) |
| `browser_suite` | W8 | `browser_event`, `browser_policy_denied` |
| `computer_use` | W8 | `cua_event`, `cua_policy_denied` |
| `plugins` | W2.5/W8 | `plugin_event` (plus plugin-registered tools appear in `tool_request`/`tool_result`) |
| `gepa_enabled` | W10B | `evolution_event` |

#### Host-tolerated additive variants

Some event variants ship without a dedicated `Capabilities.*` flag.
They are always-emitted; hosts that do not know about them silently
drop the line per the W0 host decoder contract. As of W8c.3:
`budget_exceeded` is the only host-tolerated variant on this list
(plus the long-standing `provider_circuit_event`, see §1.N+5).
Rationale: `BudgetExceeded` is a singular event per session (fires
once when the first budget cap trips); the flag-per-variant overhead
exceeds the wire-surface savings.

#537/#141 adds `host_send_message_request` (§1.N+12) to this list:
it is only ever emitted when the host itself opted in by spawning the
engine with `WAYLAND_SEND_MESSAGE_HOST_DELEGATE=1`, so a flag would be
redundant with the env-var opt-in.

W10B's `gepa_enabled` flag is INDEPENDENT of `structured_traces` — F6 audit
fix in the W10B revision. Hosts that want only W1 turn traces aren't forced
to accept thousands of W10B per-child evolution events per `wcore-evolve`
run, and hosts that want only `evolve` observability can advertise
`gepa_enabled` without `structured_traces`. Each event family has its own
opt-in, matching the W0 "one flag per event family" discipline.

This table is the authoritative mapping. When a future wave lands a new
event variant, it MUST update this table in the same PR.

### v0.1.21 baseline event types

The set of `type` strings emitted by wcore as of v0.1.21:
`ready`, `stream_start`, `text_delta`, `thinking`, `tool_request`,
`tool_running`, `tool_result`, `tool_cancelled`, `stream_end`,
`error`, `info`, `config_changed`, `mcp_ready`, `pong`.

W1 adds: `trace_event` (gated by `capabilities.structured_traces`).
W10B adds: `evolution_event` (gated by `capabilities.gepa_enabled`).
W6 adds: `session_cost` (gated by `capabilities.cost_attribution`).
W7 adds: `sub_agent_event` (gated by `capabilities.sub_agent_traces`),
`tool_chunk` (gated by `capabilities.streaming_tools`),
`approval_required` and `suspend` (gated by `capabilities.hitl_suspend`),
`approval_resume` (echo of the host's resolution, ungated — it mirrors the
host's `approval_resume` command), and `provider_circuit_event` (always-on
diagnostic, see §1.N+5).

### 1.N trace_event (W1)

Emitted at the end of each turn when the engine has been configured with
`observability.structured_traces = true` in `wcore.toml` AND the
corresponding `capabilities.structured_traces` flag is `true` on the
Ready event for the session.

```json
{
  "type": "trace_event",
  "msg_id": "...",
  "trace": {
    "turn": 0,
    "model": "claude-3-5-haiku",
    "provider": "anthropic-family",
    "input_tokens": 1000,
    "output_tokens": 50,
    "cache_read": 800,
    "cache_write": 0,
    "cache_hit_rate": 0.8,
    "cost_usd": 0.0,
    "tool_calls": [
      {
        "call_id": "tu_01",
        "tool_name": "Read",
        "input": { "path": "/etc/hosts" },
        "output_summary": "127.0.0.1 localhost",
        "duration_ms": 12,
        "bytes_in": 24,
        "bytes_out": 19,
        "source_product": "wayland-core"
      }
    ],
    "hook_actions": [],
    "source_product": "wayland-core"
  }
}
```

| Field | Type | Description |
|---|---|---|
| `msg_id` | string | Same `msg_id` as the surrounding `stream_start` / `stream_end`. |
| `trace.turn` | u64 | Zero-indexed turn within the session. |
| `trace.model` | string | Model identifier passed to the provider. |
| `trace.provider` | string | **Schema-versioned.** W1 emitted coarse provider family (`"anthropic-family"` / `"openai-family"`). W6 upgraded this to the structured per-provider identity sourced from `ProviderCompat.provider_type`: one of `"anthropic"`, `"bedrock"`, `"vertex"`, `"openai"`, `"ollama"`, or `"unknown"`. Hosts MUST tolerate both shapes during the migration window. |
| `trace.input_tokens` | u64 | Prompt tokens reported by the provider. |
| `trace.output_tokens` | u64 | Completion tokens reported by the provider. |
| `trace.cache_read` | u64 | Provider-reported cache read tokens. |
| `trace.cache_write` | u64 | Provider-reported cache creation tokens. |
| `trace.cache_hit_rate` | f64 | `cache_read / input_tokens`. 0.0 when input_tokens is 0. |
| `trace.cost_usd` | f64 | USD cost for the turn. W6 populates this from the per-provider list-price rows on `ProviderCompat` (per-model pricing is W6.1). Stays `0.0` when no cost row is set (e.g. local providers like Ollama). |
| `trace.tool_calls` | array | One `ToolCallTrace` per tool call executed in this turn. |
| `trace.hook_actions` | array | Hook action records. Empty until W2 wires the hook engine. |
| `trace.source_product` | string | Always `"wayland-core"` (S5 attribution). |

#### Host conformance

`trace_event` is gated by the W0-reserved `capabilities.structured_traces`
flag. Hosts that haven't learned about the type MUST drop it silently per
the Host Decoder Contract (Section X). Hosts that opt in render the trace
via their own trace UI.

### 1.N+1 session_cost (W6)

Emitted once per session, after the final `stream_end`, when
`AdvertisedCapabilitiesConfig.cost_attribution` is `true` — flipped by the
engine bootstrap when the active `ProviderCompat` has any non-`None` cost
row (`cost_per_input_token` or `cost_per_output_token`). The same flag is
mirrored to `Ready.capabilities.cost_attribution` so hosts can decide whether
to subscribe.

```json
{
  "type": "session_cost",
  "session_id": "sess-001",
  "total_cost_usd": 0.123456,
  "per_turn": [
    { "turn": 0, "model": "claude-opus-4-7", "provider": "anthropic", "cost_usd": 0.05 },
    { "turn": 1, "model": "claude-opus-4-7", "provider": "anthropic", "cost_usd": 0.073456 }
  ]
}
```

| Field | Type | Description |
|---|---|---|
| `session_id` | string | The session id that just terminated. |
| `total_cost_usd` | f64 | Sum of `per_turn[].cost_usd`. Floating-point arithmetic — hosts that need exact accounting should sum `per_turn` themselves. |
| `per_turn` | array | Per-turn cost rows. Each is `{ turn, model, provider, cost_usd }`. `provider` matches the structured per-provider identity used in `trace_event.trace.provider`. |

#### Host conformance

`session_cost` is gated by the W0-reserved `capabilities.cost_attribution`
flag. Hosts that did NOT see `cost_attribution: true` on the Ready event
MUST drop the variant silently per the Host Decoder Contract. Hosts that
opt in surface it via their cost UI (totals, per-session breakdown,
billing-export, etc.). Per-turn cost remains available inline on
`trace_event.trace.cost_usd` when `structured_traces` is also enabled.

### 1.N+2 sub_agent_event (W7)

Emitted by the parent session whenever a child session (spawned via the
`Spawn` tool) produces a `ProtocolEvent`. Gated by the W0-reserved
`capabilities.sub_agent_traces` flag — the engine emits this variant only
when the `ProtocolSink` was built with `with_sub_agent_traces(true)`. The
parent session forwards each child event wrapped in this envelope; the
child's original event is carried verbatim inside `inner`.

```json
{
  "type": "sub_agent_event",
  "parent_call_id": "tool-call-007",
  "agent_name": "research-subagent",
  "inner": {
    "type": "text_delta",
    "text": "Searching the codebase for references...",
    "msg_id": "sub-m1"
  }
}
```

| Field | Type | Description |
|---|---|---|
| `parent_call_id` | string | The `call_id` of the parent's `Spawn` tool invocation. Groups every event from that sub-agent. |
| `agent_name` | string | Sub-agent identifier (typically the role / skill name passed to `Spawn`). |
| `inner` | object | A fully-formed `ProtocolEvent` from the sub-agent's stream. May be any event variant (including `tool_request`, `tool_result`, `stream_start`, `stream_end`, etc.). Carried as `serde_json::Value` so this envelope stays non-recursive. |

**Emission trigger.** Every event the sub-agent's own `ProtocolSink` would
have emitted to its parent's stdout is re-wrapped here when sub-agent
tracing is enabled. The sub-agent's `msg_id`s live in their own namespace
and MUST NOT be confused with the parent session's `msg_id`s — hosts
correlating events should key off `parent_call_id` + `inner.msg_id`.

#### Host conformance

`sub_agent_event` is gated by the W0-reserved
`capabilities.sub_agent_traces` flag. Hosts that haven't learned the type
MUST drop it silently per the Host Decoder Contract. Hosts that opt in
typically render sub-agent activity inline under the parent's `Spawn`
tool result (tree-style trace view, separate transcript pane, etc.).

### 1.N+3 tool_chunk (W7)

Incremental partial output from a long-running tool (e.g. `Bash` running
a multi-minute build, `Spawn` streaming a child agent's text). Emitted
ahead of the tool's final `tool_result`. Gated by the W0-reserved
`capabilities.streaming_tools` flag (`ProtocolSink::with_streaming_tools(true)`).

```json
{
  "type": "tool_chunk",
  "msg_id": "abc-123",
  "call_id": "tool-call-001",
  "tool_name": "Bash",
  "chunk": "Compiling wcore-agent v0.1.0\n"
}
```

| Field | Type | Description |
|---|---|---|
| `msg_id` | string | Same `msg_id` as the surrounding `stream_start` / `tool_running`. |
| `call_id` | string | Matches the `tool_request` / `tool_running` / `tool_result` triplet for this invocation. |
| `tool_name` | string | The tool emitting the chunk. |
| `chunk` | string | Raw partial output — typically a stdout/stderr line. Hosts append; they MUST NOT assume framing semantics (chunks may split mid-line if the underlying process flushes mid-byte). |

**Emission trigger.** Tools that support streaming (currently `Bash`,
extensible via the tool trait) call `OutputSink::emit_tool_chunk` from
their execution loop. The full buffered output still arrives as the
final `tool_result.output`, so buffered hosts (i.e. hosts that don't
opt into `streaming_tools`) lose nothing — they just don't see live
progress.

#### Host conformance

`tool_chunk` is gated by the W0-reserved `capabilities.streaming_tools`
flag. Hosts that haven't learned the type MUST drop it silently per the
Host Decoder Contract; the final `tool_result` will still close the
call_id with the complete output. Hosts that opt in render chunks
progressively (incremental terminal pane, live build log, etc.) and
treat `tool_result` as the "stream ended" signal for that call.

### 1.N+4 approval_required / suspend / approval_resume (W7)

Three correlated events for human-in-the-loop (HITL) approval flows. The
engine pauses the active turn, emits `approval_required` and `suspend`
together, then resumes (and emits `approval_resume` as confirmation)
once the host returns the matching `approval_resume` command (§2.10).
All three are gated by the W0-reserved `capabilities.hitl_suspend` flag.

**`approval_required`** — engine asks the host for permission.

```json
{
  "type": "approval_required",
  "call_id": "tool-call-001",
  "resume_token": "rt-9b3c",
  "reason": "Edit outside workspace root",
  "context": "Write /etc/hosts (denied by policy; needs explicit approval)"
}
```

| Field | Type | Description |
|---|---|---|
| `call_id` | string | The pending tool/operation `call_id` awaiting approval. |
| `resume_token` | string | Opaque server-generated token. Host MUST echo this back verbatim in the corresponding `approval_resume` command — it routes the decision to the right pending `ApprovalBridge`. |
| `reason` | string | Short machine-readable reason category (e.g. `"Edit outside workspace root"`, `"Exec — destructive command"`). |
| `context` | string | Human-readable detail — the host displays this in the approval modal. |

**`suspend`** — session-level state transition emitted alongside
`approval_required`. Hosts that render a state pill (Idle / Streaming /
Suspended) update from this event independently of the modal flow.

```json
{
  "type": "suspend",
  "reason": "Edit outside workspace root",
  "resume_token": "rt-9b3c"
}
```

| Field | Type | Description |
|---|---|---|
| `reason` | string | Same `reason` string carried on the paired `approval_required`. |
| `resume_token` | string | Same `resume_token` carried on the paired `approval_required`. |

**`approval_resume`** — engine echoes the host's decision back so other
attached hosts (CLI mirror, UI, plugins) can clear their pending state
regardless of who emitted the resolving command.

```json
{
  "type": "approval_resume",
  "resume_token": "rt-9b3c",
  "approved": true
}
```

| Field | Type | Description |
|---|---|---|
| `resume_token` | string | The token from the original `approval_required` / `suspend`. |
| `approved` | bool | `true` if the host approved, `false` if denied. |

**Emission trigger.** A tool or operation that needs HITL approval calls
`ApprovalBridge::request(...)`, which routes through the
`ProtocolSink` and emits `approval_required` + `suspend`. The session
remains parked (no further events on that `msg_id`) until the host
returns the matching `approval_resume` command. On resume, the engine
emits `approval_resume` as confirmation and the original tool either
proceeds (approved) or fails with a deny reason (denied) — visible via
the usual `tool_result` / `tool_cancelled` events.

#### Host conformance

All three event types are gated by the W0-reserved
`capabilities.hitl_suspend` flag. Hosts that haven't learned the types
MUST drop them silently per the Host Decoder Contract — but in that
case the session will stall indefinitely on any HITL-eligible operation
(no resume command will ever arrive). Hosts opting in MUST surface the
approval modal AND wire a corresponding `approval_resume` command path
(§2.10).

### 1.N+5 provider_circuit_event (W7)

Provider circuit-breaker state transition. Emitted when
`ResilientProvider` transitions between Closed / Open / HalfOpen, or
when a fallback provider is engaged.

**NOT gated by an opt-in capability flag.** Circuit transitions are
always-visible diagnostics — same policy as `error`. The W0 capability
pattern advertises host *decoder* capability, not host *emission*
opt-in. A buggy host that ignored circuit events would render no
fallback indication for an entire incident; the always-on choice is
consistent with how `error` is already handled (cross-audit approved
2026-05-15).

```json
{
  "type": "provider_circuit_event",
  "primary": "anthropic",
  "fallback": "openai",
  "state": "open",
  "error": "5 consecutive failures — circuit opened, falling back"
}
```

| Field | Type | Description |
|---|---|---|
| `primary` | string | Identifier of the primary provider that tripped the breaker. Structured per-provider id (matches `ProviderCompat.provider_type()`). |
| `fallback` | string? | Identifier of the fallback provider, if one was engaged. Omitted when the transition is "closed → half_open" or "half_open → closed" recovery (no fallback in play). |
| `state` | string | Breaker state after the transition: `"closed"`, `"open"`, or `"half_open"`. |
| `error` | string? | Short error/reason that caused the transition. Omitted on recovery transitions. |

**Emission trigger.** `ResilientProvider` wraps a primary provider with
a `CircuitBreaker` (configurable failure threshold and recovery
timeout). On each state change, it routes through `ProtocolCircuitReporter`,
which calls `OutputSink::emit_provider_circuit_event`. The wrap is
enabled by `ProviderChain` config (off by default; see `wcore-config`).

#### Host conformance

Per the W0 Host Decoder Contract, hosts that haven't learned the
`provider_circuit_event` `type` MUST drop it silently — same forward-compat
baseline as any other unknown event. Hosts that render it typically
surface a banner ("anthropic down, fallback active") and a transient
indicator on recovery.

### 1.N+6 browser_event (W8c.1)

Browser-suite op event. Emitted by the engine once per completed
browser op (`Navigate`, `Snapshot`, `Click`, ...) so the host can
render a compact tool-call trail.

**Gated by `capabilities.browser_suite`.** The engine advertises the
flag when the `wayland-browser` plugin is loaded (W8c.3 H.2 wire-up).
Hosts that don't recognise `browser_event` MUST drop it silently per
the W0 host decoder contract.

```json
{
  "type": "browser_event",
  "msg_id": "msg_42",
  "call_id": "call_7",
  "op": "navigate",
  "url": "https://example.com",
  "summary": "loaded"
}
```

| Field | Type | Description |
|---|---|---|
| `msg_id` | string | Parent assistant message id (correlates with the `tool_request` that triggered the op). |
| `call_id` | string | Tool call id (matches `tool_request.call_id`). |
| `op` | string | Op kind as serialized by `BrowserOp` (e.g. `"navigate"`, `"snapshot"`, `"click"`). |
| `url` | string? | Origin / target URL when relevant (`Navigate`, `NewTab`, `Download`). Omitted for ops without a URL (`Snapshot`, `Click`). |
| `summary` | string | One-line human-readable summary (e.g. `"loaded"`, `"clicked @e3 button \"Submit\""`). |

### 1.N+7 browser_policy_denied (W8c.1)

A browser op was blocked by `BrowserPolicy` before dispatch — the
host renders an explicit block notification so the user can react.
Always emitted alongside the corresponding error `tool_result`; the
dedicated variant gives hosts a typed surface for blocked-URL
telemetry.

**Gated by `capabilities.browser_suite`.**

```json
{
  "type": "browser_policy_denied",
  "msg_id": "msg_42",
  "url": "https://malicious.example",
  "reason": "origin not in policy.allowed_origins"
}
```

### 1.N+8 cua_event (W8c.2)

Computer-use op event. Emitted by the engine once per completed CUA
op (`LeftClick`, `Type`, `Screenshot`, ...) so the host can render
a compact action trail.

**Gated by `capabilities.computer_use`.** The engine advertises the
flag when the `wayland-cua` plugin is loaded (W8c.3 H.2 wire-up).

```json
{
  "type": "cua_event",
  "msg_id": "msg_42",
  "call_id": "call_8",
  "op": "left_click",
  "coords": [100, 200],
  "summary": "clicked at (100, 200)"
}
```

| Field | Type | Description |
|---|---|---|
| `msg_id` | string | Parent assistant message id. |
| `call_id` | string | Tool call id. |
| `op` | string | Op kind as serialized by `CuaOp` (e.g. `"left_click"`, `"type"`, `"screenshot"`). |
| `coords` | [int, int]? | `[x, y]` screen coords for ops that have them (mouse/key). Omitted for `Screenshot`, `AxTree`, `Wait`, `FrontmostApp`. |
| `summary` | string | One-line human-readable summary. |

### 1.N+9 cua_policy_denied (W8c.2)

A CUA op was blocked by `CuaPolicy` before dispatch. Mirrors
`browser_policy_denied`; gives hosts a typed channel to render
policy violations as a distinct notification kind.

**Gated by `capabilities.computer_use`.**

```json
{
  "type": "cua_policy_denied",
  "msg_id": "msg_42",
  "op": "left_click",
  "app": "com.apple.terminal",
  "reason": "forbidden app"
}
```

### 1.N+10 plugin_event (W2.5 / W8c.3)

Plugin-emitted free-form event. The `plugin_name` is the registered
plugin manifest name; `event_type` is plugin-defined free-form (e.g.
`"memory_capture"`, `"index_rebuild_complete"`); `payload` is the
plugin-supplied JSON value.

**Gated by `capabilities.plugins`.** Engine advertises the flag when
any plugin has loaded (W8c.3 H.2 wire-up).

```json
{
  "type": "plugin_event",
  "plugin_name": "wayland-ijfw",
  "event_type": "memory_capture",
  "payload": {"key": "abc", "tier": "P2"}
}
```

### 1.N+11 budget_exceeded (W8a)

Singular per-session event — fires once when the first
`ExecutionBudget` cap (turns / tokens / cost / wall time) trips. The
event is paired with a `cancellation_token.cancel()` that propagates
into every in-flight tool's `ToolContext.cancel`.

**Host-tolerated, no dedicated capability flag** (see "Host-tolerated
additive variants" subsection above). Older hosts that don't know
about `budget_exceeded` drop the line silently per W0.

```json
{
  "type": "budget_exceeded",
  "reason": "max_tokens",
  "observed": "12345",
  "limit": "10000"
}
```

### 1.N+12 host_send_message_request (#537/#141)

Host-delegated `send_message`: when the host spawned the engine with
`WAYLAND_SEND_MESSAGE_HOST_DELEGATE=1`, an **approved** `send_message`
tool call is fulfilled by the HOST — the engine emits this request and
parks the tool call awaiting the host's `host_send_message_result`
command (§2.11), correlated by `call_id`. The wait is bounded (30s);
no reply resolves the tool call as a loud error, never a hang or a
false success.

**Host-tolerated, no dedicated capability flag** — only hosts that
opted in via the env var ever receive it; others never see it (and
would drop it silently per W0).

> **Security invariant (wayland#543 audit finding 4).** The host
> performs the delivery WITHOUT re-gating: it trusts that the engine's
> tool-approval flow (`tool_request` / allow-list / mode gate) already
> ran for this `send_message` call. The engine guarantees this — the
> event is only emitted from inside the tool's `execute`, which the
> orchestration approval gate fronts; `send_message` is Exec-category
> and in no auto-approve default
> (`crates/wcore-agent/tests/host_send_delegation.rs` pins it).
> `ApprovalScope::Always` on `send_message` deliberately downgrades to
> `Once` — every send gets its own confirmation card.
>
> The approval gate IS the delegation contract: a host that spawns the
> engine with `--auto-approve` / `--force` (or grants wire-force via
> `WAYLAND_ALLOW_WIRE_FORCE=1`) is opting out of that gate and MUST
> supply its own confirmation UX before fulfilling these requests.

```json
{
  "type": "host_send_message_request",
  "call_id": "hsm-3f6c…",
  "platform": "email",
  "chat_id": "mike@example.com",
  "thread_id": "t-17",
  "body": "hello from the agent",
  "subject": "Re: invoice",
  "conversation_id": "abc123"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `call_id` | string | yes | Engine-minted correlation id (`hsm-{uuid}`). Echo it back verbatim on the result. |
| `platform` | string | yes | `MessagingPlatform::as_str()` token (`"email"`, `"telegram"`, …). |
| `chat_id` | string | no | Recipient (for email: the destination address). Omitted when the target carried none. |
| `thread_id` | string | no | Reply-to / thread handle. Omitted when absent. |
| `body` | string | yes | The message text. |
| `subject` | string | no | Subject line. The current `send_message` schema has no subject input, so the engine omits it today; part of the wire contract for forward-compat. |
| `conversation_id` | string | no | Session id of the emitting engine, when known. |

### 2.11 `host_send_message_result` (#537/#141)

The host's reply to `host_send_message_request` (§1.N+12). Accepted
both between turns and MID-turn (the tool call is parked inside the
active turn — same mid-turn routing as `approval_resume`). An unknown
/ stale `call_id` resolves nothing and is surfaced as an `info` event.

```json
{
  "type": "host_send_message_result",
  "call_id": "hsm-3f6c…",
  "ok": true,
  "message_id": "smtp-250-2.0.0"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `call_id` | string | yes | Echoed verbatim from the request. |
| `ok` | bool | yes | `true` → the tool call resolves as sent; `false` → the tool call fails with `error`. |
| `message_id` | string | no | Platform-assigned receipt for a successful send. |
| `error` | string | no | Human-readable failure reason; surfaced verbatim to the model when `ok` is `false`. |
