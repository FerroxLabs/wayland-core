#!/usr/bin/env bash
# Four-quadrant live harness — lane/fix-channel-auth-producers.
#
# Usage:  authprod-harness.sh <arm> <slack|discord> <bogus|absent|real> <binary>
# Secrets arrive on STDIN as KEY=VALUE lines and are never echoed, never in
# argv, and reach disk only through the product's own credential store inside
# this arm's private WAYLAND_HOME (removed by the caller afterwards).
set -uo pipefail

ARM="$1"; PLATFORM="$2"; MODE="$3"; BIN="$4"

WLHOME="/root/wl-authprod-$ARM"
LOG="/tmp/lane-authprod-$ARM-gateway.log"
OUT="/tmp/lane-authprod-$ARM.log"

# --- read secrets from stdin only -------------------------------------------
SLACK_BOT_TOKEN=""; SLACK_SIGNING_SECRET=""; DISCORD_BOT_TOKEN=""
while IFS= read -r line || [ -n "$line" ]; do
  key="${line%%=*}"; val="${line#*=}"
  val="${val%\"}"; val="${val#\"}"; val="${val%\'}"; val="${val#\'}"
  case "$key" in
    SLACK_BOT_TOKEN) SLACK_BOT_TOKEN="$val" ;;
    SLACK_SIGNING_SECRET) SLACK_SIGNING_SECRET="$val" ;;
    DISCORD_BOT_TOKEN) DISCORD_BOT_TOKEN="$val" ;;
  esac
done


# Bogus credentials, assembled rather than written literally (see the file).
. "$(dirname "${BASH_SOURCE[0]}")/bogus-credentials.sh"

count_owners() {
  local home="$1" n=0 p
  for p in /proc/[0-9]*; do
    [ -r "$p/environ" ] || continue
    if tr '\0' '\n' < "$p/environ" 2>/dev/null | grep -qx "WAYLAND_HOME=$home"; then
      n=$((n+1))
    fi
  done
  echo "$n"
}

rm -rf "$WLHOME"; mkdir -p "$WLHOME/channels"
: > "$OUT"

{
echo "=== ARM $ARM ==="
echo "binary: $BIN"
"$BIN" --version 2>&1 | head -1
echo "sha256: $(sha256sum "$BIN" | awk '{print $1}')"
echo "platform: $PLATFORM   token mode: $MODE"
echo "WAYLAND_HOME: $WLHOME"
} >> "$OUT"

# --- config -----------------------------------------------------------------
if [ "$PLATFORM" = "slack" ]; then
  HANDLE="slack.authprod.bot_token"
  [ "$MODE" = "absent" ] && HANDLE="slack.authprod.ABSENT_KEY"
  cat > "$WLHOME/channels/slack.toml" <<EOF
name = "slack"
platform = "slack"
enabled = true

[options]
workspace_name = "authprod"
default_channel_id = ""
credential_handle_bot_token = "$HANDLE"
credential_handle_signing_secret = "slack.authprod.signing_secret"
EOF
else
  HANDLE="discord.authprod.bot_token"
  [ "$MODE" = "absent" ] && HANDLE="discord.authprod.ABSENT_KEY"
  cat > "$WLHOME/channels/discord.toml" <<EOF
name = "discord"
platform = "discord"
enabled = true

[options]
credential_handle = "$HANDLE"
allowed_channel_ids = ["1532226655102173318"]
EOF
fi

# --- credentials, via the product's own stdin-only setter -------------------
set_cred() { printf %s "$2" | WAYLAND_HOME="$WLHOME" "$BIN" channel credential set "$1" >/dev/null 2>&1; }

if [ "$PLATFORM" = "slack" ]; then
  set_cred "slack.authprod.signing_secret" "$SLACK_SIGNING_SECRET"
  case "$MODE" in
    bogus) set_cred "slack.authprod.bot_token" "$BOGUS_SLACK" ;;
    real)  set_cred "slack.authprod.bot_token" "$SLACK_BOT_TOKEN" ;;
    absent) : ;;   # deliberately not stored
  esac
else
  case "$MODE" in
    bogus) set_cred "discord.authprod.bot_token" "$BOGUS_DISCORD" ;;
    real)  set_cred "discord.authprod.bot_token" "$DISCORD_BOT_TOKEN" ;;
    absent) : ;;
  esac
fi

