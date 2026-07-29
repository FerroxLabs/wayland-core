#!/bin/bash
# F24-C1-abandoned — LIVE proof that an abandoned delivery can be DISPOSED OF.
#
# Extends `f24c1-live3.sh` (lane/24-abandon-surface, which proved an abandonment
# can be NAMED) with the three things that were still missing: the re-send
# refusal, a real re-send that puts the message at an independent destination,
# and the acknowledgement that retires the record.
#
# Everything real: a systemd-supervised gateway, an independent sink process, a
# real kill -9 with a delivery in flight, a platform restart, the shipped
# binary's own drain deciding to give up, and the shipped binary's own verbs
# disposing of the result.
#
# TRAP DISCIPLINE (each of these has silently faked a green on this programme):
#  - positive delivery is COUNTED at the independent sink BEFORE any claim;
#  - the surface is shown EMPTY first — a list that is never empty proves nothing;
#  - the re-send sink owns a SEPARATE journal, asserted 0 before and 1 after, so
#    the arrival cannot be an earlier delivery being recounted;
#  - the refusal is exercised as a known-negative: the same command WITHOUT the
#    flag must fail, or the flag is decorative.
set -u
export PATH=/root/.cargo/bin:$PATH
R=/root/wayland-24-c1-abandoned
RUN=/tmp/24c1ab-run                # lane-unique: /tmp is shared between lanes
BIN=$R/target/release/wayland-core
SINK=$R/target/release/wayland-channel-sink
PORT=18477
N=6
STALL_AFTER=3
P=24c1ab

systemctl --user stop wayland-core-gateway-$P 2>/dev/null
rm -rf $RUN; mkdir -p $RUN
export WAYLAND_HOME=$RUN/home
export WAYLAND_PROFILE=$P
mkdir -p "$WAYLAND_HOME/channels"

echo "=== binary under test ==="; git -C $R rev-parse HEAD

echo ""
echo "=== 1. independent sink first (stalls after $STALL_AFTER) ==="
nohup $SINK --port $PORT --journal $RUN/arrivals.jsonl --stall-after $STALL_AFTER \
  > $RUN/sink.log 2>&1 &
SINK_PID=$!
sleep 2; cat $RUN/sink.log

cat > "$WAYLAND_HOME/channels/${P}sink.toml" <<EOF
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
cat > "$WAYLAND_HOME/credentials.toml" <<EOF
[secrets]
"slack.$P.bot_token" = "xoxb-$P-fixture-not-a-real-credential"
"slack.$P.signing_secret" = "$P-fixture-signing-secret"
EOF
chmod 600 "$WAYLAND_HOME/credentials.toml"

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
  echo "  t=$((t*2))s arrivals_at_sink=$A"
  [ "$A" -ge $((STALL_AFTER + 1)) ] && break
  sleep 2
done
ARRIVALS=$(wc -l < $RUN/arrivals.jsonl 2>/dev/null || echo 0)
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
echo "=== 7+8. restart, then drain inside the carried-work window ==="
# THE WINDOW IS ~1s AND IT IS EASY TO MISS. `resume()` leaves the carried
# delivery Attempted (pending=1) but does NOT re-dispatch it; the first cron
# tick does, and the send then settles, taking pending back to 0. The run loop
# reads the drain request BEFORE it ticks, so a request that lands before the
# first tick is processed against pending=1 and forces an abandonment.
#
# A first version of this script polled `is-active` every 2s and ran
# `journalctl` before draining. That burned the whole window: the drain arrived
# at pending=0 and reported `Clean: abandoned=0`. So the drain is now fired in a
# tight loop that spins while the gateway is DOWN — each call fails fast with
# "cannot drain: no gateway is running" — and lands within ~100ms of the
# projection being published.
for i in $(seq 1 400); do
  OUT=$($BIN gateway drain --profile $P --budget-ms 1 2>&1)
  if ! echo "$OUT" | grep -q "cannot drain"; then
    echo "drain fired on spin $i at $(date -u +%H:%M:%S.%3N)"
    echo "$OUT" | tail -4
    break
  fi
  sleep 0.1
done
sleep 6
journalctl --user -u wayland-core-gateway-$P --no-pager -n 60 2>&1 \
  | grep -E "started pid|drain |ABANDONED" | tail -6

echo ""
echo "=== 9. THE SURFACE: what did you give up on? ==="
$BIN gateway abandoned --profile $P 2>&1
echo "--- json ---"
$BIN gateway abandoned --profile $P --json 2>&1

ID=$($BIN gateway abandoned --profile $P --json 2>/dev/null | python3 -c \
  'import json,sys; d=json.load(sys.stdin); print(d["abandoned"][0]["id"] if d["abandoned"] else "")')
echo "TARGET_DELIVERY_ID=$ID"
if [ -z "$ID" ]; then
  echo "ABORT: no abandonment to dispose of; the rest of this run would be vacuous"
  exit 1
fi

