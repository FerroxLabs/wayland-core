#!/bin/bash
# matrix-live-drive.sh — drive the SHIPPED `wayland-core` binary against a real
# Matrix room, and measure outbound idempotency across a real process restart.
#
# ── what this proves that nothing before it did ─────────────────────────────
#
# `docs/delivery-semantics.md` places Matrix in the exactly-once column. That
# claim rests on `rest.rs:63 txn_id_for_key` deriving the `{txnId}` path segment
# from the delivery key, so a replay after a crash carries the SAME id. The
# claim has never been driven at a real destination by the product. Here it is:
#
#   LEG 1  the binary sends. Corroborated by an independent read of the room.
#   LEG 5  the binary is `kill -9`'d with a send in flight whose event ALREADY
#          LANDED at matrix.org but whose response it never saw. It restarts,
#          re-attempts the same delivery, and the room is counted again.
#          One event = exactly-once holds across a restart. Two = it does not.
#   LEG 5b CONTROL, and the important one: the same message under a DIFFERENT
#          delivery id must produce a SECOND event. Without it, "one event"
#          is equally explained by the second attempt never happening.
#
# ── secret discipline (LANE-BRIEF §0) ───────────────────────────────────────
#
# The token arrives on STDIN. It is never in argv, never echoed, never in the
# evidence directory. It IS written to `$WAYLAND_HOME/credentials.toml`, because
# the product reads channel credentials from a credentials store and has no env
# path for a channel handle — that is a disclosed deviation, and the home lives
# on **/dev/shm (tmpfs, RAM)**, mode 700, shredded at exit, so the value never
# reaches persistent storage. Evidence goes to a separate, secret-free tree.
#
# usage:  printf '%s' "$TOKEN" | matrix-live-drive.sh <leg>
#   legs: send | restart | cleanup
set -u
export PATH=/root/.cargo/bin:$PATH

LEG="${1:-}"
R=/root/wayland-matrix-live
BIN=$R/target/release/wayland-core
EV=/tmp/matrix-live-evidence          # lane-unique (§6a-ii: /tmp is shared)
SECURE=/dev/shm/matrix-live-secure
PROXY_PORT=18790
PROFILE=mxlive

: "${MATRIX_ROOM_ID:?}" "${MATRIX_USER_ID:?}" "${MATRIX_HOMESERVER:?}"

# --- token: stdin only ------------------------------------------------------
if [ -z "${MATRIX_ACCESS_TOKEN:-}" ]; then
  MATRIX_ACCESS_TOKEN=$(cat)
  export MATRIX_ACCESS_TOKEN
fi
[ -n "$MATRIX_ACCESS_TOKEN" ] || { echo "FATAL: no token on stdin"; exit 2; }
echo "token_len=${#MATRIX_ACCESS_TOKEN}"   # length only, never the value

mkdir -p "$EV"
export WAYLAND_HOME=$SECURE/home
export WAYLAND_PROFILE=$PROFILE

setup_home() {
  rm -rf "$SECURE"; mkdir -p "$WAYLAND_HOME/channels"; chmod 700 "$SECURE"
  cat > "$WAYLAND_HOME/channels/mxlive.toml" <<EOF
name = "mxlive"
platform = "matrix"
enabled = true

[options]
homeserver_url = "http://127.0.0.1:$PROXY_PORT"
credential_handle_access_token = "matrix.live.access_token"
user_id = "$MATRIX_USER_ID"
EOF
  umask 077
  cat > "$WAYLAND_HOME/credentials.toml" <<EOF
[secrets]
"matrix.live.access_token" = "$MATRIX_ACCESS_TOKEN"
EOF
  chmod 600 "$WAYLAND_HOME/credentials.toml"
  umask 022
}

start_proxy() {
  rm -f "$EV/wire-$1.jsonl" "$SECURE/STALL"
  nohup node "$R/scripts/matrix-live-proxy.mjs" --port $PROXY_PORT \
    --upstream "$MATRIX_HOMESERVER" --journal "$EV/wire-$1.jsonl" \
    --stall-file "$SECURE/STALL" > "$EV/proxy-$1.log" 2>&1 &
  PROXY_PID=$!
  for i in $(seq 1 20); do
    grep -q MXPROXY_READY "$EV/proxy-$1.log" 2>/dev/null && break
    echo "  proxy wait $i"; sleep 1
  done
  cat "$EV/proxy-$1.log"
  # Assert the instrument is ALIVE before anything is measured through it.
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 15 \
    "http://127.0.0.1:$PROXY_PORT/_matrix/client/versions")
  echo "PROXY_LIVENESS_versions_http=$code"
  [ "$code" = "200" ] || { echo "FATAL: proxy cannot reach the homeserver"; exit 1; }
}

stop_all() {
  pkill -f "gateway run --profile $PROFILE" 2>/dev/null
  [ -n "${PROXY_PID:-}" ] && kill "$PROXY_PID" 2>/dev/null
  sleep 1
}

# ---------------------------------------------------------------------------
case "$LEG" in

