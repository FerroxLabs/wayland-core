#!/usr/bin/env bash
# F22-C1 live drive: durable Goals become observable on the protocol surface.
# Drives the SHIPPED binary only. No examples/, no harness-minted state.
set -u
BIN=/root/wayland-22-c1/target/debug/wayland-core
NONCE="$1"                    # caller-generated, so a replayed capture is detectable
OUT=/root/wayland-22-c1/live-$NONCE
rm -rf "$OUT"; mkdir -p "$OUT"

say() { echo "=== $* ==="; }

say "0. BUILD IDENTITY (assert before any measurement)"
$BIN --build-info > "$OUT/build-info.txt" 2>&1
cat "$OUT/build-info.txt"
EXPECTED_SHA="$2"
if ! grep -q "$EXPECTED_SHA" "$OUT/build-info.txt"; then
  echo "FATAL: binary is not at $EXPECTED_SHA"; exit 90
fi
echo "build-sha OK"

J="$OUT/session.journal"
E="$OUT/effects"
mkdir -p "$E"

say "1. OPEN a durable Goal through the shipped verb"
$BIN goal open --journal "$J" --goal "g-$NONCE" \
  --objective "prove goals reach the protocol ($NONCE)" \
  --iterations 4 --max-tokens 5000 > "$OUT/1-open.txt" 2>&1
echo "rc=$?"; cat "$OUT/1-open.txt"

say "2. DECLARE tasks with a real dependency"
$BIN goal task --journal "$J" --goal "g-$NONCE" --task build > "$OUT/2-task-a.txt" 2>&1
echo "rc=$?"
$BIN goal task --journal "$J" --goal "g-$NONCE" --task publish --depends-on build > "$OUT/2-task-b.txt" 2>&1
echo "rc=$?"
cat "$OUT/2-task-a.txt" "$OUT/2-task-b.txt"

say "3. RUN the Goal through the real Fleet dispatcher, terminating canonically"
$BIN goal run --journal "$J" --goal "g-$NONCE" --effects-dir "$E" \
  --worker-command "/bin/echo worked" --width 4 --shard-size 2 --terminate \
  > "$OUT/3-run.txt" 2>&1
echo "rc=$?"; tail -20 "$OUT/3-run.txt"

say "4. STATUS (the pre-existing CLI surface, for comparison)"
$BIN goal status --journal "$J" --goal "g-$NONCE" > "$OUT/4-status.json" 2>&1
echo "rc=$? bytes=$(wc -c < "$OUT/4-status.json")"

say "5. STREAM — the NEW protocol surface, driven from the real binary"
$BIN goal stream --journal "$J" --goal "g-$NONCE" > "$OUT/5-stream.jsonl" 2> "$OUT/5-stream.err"
STREAM_RC=$?
echo "rc=$STREAM_RC"
echo "stdout bytes=$(wc -c < "$OUT/5-stream.jsonl")  stderr bytes=$(wc -c < "$OUT/5-stream.err")"
cat "$OUT/5-stream.err"

say "6. COUNTS — measured from the real stdout, by type"
TOTAL=$(wc -l < "$OUT/5-stream.jsonl")
SNAP=$(grep -c '"type":"goal_snapshot"' "$OUT/5-stream.jsonl")
TRANS=$(grep -c '"type":"goal_transition"' "$OUT/5-stream.jsonl")
echo "lines=$TOTAL goal_snapshot=$SNAP goal_transition=$TRANS"
echo "--- transition kinds, in emitted order ---"
grep -o '"transition":"[a-z_]*"' "$OUT/5-stream.jsonl" | sed 's/.*://'
echo "--- every line is valid JSON? ---"
VALID=0; INVALID=0
while IFS= read -r line; do
  if echo "$line" | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
    VALID=$((VALID+1)); else INVALID=$((INVALID+1)); fi
done < "$OUT/5-stream.jsonl"
echo "valid_json_lines=$VALID invalid=$INVALID"

say "7. FALSIFY the gate — --expect with a wrong count MUST exit non-zero"
$BIN goal stream --journal "$J" --goal "g-$NONCE" --expect 999 > /dev/null 2> "$OUT/7-falsify.err"
FALSIFY_RC=$?
echo "wrong-expect rc=$FALSIFY_RC (MUST be non-zero)"; cat "$OUT/7-falsify.err"
$BIN goal stream --journal "$J" --goal "g-$NONCE" --expect "$TOTAL" > /dev/null 2> "$OUT/7-correct.err"
CORRECT_RC=$?
echo "right-expect rc=$CORRECT_RC (MUST be 0)"

say "8. FALSIFY absence — an unknown goal MUST fail, not print an empty Goal"
$BIN goal stream --journal "$J" --goal "g-does-not-exist" > "$OUT/8-absent.out" 2> "$OUT/8-absent.err"
ABSENT_RC=$?
echo "absent rc=$ABSENT_RC (MUST be non-zero)  stdout bytes=$(wc -c < "$OUT/8-absent.out")"
cat "$OUT/8-absent.err"

say "9. DETERMINISM — a second stream of the same chain must be byte-identical"
$BIN goal stream --journal "$J" --goal "g-$NONCE" > "$OUT/9-stream-again.jsonl" 2>/dev/null
if cmp -s "$OUT/5-stream.jsonl" "$OUT/9-stream-again.jsonl"; then
  echo "deterministic=YES (byte-identical replay)"
else
  echo "deterministic=NO"; fi
sha256sum "$OUT/5-stream.jsonl" "$OUT/9-stream-again.jsonl"

say "10. VERDICT"
echo "GOAL_EVENTS_ON_PROTOCOL=$TOTAL"
echo "SNAPSHOTS=$SNAP TRANSITIONS=$TRANS"
echo "GATE_FALSIFIABLE=$([ "$FALSIFY_RC" -ne 0 ] && [ "$CORRECT_RC" -eq 0 ] && echo YES || echo NO)"
echo "ABSENCE_REFUSED=$([ "$ABSENT_RC" -ne 0 ] && echo YES || echo NO)"
echo "WLDRIVE_DONE nonce=$NONCE"
