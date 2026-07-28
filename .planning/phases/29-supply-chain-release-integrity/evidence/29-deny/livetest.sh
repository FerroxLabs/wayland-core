#!/usr/bin/env bash
# LIVE TEST (lane brief 3.1) — drive the REAL wayland-core binary over real HTTP
# and read the OpenAPI document it actually serves. The utoipa 4->5 bump changes
# this wire surface, so a green unit test is not evidence on its own.
set -u
cd /root/wayland-29-deny || exit 99
BIN=./target/debug/wayland-core
PORT=18929
OUT=/root/livetest

rm -rf "$OUT"; mkdir -p "$OUT"
echo "binary: $($BIN --version 2>&1 | head -1)"
echo "binary path: $(readlink -f $BIN)"
echo "binary mtime: $(stat -c %y $BIN)"

# NO real credential is used and none is needed. /openapi.json and /doc are a
# documented public carve-out requiring no key. WAYLAND_ACP_SERVER_KEY is set to
# an obvious placeholder purely to bypass the keychain mint, which cannot work on
# a headless box (no Secret Service backend) — that is a separate, known issue
# owned by another lane, not a finding of this one.
WAYLAND_HOME=$OUT/home \
WAYLAND_ACP_SERVER_KEY=livetest-placeholder-not-a-secret \
ANTHROPIC_API_KEY=placeholder-not-a-secret \
  "$BIN" acp serve --bind "127.0.0.1:$PORT" --provider anthropic \
  > "$OUT/server.log" 2>&1 &
SRV=$!
echo "server pid=$SRV bound to 127.0.0.1:$PORT"

up=0
for i in $(seq 1 30); do
  if curl -sS -o /dev/null -m 2 "http://127.0.0.1:$PORT/openapi.json" 2>/dev/null; then
    echo "server up after ${i}s"; up=1; break
  fi
  echo "waiting for listener: iteration $i, $(date +%H:%M:%S)"
  sleep 1
done

if [ "$up" -ne 1 ]; then
  echo "LIVE_RESULT=SERVER_NEVER_CAME_UP"
  echo "--- server.log ---"; cat "$OUT/server.log"
  kill $SRV 2>/dev/null; exit 1
fi

echo "=== GET /openapi.json (real HTTP, real binary) ==="
echo "http_status=$(curl -sS -o "$OUT/openapi.json" -w '%{http_code}' "http://127.0.0.1:$PORT/openapi.json")"
echo "bytes=$(wc -c < "$OUT/openapi.json")"
echo "openapi_version=$(python3 -c 'import json;print(json.load(open("'"$OUT"'/openapi.json"))["openapi"])')"
echo "path_count=$(python3 -c 'import json;print(len(json.load(open("'"$OUT"'/openapi.json"))["paths"]))')"
echo "schema_count=$(python3 -c 'import json;print(len(json.load(open("'"$OUT"'/openapi.json"))["components"]["schemas"]))')"

echo "--- keystone paths ---"
python3 - "$OUT/openapi.json" <<'PY'
import json,sys
d=json.load(open(sys.argv[1]))
for p in ["/v1/sessions","/v1/sessions/{id}","/v1/sessions/{id}/prompt"]:
    print(f"  {p}: {'PRESENT' if p in d['paths'] else 'MISSING'}")
PY

echo "--- 3.0-vs-3.1 SHAPE differential (repaired instrument + self-test) ---"
python3 /root/shapecheck.py "$OUT/openapi.json"

echo "=== GET /doc (spec viewer, also public carve-out) ==="
echo "doc_status=$(curl -sS -o "$OUT/doc.html" -w '%{http_code}' "http://127.0.0.1:$PORT/doc")"

echo "=== NEGATIVE control: a NON-carve-out endpoint without a key ==="
echo "  (if this were also 200, the 200s above would prove nothing)"
echo "sessions_no_key_status=$(curl -sS -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/v1/sessions")"

kill $SRV 2>/dev/null
wait $SRV 2>/dev/null
echo "server stopped"
echo "LIVE_RESULT=DONE"
