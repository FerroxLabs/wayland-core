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

> **Normative source.** This document is prose guidance. The machine-readable,
> digest-pinned producer contract in `crates/wcore-protocol/contracts/desktop/v1/`
> (JSON Schemas, canonical fixtures, adversarial vectors, `manifest.json`) is the
> normative wire definition and is byte-checked in CI by `wcore-contract check`.
> It currently covers 18 commands and 49 events; this document does not yet
> narrate every one of them. Where the two differ, the corpus wins — and the
> difference is a documentation bug worth reporting.

## 1. Agent → Client Events (stdout)

Every line is a JSON object with a `type` field.

### 1.1 `ready`

Emitted once after initialization completes. Client MUST wait for this before sending messages.

```json
{
  "type": "ready",
  "version": "0.12.25",
  "session_id": "a1b2c3",
  "session_persistence": "durable",
  "capabilities": {
    "tool_approval": true,
    "thinking": true,
    "effort": false,
    "effort_levels": [],
    "modes": ["default", "auto_edit", "force"],
    "current_mode": "default",
    "mcp": true
  },
  "contract": {
    "name": "wayland-desktop-core",
    "major": 1,
    "minor": 11,
    "generator": "wcore-desktop-contract-gen/12",
    "fixture_digest": "sha256:0704...",
    "schema_digest": "sha256:e5d1...",
    "source_inputs_digest": "sha256:9d59...",
    "capabilities": { "contract_negotiation": "available" }
  },
  "execution_policy": {
    "critical": true,
    "contract_version": "1.0",
    "revision": 0,
    "reason": "launch",
    "effective_at_unix_ms": 1721000000000,
    "policy": {
      "posture": "smart",
      "approvals": "prompt",
      "sandbox": "required",
      "source": "desktop_local_launch",
      "managed_floor_active": false
    }
  }
}
```

`contract` and `execution_policy` are **required**. The reference host observer
(`wcore_protocol::contract::HostContractObserver`) fails closed before
negotiation when either is absent or malformed, and `ready` must be the first
line on the stream. See the pinned corpus at
`crates/wcore-protocol/contracts/desktop/v1/` for the byte-exact shape.

`session_id` is **required and nullable**, never omitted. It is this event's
correlation key, and a host keys its own session tracking on it — so a producer
that drops the key hands the host `undefined` with no accompanying signal, and
the host cannot tell a degraded Core from a malformed frame from a Core too old
to know. `session_persistence` states which cause produced the value it holds:

| Value | `session_id` | Meaning |
|-------|--------------|---------|
| `durable` | string | A journaled session with crash replay. It survives a restart, can be resumed, and a turn interrupted mid-dispatch resumes itself from the sealed provider request |
| `journaled_without_replay` | string | A journaled session **without** crash replay. History, provider attempts, tool calls, approvals and deliveries are all recorded and survive a restart; what is missing is the sealed copy of the exact provider request, because this host has no usable OS keyring and no unlocked credentials vault |
| `disabled_by_operator` | `null` | `[session] enabled = false`. Nothing is journaled, by request |
| `disabled_by_host` | `null` | **Decode-only, from a producer older than contract minor 12.** Such a Core answered a missing key by turning durable sessions off. A current Core journals instead and never sends this |

### What a host should do with `journaled_without_replay`

Treat the session as durable for history and audit: list it, offer resume, keep
it. Do **not** show auto-recovery affordances or wait on one.

- A turn interrupted mid-dispatch does not resume itself. The next message on
  that session is refused with a reconciliation error naming the interrupted
  turn — surface a resume / reconcile / cancel choice, not a retry spinner.
- Every turn on such a session also carries a per-turn `info` frame saying
  replay is off, correlated to that turn's `msg_id`.
- A `--resume` of a session whose sealed state cannot be opened is refused
  **by name**, as a single non-retryable `error` frame with **no preceding
  `ready`**. That session is LOCKED pending a key, not corrupt: leave its
  journal alone, because restoring `WAYLAND_VAULT_PASSPHRASE_FD` and resuming
  again recovers it. Only that session is refused — a launch that does not name
  it starts and journals normally.

To refuse to run this way at all, set `[session] require_durability = true`;
Core then declines to start rather than accepting turns it could not recover.

Feature-detect via two capabilities on `ready.contract.capabilities`, and they
are **additive rather than successive**:

- `session_persistence_v1` — the frame SHAPE: `session_id` is always on the
  wire and `session_persistence` always states the cause. A current Core still
  declares it, because it still keeps that promise. A Core that declares
  neither may omit `session_id` entirely, and its absence means nothing in
  particular.
- `session_persistence_v2` — the wider VOCABULARY: `session_persistence` may be
  `journaled_without_replay`. A host that feature-detected on v1 alone minted
  its switch when the enum had three values; since the enum is closed, such a
  host must detect v2 before it can accept the fourth.

The keyring-less frame is pinned byte-exact at
`crates/wcore-protocol/contracts/desktop/v1/compat/events/ready.journaled-without-replay.json`,
and the legacy value the schema must still accept at
`.../compat/events/ready.disabled-by-host.legacy.json`.

| Field | Type | Description |
|-------|------|-------------|
| `version` | string | Protocol version (semver) |
| `session_id` | string \| null | Session ID. **Always present**, `null` when this run has no durable session. Never omitted — see below. `null` means, and now only means, that the operator set `[session] enabled = false`: since 2026-08-02 a host that cannot protect a durable session journals anyway, without the sealed replay copy of the provider request, so it has a real session and names it |
| `session_persistence` | string | Why `session_id` holds what it holds: `durable`, `journaled_without_replay`, `disabled_by_operator`, or (decode-only, from an older producer) `disabled_by_host`. Required. See §1.1b |
| `contract` | object | Pinned producer-contract descriptor. Required. Host compares `name`, `major`, `minor`, `generator` and all three digests against its own pin and fails closed on any mismatch |
| `execution_policy` | object | Launch policy snapshot at `revision` 0 with `reason` `launch` or `resume`. Required. Same envelope as the `execution_policy` event (§1.1a) |
| `capabilities.tool_approval` | bool | Whether agent supports pause-and-wait tool approval |
| `capabilities.thinking` | bool | Whether current provider supports extended thinking |
| `capabilities.effort` | bool | Whether current provider supports reasoning_effort |
| `capabilities.effort_levels` | string[] | Valid effort values (e.g., `["low", "medium", "high"]`). Empty when effort is false |
| `capabilities.modes` | string[] | Available approval modes for `set_mode` command |
| `capabilities.current_mode` | string | Currently active approval mode |
| `capabilities.mcp` | bool | Whether MCP tools are available |
| `capabilities.memory_enabled` | bool (W0) | Long-term cross-session memory is active |
| `capabilities.online_evolution` | bool (W0) | Online GEPA evolution is active |
| `capabilities.user_model_backend` | string (W0) | Backend serving the user-selected model (e.g. `local`) |
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

### 1.1a `execution_policy`

Emitted immediately after `ready`. It reports the immutable launch authority
that Core actually enforced; it is not a command and cannot be sent back to
mint authority.

```json
{
  "type": "execution_policy",
  "critical": true,
  "contract_version": "1.0",
  "revision": 1,
  "reason": "mode_change",
  "effective_at_unix_ms": 1721000000100,
  "policy": {
    "posture": "smart",
    "approvals": "auto_edit",
    "sandbox": "required",
    "source": "protocol",
    "managed_floor_active": false
  }
}
```

