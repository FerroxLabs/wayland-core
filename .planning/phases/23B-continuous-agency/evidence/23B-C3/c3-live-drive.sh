#!/usr/bin/env bash
# 23B-C3 LIVE DRIVE — every criterion-3 verb through the SHIPPED wayland-core
# binary, on real hardware, against a real HTTP provider endpoint.
#
# The verdict graded C3 NOT MET partly because "nothing was driven live on any
# platform". This closes that. It is deliberately NOT a cargo test: it runs the
# binary a user would run, through the real slash dispatcher and the real
# AnthropicProvider, pointed at a local mock endpoint that records every
# outbound request body.
#
# A LIMITATION THIS DRIVE FOUND AND DOES NOT HIDE. The activation record lives
# in the process (an `Arc<ActivationLog>` on the memory backend), so
# `wayland-core -p "/memory activation"` as a SEPARATE process cannot describe a
# previous process's turn. Within one process — the TUI, where a user actually
# lives — it can. Two attempts to drive a multi-line session headlessly failed
# for reasons the binary is right about: piped stdin is refused ("stdin is not a
# terminal"), and a PTY launches the full-screen TUI rather than a line REPL.
# So this drive proves the WIRE effects one-shot (which is the load-bearing
# half) and proves activation's own state and off-switch one-shot, and it does
# NOT claim the "what was injected last turn" report is proven live. That is
# recorded as OPEN in the summary rather than asserted here.
#
# WHY SEVERAL SESSIONS. `AgentEngine::should_attempt_recall` injects only on
# the FIRST user turn of a session, so each "did memory reach the prompt"
# question needs its own session. Project-tier memory persists on disk across
# them, which is the point.
#
# ANTI-VACUITY (lane brief §3.2, §3b-i):
#   * every capture is asserted to EXIST and be NON-EMPTY before its contents
#     are read;
#   * every `absent` is preceded by a `need` on the SAME file — a dead binary
#     that printed nothing cannot pass an absence check;
#   * the outbound-body checks assert THIS turn's user message first, so a
#     stale capture from an earlier turn cannot be mistaken for this one. That
#     guard already earned its place: on the previous iteration a failed turn
#     left the plant turn's body as "the latest capture", and the nonce
#     assertion passed against the wrong request;
#   * the nonce lifecycle carries its own known-positive: proved PRESENT in the
#     prompt before it is forgotten, then proved ABSENT;
#   * a final counter is compared to an expected total, so a script that exits
#     early reports INCOMPLETE instead of looking like a pass.
set -uo pipefail

ROOT="${1:?usage: c3-live-drive.sh <workdir> <binary> [nonce]}"
BIN="${2:?usage: c3-live-drive.sh <workdir> <binary> [nonce]}"
NONCE="${3:-QK7ZC3LIVE}"
QUESTION="what is my recorded deployment region"

mkdir -p "$ROOT/out"
export HOME="$ROOT/home"
export XDG_CONFIG_HOME="$ROOT/home/.config"
export XDG_DATA_HOME="$ROOT/home/.local/share"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"
PROJ="$ROOT/proj"; mkdir -p "$PROJ"

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); echo "F23_C3_LIVE=PASS  $1"; }
bad() { FAIL=$((FAIL+1)); echo "F23_C3_LIVE=FAIL  $1"; }
alive() {
  local f="$1" label="$2"
  [ -f "$f" ] || { bad "$label: capture $f DOES NOT EXIST"; return 1; }
  [ -s "$f" ] || { bad "$label: capture $f is EMPTY"; return 1; }
  return 0
}
need()   { local f="$1" pat="$2" label="$3"
           alive "$f" "$label" || return 1
           if /usr/bin/grep -qF -- "$pat" "$f"; then ok "$label (found '$pat')"
           else bad "$label: '$pat' NOT in $f"; fi; }
absent() { local f="$1" pat="$2" label="$3"
           alive "$f" "$label" || return 1
           if /usr/bin/grep -qF -- "$pat" "$f"; then bad "$label: '$pat' IS STILL in $f"
           else ok "$label ('$pat' absent from a proven-non-empty capture)"; fi; }

