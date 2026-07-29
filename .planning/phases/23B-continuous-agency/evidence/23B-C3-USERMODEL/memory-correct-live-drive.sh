#!/usr/bin/env bash
# 23B-C3 MEMORY-HALF ADDENDUM — `/memory correct` driven LIVE.
#
# The memory half's own summary lists this as open: "`/memory correct` and the
# retention wire effect are not in the live drive (both are proved at the wire
# by test)". This closes the `correct` half of that.
#
# It plants a fact THROUGH the product (a real `assert_fact` tool call), proves
# the original text reaches the outbound provider body, corrects it through the
# shipped `/memory correct` slash surface, and then proves at the wire that the
# NEW text arrives and the OLD text does not. The old-text absence is the
# load-bearing half: `MemoryControls::correct_fact` re-embeds precisely because
# keeping the stale vector would leave the corrected fact recallable by the
# query that matched the WRONG text.
#
# ANTI-VACUITY: same discipline as the sibling drive — every capture proved to
# exist and be non-empty; every absence preceded by a known-positive on the same
# file; every outbound body asserted to carry THIS turn's user message so a
# stale capture cannot be read as the current one; a final count compared to an
# expected total so an early exit reports INCOMPLETE.
set -uo pipefail

ROOT="${1:?usage: memory-correct-live-drive.sh <workdir> <binary>}"
BIN="${2:?usage: memory-correct-live-drive.sh <workdir> <binary>}"
OLD_NONCE="${3:-QK7MCOLD}"
NEW_NONCE="QK7MCNEW"
QUESTION="what is my recorded deployment region"

mkdir -p "$ROOT/out"
export HOME="$ROOT/home"
export XDG_CONFIG_HOME="$ROOT/home/.config"
export XDG_DATA_HOME="$ROOT/home/.local/share"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"
PROJ="$ROOT/proj"; mkdir -p "$PROJ"

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); echo "MC_LIVE=PASS  $1"; }
bad() { FAIL=$((FAIL+1)); echo "MC_LIVE=FAIL  $1"; }
alive() { local f="$1" label="$2"
          [ -f "$f" ] || { bad "$label: capture $f DOES NOT EXIST"; return 1; }
          [ -s "$f" ] || { bad "$label: capture $f is EMPTY"; return 1; }
          return 0; }
need()   { local f="$1" pat="$2" label="$3"
           alive "$f" "$label" || return 1
           if /usr/bin/grep -qF -- "$pat" "$f"; then ok "$label (found '$pat')"
           else bad "$label: '$pat' NOT in $f"; fi; }
absent() { local f="$1" pat="$2" label="$3"
           alive "$f" "$label" || return 1
           if /usr/bin/grep -qF -- "$pat" "$f"; then bad "$label: '$pat' IS STILL in $f"
           else ok "$label ('$pat' absent from a proven-non-empty capture)"; fi; }

cat > "$ROOT/mock.py" <<'PYEOF'
import json, os, sys, threading
from http.server import BaseHTTPRequestHandler, HTTPServer
CAP = os.environ["CAP_DIR"]; NONCE = os.environ["OLD_NONCE"]
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

PORT=18752
export CAP_DIR="$ROOT/cap"; mkdir -p "$CAP_DIR"
export OLD_NONCE
python3 "$ROOT/mock.py" "$PORT" & MOCK_PID=$!
trap 'kill $MOCK_PID 2>/dev/null' EXIT
for i in $(seq 1 20); do
  if python3 -c "import socket,sys; s=socket.socket(); sys.exit(0 if s.connect_ex(('127.0.0.1',$PORT))==0 else 1)"; then break; fi
  echo "waiting for mock: iteration $i"; sleep 1
done