All six top-level fields are **required**. `critical` is always `true`: this is
an authority-critical sub-contract, so a contract-aware host that does not
understand the event or its `contract_version` major must fail closed rather
than drop it. `contract_version` is the execution-policy sub-contract version
(currently `1.0`; only major `1` is accepted).

`revision` is session-monotonic. It starts at `0` in the `ready` snapshot
(`reason` `launch` or `resume`) and advances by exactly one for every accepted
policy change whose serialized `policy` bytes actually changed — an accepted
no-op `set_mode` therefore does **not** consume a revision. `reason` is
`launch`, `mode_change`, `resume`, or `expiry`. `effective_at_unix_ms` is
audit/display evidence only; monotonic runtime deadlines remain the authority
for dangerous-session expiry.

Reducer rules (`wcore_protocol::execution_policy::ExecutionPolicySequence`):
a byte-identical repeat of the current revision is an idempotent `Duplicate`; a
same-revision snapshot with different bytes, a gapped or stale revision, an
unsupported `contract_version` major, and `critical: false` all fail closed.

`posture` is `smart`, `managed`, or `dangerous`. `approvals` is `prompt`,
`auto_edit`, or `bypass`; `sandbox` is `required` or `bypass`. **`sandbox:
required` reports session AUTHORITY — a real backend was selected and this is
not a Dangerous launch. It is not a claim about how much that backend
enforces**, which varies by platform: bubblewrap (Linux) and `sandbox-exec`
(macOS) confine the child's filesystem, the Windows session default
(`windows_job_object`) does not. A host must not render `required` as
"the workspace is a boundary"; `wayland-core sandbox status` reports the
per-capability breakdown, including `confines_filesystem`. Dangerous
snapshots also carry `dangerous_activation_id` and
`dangerous_expires_at_unix_ms`; non-dangerous snapshots must omit both.

### 1.1b `workspace_policy`

Emitted after `execution_policy` and again when a local host-approved developer
capability changes the effective read roots. It is an output-only receipt of
what Core enforces; echoing any field back cannot mint trust or authority.

`writable_roots` / `readable_roots` are the roots Core applies to its OWN file
tools (Read / Write / Edit, via the VFS jail) on every platform, and hands to
the OS sandbox as the child's grants. Whether the OS then STOPS a shell child
from leaving them is the `backend`'s property, not the receipt's: on the Windows
default (`backend: "windows_job_object"`) it does not, so a shell command can
write outside these roots. Hosts presenting these as a containment boundary
should qualify it by `backend`.

```json
{
  "type": "workspace_policy",
  "policy": {
    "trust": {
      "level": "trusted",
      "source": "user",
      "fingerprint": "d14a...",
      "explanation": "fingerprint-bound local trust decision is current"
    },
    "profile": "trusted_local_smart",
    "backend": "sandbox-exec",
    "writable_roots": ["/workspace", "/private/tmp"],
    "readable_roots": ["/opt/homebrew", "/workspace"],
    "capabilities": []
  }
}
```

The default for a repository without a current external fingerprint decision is
`strict`. Managed, remote, and child constraints remain strict regardless of a
stored local decision. Hosts must display the receipt as effective state, not
as a selectable trust claim.

### 1.1c Durable turn recovery events (contract v1.1)

Recovery v1 is a fail-closed, content-free view of the durable session
journal. It lets Desktop reconnect without treating transcript text, provider
payloads, tool arguments, tool output, paths, or approval secrets as recovery
authority. Every recovery frame carries `recovery_version: 1` and opaque
correlation IDs. A recovery cursor binds a journal sequence to a lowercase,
raw 64-hex SHA-256 content digest; neither component is authoritative on its
own. Recovery cursor and state digests deliberately omit the `sha256:` prefix
used by evidence, artifact, and contract digests.

`session_recovery_snapshot` reports one sanitized committed state:

```json
{
  "type": "session_recovery_snapshot",
  "recovery_version": 1,
  "request_id": "recovery-request-001",
  "session_id": "session-desktop-001",
  "cursor": {
    "journal_sequence": 40,
    "journal_digest": "4444444444444444444444444444444444444444444444444444444444444444"
  },
  "state_digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "lifecycle": "reconciliation_required",
  "pending_turn": {
    "turn_id": "turn-002",
    "msg_id": "msg-002",
    "lifecycle": "reconciliation_required",
    "pending_call_id": "call-tool-002",
    "reconcile_reason": "tool_outcome_unknown"
  },
  "budget": {
    "tokens_used": 12000,
    "token_limit": 20000,
    "cost_used_usd": 1.25,
    "cost_limit_usd": 5.0
  }
}
```

`session_recovery_replay` contains ordered, content-free milestones after the
requested cursor. `from` must exactly match the host's accepted cursor, item
sequences must be contiguous, and `through` must equal the final item cursor.
An identical sequence with a different digest is a conflict, not a duplicate.
Transitions without a more specific public milestone use `state_advanced` so
the cursor sequence remains contiguous without exposing private payloads.

`session_recovery_unavailable` refuses recovery with one typed reason:
`session_not_found`, `unsupported_version`, `cursor_invalid`, `cursor_ahead`,
`cursor_digest_mismatch`, `history_gap`, `journal_corrupt`,
`snapshot_unavailable`, or `unknown_critical_state`. Hosts must not silently
restart a turn after this event.

`turn_recovery_lifecycle` reports a durable transition for one turn. Lifecycle
values are `ready`, `streaming`, `awaiting_approval`, `tool_in_flight`,
`reconciliation_required`, `suspended`, `completed`, `cancelled`, and
`failed`. A reconciliation reason is required whenever Core cannot prove that
direct continuation is safe.

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

**`text` never contains inline reasoning tags.** Stripping is the AGENT's job,
not the client's — see [1.4 `thinking`](#14-thinking). A client must not
implement its own `<think>` stripper; if a reasoning tag ever reaches
`text_delta`, that is an agent defect, and the client rendering it verbatim is
the correct behaviour for surfacing it.

### 1.4 `thinking`

The model's private reasoning. Two producers land on this one event:

1. **Native reasoning** — a provider that returns reasoning as its own content
   block (Anthropic extended thinking, a provider reasoning summary). Requires
   `capabilities.thinking`.
2. **Inline reasoning** — open-weights models (DeepSeek-R1 / Qwen-QwQ class,
   reached through Flux or Ollama) that emit reasoning INSIDE the ordinary text
   stream wrapped in `<think>…</think>`, `<thinking>…</thinking>`,
   `<reasoning>…</reasoning>` or `<thought>…</thought>` (case-insensitive,
   attributes and self-closing forms included). The agent strips those from
   `text_delta` and re-emits the body here. This holds for a SPAWNED SUB-AGENT
   too: the relayed `text_delta` inside `sub_agent_event.inner` is split the
   same way, and the child's reasoning arrives as a relayed `thinking` on the
   same `parent_call_id`. `capabilities.thinking` may be `false` for these providers — it
   describes the provider's native reasoning feature, not this split.

Case 2 is advertised as the `inline_reasoning_split_v1` contract capability
(`contract.capabilities` on the `ready` event, contract `1.18` and later).
Before `1.18` the tags reached `text_delta` verbatim and every client rendered
them inside the assistant bubble.