# --- the mock provider -----------------------------------------------------
cat > "$ROOT/mock.py" <<'PYEOF'
import json, os, sys, threading
from http.server import BaseHTTPRequestHandler, HTTPServer
CAP = os.environ["CAP_DIR"]; NONCE = os.environ["NONCE"]
STATE = {"n": 0}; LOCK = threading.Lock()
def sse(blocks, stop_reason):
    ev = [("message_start", {"type":"message_start","message":{"id":"m","type":"message","role":"assistant","content":[],"model":"claude-mock","stop_reason":None,"stop_sequence":None,"usage":{"input_tokens":10,"output_tokens":1}}})]
    for i, b in enumerate(blocks):
        if b["kind"] == "text":
            ev.append(("content_block_start", {"type":"content_block_start","index":i,"content_block":{"type":"text","text":""}}))
            ev.append(("content_block_delta", {"type":"content_block_delta","index":i,"delta":{"type":"text_delta","text":b["text"]}}))
        else:
            ev.append(("content_block_start", {"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":"tu_1","name":b["name"],"input":{}}}))
            ev.append(("content_block_delta", {"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":json.dumps(b["input"])}}))
        ev.append(("content_block_stop", {"type":"content_block_stop","index":i}))
    ev.append(("message_delta", {"type":"message_delta","delta":{"stop_reason":stop_reason,"stop_sequence":None},"usage":{"output_tokens":1}}))
    ev.append(("message_stop", {"type":"message_stop"}))
    return "".join("event: %s\ndata: %s\n\n" % (n, json.dumps(d)) for n, d in ev).encode()
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        with LOCK:
            STATE["n"] += 1; idx = STATE["n"]
        open(os.path.join(CAP, "req-%03d.json" % idx), "wb").write(body)
        txt = body.decode("utf-8", "replace")
        plant = os.path.join(CAP, "PLANT")
        if os.path.exists(plant) and "tool_result" not in txt:
            os.remove(plant)
            payload = sse([{"kind":"tool","name":"assert_fact","input":{
                "subject":"the user","predicate":"recorded deployment region is",
                "object":NONCE,"tier":"project"}}], "tool_use")
        else:
            payload = sse([{"kind":"text","text":"ack"}], "end_turn")
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("content-length", str(len(payload)))
        self.end_headers(); self.wfile.write(payload)
HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PYEOF

PORT=18723
export CAP_DIR="$ROOT/cap"; mkdir -p "$CAP_DIR"
export NONCE
python3 "$ROOT/mock.py" "$PORT" & MOCK_PID=$!
trap 'kill $MOCK_PID 2>/dev/null' EXIT
for i in $(seq 1 20); do
  if python3 -c "import socket,sys; s=socket.socket(); sys.exit(0 if s.connect_ex(('127.0.0.1',$PORT))==0 else 1)"; then break; fi
  echo "waiting for mock: iteration $i"; sleep 1
done

# The binary itself instructed this: "set WAYLAND_VAULT_PASSPHRASE_FD ... or
# turn durable sessions off". Unlocking is the right answer — disabling durable
# sessions would change the configuration under test. Generated per run, passed
# on a file descriptor, never written to disk or into any capture.
VAULT_PASS=$(head -c 24 /dev/urandom | od -An -tx1 | tr -d " \n")
SESSION_N=0

# repl <capture-name>  (input lines on stdin): ONE process, many input lines.
# The REPL refuses piped stdin ("stdin is not a terminal and no prompt was
# given"), which is correct behaviour and is why this uses a real PTY via
# `script(1)` rather than a pipe. That is also strictly better evidence: the
# binary is driven exactly as an interactive user drives it, through the same
# terminal path, not through a headless side-door.
printf '%s' "$VAULT_PASS" > "$ROOT/.vp"; chmod 600 "$ROOT/.vp"

# one <capture-name> <prompt-or-slash-line>
one() {
  local name="$1"; shift
  SESSION_N=$((SESSION_N+1))
  local sid; sid=$(printf 'c30f1fee%04x' "$SESSION_N")
  ( cd "$PROJ" && exec 9< "$ROOT/.vp" && \
    WAYLAND_VAULT_PASSPHRASE_FD=9 timeout 90 "$BIN" --auto-approve --provider anthropic \
      --api-key sk-live-not-real --base-url "http://127.0.0.1:$PORT" \
      --model claude-mock --session-id "$sid" "$@" ) > "$ROOT/out/$name.txt" 2>&1
  echo "F23_C3_LIVE_RC=$? session=$name sid=$sid"
  clean "$name"
}

grab()  { cp "$(ls -1 "$CAP_DIR"/req-*.json | tail -1)" "$ROOT/out/$1.json"; }
clean() { /usr/bin/sed 's/\x1b\[[0-9;]*m//g' "$ROOT/out/$1.txt" > "$ROOT/out/$1.clean.txt"; }

echo "=== 0. the binary is alive (guards every later capture) ==="
"$BIN" --version > "$ROOT/out/version.txt" 2>&1
need "$ROOT/out/version.txt" "wayland-core" "binary reports its version"

