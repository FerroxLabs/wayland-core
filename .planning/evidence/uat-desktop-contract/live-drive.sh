#!/bin/bash
# Live-drive the REAL wayland-core binary over the JSON stream protocol and
# capture its actual stdout event stream.
# Lane-scoped paths only (/tmp is shared between lanes -- LANE-BRIEF 6a-ii).
set -u
LANE=uat-desktop-contract
BIN=/root/wayland/target/release/wayland-core
WORK=/tmp/${LANE}-live
rm -rf "$WORK"; mkdir -p "$WORK/home"

export WAYLAND_HOME="$WORK/home"

# The command script fed to stdin. Deliberately exercises the surface that does
# NOT require a provider credential, plus the adversarial command shapes.
cat > "$WORK/commands.jsonl" <<'CMDS'
{"type":"ping"}
{"type":"get_runtime_diagnostics"}
{"type":"session_resync"}
{"type":"goal_resync"}
{"type":"set_mode","mode":"force"}
{"type":"set_mode","mode":"yolo"}
{"type":"set_config","model":"test-model"}
{"type":"add_mcp_server","name":"uat-bogus","transport":"stdio","command":"/nonexistent/uat/binary"}
{"type":"remove_mcp_server","name":"uat-bogus"}
{"type":"remove_mcp_server","name":"never-existed"}
{"type":"tool_approve","call_id":"no-such-call","scope":"once"}
{"type":"approval_resume","resume_token":"no-such-token","approved":true}
{"type":"host_send_message_result","call_id":"no-such-call","ok":true}
{"type":"continue_with_budget","request_id":"uat-req-1","additional_cost_usd":0.5,"additional_tokens":1000}
{"type":"zzz_unknown_command_type"}
{"type":"tool_approve"}
not json at all
[1,2,3]
{"no_type_field":true}
{"type":"ping"}
{"type":"stop"}
CMDS

echo "=== driving $BIN --json-stream ===" > "$WORK/run.log"
"$BIN" --version >> "$WORK/run.log" 2>&1

# Feed commands with small gaps so the engine can interleave its replies, then
# hold stdin open briefly so late frames land before EOF.
( while IFS= read -r line; do printf '%s\n' "$line"; sleep 0.4; done < "$WORK/commands.jsonl"; sleep 6 ) \
  | timeout 120 "$BIN" --json-stream --session-id abcdef123456 --provider anthropic --api-key uat-dc-dummy-key-not-real --model claude-sonnet-4-5 \
      > "$WORK/stdout.jsonl" 2> "$WORK/stderr.log"
RC=$?

# Status file pattern: never trust the exit status alone.
{
  echo "WLRC=${RC}"
  echo "STDOUT_LINES=$(wc -l < "$WORK/stdout.jsonl" | tr -d ' ')"
  echo "STDERR_LINES=$(wc -l < "$WORK/stderr.log" | tr -d ' ')"
  echo "WLDONE"
} > "$WORK/status.txt"
cat "$WORK/status.txt"
echo "--- first 3 stderr ---"
head -3 "$WORK/stderr.log"
