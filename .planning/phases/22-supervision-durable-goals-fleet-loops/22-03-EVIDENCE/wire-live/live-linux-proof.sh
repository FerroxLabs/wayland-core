#!/bin/bash
# LIVE proof against the SHIPPED wayland-core binary: a real fanout, a real
# uncatchable kill of the whole process tree, a real restart.
#
# The scenario deliberately carries state. A clean one cannot reach the defect:
#   - 12 tasks, not 1, so the wave shards (width 6 / shard-size 2 = 3 shards)
#   - a dependency layer, so more than one wave is required
#   - the kill lands MID-WAVE, with some tasks recorded-but-undelivered and
#     others still running, not at a quiescent boundary
#   - one task's effect is placed on disk with NO completion before the restart,
#     which is the exact state a kill leaves and the only state in which the
#     idempotency key can be observed doing anything
set -u
BIN=/root/wayland-22-wire/target/release/wayland-core
ROOT=/root/p22-wire-live
rm -rf "$ROOT"; mkdir -p "$ROOT/effects"
J="$ROOT/fleet.journal"
G="g-live"

cat > "$ROOT/worker.sh" <<'EOF'
#!/bin/sh
case "$WAYLAND_GOAL_TASK" in
  t00|t01) sleep 1 ;;
  t02|t03) sleep 2 ;;
  t04|t05) sleep 40 ;;
  *)       sleep 1 ;;
esac
EOF
chmod +x "$ROOT/worker.sh"

echo "=== BINARY ==="
$BIN --version
echo "=== 1. OPEN THE GOAL (shipped verb) ==="
$BIN goal open --journal "$J" --goal "$G" --objective "prove the wire survives a kill" \
  --iterations 8 --max-tokens 10000 || exit 1

echo "=== 2. DECLARE 12 TASKS, 6 OF THEM DEPENDENT ==="
for i in 0 1 2 3 4 5; do
  $BIN goal task --journal "$J" --goal "$G" --task "t0$i" || exit 1
done
for i in 0 1 2 3 4 5; do
  $BIN goal task --journal "$J" --goal "$G" --task "t0$((i+6))" --depends-on "t0$i" || exit 1
done

echo "=== 3. RUN 1, in its own process group, to be killed mid-wave ==="
setsid "$BIN" goal run --journal "$J" --goal "$G" --effects-dir "$ROOT/effects" \
  --worker-command "$ROOT/worker.sh" --width 6 --shard-size 2 --lease 5s \
  > "$ROOT/run1.log" 2>&1 &
RUNPID=$!
sleep 1
PGID=$(ps -o pgid= -p $RUNPID 2>/dev/null | tr -d ' ')
echo "RUN1 pid=$RUNPID pgid=$PGID"

# Wait until the state we want to kill in is REAL, not hoped for: four effects
# on disk (t00..t03 done and recorded) with t04/t05 still running.
for _ in $(seq 1 200); do
  n=$(ls "$ROOT/effects/effects" 2>/dev/null | wc -l)
  [ "$n" -ge 4 ] && break
  sleep 0.2
done
echo "PRE-KILL effects=$(ls "$ROOT/effects/effects" 2>/dev/null | wc -l)"
DESC_BEFORE=$(ps -eo pgid=,pid= | awk -v g="$PGID" '$1==g' | wc -l)
SLEEPS_BEFORE=$(pgrep -g "$PGID" -f 'sleep 40' | wc -l)
echo "PRE-KILL pgid_processes=$DESC_BEFORE sleep40_workers=$SLEEPS_BEFORE"
echo "KILL_AT_UTC=$(date -u +%Y-%m-%dT%H:%M:%SZ)"

kill -9 -"$PGID"
sleep 3
DESC_AFTER=$(ps -eo pgid=,pid= | awk -v g="$PGID" '$1==g' | wc -l)
SLEEPS_AFTER=$(pgrep -g "$PGID" -f 'sleep 40' | wc -l)
echo "POST-KILL pgid_processes=$DESC_AFTER sleep40_workers=$SLEEPS_AFTER"
echo "POST-KILL effects=$(ls "$ROOT/effects/effects" 2>/dev/null | wc -l)"
echo "--- run1.log ---"; cat "$ROOT/run1.log"

echo "=== 4. PLACE t04's EFFECT ON DISK WITH NO COMPLETION ==="
# The state a kill leaves between a worker's effect and its parent's record.
# Produced with the SHIPPED exec-task verb, so the restart's refusal below is a
# product behaviour and not a fixture's.
WAYLAND_GOAL_TASK=t04 WAYLAND_GOAL_IDEMPOTENCY_KEY=idem-t04 \
  $BIN goal exec-task --effects-dir "$ROOT/effects" || exit 1
echo "AFTER-PLANT effects=$(ls "$ROOT/effects/effects" 2>/dev/null | wc -l)"

echo "=== 5. WAIT PAST THE 5s LEASE, THEN RESTART ==="
sleep 6
echo "RESTART_AT_UTC=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
"$BIN" goal run --journal "$J" --goal "$G" --effects-dir "$ROOT/effects" \
  --worker-command "$ROOT/worker.sh" --width 6 --shard-size 2 --lease 60s \
  > "$ROOT/run2.log" 2>&1
echo "RUN2_EXIT=$?"
echo "--- run2.log ---"; cat "$ROOT/run2.log"

echo "=== 6. THE GATE: exactly 12 effects, counted by the product ==="
"$BIN" goal effects --effects-dir "$ROOT/effects" --expect 12
echo "EFFECTS_GATE_EXIT=$?"

echo "=== 7. FALSIFY THE GATE (it must be able to go red) ==="
cp "$ROOT/effects/effects/idem-t00" "$ROOT/effects/effects/idem-t00-DUPLICATE"
"$BIN" goal effects --effects-dir "$ROOT/effects" --expect 12
echo "FALSIFIED_GATE_EXIT=$?   (nonzero is the PASS here)"
rm -f "$ROOT/effects/effects/idem-t00-DUPLICATE"
"$BIN" goal effects --effects-dir "$ROOT/effects" --expect 12
echo "RESTORED_GATE_EXIT=$?"

echo "=== 8. LEDGER STATE (shipped status verb) ==="
"$BIN" goal status --journal "$J" --goal "$G" > "$ROOT/status.json"
python3 - "$ROOT/status.json" <<'PY'
import json,sys
s=json.load(open(sys.argv[1]))
tasks=s["tasks"]
print("GOAL lifecycle=%s iterations=%s resume_count=%s tasks=%d"%(
  s["lifecycle"], s["iterations_started"], s["resume_count"], len(tasks)))
comp=sum(1 for t in tasks.values() if t.get("completion"))
deliv=sum(1 for t in tasks.values() if (t.get("completion") or {}).get("delivered"))
att=sum(len(t.get("attempts",[])) for t in tasks.values())
rel=sum(t.get("dependency_releases",0) for t in tasks.values())
unres=sum(1 for t in tasks.values()
          if t.get("attempts") and t["attempts"][-1]["status"]=="unknown")
print("LEDGER completed=%d delivered=%d attempts=%d dependency_releases=%d unresolved=%d"%(
  comp,deliv,att,rel,unres))
for k in sorted(tasks):
    t=tasks[k]
    print("  %s attempts=%d epoch=%d completed=%s delivered=%s"%(
      k, len(t.get("attempts",[])), t["attempts"][-1]["epoch"] if t.get("attempts") else 0,
      bool(t.get("completion")), (t.get("completion") or {}).get("delivered")))
PY