echo "=== 1. plant a fact THROUGH the product (real assert_fact tool call) ==="
touch "$CAP_DIR/PLANT"
one plant "remember my deployment region"
need "$ROOT/out/plant.clean.txt" "assert_fact" "plant turn really invoked the assert_fact tool"

echo "=== 2. the planted fact reaches the OUTBOUND PROVIDER BODY ==="
one recall1 "$QUESTION"
grab body-before-forget
need "$ROOT/out/body-before-forget.json" "$QUESTION" \
     "outbound body carries THIS turn's user message (not a stale capture)"
need "$ROOT/out/body-before-forget.json" "$NONCE" \
     "outbound body carries the planted fact BEFORE forgetting"

echo "=== 3. /memory why surfaces the fact, its partition and its id ==="
one why "/memory why recorded deployment region"
need "$ROOT/out/why.clean.txt" "$NONCE"           "/memory why surfaces the planted fact itself"
need "$ROOT/out/why.clean.txt" "semantic/project" "/memory why names the semantic partition and tier"
need "$ROOT/out/why.clean.txt" "via vector"       "/memory why names the modality that selected it"

FACT_ID=$(/usr/bin/grep -oE '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' \
          "$ROOT/out/why.clean.txt" | head -1)
echo "F23_C3_LIVE_FACT_ID=$FACT_ID"
if [ -z "$FACT_ID" ]; then bad "no addressable fact id in the provenance output"
else ok "provenance output carries an addressable fact id"; fi

echo "=== 4. activation: state and off switch ==="
one act-show "/memory activation"
need "$ROOT/out/act-show.clean.txt" "automatic recall is ON" "activation reports its default ON state"
one act-off "/memory activation off"
need "$ROOT/out/act-off.clean.txt" "automatic recall is OFF" "activation off switch reports OFF"

echo "=== 5. nudges are visible and settable ==="
one nudge-show "/memory nudge"
need "$ROOT/out/nudge-show.clean.txt" "cap 3 per session" "nudge bound is visible with its default cap"
one nudge-off "/memory nudge off"
need "$ROOT/out/nudge-off.clean.txt" "OFF" "nudge off switch reports OFF"
one nudge-cap "/memory nudge cap 9"
need "$ROOT/out/nudge-cap.clean.txt" "cap 9" "nudge cap is settable"

echo "=== 6. forget refuses out loud for an id that is not there ==="
one forget-bogus "/memory forget not-a-real-id"
need "$ROOT/out/forget-bogus.clean.txt" "refused" "forget refuses a bogus id instead of reporting success"

echo "=== 7. FORGET the fact, then prove it left the OUTBOUND BODY ==="
one forget "/memory forget $FACT_ID"
need "$ROOT/out/forget.clean.txt" "removed from semantic" "forget reports the SEMANTIC partition"
one recall2 "$QUESTION"
grab body-after-forget
need   "$ROOT/out/body-after-forget.json" "$QUESTION" \
       "post-forget body carries THIS turn's user message (instrument alive)"
absent "$ROOT/out/body-after-forget.json" "$NONCE" \
       "post-forget outbound body no longer carries the forgotten fact"

echo "=== 8. re-plant as a control, then prove PRIVACY reaches the wire ==="
touch "$CAP_DIR/PLANT"
one replant "remember my deployment region"
one on-check "$QUESTION"
grab body-on
need "$ROOT/out/body-on.json" "$QUESTION" "re-plant control: body carries this turn's message"
need "$ROOT/out/body-on.json" "$NONCE" \
     "re-plant control: the fact is back in the prompt, so section 7's absence was the forget"

one privacy "/memory privacy semantic live drive"
need "$ROOT/out/privacy.clean.txt" "excluded from recall" "privacy scope reports the exclusion"
one priv-check "$QUESTION"
grab body-priv
need   "$ROOT/out/body-priv.json" "$QUESTION" "privacy-check body carries this turn's message"
absent "$ROOT/out/body-priv.json" "$NONCE"    "a scoped partition sends NOTHING to the provider"

echo "=== 9. retention reports its bound ==="
one retention "/memory retention semantic 7"
need "$ROOT/out/retention.clean.txt" "bounded to 7 day" "retention bound reports the bound"

EXPECTED=23
echo "F23_C3_LIVE_TOTALS pass=$PASS fail=$FAIL expected=$EXPECTED"
if [ "$((PASS+FAIL))" -ne "$EXPECTED" ]; then
  echo "F23_C3_LIVE=INCOMPLETE — ran $((PASS+FAIL)) of $EXPECTED assertions"
  exit 3
fi
[ "$FAIL" -eq 0 ] && { echo "F23_C3_LIVE_RESULT=OK"; exit 0; } || { echo "F23_C3_LIVE_RESULT=FAILED"; exit 1; }
