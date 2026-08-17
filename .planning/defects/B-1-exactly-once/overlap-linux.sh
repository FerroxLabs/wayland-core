#!/bin/bash
# FINDING 6: ordinary lease overlap, no kill anywhere. 40 concurrent pairs both
# claiming the SAME key. Before: 14 of 40 pairs ended parked (exit 90) with the
# effect having landed exactly once -- a task needing a human for no reason.
set -u
BIN=/root/b1/target/release/wayland-core
ROOT=/root/b1-conc; rm -rf "$ROOT"; mkdir -p "$ROOT"
cat > "$ROOT/w.sh" <<'EOF'
#!/bin/sh
mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
printf 'task=%s msg_id=%s\n' "$WAYLAND_GOAL_TASK" "$$-$(date +%s%N)" \
  > "$WAYLAND_GOAL_EFFECT_SINK/r.$$.$(date +%s%N)"
EOF
chmod +x "$ROOT/w.sh"
PARKED=0; PRODUCED=0; DECLINED=0; OTHER=0
for i in $(seq 1 40); do
  D="$ROOT/p$i"; mkdir -p "$D"
  for r in a b; do
    env -u API_KEY -u FLUX_API_KEY WAYLAND_GOAL_ID=g-conc WAYLAND_GOAL_TASK="t$i" \
      WAYLAND_GOAL_IDEMPOTENCY_KEY="idem-t$i" \
      $BIN goal exec-task --effects-dir "$D" -- "$ROOT/w.sh" > "$D/$r.out" 2>&1 &
    eval "PID_$r=\$!"
  done
  wait $PID_a; EA=$?
  wait $PID_b; EB=$?
  for e in $EA $EB; do
    case $e in
      0) PRODUCED=$((PRODUCED+1));;
      90) PARKED=$((PARKED+1));;
      *) OTHER=$((OTHER+1));;
    esac
  done
  grep -lq 'effect-already-committed\|concurrent-attempt-committed-first' "$D"/*.out 2>/dev/null && DECLINED=$((DECLINED+1))
done
echo "PAIRS=40 exit0=$PRODUCED parked90=$PARKED other=$OTHER pairs_with_a_decline=$DECLINED"
TOT=$($BIN goal effects --effects-dir "$ROOT/p1" 2>/dev/null | grep -o 'observed_total=[0-9]*')
echo "sample pair 1: $TOT"
DUPS=0; MISSING=0
for i in $(seq 1 40); do
  d=$($BIN goal effects --effects-dir "$ROOT/p$i" 2>/dev/null | grep -o 'duplicates=[0-9]*' | cut -d= -f2)
  t=$($BIN goal effects --effects-dir "$ROOT/p$i" 2>/dev/null | grep -o 'observed_total=[0-9]*' | cut -d= -f2)
  [ "$d" != "0" ] && DUPS=$((DUPS+1))
  [ "$t" = "0" ] && MISSING=$((MISSING+1))
done
echo "PAIRS_WITH_DUPLICATE=$DUPS  (0 is the PASS)"
echo "PAIRS_WITH_NO_EFFECT=$MISSING  (0 is the PASS)"
echo "PAIRS_PARKED=$PARKED  (0 is the PASS: before the overlap wait this was 14/40)"
