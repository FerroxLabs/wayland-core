#!/usr/bin/env bash
# Phase 27 / plan 27-02 Task 1 — for every flag the handshake sets TRUE, attempt
# the corresponding operation through the product and record what it did.
#
# A flag that claims a capability the very next operation cannot deliver is the
# finding. Both captures are kept so they can sit side by side in the audit.
#
#   f27-capability-operation.sh <repo-root> <out-dir>
set -u

REPO="${1:?repo root}"
OUT="${2:?out dir}"
BIN="$REPO/target/release/wayland-core"
MOCK="$REPO/.planning/scripts/f27-mock-provider.py"
PORT=18931

mkdir -p "$OUT/ops"
: > "$OUT/OPERATIONS.log"
log() { echo "$*" | tee -a "$OUT/OPERATIONS.log"; }

# $1 label, $2 tool name, $3 tool input JSON
attempt() {
  local label="$1" tool="$2" input="$3"
  local home; home="$(mktemp -d)"
  cat > "$home/config.toml" <<CFG
[default]
provider = "anthropic"
model = "claude-sonnet-4-20250514"

[providers.anthropic]
api_key = "f27-fixture-credential"
base_url = "http://127.0.0.1:$PORT"

[session]
enabled = false
CFG

  python3 "$MOCK" "$PORT" "$OUT/ops/$label-wire.jsonl" "$tool" "$input" \
    > "$OUT/ops/$label-mock.out" 2>&1 &
  local mpid=$!
  for _ in $(seq 1 60); do
    grep -q F27-MOCK-READY "$OUT/ops/$label-mock.out" 2>/dev/null && break
    sleep 0.1
  done

  timeout 120 env -i HOME="$home" WAYLAND_HOME="$home" TERM=dumb \
    PATH=/usr/local/bin:/usr/bin:/bin \
    "$BIN" --provider anthropic --dangerously-skip-permissions \
    "Call the $tool tool." > "$OUT/ops/$label.txt" 2>&1
  local rc=$?
  kill "$mpid" 2>/dev/null; wait "$mpid" 2>/dev/null

  log "=== OPERATION $label ==="
  log "TOOL: $tool  INPUT: $input"
  log "RC: $rc"
  log "TOOL-RESULT (as the model saw it):"
  python3 - "$OUT/ops/$label-wire.jsonl" <<'PY' | tee -a "$OUT/OPERATIONS.log"
import json, sys
try:
    lines = open(sys.argv[1]).read().splitlines()
except OSError:
    print("  <no outbound capture>"); raise SystemExit
for line in lines:
    body = json.loads(line)["body"]
    for m in body.get("messages", []):
        c = m.get("content")
        if isinstance(c, list):
            for p in c:
                if p.get("type") == "tool_result":
                    print("  is_error=%s" % p.get("is_error"))
                    print("  " + str(p.get("content"))[:600].replace("\n", "\n  "))
PY
  log ""
  rm -rf "$home"
}

log "F27-02 CAPABILITY-VERSUS-OPERATION"
log "HOST: $(hostname)"
log "SHA: $(cd "$REPO" && git rev-parse HEAD)"
log ""

# `browser_suite: true` was claimed on this machine. Attempt a navigate.
attempt browser-navigate Browser '{"op":{"kind":"navigate","url":"http://127.0.0.1:1/"}}'

log "DONE"