Tag bodies may straddle chunk boundaries, so a single inline block can arrive as
several `thinking` events; concatenating the `text` of every `thinking` event on
one `msg_id` reconstructs the block. Two blocks in one turn are separated by a
newline. An unclosed block is flushed immediately before `stream_end`.

The event carries no obligation to display. A client may render it collapsed
(the Wayland CLI TUI shows a one-line `▶ Thought: …` the user can expand), or
drop it entirely. What it must not do is treat it as answer text.

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
| `tool.escalation` | object \| absent | Why this call is being shown beyond the ordinary gate. Absent on almost every request. See below. |

**`tool.escalation` — the pre-flight boundary prompt**

Requires the `path_boundary_prompt_v1` capability. When the field is absent
(the overwhelmingly common case) nothing has changed and the frame is
byte-identical to what older Core emitted.

```json
"escalation": {
  "kind": "path_boundary",
  "target": "/Users/me/Documents/notes/q3.md",
  "access": "read",
  "suggested_root": "/Users/me/Documents/notes"
}
```

This appears when a read tool names a path outside every root the session can
reach. Before it existed, such a call ran, failed with an out-of-sandbox tool
error, and the model was left to explain the dead end to the user.

* `target` — the path the call named, canonicalized.
* `access` — always `"read"`. Core raises this escalation only for a read tool;
  a write grant is minted from a folder the OPERATOR chose, never from a path
  the model named. Write access outside the workspace is not
  grantable, so a write never raises this escalation.
* `suggested_root` — the **containing folder**, which is what a grant actually
  opens. Putting `target` on an "always allow this folder" button would label
  the button with a scope it does not have.

Answering the approval with
`{"scope": {"always_path": {"root": "<suggested_root>", "write": false}}}`
(§2.3.2) is **guaranteed to be accepted**: Core dry-runs that exact grant
against the session's workspace policy before emitting the frame, so this is
never a button that silently fails.

`always_path` is the ONLY answer that makes the call succeed, and a host that
raises this card must offer it. The other two answers do not:

* `once` releases the gate without minting a grant — so the call runs, reaches
  the sandbox it was flagged for crossing, and fails with an out-of-sandbox
  tool error. The prompt is not what makes the read work; the grant is.
* `always` registers the TOOL name and nothing else. The boundary check runs in
  front of every call and forces the gate past a tool-name grant, so the next
  call on the same path prompts again, and the one after that, indefinitely.

Denying mints nothing and skips the call, which is the honest refusal.

The gate is forced for these calls even when the tool is on the allow-list or
carries a tool-name/prefix auto-approval — those grant the tool, not the path.
`force` mode still bypasses everything, so the field never appears there.

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
| `finish_reason` | `"stop" \| "length" \| "error" \| "max_turns"` | Why the turn ended. `stop`: model finished normally. `length`: hit max_tokens. `error`: provider/runtime error. `max_turns`: the engine hit the per-turn `max_turns` cap (the model did **not** fail) — offer a "Continue" affordance to resume the run rather than a model-error message. Hosts should treat `finish_reason` as an open string and tolerate future values. |
| `usage` | object? | Token counts (optional; omitted when provider does not report usage). In protocol v0.2.0 the three input categories are disjoint: `input_tokens` is uncached input, `cache_read_tokens` is cached input read, and `cache_write_tokens` is input written to cache. Total input processed is their saturating sum. Billing consumers MUST price each category once at its applicable rate; they must not add cache counters to `input_tokens` and then price the cache counters again. |

This v0.2.0 accounting semantic corrects the ambiguous v0.1.21 description
without changing the JSON field names. A host that previously interpreted
`input_tokens` as already including the cache counters must update before using
these values for total-input telemetry, quota enforcement, or billing.

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
| `auth_required` | Provider rejected the credential (HTTP 401). Refreshable — the host should re-auth / refresh the OAuth token and re-send the turn. `retryable` is left as the engine set it (typically `false`, since re-sending the same credential just burns budget), so hosts drive retry off the **code**, not the flag. |
| `auth_invalid` | Provider denied access (HTTP 403). Hard failure — do not retry. |
| `init_failed` | The engine failed during startup and is exiting. Terminal. |
| `recovery_busy` | A recovery action was refused because another is active. Resync and retry. |
| `engine_error` | **The default.** Every error that is not one of the above arrives with this code, including all tool, config, provider and `add_mcp_server` failures, and the rejection of a malformed host command (§4.1). |

> Hosts should branch on `error.code` where a specific code exists — but note
> that `engine_error` is by far the most common code, and it is not a
> classification. To distinguish causes within `engine_error` you must either
> match the message text or correlate with the typed frame that accompanies it
> (for example `mcp_failed`).

**Codes this document used to list that the engine has never emitted:**
`tool_error`, `config_error`, `protocol_error`, `internal_error`. They were
aspirational and are removed rather than left to be branched on. A host with a
`case "protocol_error"` arm has dead code.

One further caveat, stated because it is confusing rather than because it is
correct: the Desktop contract corpus ships `events/error.json` with
`"code": "provider_error"`, and no production path emits that code either — it
is a fixture value. Do not infer the emitted vocabulary from that one fixture.

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

### 1.15 `anvil_receipt` (Anvil A1)

The engine's honest verdict for a gated-forge (`/forge`) climb. Additive
variant; a host that doesn't render receipts drops it silently per the Host
Decoder Contract (like `budget_exceeded`).

```json
{
  "type": "anvil_receipt",
  "terminal_state": "verified",
  "stamp": "verified",
  "checks_passed": 14,
  "checks_total": 14,
  "iterations": 3,
  "valve_fires": 1,
  "cost_microcents": 7000,
  "priced": true,
  "gate_closure_digest": "sha256:...",
  "artifact_digest": "sha256:...",
  "task_id": "task-1",
  "engine_version": "0.12.24",
  "sequence": 1
}
```

`valve_fires` counts escalation-valve diagnostic turns bought during the
climb (0 on the happy path; decoders of pre-valve receipts default it to 0).
`stamp` is the trust tier actually earned — `verified` ONLY for a real
executable gate; `criteria_checked` / `self_checked` / `format_validated` /
`consensus_only` otherwise (never green-parity). `coverage` and `session_id`
are optional (omitted when absent). When `priced` is `false`, the host renders
cost as **"unpriced"**, never `$0`.

**TRUST BOUNDARY (normative):** a host MUST render a receipt "chip" ONLY from
a **top-level** `anvil_receipt` event. Receipt-shaped content arriving nested
inside `sub_agent_event.inner` or `plugin_event.payload` is INERT — a
sub-agent or plugin can never forge a verified verdict. (Same class as the
tool-approval rule: a previewed fragment cannot forge the Approve/Reject
verdict.) Emission is engine-only, from the climb exit path.

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

#### 2.3.1 Scoped grants

`scope` also accepts two object forms. Both are additive: a host that only ever
sends `"once"` / `"always"` is unaffected by their existence, and neither
changes the bare-string wire shape.

| Form | Meaning |
|---|---|
| `{"always_prefix": {"prefix": "cargo "}}` | Auto-approve later commands in the same category whose normalized head matches `prefix`. |
| `{"always_path": {"root": "/Users/me/reports", "write": false}}` | Grant the session standing access to a folder outside the workspace. `write` defaults to `false`, so the bare object grants READ only; `write: true` is a separate, stricter grant (§2.3.2 rule 2). |