# --- preconditions ----------------------------------------------------------
HANDLE_IN_CONFIG=$(grep -c "$HANDLE" "$WLHOME/channels/$PLATFORM.toml")
# REPAIRED 2026-07-30. The first version was `grep -c "$HANDLE"` over
# `channel credential list`, which lists every handle a CONFIG REFERENCES and
# marks it `stored` or `MISSING`. It therefore returned 1 for a deliberately
# ABSENT credential — the arm-2 precondition could not fail. Now keyed on the
# status column. Self-test with three assertions in matcher-selftest.sh,
# including "the old matcher would have missed this".
HANDLE_IN_STORE=$(WAYLAND_HOME="$WLHOME" "$BIN" channel credential list 2>/dev/null \
  | awk -v h="$HANDLE" 'index($0,h)>0 && $NF=="stored" {n++} END{print n+0}')
OWNERS_BEFORE=$(count_owners "$WLHOME")
{
echo "-- PRECONDITIONS, asserted before any measurement --"
echo "handle_in_config=$HANDLE_IN_CONFIG   (must be 1)"
echo "handle_in_store=$HANDLE_IN_STORE   (bogus/real: 1, absent: 0)"
echo "owners_of_this_home_BEFORE_start=$OWNERS_BEFORE   (must be 0)"
} >> "$OUT"

# --- start the gateway ------------------------------------------------------
WAYLAND_HOME="$WLHOME" "$BIN" gateway run > "$LOG" 2>&1 &
GW_PID=$!
echo "" >> "$OUT"
echo "-- start gateway --" >> "$OUT"
echo "gateway_pid=$GW_PID" >> "$OUT"

# Poll (never silently) until the health file appears or we time out.
HEALTH_FILE=""
for i in $(seq 1 30); do
  sleep 2
  f=$(ls "$WLHOME"/*health* 2>/dev/null | head -1)
  if [ -n "$f" ]; then HEALTH_FILE="$f"; echo "health file after $((i*2))s: $f" >> "$OUT"; break; fi
  echo "waiting for health file: iteration $i" >> "$OUT"
done

# --- participant assertion (a run with no live gateway is not the experiment)
if kill -0 "$GW_PID" 2>/dev/null; then ALIVE=yes; else ALIVE=no; fi
OWNERS_DURING=$(count_owners "$WLHOME")
{
echo "-- PARTICIPANT ASSERTION --"
echo "gateway_alive=$ALIVE   (must be yes)"
echo "owners_of_this_home_DURING=$OWNERS_DURING   (must be exactly 1: mine)"
} >> "$OUT"

# Give the adapter time to reach the platform and be refused.
sleep 12

# --- platform-facing evidence from the gateway's own log --------------------
{
echo ""
echo "-- gateway log: platform-facing evidence --"
echo "count_4004=$(grep -c '4004' "$LOG")"
echo "count_invalid_auth=$(grep -c 'invalid_auth' "$LOG")"
echo "count_auth_test_rejected=$(grep -c 'auth.test rejected' "$LOG")"
echo "count_mgr_terminal=$(grep -c 'credential was rejected by the platform' "$LOG")"
echo "count_gateway_stop=$(grep -c 'stopping the gateway loop' "$LOG")"
grep -E 'registered=|started pid=|rejected|4004|Unauthenticated' "$LOG" | head -12
} >> "$OUT"

# --- the measurement --------------------------------------------------------
{
echo ""
echo "-- channel health --json (DEFAULT exit status) --"
} >> "$OUT"
WAYLAND_HOME="$WLHOME" "$BIN" channel health --json >> "$OUT" 2>&1
echo "HEALTH_RC=$?" >> "$OUT"

echo "" >> "$OUT"
echo "-- STICKINESS: same query again after 20s (state must not drift) --" >> "$OUT"
sleep 20
WAYLAND_HOME="$WLHOME" "$BIN" channel health --json 2>&1 | grep -E '"state"|"reason"' >> "$OUT"
echo "HEALTH2_RC=$?" >> "$OUT"

echo "" >> "$OUT"
echo "-- channel health --require-healthy (the gate), FULL output --" >> "$OUT"
WAYLAND_HOME="$WLHOME" "$BIN" channel health --require-healthy >> "$OUT" 2>&1
echo "REQUIRE_RC=$?" >> "$OUT"

# --- teardown BY PID, verified ---------------------------------------------
kill "$GW_PID" 2>/dev/null
for i in $(seq 1 15); do
  kill -0 "$GW_PID" 2>/dev/null || break
  sleep 1
done
if kill -0 "$GW_PID" 2>/dev/null; then kill -9 "$GW_PID" 2>/dev/null; sleep 2; fi
if kill -0 "$GW_PID" 2>/dev/null; then TD="FAILED pid still alive"; else TD="ok pid dead"; fi
OWNERS_AFTER=$(count_owners "$WLHOME")
{
echo ""
echo "-- teardown BY PID, verified --"
echo "teardown=$TD"
echo "owners_of_this_home_AFTER=$OWNERS_AFTER   (must be 0)"
} >> "$OUT"

cat "$OUT"