VAULT_PASS=$(head -c 24 /dev/urandom | od -An -tx1 | tr -d " \n")
printf '%s' "$VAULT_PASS" > "$ROOT/.vp"; chmod 600 "$ROOT/.vp"
SESSION_N=0
one() {
  local name="$1"; shift
  SESSION_N=$((SESSION_N+1))
  local sid; sid=$(printf 'c3dc1fee%04x' "$SESSION_N")  # hex only: a non-hex char makes the binary refuse the session id
  ( cd "$PROJ" && exec 9< "$ROOT/.vp" && \
    WAYLAND_VAULT_PASSPHRASE_FD=9 timeout 90 "$BIN" --auto-approve --provider anthropic \
      --api-key sk-live-not-real --base-url "http://127.0.0.1:$PORT" \
      --model claude-mock --session-id "$sid" "$@" ) > "$ROOT/out/$name.txt" 2>&1
  echo "MC_LIVE_RC=$? session=$name sid=$sid"
  /usr/bin/sed 's/\x1b\[[0-9;]*m//g' "$ROOT/out/$name.txt" > "$ROOT/out/$name.clean.txt"
}
grab() { local latest; latest=$(ls -1 "$CAP_DIR"/req-*.json 2>/dev/null | tail -1)
         if [ -n "$latest" ]; then cp "$latest" "$ROOT/out/$1.json"; fi; }

echo "=== 0. binary alive ==="
"$BIN" --version > "$ROOT/out/version.txt" 2>&1
need "$ROOT/out/version.txt" "wayland-core" "binary reports its version"

echo "=== 1. plant a fact THROUGH the product ==="
touch "$CAP_DIR/PLANT"
one plant "remember my deployment region"
need "$ROOT/out/plant.clean.txt" "assert_fact" "plant turn really invoked the assert_fact tool"

echo "=== 2. the ORIGINAL text reaches the outbound provider body ==="
one recall1 "$QUESTION"
grab body-before
need "$ROOT/out/body-before.json" "$QUESTION" \
     "outbound body carries THIS turn's user message (not a stale capture)"
need "$ROOT/out/body-before.json" "$OLD_NONCE" \
     "the ORIGINAL fact text is on the wire BEFORE the correction"
absent "$ROOT/out/body-before.json" "$NEW_NONCE" \
     "the corrected text is absent before any correction is made"

echo "=== 3. address the fact via /memory why ==="
one why "/memory why recorded deployment region"
need "$ROOT/out/why.clean.txt" "$OLD_NONCE" "/memory why surfaces the planted fact itself"
FACT_ID=$(/usr/bin/grep -oE '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' \
          "$ROOT/out/why.clean.txt" | head -1)
echo "MC_LIVE_FACT_ID=$FACT_ID"
if [ -z "$FACT_ID" ]; then bad "no addressable fact id in the provenance output"; FACT_ID=none
else ok "provenance output carries an addressable fact id"; fi

echo "=== 4. correct it through the shipped /memory correct surface ==="
one correct "/memory correct $FACT_ID the user recorded deployment region is $NEW_NONCE"
need "$ROOT/out/correct.clean.txt" "$NEW_NONCE" "/memory correct echoes the new text"

echo "=== 5. THE WIRE CLAIM: new text arrives, old text does not ==="
one recall2 "$QUESTION"
grab body-after
need "$ROOT/out/body-after.json" "$QUESTION" \
     "post-correction outbound body carries THIS turn's user message"
need "$ROOT/out/body-after.json" "$NEW_NONCE" \
     "THE CORRECTED TEXT IS IN THE OUTBOUND PROVIDER REQUEST BODY"
absent "$ROOT/out/body-after.json" "$OLD_NONCE" \
     "the SUPERSEDED text is gone from the outbound provider request body"

EXPECTED=11
TOTAL=$((PASS+FAIL))
echo "MC_LIVE_TOTALS pass=$PASS fail=$FAIL total=$TOTAL expected=$EXPECTED"
if [ "$TOTAL" -ne "$EXPECTED" ]; then
  echo "MC_LIVE_RESULT=INCOMPLETE ran $TOTAL of $EXPECTED checks"; exit 2
fi
[ "$FAIL" -eq 0 ] && echo "MC_LIVE_RESULT=GREEN $PASS/$EXPECTED" || echo "MC_LIVE_RESULT=RED $FAIL failed"
[ "$FAIL" -eq 0 ]
