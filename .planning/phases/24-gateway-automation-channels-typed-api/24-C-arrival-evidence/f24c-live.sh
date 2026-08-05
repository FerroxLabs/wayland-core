#!/bin/bash
# Phase 24 lane 24c — Criterion 1 DELIVERY-ARRIVAL measurement.
#
# The scenario deliberately carries state (AGENTS.md §11): 12 distinct
# deliveries, an existing profile, a real registered service, a non-empty
# ledger, and a destination that ACCEPTS one delivery and never answers it.
# A sink that answers everything can only ever produce Settled deliveries, so
# the outcome-unknown class — the one that decides whether a restart
# duplicates or loses — would be unreachable.
set -u
export PATH=/root/.cargo/bin:$PATH
R=/root/wayland-24c
RUN=/tmp/f24c-run
BIN=$R/target/release/wayland-core
SINK=$R/target/release/wayland-channel-sink
PORT=18471
N=12
STALL_AFTER=8

rm -rf $RUN; mkdir -p $RUN
export WAYLAND_HOME=$RUN/home
export WAYLAND_PROFILE=f24c
mkdir -p $WAYLAND_HOME/channels

# --- 1. The independent sink comes up FIRST and owns the arrivals journal ----
nohup $SINK --port $PORT --journal $RUN/arrivals.jsonl --stall-after $STALL_AFTER \
  > $RUN/sink.log 2>&1 &
SINK_PID=$!
sleep 2
echo "== SINK =="; cat $RUN/sink.log; echo "sink_pid=$SINK_PID"

# --- 2. Point the REAL slack adapter at it (fixture token, not a credential) --
cat > $WAYLAND_HOME/channels/f24csink.toml <<EOF
name = "f24csink"
platform = "slack"
enabled = true

[options]
workspace_name = "f24c-fixture"
default_channel_id = "f24c-room"
credential_handle_bot_token = "slack.f24c.bot_token"
credential_handle_signing_secret = "slack.f24c.signing_secret"
api_base_url = "http://127.0.0.1:$PORT"
max_retry_attempts = 1
EOF

cat > $WAYLAND_HOME/credentials.toml <<'EOF'
[secrets]
"slack.f24c.bot_token" = "xoxb-f24c-fixture-not-a-real-credential"
"slack.f24c.signing_secret" = "f24c-fixture-signing-secret"
EOF
chmod 600 $WAYLAND_HOME/credentials.toml

# --- 3. Seed N distinct deliveries through the SHIPPED binary ----------------
# Distinct bodies are the discriminator the sink tallies over: two records with
# the same body are the same logical delivery, seen twice.
for i in $(seq -w 1 $N); do
  $BIN cron add --trigger every:30 --channel f24csink \
       --text "f24c-delivery-$i" >> $RUN/seed.log 2>&1
done
echo "== SEEDED =="; $BIN cron list 2>&1 | head -20

# --- 4. Install and start the real service ----------------------------------
$BIN gateway install --profile f24c 2>&1 | tail -3
systemctl --user daemon-reload 2>/dev/null
$BIN gateway start --profile f24c 2>&1 | tail -3
sleep 3
echo "== STATUS AFTER START =="; $BIN gateway status --profile f24c --json 2>&1

# --- 5. Wait until the sink has taken the stalled delivery -------------------
# Poll the SINK's journal, not the gateway's ledger. The gateway's own record
# is the thing under test and cannot be the instrument.
for t in $(seq 1 60); do
  A=$(wc -l < $RUN/arrivals.jsonl 2>/dev/null || echo 0)
  echo "t=${t}s arrivals_at_sink=$A"
  [ "$A" -ge $((STALL_AFTER + 1)) ] && break
  sleep 1
done
echo "== ARRIVALS BEFORE KILL =="; cat $RUN/arrivals.jsonl 2>/dev/null | wc -l
echo "== LEDGER STATE BEFORE KILL (the sender's own record, for contrast) =="
$BIN gateway status --profile f24c --json 2>&1

# --- 6. kill -9 while a delivery is in flight at the destination -------------
GPID=$(systemctl --user show -p MainPID --value wayland-core-gateway-f24c 2>/dev/null)
echo "gateway_pid=$GPID"
kill -9 "$GPID" 2>&1
echo "== KILLED =="; date -u +%H:%M:%S

# --- 7. Let the PLATFORM bring it back --------------------------------------
sleep 12
systemctl --user show -p NRestarts --value wayland-core-gateway-f24c 2>&1 | sed 's/^/NRestarts=/'
echo "== STATUS AFTER PLATFORM RESTART =="; $BIN gateway status --profile f24c --json 2>&1
echo "== GATEWAY LOG (resume line) =="
journalctl --user -u wayland-core-gateway-f24c --no-pager -n 40 2>&1 | grep -E "gateway\]" | tail -15

# --- 8. Give the restarted gateway time to do whatever it does --------------
for t in $(seq 1 45); do
  A=$(wc -l < $RUN/arrivals.jsonl 2>/dev/null || echo 0)
  echo "post-restart t=${t}s arrivals_at_sink=$A"
  sleep 2
done

echo "== FINAL ARRIVALS JOURNAL (independent sink) =="
cat $RUN/arrivals.jsonl
echo "== FINAL LEDGER (sender's own record) =="
cat $WAYLAND_HOME/deliveries.jsonl 2>/dev/null | tail -30
echo "== FINAL STATUS =="; $BIN gateway status --profile f24c --json 2>&1
