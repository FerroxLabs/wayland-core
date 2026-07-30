#!/usr/bin/env bash
# CONT-* cache economics — live proof for F05-TRUTH-1's runtime outcome proof.
#
# ONE variable: $WAYLAND_PRICING_AUTO_REFRESH.
#   on  -> the opt-in live cache refresh runs; a successful fetch+publish is the
#          capability's real side effect and must emit reached/outcome_changed/observed.
#   off -> the refresher is still CONSTRUCTED (ready), the bundled catalog is still
#          used, but nothing was published, so NO occurrence may appear.
#
# The provider is the same canned OK endpoint used by the cooldown proof, so the
# turn itself is identical between arms and the pricing fetch is the only thing
# that differs.
set -u

BIN="${BIN:-/root/wayland-cont-skills-cache/target/debug/wayland-core}"
PORT="${PORT:-18946}"
ARM="${ARM:-on}"
ROOT="/root/cont-skills-cache-price/$ARM"

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

# The refresher is only built when the chain is on -- that is the construction
# fact under test, separately from the refresh outcome.
[provider_chain]
enabled = true
failure_threshold = 3
recovery_timeout_secs = 30

[session]
enabled = false
TOML

export CANNED_LOG="$ROOT/out/canned-requests.log"
export CANNED_PORT="$PORT"
export CANNED_MODE=ok

python3 /root/cont-skills-cache-cooldown-provider.py &
SRV=$!
trap 'kill $SRV 2>/dev/null' EXIT
sleep 1

echo "ARM=$ARM"
# Is the real pricing origin reachable at all from this host? Recorded so an
# absent occurrence in the ON arm can be told apart from a blocked network.
UPSTREAM=$(curl -s -o /dev/null -w '%{http_code}' --max-time 20 https://openrouter.ai/api/v1/models)
echo "UPSTREAM_OPENROUTER_HTTP=$UPSTREAM"
echo "CACHE_PRESENT_BEFORE=$(ls "$ROOT/home/.wayland/pricing-cache.json" 2>/dev/null | wc -l)"

cd "$ROOT/work" || exit 1
if [ "$ARM" = "on" ]; then
  export WAYLAND_PRICING_AUTO_REFRESH=1
else
  unset WAYLAND_PRICING_AUTO_REFRESH
fi

HOME="$ROOT/home" XDG_CONFIG_HOME="$ROOT/home/.config" WAYLAND_HOME="$ROOT/home/.wayland" \
  timeout 180 "$BIN" --json-stream --auto-approve --no-tui \
  > "$ROOT/out/stream.jsonl" 2> "$ROOT/out/stream.err" <<'STDIN'
{"type":"message","msg_id":"price-1","content":"Say hello."}
STDIN
echo "PRODUCT_RC=$?"

S="$ROOT/out/stream.jsonl"
echo "STREAM_BYTES=$(wc -c < "$S")"
echo "CANNED_MODE_SEEN=$(/usr/bin/grep -o 'mode=[a-z]*' "$CANNED_LOG" | sort -u | tr '\n' ' ')"
echo "PRICING_READY=$(/usr/bin/grep -c '"capability":"pricing_refresher","stage":"ready"' "$S")"
echo "PRICING_REACHED=$(/usr/bin/grep -c '"capability":"pricing_refresher","stage":"reached"' "$S")"
echo "PRICING_OUTCOME_CHANGED=$(/usr/bin/grep -c '"capability":"pricing_refresher","stage":"outcome_changed"' "$S")"
echo "PRICING_OBSERVED=$(/usr/bin/grep -c '"capability":"pricing_refresher","stage":"observed"' "$S")"
echo "CACHE_WRITTEN_AFTER=$(find "$ROOT/home" -name '*pricing*' 2>/dev/null | wc -l)"
echo "KNOWN_POSITIVE_any_capability_activation=$(/usr/bin/grep -c 'capability_activation' "$S")"
echo "KNOWN_NEGATIVE_bogus=$(/usr/bin/grep -c '"capability":"zzz_not_a_capability"' "$S")"
echo "=== ARM $ARM END ==="
