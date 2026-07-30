# NOTES — lane/fix-tui-tool-results (UAT-T3)

Append-only working notes. Committed early and re-committed after every measurement
(LANE-BRIEF §6b-i). Base: `e7bc6d883027102ff1e5bbaa2dd19f9265268cab`.

---

## T0 — premise verification (brief's traced root cause)

The orchestrator brief says its own measurements are probably stale and must be
re-verified (LANE-BRIEF "Your brief's MEASUREMENTS are probably stale"). Every claim
below was re-read at the lane base commit.

### Claim 1 — `crates/wcore-tools/src/bash.rs` returns plain text, not JSON. **TRUE.**

Three construction sites, byte-identical format string:

```
crates/wcore-tools/src/bash.rs:225   output_to_result()          (sandbox non-streaming)
crates/wcore-tools/src/bash.rs:450   Tool::execute (streaming)
crates/wcore-tools/src/bash.rs:665   Tool::execute_with_ctx
```

all building

```rust
let content = format!("Exit code: {}\nSTDOUT:\n{}\nSTDERR:\n{}", exit_code, stdout, stderr);
ToolResult { content, is_error: exit_code != 0 }
```

### Claim 2 — `toolcard.rs:267` `parse_payload` degrades non-JSON to `Value::String`. **TRUE.**

```rust
fn parse_payload(card: &ToolCardModel) -> Value {
    match card.output.as_deref() {
        None | Some("") => Value::Null,
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string())),
    }
}
```

`Value::String::get(_)` is always `None`, so every `payload.get("…")` in every
formatter returns `None` for a plain-text payload.

**NEW (not in the brief): `parse_payload` is DUPLICATED.** A second, identical copy
lives at `crates/wcore-cli/src/tui/surfaces/workspace.rs:3446` as
`parse_card_payload`, and that is the copy the *inline transcript* path uses
(`push_tool_card_lines`). The widget copy in `toolcard.rs` serves the card widget.
Both must be fixed, or the defect survives on one of the two render paths.

### Claim 3 — `tool_formatters/bash.rs:29-38` yields `"?"` / `0` / `0`. **TRUE.**

```rust
let cmd = str_or(payload, "cmd", "?");
let exit = i64_or(payload, "exit_code", 0);
let stdout_bytes = payload.get("stdout").and_then(Value::as_str).map(|s| s.len()).unwrap_or(0);
format!("Ran `{}` · exit {} · {} bytes", preview, exit, stdout_bytes)
```

Produces exactly ``Ran `?` · exit 0 · 0 bytes`` on a `Value::String` payload.
`detail_lines` reads `str_or(payload, "stdout", "")` → empty → **zero detail lines**,
which is why actual stdout is never shown.

### Claim 4 — the unit tests feed the formatter an invented payload. **TRUE.**

`tool_formatters/bash.rs` tests all construct `json!({"cmd":…,"exit_code":…,"stdout":…})`.
No test anywhere feeds a formatter the string its real tool produces.

---

## T0b — the full chain, end to end (established, not assumed)

1. `BashTool::execute*` → `wcore_types::tool::ToolResult { content: "Exit code: …", is_error }`
2. `wcore-agent/src/orchestration/mod.rs:3202-3220` — `execute_tools` emits
   `ProtocolEvent::ToolResult { output: content.clone(), output_type: Text, metadata: None }`
3. `wcore-cli/src/tui/protocol_bridge.rs:508` — `card.output = Some(output)`
4. `parse_payload` / `parse_card_payload` → `Value::String(...)`
5. formatter reads `.get(...)` → `None` → defaults

**Finding not in the brief:** `ProtocolEvent::ToolResult` already carries an unused
`metadata: Option<Value>` field (`wcore-protocol/src/events.rs:673`), and **every**
emit site in the product passes `metadata: None`. That is a fourth possible contract
(structured data on a side channel the model never sees) — but it is a wire-format
change to a contract-tested event, so it is not free. Recorded for the cross-audit.

**Finding not in the brief:** `cmd` is not present in the bash tool result *at all*
and never could be — the command lives in the tool **input**, not the output. So
option (a) "tools emit structured JSON" would still not supply `cmd` unless the bash
tool started echoing its own input back. Meanwhile the compact card ALREADY renders
the input: `render_compact` builds `<icon> <name>(<args>) · <summary>` where `<args>`
is `card.input_pretty`. So the command is on screen twice, once truthfully from the
input and once as a fabricated `?` from the formatter.

---

## Status

- [x] Premise verified (4/4 brief claims TRUE, 2 additions found)
- [ ] Task 1 — live pty repro BEFORE
- [ ] Task 2 — 13-formatter audit table
- [ ] Task 3 — cross-audit + contract decision
- [ ] Task 4 — can-fail test