echo ""
echo "=== 10. KNOWN-NEGATIVE: resend must REFUSE without --confirm-not-delivered ==="
$BIN gateway resend --profile $P "$ID" > $RUN/refuse.out 2>&1
REFUSE_RC=$?
echo "REFUSE_RC=$REFUSE_RC  (non-zero REQUIRED — a guard that never fires is decorative)"
cat $RUN/refuse.out

echo ""
echo "=== 11. bring an independent sink back, with its OWN journal ==="
# The gateway is STOPPED first, and that is load-bearing. A running gateway
# would keep firing its 30s schedule into the same port, and an arrival in
# `arrivals2.jsonl` would then prove nothing about the re-send. Stopping it
# makes `gateway resend` the only thing that can put a line in that file.
systemctl --user stop wayland-core-gateway-$P 2>/dev/null
sleep 1
echo "gateway_active=$(systemctl --user is-active wayland-core-gateway-$P 2>&1) (must NOT be active)"
nohup $SINK --port $PORT --journal $RUN/arrivals2.jsonl > $RUN/sink2.log 2>&1 &
SINK2_PID=$!
sleep 2; cat $RUN/sink2.log
B=$(wc -l < $RUN/arrivals2.jsonl 2>/dev/null || echo 0)
echo "ARRIVALS2_BEFORE_RESEND=$B  (must be 0: a re-send arrival cannot be an old delivery recounted)"

echo ""
echo "=== 12. THE RE-SEND, confirmed ==="
$BIN gateway resend --profile $P "$ID" --confirm-not-delivered 2>&1
RESEND_RC=$?
echo "RESEND_RC=$RESEND_RC"
sleep 2
A2=$(wc -l < $RUN/arrivals2.jsonl 2>/dev/null || echo 0)
echo "ARRIVALS2_AFTER_RESEND=$A2  (must be 1)"
echo "--- the re-sent message, as the INDEPENDENT sink recorded it ---"
cat $RUN/arrivals2.jsonl 2>/dev/null

echo ""
echo "=== 13. the abandonment is STILL listed, now marked resent, still unacked ==="
$BIN gateway abandoned --profile $P 2>&1

echo ""
echo "=== 14. acknowledge it ==="
$BIN gateway ack --profile $P "$ID" 2>&1
echo "--- re-running ack must be idempotent, not a second signature ---"
$BIN gateway ack --profile $P "$ID" 2>&1
echo "--- surface after ack ---"
$BIN gateway abandoned --profile $P 2>&1

echo ""
echo "=== 15. KNOWN-NEGATIVE: both verbs refuse an id that is not abandoned ==="
# NOT piped. The first version of this block was `... 2>&1 | tail -3` followed by
# `$?`, which reports TAIL's status, not the command's — it printed
# ACK_BOGUS_RC=0 for a command that had genuinely failed, i.e. the assertion
# passed on a dead instrument. That is LANE-BRIEF §3.2's first named
# self-passing class, reproduced by this harness. Repaired here rather than
# noted, per §6b-ii.
$BIN gateway ack --profile $P "cron:not-a-real-delivery:1" > $RUN/ack-bogus.out 2>&1
ACK_BOGUS_RC=$?
$BIN gateway resend --profile $P "cron:not-a-real-delivery:1" --confirm-not-delivered \
  > $RUN/resend-bogus.out 2>&1
RESEND_BOGUS_RC=$?
echo "ACK_BOGUS_RC=$ACK_BOGUS_RC     (non-zero REQUIRED)"
echo "RESEND_BOGUS_RC=$RESEND_BOGUS_RC  (non-zero REQUIRED)"
tail -2 $RUN/ack-bogus.out
tail -2 $RUN/resend-bogus.out
# Self-test of the repair, third assertion per §6b-ii: the OLD shape would have
# missed this. Demonstrated, not asserted.
$BIN gateway ack --profile $P "cron:not-a-real-delivery:1" 2>&1 | tail -1 > /dev/null
echo "OLD_PIPED_SHAPE_RC=$?  (0 — this is the defect being repaired: same failing command, wrong status)"

echo ""
echo "=== 16. cross-check the surface against the RAW ledger ==="
python3 - <<'PYEOF'
import json,os
p=os.path.join(os.environ["WAYLAND_HOME"],"deliveries.jsonl")
st={}
for l in open(p):
    l=l.strip()
    if l: r=json.loads(l); st[r["id"]]=r
ab=[v for v in st.values() if v["state"]=="abandoned"]
print("raw_abandoned_records =", len(ab))
for v in ab:
    print("  id=", v["id"])
    print("     was_attempted=", v.get("was_attempted"),
          "resent=", bool(v.get("resent")), "acknowledged=", bool(v.get("acknowledged")))
PYEOF

kill $SINK2_PID 2>/dev/null
systemctl --user stop wayland-core-gateway-$P 2>/dev/null
$BIN gateway uninstall --profile $P > /dev/null 2>&1
echo "F24C1_ACK_RESEND_LIVE_DONE"
