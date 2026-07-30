#!/usr/bin/env bash
# Characterise BASE Discord's health under a rejected token.
#
# A single sample cannot distinguish "always degraded" from "flaps through
# healthy". Source analysis says the gateway pushes Connected immediately after
# sending IDENTIFY, BEFORE Discord accepts it (gateway.rs:926), then the 4004
# close becomes a generic error -> Reconnecting. So the inbox should carry
# [Connected, Reconnecting, Connected, ...] and the manager records each in
# order, last-in-batch winning. Sample repeatedly and COUNT the states.
set -uo pipefail
BIN="$1"; TAG="$2"
WLHOME="/root/wl-authprod-$TAG"
LOG="/tmp/lane-authprod-$TAG-gateway.log"

DISCORD_BOT_TOKEN=""
while IFS= read -r line || [ -n "$line" ]; do
  key="${line%%=*}"; val="${line#*=}"; val="${val%\"}"; val="${val#\"}"
  [ "$key" = "DISCORD_BOT_TOKEN" ] && DISCORD_BOT_TOKEN="$val"
done

. "$(dirname "${BASH_SOURCE[0]}")/bogus-credentials.sh"

rm -rf "$WLHOME"; mkdir -p "$WLHOME/channels"
cat > "$WLHOME/channels/discord.toml" <<EOF
name = "discord"
platform = "discord"
enabled = true

[options]
credential_handle = "discord.authprod.bot_token"
allowed_channel_ids = ["1532226655102173318"]
EOF
printf %s "$BOGUS_DISCORD" | WAYLAND_HOME="$WLHOME" "$BIN" channel credential set discord.authprod.bot_token >/dev/null 2>&1

echo "=== BASE DISCORD STATE SAMPLER ($TAG) ==="
echo "binary sha256: $(sha256sum "$BIN" | awk '{print $1}')"

WAYLAND_HOME="$WLHOME" "$BIN" gateway run > "$LOG" 2>&1 &
GW_PID=$!
sleep 5
kill -0 "$GW_PID" 2>/dev/null && echo "gateway_alive=yes pid=$GW_PID" || echo "gateway_alive=NO"

HEALTHY=0; DEGRADED=0; UNAUTH=0; OTHER=0; SAMPLES=0
for i in $(seq 1 45); do
  s=$(WAYLAND_HOME="$WLHOME" "$BIN" channel health --json 2>/dev/null | grep '"state"' | head -1 | sed -E 's/.*"state": *"([a-z]+)".*/\1/')
  case "$s" in
    healthy) HEALTHY=$((HEALTHY+1)) ;;
    degraded) DEGRADED=$((DEGRADED+1)) ;;
    unauthenticated) UNAUTH=$((UNAUTH+1)) ;;
    *) OTHER=$((OTHER+1)) ;;
  esac
  SAMPLES=$((SAMPLES+1))
  echo "sample $i: $s"
  sleep 2
done

echo "--- TOTALS over $SAMPLES samples (~90s) ---"
echo "healthy=$HEALTHY  degraded=$DEGRADED  unauthenticated=$UNAUTH  other=$OTHER"
echo "--- how hard did it hammer the platform? ---"
echo "identify_attempts=$(grep -c 'sent IDENTIFY' "$LOG")"
echo "reconnecting_events=$(grep -c 'Reconnecting' "$LOG")"
echo "close_frame_errs=$(grep -c 'close frame' "$LOG")"
echo "4004_mentions=$(grep -c '4004' "$LOG")"

kill "$GW_PID" 2>/dev/null; sleep 3
kill -0 "$GW_PID" 2>/dev/null && kill -9 "$GW_PID" 2>/dev/null
kill -0 "$GW_PID" 2>/dev/null && echo "teardown=FAILED" || echo "teardown=ok pid dead"
