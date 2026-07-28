#!/bin/bash
# F24-C1 Task 1 — LIVE proof of a REAL abandonment, named through the surface.
#
# All real: a systemd-supervised gateway, an independent sink process, a real
# kill -9 while a delivery is in flight, a platform restart, and the shipped
# binary's own drain deciding to give up.
#
# Anti-"universal denial": positive delivery is COUNTED at the independent sink
# BEFORE any abandonment claim is made. If nothing is delivered, abort.
set -u
export PATH=/root/.cargo/bin:$PATH
R=/root/wayland-24-abandon-surface
RUN=/tmp/f24c1c-run
BIN=$R/target/release/wayland-core
SINK=$R/target/release/wayland-channel-sink
PORT=18474
N=6
STALL_AFTER=3
P=f24c1c

systemctl --user stop wayland-core-gateway-$P 2>/dev/null
rm -rf $RUN; mkdir -p $RUN
export WAYLAND_HOME=$RUN/home
export WAYLAND_PROFILE=$P
mkdir -p $WAYLAND_HOME/channels

echo "=== binary under test ==="; git -C $R rev-parse --short HEAD

echo ""
echo "=== 1. independent sink first ==="
nohup $SINK --port $PORT --journal $RUN/arrivals.jsonl --stall-after $STALL_AFTER \
  > $RUN/sink.log 2>&1 &
SINK_PID=$!
sleep 2; cat $RUN/sink.log

cat > $WAYLAND_HOME/channels/${P}sink.toml <<EOF
name = "${P}sink"
platform = "slack"
enabled = true

[options]
workspace_name = "$P-fixture"
default_channel_id = "$P-room"
credential_handle_bot_token = "slack.$P.bot_token"
credential_handle_signing_secret = "slack.$P.signing_secret"
api_base_url = "http://127.0.0.1:$PORT"
max_retry_attempts = 1
EOF
cat > $WAYLAND_HOME/credentials.toml <<EOF
[secrets]
"slack.$P.bot_token" = "xoxb-$P-fixture-not-a-real-credential"
"slack.$P.signing_secret" = "$P-fixture-signing-secret"
EOF
chmod 600 $WAYLAND_HOME/credentials.toml

echo ""
echo "=== 2. seed $N deliveries ==="
for i in $(seq -w 1 $N); do
  $BIN cron add --trigger every:30 --channel ${P}sink --text "$P-delivery-$i" >> $RUN/seed.log 2>&1
done
echo "seeded=$(grep -c . $RUN/seed.log 2>/dev/null || echo ?)"

echo ""
echo "=== 3. install + start the REAL service ==="
$BIN gateway install --profile $P 2>&1 | tail -1
systemctl --user daemon-reload 2>/dev/null
$BIN gateway start --profile $P 2>&1 | tail -1
sleep 3

echo ""
echo "=== 4. COUNT POSITIVE DELIVERY at the independent sink ==="
for t in $(seq 1 45); do
  A=$(wc -l < $RUN/arrivals.jsonl 2>/dev/null || echo 0)
  echo "  t=${t}s arrivals_at_sink=$A"
  [ "$A" -ge $((STALL_AFTER + 1)) ] && break
  sleep 2
done
ARRIVALS=$(wc -l < $RUN/arrivals.jsonl 2>/dev/null || echo 0)
DELIVERED_OK=$(grep -c . $RUN/arrivals.jsonl 2>/dev/null || echo 0)
echo "ARRIVALS_AT_INDEPENDENT_SINK=$ARRIVALS"
if [ "$ARRIVALS" -lt 1 ]; then
  echo "ABORT: nothing delivered; no claim below would mean anything"
  kill $SINK_PID 2>/dev/null; exit 1
fi

echo ""
echo "=== 5. surface BEFORE (must be EMPTY) ==="
$BIN gateway abandoned --profile $P 2>&1

echo ""
echo "=== 6. kill -9 while a delivery is stalled at the sink ==="
GPID=$(systemctl --user show -p MainPID --value wayland-core-gateway-$P 2>/dev/null)
echo "gateway_pid=$GPID at $(date -u +%H:%M:%S)"
kill -9 "$GPID" 2>&1
# The sink goes too: the restarted gateway must not re-stall for 23s, because a
# stalled send blocks the tick loop that has to notice the drain request.
kill $SINK_PID 2>/dev/null
echo "killed gateway and sink"
python3 - <<'PYEOF'
import json,os,collections
p=os.path.join(os.environ["WAYLAND_HOME"],"deliveries.jsonl")
st={}
for l in open(p):
    l=l.strip()
    if l: r=json.loads(l); st[r["id"]]=r
print("STATE_COUNTS_AFTER_KILL =", dict(collections.Counter(v["state"] for v in st.values())))
for k,v in st.items():
    if v["state"] in ("attempted","accepted"):
        print("  CARRIED:", k, "state=", v["state"], "dest=", v.get("destination"))
PYEOF

echo ""
echo "=== 7. platform restarts it; resume CARRIES the unsettled delivery ==="
for t in $(seq 1 20); do
  A=$(systemctl --user is-active wayland-core-gateway-$P 2>&1)
  NR=$(systemctl --user show -p NRestarts --value wayland-core-gateway-$P 2>&1)
  echo "  t=$((t*2))s active=$A NRestarts=$NR"
  [ "$A" = "active" ] && [ "$NR" != "0" ] && break
  sleep 2
done
journalctl --user -u wayland-core-gateway-$P --no-pager -n 20 2>&1 | grep -E "started pid" | tail -2

echo ""
echo "=== 8. ask the shipped binary to drain with a budget it cannot meet ==="
$BIN gateway drain --profile $P --budget-ms 2000 2>&1 | tail -4
sleep 12
journalctl --user -u wayland-core-gateway-$P --no-pager -n 40 2>&1 | grep -E "drain|ABANDONED|abandoned" | tail -8

echo ""
echo "=== 9. THE SURFACE: what did you give up on? ==="
echo "--- operator view ---"
$BIN gateway abandoned --profile $P 2>&1
echo "--- json ---"
$BIN gateway abandoned --profile $P --json 2>&1

echo ""
echo "=== 10. cross-check surface vs raw ledger ==="
grep -c '"abandoned"' $WAYLAND_HOME/deliveries.jsonl 2>/dev/null | sed 's/^/raw_abandoned_records=/'
grep '"abandoned"' $WAYLAND_HOME/deliveries.jsonl 2>/dev/null | tail -4

systemctl --user stop wayland-core-gateway-$P 2>/dev/null
$BIN gateway uninstall --profile $P > /dev/null 2>&1
echo "F24C1_LIVE3_DONE"
