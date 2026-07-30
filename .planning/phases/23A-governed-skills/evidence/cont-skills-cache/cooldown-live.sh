#!/usr/bin/env bash
# CONT-* cache economics — live proof that F05-TRUTH-3's runtime outcome proof is
# emitted by the SHIPPED BINARY, and that it is falsifiable.
#
# Reads everything back OUT OF THE PRODUCT'S OWN STDOUT:
#   1. capability_activation chain for cooldown_tracker            (startup truth)
#   2. the reached/outcome_changed/observed triple                 (outcome proof)
#
# ONE variable between the two arms: $CANNED_MODE (fail | ok).
set -u

BIN="${BIN:-/root/wayland-cont-skills-cache/target/debug/wayland-core}"
PORT="${PORT:-18944}"
ARM="${ARM:-fail}"
ROOT="/root/cont-skills-cache-cool/$ARM"

rm -rf "$ROOT"
mkdir -p "$ROOT/home/.config/wayland-core" "$ROOT/out" "$ROOT/work"

cat > "$ROOT/home/.config/wayland-core/config.toml" <<TOML
[default]
provider = "canned"

[providers.canned]
provider = "openai"
model = "canned-model"
api_key = "sk-synthetic-not-a-secret-cont-skills-cache"
base_url = "http://127.0.0.1:$PORT"

[providers.canned.compat]
include_usage_in_stream = false

# One failure opens the breaker. That transition is the ONLY thing that emits the
# F05-TRUTH-3 occurrence, so the threshold is what makes the positive arm reach it.
[provider_chain]
enabled = true
failure_threshold = 1
recovery_timeout_secs = 30

# Headless host with no OS keyring: durable session persistence is not under test.
[session]
enabled = false
TOML

export CANNED_LOG="$ROOT/out/canned-requests.log"
export CANNED_PORT="$PORT"
export CANNED_MODE="$ARM"

python3 /root/cont-skills-cache-cooldown-provider.py &
SRV=$!
trap 'kill $SRV 2>/dev/null' EXIT
sleep 1

# --- instrument liveness: known-positive AND known-negative probe -------------
POS=$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  "http://127.0.0.1:$PORT/v1/chat/completions" \
  -H 'Content-Type: application/json' -d '{"model":"probe","messages":[]}')
NEG=$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 -X POST \
  "http://127.0.0.1:$((PORT+1))/v1/chat/completions" \
  -H 'Content-Type: application/json' -d '{}' ; echo "rc=$?")
echo "ARM=$ARM"
echo "PROBE_POSITIVE_HTTP=$POS   (fail arm must be 503, ok arm 200)"
echo "PROBE_NEGATIVE=$NEG        (adjacent dead port)"

# --- drive the product -------------------------------------------------------
cd "$ROOT/work" || exit 1
HOME="$ROOT/home" XDG_CONFIG_HOME="$ROOT/home/.config" \
  timeout 180 "$BIN" --json-stream --auto-approve --no-tui \
  > "$ROOT/out/stream.jsonl" 2> "$ROOT/out/stream.err" <<'STDIN'
{"type":"message","msg_id":"cool-1","content":"Say hello."}
STDIN
echo "PRODUCT_RC=$?"
echo "STREAM_BYTES=$(wc -c < "$ROOT/out/stream.jsonl")"

# --- read the ARM back from the endpoint's own log (LANE-BRIEF 3b-ii) --------
echo "CANNED_REQUESTS=$(/usr/bin/grep -c '^.*POST' "$CANNED_LOG" 2>/dev/null || echo 0)"
echo "CANNED_MODE_SEEN=$(/usr/bin/grep -o 'mode=[a-z]*' "$CANNED_LOG" | sort -u | tr '\n' ' ')"

# --- measurements ------------------------------------------------------------
S="$ROOT/out/stream.jsonl"
echo "COOLDOWN_READY=$(/usr/bin/grep -c '"capability":"cooldown_tracker","stage":"ready"' "$S")"
echo "COOLDOWN_REACHED=$(/usr/bin/grep -c '"capability":"cooldown_tracker","stage":"reached"' "$S")"
echo "COOLDOWN_OUTCOME_CHANGED=$(/usr/bin/grep -c '"capability":"cooldown_tracker","stage":"outcome_changed"' "$S")"
echo "COOLDOWN_OBSERVED=$(/usr/bin/grep -c '"capability":"cooldown_tracker","stage":"observed"' "$S")"
echo "PRICING_STAGES=$(/usr/bin/grep -o '"capability":"pricing_refresher","stage":"[a-z_]*"' "$S" | sort -u | tr '\n' ' ')"
echo "CIRCUIT_OPEN_EVENTS=$(/usr/bin/grep -c '"state":"open"' "$S")"
# KNOWN-POSITIVE for the grep itself: a capability that must always appear.
echo "KNOWN_POSITIVE_any_capability_activation=$(/usr/bin/grep -c 'capability_activation' "$S")"
echo "KNOWN_NEGATIVE_bogus_capability=$(/usr/bin/grep -c '"capability":"zzz_not_a_capability"' "$S")"
echo "=== ARM $ARM END ==="