#### 2.3.2 `always_path` — "always allow this folder"

This is the answer to an approval prompt raised because a path sits outside the
session workspace. It exists so that case has a third option beside "do it this
once" and "refuse", without ever turning the sandbox off.

Core raises that prompt itself when it declares `path_boundary_prompt_v1`: the
`tool_request` carries `tool.escalation` (§1.5) and its `suggested_root` is the
root to send back here. Without that capability a host can still send
`always_path`, but it has to attach it to an approval it already has.

```json
{
  "type": "tool_approve",
  "call_id": "tool-call-001",
  "scope": { "always_path": { "root": "/Users/me/reports" } }
}
```

Contract, in the order a host needs it:

1. **`root` may be a file.** The host may send the exact path the user was
   looking at; the agent grants the directory that contains it. That is what a
   person answering "always allow this folder" believes they said. The prompt
   SHOULD name the folder that will actually be granted, not the file.
2. **`write` is a separate, STRICTER grant — never the same grant with a flag
   set.** `write: false` (the default) grants read. `write: true` applies every
   rule below and then four more, any of which refuses the grant outright
   rather than downgrading it silently:

   * **The OS sandbox must actually confine the filesystem.** On a backend that
     does not — today the Windows `windows_job_object` default — a shell command
     in the session can already create or overwrite files anywhere the user
     account can, so a "write access to this one folder" grant would name a
     boundary that does not exist. Read grants are unaffected there: they widen
     the in-process file tools, which are real on every platform. Hosts can read
     the backend's answer from `sandbox status` (`confines_filesystem`).
   * **No overlap with an auto-run location**, in either direction — the grant
     is inside one, or it contains one. `~/Library/LaunchAgents`,
     `~/.config/autostart`, `~/.config/systemd/user`, `~/.local/bin`, the
     Windows `Startup` folder, `/etc/cron.d`, and any `.git` (whose `hooks/`
     runs on the operator's next commit).
   * **Nothing already runnable inside it.** The folder is scanned; a regular
     file that is executable by its owner, or that carries an executable
     extension (`.exe`, `.msi`, `.ps1`, `.pkg`, …), refuses the grant. This is
     why `~/Downloads` is often refused for write and always grantable for
     read: it is the single most likely place an unsigned binary is sitting,
     and a write grant there is a write-to-RCE.
   * **No secret inside it.** A `.env`, `id_rsa` or `*.pem` under the folder
     refuses the write grant. The read deny-list already keeps such a file
     unreadable; there is no `fs_write_deny` in the OS sandbox manifest to
     express the write half with, so the honest narrowing is to refuse the
     ROOT rather than promise a per-file rule that only half the layers could
     enforce.

   A folder too large to scan within the budget is refused for write as well:
   the scan cannot prove the absence of an executable it never reached.

   A read grant NEVER implies write, and a live read grant is not cover for a
   later write request on the same folder — that request mints its own grant,
   with its own `grant_id` to revoke, so revoking the write leaves the read
   standing.
3. **A grant lasts for the process lifetime** and is not persisted across
   restarts. A host that wants a durable allow-list must re-send its grants on
   each launch; the agent will not remember them for you.
4. **It only applies to a genuinely local session.** A channel, remote or
   managed engine refuses every path grant. A wire peer may ASK — this is the
   same rule as `SessionMode::Force` (GHSA-8r7g), because a standing path grant
   expands filesystem authority past the sandbox root and is therefore
   precisely what a prompt-injected turn would like to arrange.
5. **Some roots are always refused**, whatever the user clicks: the filesystem
   root, `$HOME`, any directory that is or contains a credential store
   (`~/.ssh`, `~/.aws`, `~/.config/gh`, …), and any path matching the secret
   rules. A session holds at most 64 grants.
6. **A refused grant does not fail the call.** The action the user approved
   still runs — the approval degrades to `once`. The reason for the refusal is
   written to stderr. A host SHOULD NOT treat "approved" as proof that the
   standing grant was recorded.

Because of (6), the honest UI is a prompt that says what will happen for *this*
file, with "always allow this folder" as a convenience — not a setup step the
user is told has succeeded.

**What a grant does and does not protect, stated precisely** — the host is the
party choosing which folder to hand over, so it needs this to choose well.

* The `Read` tool resolves a granted file exactly once, relative to a retained
  directory handle, refusing a symlink at the leaf and at the parent. Bytes
  therefore come from the object that was checked, not from a name that could
  be re-pointed after the check (FerroxLabs/wayland#1105).
* That pin covers the file's own name and its immediate directory. Components
  **above** the granted folder are still resolved by the kernel from a
  pathname, so someone able to rename a directory higher up the tree can still
  redirect the read. Do not grant a folder whose ancestors are writable by
  anyone you would not grant the folder to.
* `exists`, `metadata` and directory listing remain path-based. They disclose
  presence, size and file names, never file contents.
* `Grep` shells out to `rg`, which opens the paths itself. Nothing inside the
  agent can pin that, so a grep over a granted folder carries the ordinary
  check-then-use exposure of any external process.
* A grant never widens what may be READ to include a secret inside the granted
  folder, whether it confers write or not.
* A WRITE grant widens exactly four operations (`write`, `remove_file`,
  `observe_file`, `compare_exchange_file`) and exactly one root. Every other
  root — including the granted folder's own parent and siblings — is refused
  as before, and a symlink out of the granted folder (live or dangling) is
  refused at the boundary rather than followed.

#### 2.3.3 `grant_path` / `revoke_path` — the flow with no pending call

`always_path` rides an approval, so it only serves the **agent-initiated** case:
the agent wanted something outside the workspace and a card was raised.

The **user-initiated** case has no pending `call_id` at all — the operator picked a
folder in a native picker, unprompted — so it gets its own command rather than a
scope on an approval that does not exist. Both land in the same grant store, so
there is exactly one mechanism to audit.

```json
{ "type": "grant_path",
  "grant_id": "3f2a…",
  "root": "/Users/me/Downloads/Mortgage",
  "access": "read",
  "expires_at_ms": 1755640000000 }
```

```json
{ "type": "revoke_path", "grant_id": "3f2a…" }
```

| Field | Required | Meaning |
|---|---|---|
| `grant_id` | yes | Host-chosen. Echo it to `revoke_path` to withdraw this exact grant |
| `root` | yes | Folder to grant. May be a file — the containing directory is granted |
| `access` | no | `read` (default) or `write`. `write` is the stricter grant of §2.3.2 rule 2; when a rule refuses it, it is refused outright and never downgraded to `read` |
| `expires_at_ms` | no | Unix ms deadline. Absent = process lifetime |

Every rule in §2.3.2 applies unchanged. In addition:

- **Launch opt-in required.** Core refuses unless started with
  `--allow-host-path-grants` (which itself requires `--json-stream`). It is a flag
  and not an environment variable on purpose: an env var set once per spawn cannot
  express "this session may, that one may not". Absent, the refusal is legible on
  the wire, not silent.
- **`revoke_path` is NOT gated.** Taking authority away is always permitted —
  requiring the opt-in to revoke would leave a host unable to clean up a grant it
  somehow held. An unknown `grant_id` is a no-op, so revoking is idempotent and a
  host that crashed mid-flow can clean up without knowing what landed.
- **Expiry is evaluated at use time**, not by a sweep, so a grant cannot outlive
  its deadline by racing whatever would otherwise reap it. This is what makes an
  unattended overnight run safe to grant to.
- **A write grant is announced as one.** The `info` frame Core emits on success
  reads `(read and write; sandbox remains active)` rather than `(read-only; …)`,
  so the confirmation the user sees matches the authority that was granted.
- **The deny-list wins.** A grant says *where* the agent may look, never *what* it
  may read. A secret inside a granted folder — `id_rsa`, `.env`, `*.pem` — stays
  refused, in the in-process file tools and in the OS sandbox's read-deny list
  alike. Checked lexically on the canonicalized path, so renaming a secret to
  `notes.txt` does not launder it, and a secret created after the grant is still
  caught.

After any `grant_path` or `revoke_path`, Core re-emits
[`workspace_policy`](#1n-workspace_policy) with the updated `readable_roots`. That
event is the authoritative answer to "what can this chat actually reach" — prefer
it over tracking grants host-side.

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

### 2.7a `grant_workspace_capability`

Ask Core to add the minimum runtime roots derived from a local executable as
read-only mounts for the rest of this process:

```json
{
  "type": "grant_workspace_capability",
  "executable": "/opt/acme-sdk/bin/acme"
}
```

Core accepts this command only when all of these are true:

1. The process was launched locally with `--json-stream` and
   `--allow-host-workspace-grants`.
2. The current repository fingerprint is trusted and no managed, remote, or
   child constraint has selected the strict profile.
3. The canonical target is an executable regular file outside known credential
   stores.

Success emits an updated `workspace_policy` receipt followed by an `info`
event. Refusal emits an `info` event explaining the failed condition. The
command never adds writable roots, changes approval posture, or disables the OS
sandbox. Hosts should expose it only behind an explicit local approval UI.

### 2.7b `continue_with_budget`

Add explicit operator-authorized headroom to the active session after a
provider budget stop:

```json
{
  "type": "continue_with_budget",
  "request_id": "budget-001",
  "additional_tokens": 250000,
  "additional_cost_usd": 2.50
}
```

`request_id` is required and must match
`^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$`: 1–128 ASCII bytes, beginning with an
alphanumeric byte. Whitespace, Unicode, shell/path punctuation, and longer
identifiers fail closed in both the JSON Schema and Core decoder. Both grant
fields are optional individually, but at least one must be positive. Token
headroom is an unsigned 64-bit integer; negative, fractional, wrong-type, and
values above `18446744073709551615` fail closed.

The grant applies only to the current session; it does not widen another
session or a per-user daily limit. Managed sessions reject interactive
increases so a host cannot override an organization-controlled ceiling.

Core returns a typed `budget_grant_result` correlated by `request_id`:

```json
{
  "type": "budget_grant_result",
  "request_id": "budget-001",
  "additional_tokens": 250000,
  "additional_cost_usd": 2.5,
  "outcome": "granted"
}
```

A `granted` result must omit `refusal_reason`. A `refused` result must include
exactly one reason from the closed refusal vocabulary. Contradictory result
shapes fail schema validation and typed deserialization.

During an active turn, Core returns terminal `refused` with
`turn_in_progress`; the host may retry after the terminal turn event with a
fresh request ID. Replaying the refused request ID returns the exact cached
terminal refusal; Core never converts it into a later grant. Core never
acknowledges a grant for later application unless that pending state is durable.
Identical replay returns the exact cached result without applying the grant
twice. Reusing a request ID with different grant content is refused with
`request_id_conflict`. Applied request bindings are
committed in the same durable budget-authority transaction as the extension,
so a crash after mutation but before response emission cannot apply the retry
twice. The durable and response ledgers are finite, fail closed with
`ledger_capacity_exceeded`, and never evict authoritative prior receipts. A
host should expose this only as an explicit local action after showing the
exhausted limit and requested headroom.

### 2.7c `session_resync`

Request a versioned recovery snapshot. Omitting `after` requests the current
committed snapshot. Supplying `after` also requests sanitized replay strictly
after that cursor.

```json
{
  "type": "session_resync",
  "recovery_version": 1,
  "request_id": "recovery-request-001",
  "session_id": "session-desktop-001",
  "after": {
    "journal_sequence": 40,
    "journal_digest": "4444444444444444444444444444444444444444444444444444444444444444"
  }
}
```

`request_id` makes retries idempotently correlatable. A genesis request omits
`after`; the genesis cursor returned by Core omits `journal_sequence` but still
carries its digest. Core responds with a recovery snapshot, optional replay,
or a typed unavailable event. Unsupported recovery versions fail closed.

### 2.7d `resume_turn`

Apply an explicit action to the interrupted turn state the operator inspected:

```json
{
  "type": "resume_turn",
  "recovery_version": 1,
  "request_id": "recovery-request-002",
  "session_id": "session-desktop-001",
  "turn_id": "turn-002",
  "cursor": {
    "journal_sequence": 42,
    "journal_digest": "6666666666666666666666666666666666666666666666666666666666666666"
  },
  "action": "reconcile"
}
```

`action` is `continue`, `reconcile`, or `cancel`. The cursor is mandatory and
must still identify the current committed state. `reconcile` invokes only
Core-registered authoritative reconcilers; the command cannot carry a
free-form claim that an external effect succeeded or failed.

### 2.7e `resolve_interrupted_approval`

Resolve the exact approval gate restored for an interrupted durable turn:

```json
{
  "type": "resolve_interrupted_approval",
  "recovery_version": 1,
  "request_id": "recovery-request-003",
  "session_id": "session-desktop-001",
  "turn_id": "turn-002",
  "cursor": {
    "journal_sequence": 42,
    "journal_digest": "6666666666666666666666666666666666666666666666666666666666666666"
  },
  "approval_id": "approval-002",
  "decision": "approve",
  "answer": "Proceed"
}
```

`decision` is `approve` or `deny`; `answer` is optional. Core binds the
decision to the request, inspected cursor, interrupted turn, and exact durable
approval ID. A stale cursor or approval ID fails closed. The
`session_resync`, `resume_turn`, and `resolve_interrupted_approval` command
objects are closed: unknown top-level fields are rejected instead of silently
ignored.

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

#### Requires an assistant identity (changed in 0.12.26)

**A host that spawns the engine without an assistant identity has every
`add_mcp_server` refused.** Pass `--assistant NAME` on the launch argv, or set
`WAYLAND_ASSISTANT=NAME` in the child's environment. Any stable, non-blank name
works — it is the same identity `only_for_assistant` matches against in config
(see [mcp.md](mcp.md#assistant-scoping)). It is *not* `--agent`, which selects a
persona.

The engine binds each wire-added server to the declaring identity, so a session
with no identity has nothing to bind to and the declaration is rejected before
any transport is started:

```
Agent → stdout: {"type":"error","error":{"code":"engine_error","message":"AddMcpServer 'tools': active assistant identity is required for a runtime MCP declaration","retryable":false}}
Agent → stdout: {"type":"mcp_failed","name":"tools","reason":"active assistant identity is required for a runtime MCP declaration"}
```

The `error` frame carries no `msg_id` (the refusal is not turn-scoped), and its
code is the generic `engine_error` — this refusal has no code of its own. **Do
not branch on the code to detect it.** Branch on `mcp_failed`, which names the
server, or match the message text if you must distinguish it from other
`engine_error`s.

**This is not a fatal error and it is the failure a host is most likely to
misread.** The session proceeds normally; it simply has none of that server's
tools. A host that does not surface `mcp_failed` presents a conversation with
zero tools and no stated cause, which looks like a broken MCP subsystem rather
than a missing launch flag. `mcp_failed` is a `safety`-criticality frame in the
Desktop contract — render it.

In 0.12.25 the same command produced an unscoped, globally visible server and
connected unconditionally, so a host upgrading from 0.12.25 that never passed
`--assistant` will see this refusal on every runtime server it declares.

`mcp_failed` carries `{name, reason}` and is also emitted for most other
`add_mcp_server` refusals — an invalid request, a name that collides with a
config declaration, a failed `${cred:}` resolution, and a transport that fails to
connect.

**One refusal is `error`-only and emits no `mcp_failed`:** a malformed transport
spec (an unknown `transport` value, or a stdio entry with no `command`) is
rejected while the server config is still being built, before the name is bound
to anything, so only the `error` frame is sent:

```
Agent → stdout: {"type":"error","error":{"code":"engine_error","message":"AddMcpServer 'tools': unknown transport: foo","retryable":false}}
```

A host that renders **only** `mcp_failed` therefore still drops this one
silently. Render `engine_error` messages prefixed `AddMcpServer '<name>':` as
well, or you reproduce the same invisible-failure trap one layer down.

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
    "code": "engine_error",
    "message": "AddMcpServer 'name': rejected — only allowed before first Message",
    "retryable": false
  }
}
```

This frame carries no `msg_id` field at all (it is omitted, not null).

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
| `resume_token` | string | yes | Echoed verbatim from the `approval_required` event. Routes the decision to the right pending bridge. Only ever valid for a NON-EMPTY token: an ordinary tool gate carries `""` and is answered with `tool_approve` / `tool_deny` instead (§1.N+4). |
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

**EOF while an approval is pending denies it immediately** (FerroxLabs/wayland#1070).
A tool parked on `approval_required` can never be answered once the command
stream is gone, so the engine resolves every pending approval as denied at EOF
rather than waiting out the 5-minute approval TTL. The turn then unwinds and the
host sees `tool_cancelled` promptly. This fails CLOSED — the same posture the
TTL reaper already took — and the two are distinguishable by reason: the TTL
path says `approval timed out (no host response)`, the EOF path says
`host closed the command stream while this approval was pending`.

## 4. Error Handling

### 4.1 Invalid Command

**A malformed or unrecognised command is answered with an `error` frame**
(FerroxLabs/wayland#1070). The reader fails to deserialize the line, logs it,
and emits exactly one `error` on stdout naming the offending command `type`
(when the line at least parsed as a JSON object carrying one) and quoting the
deserializer's own reason (`crates/wcore-protocol/src/reader.rs`).

```
Client → stdin:  {"type":"teleport"}
Agent  → stdout: {"type":"error","error":{"code":"engine_error","message":"invalid protocol command of type \"teleport\": unknown variant `teleport`, expected one of `message`, `stop`, `tool_approve`, ...","retryable":false}}
Client → stdin:  {"type":"message","msg_id":"m1"}
Agent  → stdout: {"type":"error","error":{"code":"engine_error","message":"invalid protocol command of type \"message\": missing field `content`","retryable":false}}
Client → stdin:  this is not json
Agent  → stdout: {"type":"error","error":{"code":"engine_error","message":"invalid protocol command: expected ident at line 1 column 2 (expected one JSON object per line with a string \"type\" field)","retryable":false}}
```

Properties a host can rely on:

- **The rejection carries no `msg_id`.** A line that failed to parse has no
  trustworthy correlation handle, so none is invented. Match on the message.
- **The code is `engine_error`.** Rejections deliberately introduce no new code:
  the vocabulary in §1.10 is unchanged, and the detail rides the message. There
  is still no `protocol_error` code; the engine has never emitted one.
- **The stream continues.** The bad line is dropped, the reader resumes at the
  next newline, and the commands before and after it are unaffected.
- **The echoed text is length-bounded.** The `type` and the deserializer reason
  are host-supplied, so each is truncated (with a trailing `...`) rather than
  reflected at full length.

An unknown `type`, an unknown field, a bad field type, an over-long
`request_id`, and a line over the 8 MiB per-line cap all take this path. A
command that parses but is refused later by an authority check answers through
its own correlated result frame instead (for example `budget_grant_result`,
`session_recovery_unavailable`, `mcp_removal_result`).

> **Hosts written against v0.13.1 or earlier:** a malformed command used to
> produce *nothing at all*, so any timeout-based "the engine never answered"
> recovery you built for it will now see an `error` frame first. This is
> additive — no existing frame changed shape — but a host that treats an
> uncorrelated `error` as fatal should relax that before adopting this build.

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

When the engine cannot start, it emits one terminal frame and exits non-zero.
The code is `init_failed`, and the message is the full failure chain behind a
fixed `Engine failed to start: ` prefix:

```json
{
  "type": "error",
  "error": {
    "code": "init_failed",
    "message": "Engine failed to start: ANTHROPIC_API_KEY not set",
    "retryable": false
  }
}
```

`init_failed` is emitted at most once per process, and `msg_id` is omitted (the
failure precedes any turn). Treat it as terminal: no further frames follow, and
the process is already exiting.

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
  --auto-approve          # Approvals bypassed; the OS sandbox stays on
  --allow-host-workspace-grants # Optional local read-only runtime approvals
  --workspace <PATH>      # Working directory for file operations
  --assistant <NAME>      # Host's assistant identity. REQUIRED if the host will
                          # send add_mcp_server (§2.8); also selects which
                          # only_for_assistant config servers are injected.
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

2. **Negotiate before interpreting contract-aware events.** A current Core
   `ready` includes a `contract` descriptor with the supported major/minor,
   generator, fixture/schema/source digests, and capability statuses. A host
   pinned to this contract fails closed on an unsupported major or a pinned
   schema/fixture digest mismatch. The reference observer is
   `wcore_protocol::contract::HostContractObserver`.

3. **Distinguish three outcomes per line:**
   - **Known event type**: the `type` string is in the host's known set;
     render normally.
   - **Unknown event type**: the `type` string is NOT in the host's known
     set. Drop it only when the event explicitly carries `"critical": false`.
     Reject `"critical": true`, a missing classification, or a non-boolean
     classification. Unknown criticality is critical; hosts must not guess.
   - **Malformed**: input wasn't decodable JSON, OR had no `type` field,
     OR the `type` value wasn't a string. **Log or count with rate
     limiting** — this indicates protocol corruption (framing bugs,
     truncation, injection) and is observable evidence of a problem,
     distinct from normal version skew.

4. **Tolerate unknown fields on known variants.** Read only the fields
   the host expects. Unknown fields on a known event must be ignored;
   they appear when wcore adds new optional fields in future versions.

5. **Use `capabilities` advisory, not permissive.** The `Ready` event's
   `capabilities` block advertises which event families this wcore
   session will emit. The host CAN read the flags to decide whether to
   add relevant `type` strings to its known set. The host MUST NOT
   require any `capabilities` flag to be true to render a known event
   — unknown `capabilities` keys must also be ignored.

### Authoritative test

Legacy additive decoding is characterized in
`crates/wcore-protocol/tests/host_decoder_contract.rs`. Negotiated,
fail-closed behavior is enforced by `HostContractObserver` and serialized
corpus replay in `desktop_contract_adversarial.rs`; use that observer as the
current reference when porting the Electron host's decoder. The production host code lives at
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

`render_artifact` (§1.N+13) is gated by a CONTRACT capability
(`render_artifact_v1` in `ready.contract.capabilities`), not by a
`Capabilities.*` flag — it landed after contract negotiation existed, and the
contract descriptor is where a host now feature-detects.

#### Host-tolerated additive variants

Some event variants ship without a dedicated `Capabilities.*` flag.
They are always-emitted; hosts that do not know about them silently
drop the line per the W0 host decoder contract. This includes
`budget_exceeded`, `provider_circuit_event`, provider evidence events,
`capability_activation`, and `mid_flight_monitor_decision`.
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
    "input_tokens": 200,
    "output_tokens": 50,
    "cache_read": 800,
    "cache_write": 0,
    "cache_hit_rate": 0.8,
    "cost_usd": 0.0,
    "cost_priced": false,
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
| `trace.input_tokens` | u64 | Uncached prompt tokens reported by the provider. Protocol v0.2.0 treats this as disjoint from `cache_read` and `cache_write`. |
| `trace.output_tokens` | u64 | Completion tokens reported by the provider. |
| `trace.cache_read` | u64 | Provider-reported cache read tokens. |
| `trace.cache_write` | u64 | Provider-reported cache creation tokens. |
| `trace.cache_hit_rate` | f64 | `cache_read / (input_tokens + cache_read + cache_write)`, using a saturating denominator. `0.0` when total input is zero. |
| `trace.cost_usd` | f64 | USD cost for the turn when `cost_priced` is true. A zero with `cost_priced: false` is not a free call. |
| `trace.cost_priced` | bool | True for metered prices and known-free local inference; false when the active router/model has no authoritative price. Missing on legacy traces defaults to false. |
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
    { "turn": 0, "model": "claude-opus-4-7", "provider": "anthropic", "cost_usd": 0.05, "priced": true },
    { "turn": 1, "model": "claude-opus-4-7", "provider": "anthropic", "cost_usd": 0.073456, "priced": true }
  ]
}
```

| Field | Type | Description |
|---|---|---|
| `session_id` | string | The session id that just terminated. |
| `total_cost_usd` | f64 | Sum of `per_turn[].cost_usd`. This is only a complete session price when every row has `priced: true`. |
| `per_turn` | array | Per-turn cost rows. Each is `{ turn, model, provider, cost_usd, priced }`. `priced: false` means unpriced, never free; missing on legacy rows defaults to false. `provider` matches the structured per-provider identity used in `trace_event.trace.provider`. |

#### Host conformance

`session_cost` is gated by the W0-reserved `capabilities.cost_attribution`
flag. Hosts that did NOT see `cost_attribution: true` on the Ready event
MUST drop the variant silently per the Host Decoder Contract. Hosts that
opt in surface it via their cost UI (totals, per-session breakdown,
billing-export, etc.). Hosts MUST render a row with `priced: false` as
"unpriced", not `$0`. Per-turn cost remains available inline on
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
| `resume_token` | string | Bridge secret, present ONLY for bridge-backed approvals. **EMPTY for an ordinary tool gate** — see "Which command answers this" below. Never echo an empty token. |
| `correlation_id` | string | Opaque public handle for UI matching. Always equals `call_id`. Omitted from the JSON when empty. |
| `reason` | string | Short machine-readable reason category (e.g. `"Edit outside workspace root"`, `"Exec — destructive command"`). |
| `context` | string | Human-readable detail — the host displays this in the approval modal. |
| `plan` | object | Crucible council proposal card. Present only for a council approval; absent otherwise. |

**Which command answers this.** There are two kinds of `approval_required`
and they are answered by DIFFERENT commands. A host that always replies with
`approval_resume` hangs on every ordinary tool gate, because that gate's
`resume_token` is the empty string and the engine has no bridge entry to
route it to.

| Kind | How to recognise it | Answer with |
|---|---|---|
| Ordinary tool gate (Write outside the workspace, a destructive Bash, …) | `resume_token` is `""` | [`tool_approve`](#23-tool_approve) / [`tool_deny`](#24-tool_deny), keyed by `call_id` |
| Bridge-backed gate (Crucible council, egress consent) | `resume_token` is a non-empty opaque secret | [`approval_resume`](#210-approval_resume-w7), keyed by `resume_token` |

The bridge secret is deliberately NOT the `call_id`: the model can see the
`call_id`, so routing approvals on it would let a tool approve itself
(GHSA-8r7g). `ProtocolSink` additionally strips in-flight secrets from
streaming tool output.

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

### 1.N+5a provider_attempt / provider_retry / provider_failure (F04)

Core emits provider evidence events for every physical request and retry
decision. These events are always enabled so evaluators and diagnostics can
distinguish real engine recovery from fixture-side request counts. They are
additive diagnostics: hosts that do not recognize them MUST drop them silently
under the Host Decoder Contract.

```json
{ "type": "provider_attempt", "failure": "http_503" }
{ "type": "provider_retry", "failure": "http_503" }
{ "type": "provider_attempt" }
{ "type": "provider_failure", "failure": "stream_truncated" }
```

| Event | Field | Type | Description |
|---|---|---|---|
| `provider_attempt` | `failure` | string? | One physical provider request. Omitted when that request reached a usable response. |
| `provider_retry` | `failure` | string? | Core scheduled another request after the typed failure. This is not an additional physical-attempt count. |
| `provider_failure` | `failure` | string | A failure discovered after the physical request completed, such as a truncated SSE body. It does not by itself imply a retry. |

`failure` is a stable machine-readable class such as `http_429`, `http_503`,
`timeout`, `connection`, `stream_truncated`, `context_overflow`, or
`egress_denied`. Hosts MUST treat the value as an open string and MUST NOT parse
human-readable provider error messages to infer it.

### 1.N+5b capability_activation (F05)

Immediately after `ready`, Core emits typed activation facts for each audited
capability. This is runtime truth, not a feature advertisement: a capability
ends startup either at `ready` or at `unavailable` with a stable reason. A
capability that later performs a real side effect emits a repeatable
`reached` → `outcome_changed` → `observed` cycle only after that side effect
succeeds.

```json
{ "type": "capability_activation", "capability": "smart_handoff", "stage": "declared" }
{ "type": "capability_activation", "capability": "smart_handoff", "stage": "configured" }
{ "type": "capability_activation", "capability": "smart_handoff", "stage": "constructed" }
{ "type": "capability_activation", "capability": "smart_handoff", "stage": "ready" }
{ "type": "capability_activation", "capability": "delegate_isolation", "stage": "unavailable", "reason": "isolation_not_enforced" }
```

| Field | Type | Description |
|---|---|---|
| `capability` | string | Stable identity: `pricing_refresher`, `mid_flight_monitor`, `cooldown_tracker`, `learned_policy`, `smart_handoff`, `delegate_isolation`, `procedure_skill_drafting`, or `legacy_auto_skill_drafting`. |
| `stage` | string | `declared`, `configured`, `constructed`, `ready`, `reached`, `outcome_changed`, `observed`, or terminal `unavailable`. |
| `reason` | string? | Required only for `unavailable`: `disabled_by_config`, `dependency_unavailable`, `no_production_constructor`, `runtime_path_unwired`, or `isolation_not_enforced`. |

These events are always-on additive diagnostics and have no `Ready.capabilities`
flag. Hosts SHOULD retain only the latest fact per capability for status UI,
while evaluators SHOULD validate the complete ordered chain. Unknown capability,
stage, and reason strings must be handled as forward-compatible values rather
than granting authority or implying availability.

### 1.N+5c mid_flight_monitor_decision (F10)

Core emits this event when the production mid-flight monitor changes control
flow. It is always-on and additive: hosts that do not recognize the event MUST
drop it silently under the Host Decoder Contract.

```json
{ "type": "mid_flight_monitor_decision", "directive": "replan", "reason": "repeated_tool_route" }
{ "type": "mid_flight_monitor_decision", "directive": "stop", "reason": "output_stall" }
```

| Field | Type | Description |
|---|---|---|
| `directive` | `"replan" \| "stop"` | `replan` means changed-strategy guidance was committed to the next provider request. `stop` means Core bounded the current run. |
| `reason` | string | Stable class: `output_stall`, `repeated_error`, `repeated_tool_route`, or `budget_exceeded`. Treat future values as open strings. |

For `repeated_tool_route`, the first detected normalized cycle emits `replan`.
If the same route repeats without material deviation, Core emits `stop` and
finishes with `max_turns` so the host can offer Continue. `output_stall` covers
repeated completed provider attempts that return no output after a failed tool
round; absolute request/stream hang timeouts are an F15 provider-governance
responsibility, not an implied guarantee of this event.

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
> engine with `--auto-approve` or tier 1
> (`--dangerously-skip-permissions`, aliases `--force` / `--yolo`), or grants
> wire-force via `WAYLAND_ALLOW_WIRE_FORCE=1`, is opting out of that gate and
> MUST supply its own confirmation UX before fulfilling these requests. These
> approval controls do not disable the OS sandbox. The tier-2 Dangerous posture
> (`--dangerously-skip-permissions-and-sandbox`, deprecated alias
> `--dangerous`) can only be selected at a local process launch and cannot be
> requested over the JSON stream.

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

### 1.N+13 `render_artifact` (#1098)

"Show this to the user" as a **render capability**, not an OS `open`. The
engine hands the host **content**; the host displays it. No path crosses the
boundary, so displaying something costs the host zero filesystem authority,
works headless and over SSH, and behaves identically on macOS, Linux and
Windows.

**Feature-detect on `render_artifact_v1` in `ready.contract.capabilities`**
(contract v1.16). A host that never learns the type drops the frame safely —
this event carries `"critical": false` explicitly, which is the only thing that
makes an unknown type droppable under the W0 rules above.

```json
{
  "type": "render_artifact",
  "msg_id": "msg-001",
  "call_id": "call-render-001",
  "title": "Quarterly summary",
  "mime": "text/markdown",
  "content": "# Quarterly summary\n\nRevenue held.\n",
  "truncated": false,
  "critical": false
}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `msg_id` | string | yes | Turn the artifact belongs to. Empty string when no turn is active. |
| `call_id` | string | yes | The `render_artifact` tool call that produced it. |
| `title` | string | yes | Short label for the surface. At most 256 bytes; longer titles are shortened, never refused. |
| `mime` | string | yes | Closed vocabulary — see the table below. |
| `content` | string | yes | The text to display. At most 1 MiB (plus the truncation marker). |
| `truncated` | boolean | yes | `true` when `content` is a prefix and carries the in-band marker. |
| `critical` | boolean | yes | Always the literal `false`. |

Contract, in the order a host needs it:

1. **`mime` is a CLOSED vocabulary.** Reject anything outside it rather than
   guessing — a value you have never heard of is one you cannot render, and
   rendering it as something else is worse than not rendering it. Widening the
   vocabulary is an announced contract event (a minor bump plus a schema
   change), never a silent new value.

   | `mime` | Render as |
   |---|---|
   | `text/plain` | Preformatted text. No markup interpretation. |
   | `text/markdown` | Markdown. The engine's default when the tool call omits `mime`. |
   | `text/html` | An HTML document fragment. See (2) — this one has an obligation attached. |

2. **`content` is UNTRUSTED, and `text/html` is the sharp edge.** The bytes are
   either model-authored or read out of a workspace file, so they are exactly
   the reach a prompt-injected turn would like. A host that renders
   `text/html` MUST do it in a sandboxed renderer with no bridge to the host
   process — in Electron terms: `sandbox: true`, `nodeIntegration: false`,
   `contextIsolation: true`, no preload that exposes IPC, and a CSP that
   forbids remote loads. Core cannot enforce this; it is the host's half of
   #1098. If you are not prepared to do that, render `text/html` as
   `text/plain` and say so in your UI.

3. **The engine never asks you to open a path, and you should never accept
   one.** That is the entire point. On macOS our seatbelt profile is
   `(deny default)` and does not grant the SBPL `lsopen` operation, so `open`
   fails with `-54` (#1102) — and granting it would let a sandboxed shell ask
   launchd to start any installed app OUTSIDE the profile. A render event needs
   none of that authority.

4. **What the engine may render is exactly what it may read.** The
   `render_artifact` tool obtains file content through the same vfs and policy
   path as an ordinary `read`: workspace containment, standing path grants, and
   the secret deny-list all apply unchanged. A file the agent may not read is a
   file it may not render, so this event never widens the agent's reach — it
   only changes how what it already read reaches the user.

5. **`truncated: true` means you are looking at a prefix.** Content over 1 MiB
   is cut on a UTF-8 character boundary and an in-band
   `[wcore: CONTENT TRUNCATED …]` marker is appended, so a reader looking at the
   rendered surface (rather than at the frame) is still told. Badge it in your
   chrome as well. The cap is not decoration: an over-limit frame trips the
   output pump's sticky failure and would take the session's entire stdout with
   it, so it is enforced at the single emission chokepoint and can never be
   raised per-call.

6. **A file too large to render is refused at the tool, not truncated.** The
   size is knowable before the read, and the model has a first-class way to
   pick a part (`read` with offset/limit, then render the result inline), so
   the engine returns an actionable tool error instead of silently showing the
   first megabyte of a 2 GB file. Truncation is the backstop for content that
   is already in hand.

7. **No reply is expected.** Unlike `host_send_message_request` (§1.N+12), this
   is fire-and-forget: there is no correlated result command, the tool call
   completes immediately, and nothing in the engine waits on a render. That is
   what keeps it free of authority — there is no outcome for a host to lie
   about.

8. **The event only reaches json-stream hosts.** The `render_artifact` tool is
   always in the tool list — the tool set must not move with the output
   surface, because `tool_inventory` is inside the recovery authority digest —
   but a session with no protocol sink (a TUI run, or any sub-agent, which run
   under a null or relay sink) refuses every call loudly rather than
   discarding. The model is told there is nowhere to display and puts the
   content in its reply instead.

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
