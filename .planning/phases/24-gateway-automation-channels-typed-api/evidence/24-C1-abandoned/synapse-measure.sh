#!/bin/bash
# F24-C1 Task 3 — measurement against the already-running real Synapse.
set -u
D=/root/24c1ab-synapse
BASE=http://127.0.0.1:18018

echo "=== wait ready ==="
for i in $(seq 1 60); do
  code=$(curl -s -o /dev/null -w '%{http_code}' $BASE/_matrix/client/versions 2>/dev/null || echo 000)
  echo "  wait i=$i http=$code $(date +%H:%M:%S)"
  [ "$code" = "200" ] && break
  sleep 2
done

echo "=== register ==="
curl -s -X POST $BASE/_matrix/client/v3/register \
  -H 'Content-Type: application/json' \
  -d '{"username":"24c1ab","password":"f24c1-throwaway-container-pw","auth":{"type":"m.login.dummy"},"device_id":"C1ABDEV"}' \
  > $D/reg.json 2>&1
echo "reg bytes=$(wc -c < $D/reg.json)"
TOKEN=$(python3 -c "import json;print(json.load(open('$D/reg.json')).get('access_token',''))" 2>/dev/null)
echo "token_len=${#TOKEN}"
if [ -z "$TOKEN" ]; then head -c 300 $D/reg.json; echo; exit 1; fi

echo "=== create room ==="
curl -s -X POST $BASE/_matrix/client/v3/createRoom \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"name":"24c1ab"}' > $D/room.json
ROOM=$(python3 -c "import json;print(json.load(open('$D/room.json'))['room_id'])")
ENC=$(python3 -c "import urllib.parse;print(urllib.parse.quote('$ROOM',safe=''))")
echo "room=$ROOM"

send() {
  curl -s -X PUT "$BASE/_matrix/client/v3/rooms/$ENC/send/m.room.message/$1" \
    -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
    -d "{\"msgtype\":\"m.text\",\"body\":\"$2\"}"
}
eid() { python3 -c "import json,sys;print(json.loads(sys.argv[1]).get('event_id',''))" "$1"; }

echo ""
echo "############ MEASUREMENT A: the RESETTING counter ############"
echo "--- process life 1 (counter seeded at 1, as AtomicU64::new(1)) ---"
R1=$(send 1 "MSG-A-before-restart"); echo "PUT txn=1 body=MSG-A -> $R1"
R2=$(send 2 "MSG-B-before-restart"); echo "PUT txn=2 body=MSG-B -> $R2"
E1=$(eid "$R1"); E2=$(eid "$R2")

echo "--- process life 2: SAME access token, counter RESET to 1 ---"
R3=$(send 1 "MSG-C-after-restart-GENUINELY-NEW"); echo "PUT txn=1 body=MSG-C -> $R3"
E3=$(eid "$R3")

echo ""
echo "E1 (txn=1, MSG-A) = $E1"
echo "E3 (txn=1, MSG-C) = $E3"
if [ -n "$E1" ] && [ "$E1" = "$E3" ]; then
  echo "VERDICT_A=REPLAY_SUPPRESSED"
else
  echo "VERDICT_A=NEW_EVENT_CREATED"
fi

echo ""
echo "=== ground truth: room contents (not the response, the ROOM) ==="
curl -s "$BASE/_matrix/client/v3/rooms/$ENC/messages?dir=b&limit=50" \
  -H "Authorization: Bearer $TOKEN" > $D/msgs.json
python3 - <<PYEOF
import json
d=json.load(open("$D/msgs.json"))
b=[e["content"].get("body") for e in d.get("chunk",[]) if e.get("type")=="m.room.message"]
print("BODIES_IN_ROOM =", b)
for w in ["MSG-A-before-restart","MSG-B-before-restart","MSG-C-after-restart-GENUINELY-NEW"]:
    print(f"  present({w}) = {w in b}")
print("MSG_C_LOST =", "MSG-C-after-restart-GENUINELY-NEW" not in b)
PYEOF

echo ""
echo "############ MEASUREMENT B: the key-derived txn id (the fix) ############"
K1=cron:job-a:1785121776528
K2=cron:job-a:1785121776529
R4=$(send "$K1" "MSG-D-derived");        echo "PUT txn=$K1 -> $R4"
R5=$(send "$K1" "MSG-D-derived");        echo "PUT txn=$K1 (REPLAY of same delivery) -> $R5"
R6=$(send "$K2" "MSG-E-next-occurrence"); echo "PUT txn=$K2 (DIFFERENT delivery) -> $R6"
E4=$(eid "$R4"); E5=$(eid "$R5"); E6=$(eid "$R6")
echo "E4=$E4"
echo "E5=$E5"
echo "E6=$E6"
[ -n "$E4" ] && [ "$E4" = "$E5" ] && echo "DEDUP_WORKS=yes" || echo "DEDUP_WORKS=no"
[ -n "$E4" ] && [ "$E4" != "$E6" ] && echo "DISTINCT_WORKS=yes" || echo "DISTINCT_WORKS=no"

curl -s "$BASE/_matrix/client/v3/rooms/$ENC/messages?dir=b&limit=50" \
  -H "Authorization: Bearer $TOKEN" > $D/msgs2.json
python3 - <<PYEOF
import json
d=json.load(open("$D/msgs2.json"))
b=[e["content"].get("body") for e in d.get("chunk",[]) if e.get("type")=="m.room.message"]
print("FINAL_BODIES =", b)
print("MSG_D_count =", b.count("MSG-D-derived"), " (1 = replay suppressed, 2 = duplicate)")
print("MSG_E_present =", "MSG-E-next-occurrence" in b, " (True = a genuinely new delivery survived)")
PYEOF
echo "F24C1_SYNAPSE_DONE"
