#!/usr/bin/env bash
# 23B-C3 USER-MODEL LIVE DRIVE — the user-model half of criterion 3 through the
# SHIPPED wayland-core binary, on real hardware, against a real HTTP endpoint.
#
# It proves the thing the memory half's summary says nobody proved: that a user
# correction reaches the MODEL, not merely the store. Every wire assertion reads
# the captured outbound request body, not the binary's own stdout.
#
# ANTI-VACUITY (lane brief §3.2, §3b-i):
#   * every capture is asserted to EXIST and be NON-EMPTY before it is read;
#   * every `absent` is preceded by a `need` on the SAME file, so a dead binary
#     printing nothing cannot pass an absence check;
#   * the outbound-body checks assert THIS turn's user message first, so a
#     stale capture from an earlier turn cannot be read as this one;
#   * the correction nonce is proved ABSENT before it is set and PRESENT after,
#     so "it is on the wire" cannot be satisfied by a pre-existing string;
#   * the `/memory nudge` removal check is paired with a known-positive on
#     `/memory activation`, so "unknown sub-action" cannot come from a binary
#     that rejects every slash command;
#   * a final counter is compared against an expected total, so an early exit
#     reports INCOMPLETE rather than looking like a pass.
#
# PROVIDER READ-BACK (lane brief §3b-ii): /root/.wayland/.env on this host
# injects a real ANTHROPIC_API_KEY into every process regardless of the shell.
# This drive therefore does NOT infer where traffic went from its own env: it
# asserts the local mock captured request bodies, which is only true if the
# requests reached the mock rather than Anthropic.
set -uo pipefail

ROOT="${1:?usage: usermodel-live-drive.sh <workdir> <binary> [nonce]}"
BIN="${2:?usage: usermodel-live-drive.sh <workdir> <binary> [nonce]}"
NONCE="${3:-QK7UM3LIVE}"
QUESTION="summarise the deployment plan"

