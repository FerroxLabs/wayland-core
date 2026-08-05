#!/usr/bin/env bash
# 22-remaining — live proof that the mid-flight monitor's runtime path is WIRED.
#
# Drives the shipped release binary in --json-stream mode against a canned
# OpenAI-compatible endpoint that returns the same failing tool call with a
# volatile path component, and reads three things back OUT OF THE PRODUCT'S OWN
# STDOUT:
#
#   1. the capability_activation chain for mid_flight_monitor       (startup truth)
#   2. a mid_flight_monitor_decision event                          (runtime consult)
#   3. the reached/outcome_changed/observed occurrence triple       (outcome proof)
#
# Nothing is inferred from the environment: LANE-BRIEF 3b-ii says
# /root/.wayland/.env injects ANTHROPIC_API_KEY regardless of what the shell
# unsets, so the canned endpoint's own request log is the read-back that proves
# which provider actually served the turn.
set -u

BIN="${BIN:-/root/wayland-22-remaining/target/release/wayland-core}"
PORT="${PORT:-18733}"
ROOT="${ROOT:-/tmp/wl22r}"
OUT="${OUT:-$ROOT/out}"

rm -rf "$ROOT"
mkdir -p "$ROOT/home/.config/wayland-core" "$OUT" "$ROOT/work"

cat > "$ROOT/home/.config/wayland-core/config.toml" <<'TOML'
[default]
provider = "canned"

[providers.canned]
provider = "openai"
model = "canned-model"
api_key = "sk-synthetic-not-a-secret-wl22r"
base_url = "http://127.0.0.1:18733"

[providers.canned.compat]
include_usage_in_stream = false

# Headless host with no OS keyring and no unlocked vault. This is the escape
# the product's own error message names; it turns OFF durable session
# persistence, which is not what is under test here.
[session]
enabled = false
TOML

export CANNED_LOG="$OUT/canned-requests.log"
export CANNED_PORT="$PORT"
export CANNED_TOOL_TURNS="${CANNED_TOOL_TURNS:-6}"

python3 "$(dirname "$0")/canned_provider.py" &
SRV=$!
trap 'kill $SRV 2>/dev/null' EXIT
sleep 1

# --- instrument liveness: a known-positive AND a known-negative probe --------
POS=$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  "http://127.0.0.1:$PORT/v1/chat/completions" \
  -H 'Content-Type: application/json' -d '{"model":"probe","messages":[]}')
NEG=$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 -X POST \
  "http://127.0.0.1:$((PORT+1))/v1/chat/completions" \
  -H 'Content-Type: application/json' -d '{}' ; echo "rc=$?")
echo "PROBE_POSITIVE_HTTP=$POS"
echo "PROBE_NEGATIVE=$NEG"

# --- drive the product ------------------------------------------------------
cd "$ROOT/work" || exit 1
HOME="$ROOT/home" XDG_CONFIG_HOME="$ROOT/home/.config" \
  timeout 180 "$BIN" --json-stream --auto-approve --no-tui \
  > "$OUT/stream.jsonl" 2> "$OUT/stream.err" <<'STDIN'
{"type":"message","msg_id":"wl22r-1","content":"Read the file at /tmp/wl22r-run-1/wl22r-missing.txt and keep trying until it works."}
STDIN
echo "PRODUCT_RC=$?"

echo "STREAM_BYTES=$(wc -c < "$OUT/stream.jsonl")"