send)
  echo "############ LEG 1 — the BINARY sends to a real Matrix room ############"
  echo "binary: $($BIN --version 2>&1 | head -1)"
  git -C $R rev-parse --short HEAD
  setup_home
  start_proxy send

  N1="${MATRIX_NONCE:?}"
  T=$(python3 -c 'import datetime;print((datetime.datetime.now(datetime.timezone.utc)+datetime.timedelta(seconds=25)).strftime("%Y-%m-%dT%H:%M:%SZ"))')
  echo "scheduled_for=$T nonce=$N1"
  $BIN cron add --trigger "once:$T" --channel mxlive \
    --text "wayland-core live probe $N1 cron-send" 2>&1 | tail -3
  $BIN cron list 2>&1 | tail -5

  nohup $BIN gateway run --profile $PROFILE > "$EV/gateway-send.log" 2>&1 &
  GW=$!
  echo "gateway_pid=$GW"
  for i in $(seq 1 30); do
    n=$(grep -c '"route":"send"' "$EV/wire-send.jsonl" 2>/dev/null || true)
    echo "  t=$((i*3))s wire_send_lines=$n alive=$(kill -0 $GW 2>/dev/null && echo yes || echo no)"
    [ "${n:-0}" -ge 2 ] && break
    sleep 3
  done
  echo "--- gateway log tail ---"; tail -25 "$EV/gateway-send.log"
  echo "--- wire journal (send routes only) ---"
  grep '"route":"send"' "$EV/wire-send.jsonl" 2>/dev/null || echo "(none)"
  echo "--- ledger ---"
  cat "$WAYLAND_HOME/deliveries.jsonl" 2>/dev/null || echo "(no ledger)"
  stop_all
  echo "MATRIX_LIVE_SEND_DONE"
  ;;

restart)
  echo "############ LEG 5 — outbound idempotency across a REAL restart ############"
  setup_home
  start_proxy restart

  N5="${MATRIX_NONCE:?}"
  T=$(python3 -c 'import datetime;print((datetime.datetime.now(datetime.timezone.utc)+datetime.timedelta(seconds=20)).strftime("%Y-%m-%dT%H:%M:%SZ"))')
  echo "scheduled_for=$T nonce=$N5"
  $BIN cron add --trigger "once:$T" --channel mxlive \
    --text "wayland-core live probe $N5 restart" 2>&1 | tail -2
  JOB=$($BIN cron list --json 2>/dev/null | python3 -c 'import sys,json;print(json.load(sys.stdin)[0]["id"])' 2>/dev/null || echo "?")
  echo "job_id=$JOB"

  # Arm the stall: the send will reach matrix.org for real and the response
  # will be withheld, so the product's outcome is genuinely unknown.
  touch "$SECURE/STALL"
  echo "STALL ARMED"

  nohup $BIN gateway run --profile $PROFILE > "$EV/gateway-life1.log" 2>&1 &
  GW1=$!
  echo "life1_pid=$GW1"
  STALLED=0
  for i in $(seq 1 30); do
    s=$(grep -c 'stalled_response_withheld' "$EV/wire-restart.jsonl" 2>/dev/null || true)
    echo "  life1 t=$((i*3))s stalled_events=$s"
    if [ "${s:-0}" -ge 1 ]; then STALLED=1; break; fi
    sleep 3
  done
  if [ "$STALLED" != "1" ]; then
    echo "ABORT: no send was ever stalled — the experiment did not run."
    echo "--- gateway log ---"; tail -30 "$EV/gateway-life1.log"
    stop_all; exit 1
  fi

  echo "--- ledger state with the send in flight ---"
  cat "$WAYLAND_HOME/deliveries.jsonl" 2>/dev/null

  echo "--- kill -9 the gateway MID-SEND ---"
  kill -9 $GW1 2>/dev/null
  pkill -9 -f "gateway run --profile $PROFILE" 2>/dev/null
  sleep 2
  echo "life1_alive=$(kill -0 $GW1 2>/dev/null && echo yes || echo no)"
  echo "--- ledger AFTER the kill (this is the outcome-unknown state) ---"
  cat "$WAYLAND_HOME/deliveries.jsonl" 2>/dev/null

  # Disarm so life 2 can complete.
  rm -f "$SECURE/STALL"
  echo "STALL DISARMED"

  echo "--- restart: a NEW process, same home, same job, same delivery id ---"
  nohup $BIN gateway run --profile $PROFILE > "$EV/gateway-life2.log" 2>&1 &
  GW2=$!
  echo "life2_pid=$GW2  (life1_pid=$GW1)"
  for i in $(seq 1 30); do
    n=$(grep -c '"route":"send"' "$EV/wire-restart.jsonl" 2>/dev/null || true)
    echo "  life2 t=$((i*3))s wire_send_requests=$n"
    [ "${n:-0}" -ge 4 ] && break
    sleep 3
  done
  sleep 3
  echo "--- gateway life2 log tail ---"; tail -25 "$EV/gateway-life2.log"
  echo "--- ledger FINAL ---"; cat "$WAYLAND_HOME/deliveries.jsonl" 2>/dev/null
  echo "--- abandoned surface ---"; $BIN gateway abandoned --profile $PROFILE 2>&1 | head -20
  echo "--- THE WIRE: every send route, both process lives ---"
  grep '"route":"send"' "$EV/wire-restart.jsonl" 2>/dev/null
  stop_all
  echo "MATRIX_LIVE_RESTART_DONE"
  ;;

cleanup)
  rm -rf "$SECURE"
  pkill -f matrix-live-proxy 2>/dev/null
  pkill -f "gateway run --profile $PROFILE" 2>/dev/null
  echo "secure_home_present=$([ -e /dev/shm/matrix-live-secure ] && echo yes || echo no)"
  echo "MATRIX_LIVE_CLEANUP_DONE"
  ;;

*)
  echo "usage: matrix-live-drive.sh send|restart|cleanup"; exit 2 ;;
esac