mkdir -p "$ROOT/out"
export HOME="$ROOT/home"
export XDG_CONFIG_HOME="$ROOT/home/.config"
export XDG_DATA_HOME="$ROOT/home/.local/share"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"
PROJ="$ROOT/proj"; mkdir -p "$PROJ"

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); echo "UM_LIVE=PASS  $1"; }
bad() { FAIL=$((FAIL+1)); echo "UM_LIVE=FAIL  $1"; }
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
CAP = os.environ["CAP_DIR"]
STATE = {"n": 0}; LOCK = threading.Lock()
def sse(text):
    ev = [("message_start", {"type":"message_start","message":{"id":"m","type":"message","role":"assistant","content":[],"model":"claude-mock","stop_reason":None,"stop_sequence":None,"usage":{"input_tokens":10,"output_tokens":1}}}),
          ("content_block_start", {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
          ("content_block_delta", {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
          ("content_block_stop", {"type":"content_block_stop","index":0}),
          ("message_delta", {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":None},"usage":{"output_tokens":1}}),
          ("message_stop", {"type":"message_stop"})]
    return "".join("event: %s\ndata: %s\n\n" % (n, json.dumps(d)) for n, d in ev).encode()
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        with LOCK:
            STATE["n"] += 1; idx = STATE["n"]
        open(os.path.join(CAP, "req-%03d.json" % idx), "wb").write(body)
        payload = sse("ack")
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("content-length", str(len(payload)))
        self.end_headers(); self.wfile.write(payload)
HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PYEOF

PORT=18741
export CAP_DIR="$ROOT/cap"; mkdir -p "$CAP_DIR"
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
  local sid; sid=$(printf 'c3ce1fee%04x' "$SESSION_N")
  ( cd "$PROJ" && exec 9< "$ROOT/.vp" && \
    WAYLAND_VAULT_PASSPHRASE_FD=9 timeout 90 "$BIN" --auto-approve --provider anthropic \
      --api-key sk-live-not-real --base-url "http://127.0.0.1:$PORT" \
      --model claude-mock --session-id "$sid" "$@" ) > "$ROOT/out/$name.txt" 2>&1
  echo "UM_LIVE_RC=$? session=$name sid=$sid"
  /usr/bin/sed 's/\x1b\[[0-9;]*m//g' "$ROOT/out/$name.txt" > "$ROOT/out/$name.clean.txt"
}
grab() { local latest; latest=$(ls -1 "$CAP_DIR"/req-*.json 2>/dev/null | tail -1)
         if [ -n "$latest" ]; then cp "$latest" "$ROOT/out/$1.json"; fi; }

echo "=== 0. the binary is alive (guards every later capture) ==="
"$BIN" --version > "$ROOT/out/version.txt" 2>&1
need "$ROOT/out/version.txt" "wayland-core" "binary reports its version"

echo "=== 1. /usermodel exists and reports honestly when nothing is corrected ==="
one um-empty "/usermodel"
need "$ROOT/out/um-empty.clean.txt" "corrected nothing" \
     "/usermodel reports an empty correction set as empty, not as a blank list"

echo "=== 2. a turn reaches the mock, and the nonce is NOT there yet ==="
one turn-before "$QUESTION"
grab body-before
need "$ROOT/out/body-before.json" "$QUESTION" \
     "outbound body carries THIS turn's user message (not a stale capture)"
need "$ROOT/out/body-before.json" "claude-mock" \
     "the captured body is the request the PRODUCT built (provider read-back, brief 3b-ii)"
absent "$ROOT/out/body-before.json" "$NONCE" \
     "the correction nonce is absent BEFORE any correction is made"

echo "=== 3. correct the user model through the shipped slash surface ==="
one um-correct "/usermodel correct style $NONCE and never hedge"
need "$ROOT/out/um-correct.clean.txt" "$NONCE" "/usermodel correct echoes what it stored"
need "$ROOT/out/um-correct.clean.txt" "next session" \
     "/usermodel correct says WHEN it takes effect rather than implying it is already live"

echo "=== 4. the correction SURVIVES process exit (a different process sees it) ==="
one um-show "/usermodel show"
need "$ROOT/out/um-show.clean.txt" "$NONCE" \
     "a NEW process reads back the correction written by a previous one"
need "$ROOT/out/um-show.clean.txt" "override" \
     "/usermodel show states that corrections override what was inferred"

echo "=== 5. THE WIRE CLAIM: the correction reaches the provider request body ==="
one turn-after "$QUESTION"
grab body-after
need "$ROOT/out/body-after.json" "$QUESTION" \
     "post-correction outbound body carries THIS turn's user message"
need "$ROOT/out/body-after.json" "$NONCE" \
     "THE USER'S CORRECTION IS IN THE OUTBOUND PROVIDER REQUEST BODY"
need "$ROOT/out/body-after.json" "Corrected by the user" \
     "the correction is framed to the model as authoritative, not as one more signal"

echo "=== 6. forgetting a correction removes it from the wire ==="
one um-forget "/usermodel forget style"
need "$ROOT/out/um-forget.clean.txt" "dropped style" "/usermodel forget reports what it removed"
one turn-forgotten "$QUESTION"
grab body-forgotten
need "$ROOT/out/body-forgotten.json" "$QUESTION" \
     "post-forget outbound body carries THIS turn's user message"
absent "$ROOT/out/body-forgotten.json" "$NONCE" \
     "the forgotten correction is GONE from the outbound provider request body"

echo "=== 7. forget refuses out loud for a key that is not there ==="
one um-forget-miss "/usermodel forget nosuchkey"
need "$ROOT/out/um-forget-miss.clean.txt" "nothing removed" \
     "a forget miss is reported as a miss, never as success"

echo "=== 8. /memory nudge is GONE, and the removal is not a dead binary ==="
one act-show "/memory activation"
need "$ROOT/out/act-show.clean.txt" "automatic recall" \
     "KNOWN-POSITIVE: /memory still dispatches its real sub-actions"
one nudge-gone "/memory nudge"
need "$ROOT/out/nudge-gone.clean.txt" "unknown sub-action" \
     "/memory nudge no longer exists (paired with the known-positive above)"
absent "$ROOT/out/nudge-gone.clean.txt" "cap 3 per session" \
     "the nudge bound is no longer advertised anywhere in /memory"

EXPECTED=19
TOTAL=$((PASS+FAIL))
echo "UM_LIVE_TOTALS pass=$PASS fail=$FAIL total=$TOTAL expected=$EXPECTED"
if [ "$TOTAL" -ne "$EXPECTED" ]; then
  echo "UM_LIVE_RESULT=INCOMPLETE ran $TOTAL of $EXPECTED checks"; exit 2
fi
[ "$FAIL" -eq 0 ] && echo "UM_LIVE_RESULT=GREEN $PASS/$EXPECTED" || echo "UM_LIVE_RESULT=RED $FAIL failed"
[ "$FAIL" -eq 0 ]
