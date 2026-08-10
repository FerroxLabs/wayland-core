#!/bin/bash
# DEFECT B-1 — Linux live proof against the SHIPPED wayland-core binary.
#
# Three legs, in this order, because the third is worthless without the first two:
#
#   A. NEGATIVE CONTROL   the instrument must NOT over-count a legitimate re-run
#   B. POSITIVE CONTROL   a deliberate, product-driven double execution; the
#                         instrument must report 2 where the marker count says 1
#   C. LIVE REPRODUCTION  real fanout, real uncatchable process-tree kill mid-wave,
#                         real restart; count what actually ran
set -u
BIN=${BIN:-/root/b1/target/release/wayland-core}
ROOT=${ROOT:-/root/b1-live}
rm -rf "$ROOT"; mkdir -p "$ROOT"

say() { echo; echo "=== $* ==="; }

# The worker's REAL external effect: one uniquely-named file per invocation.
# The product never writes into this directory — that is the whole point.
cat > "$ROOT/worker.sh" <<'EOF'
#!/bin/sh
mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
printf '%s\n' "$WAYLAND_GOAL_TASK" \
  > "$WAYLAND_GOAL_EFFECT_SINK/$WAYLAND_GOAL_TASK.$$.$(date +%s%N)"
case "$WAYLAND_GOAL_TASK" in
  t00|t01) sleep 1 ;;
  t02|t03) sleep 2 ;;
  t04|t05) sleep 40 ;;
  *)       sleep 1 ;;
esac
EOF
chmod +x "$ROOT/worker.sh"

# The same effect, then an uncatchable kill of its OWN parent (`exec-task`).
# That reproduces, deterministically and without any harness sleight of hand,
# the exact window the defect lives in: effect landed, marker not yet written.
cat > "$ROOT/worker-kill-parent.sh" <<'EOF'
#!/bin/sh
mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
printf '%s\n' "$WAYLAND_GOAL_TASK" \
  > "$WAYLAND_GOAL_EFFECT_SINK/$WAYLAND_GOAL_TASK.$$.$(date +%s%N)"
kill -9 "$PPID"
sleep 5
EOF
chmod +x "$ROOT/worker-kill-parent.sh"

say "BINARY"
$BIN --version
sha256sum "$BIN"

# ---------------------------------------------------------------- A ----------
say "A. NEGATIVE CONTROL: a second exec-task on a committed key must not re-run"
A="$ROOT/a"; mkdir -p "$A"
for _ in 1 2; do
  env -u API_KEY -u FLUX_API_KEY \
    WAYLAND_GOAL_TASK=t-neg WAYLAND_GOAL_IDEMPOTENCY_KEY=idem-t-neg \
    $BIN goal exec-task --effects-dir "$A" -- "$ROOT/worker.sh"
done
echo -n "A CENSUS: "; $BIN goal effects --effects-dir "$A"
$BIN goal effects --effects-dir "$A" --expect 1; echo "A_GATE_EXIT=$? (0 is the PASS here)"

# ---------------------------------------------------------------- B ----------
say "B. POSITIVE CONTROL: deliberate double execution"
B="$ROOT/b"; mkdir -p "$B"
echo "--- B1: worker performs its effect, then kills exec-task before the marker"
env -u API_KEY -u FLUX_API_KEY \
  WAYLAND_GOAL_TASK=t-pos WAYLAND_GOAL_IDEMPOTENCY_KEY=idem-t-pos \
  $BIN goal exec-task --effects-dir "$B" -- "$ROOT/worker-kill-parent.sh"
echo "B1_EXEC_EXIT=$? (nonzero/killed expected)"
echo -n "B AFTER FIRST: "; $BIN goal effects --effects-dir "$B"

echo "--- B2: the honest retry a supervisor would make"
env -u API_KEY -u FLUX_API_KEY \
  WAYLAND_GOAL_TASK=t-pos WAYLAND_GOAL_IDEMPOTENCY_KEY=idem-t-pos \
  $BIN goal exec-task --effects-dir "$B" -- "$ROOT/worker.sh"
echo "B2_EXEC_EXIT=$?"
echo -n "B CENSUS: "; $BIN goal effects --effects-dir "$B"
echo "B RAW effect files:"; ls -1 "$B/observed" 2>/dev/null
echo "B RAW marker files:"; ls -1 "$B/effects" 2>/dev/null
$BIN goal effects --effects-dir "$B" --expect 1
echo "B_GATE_EXIT=$? (NONZERO is the PASS here: the instrument caught the duplicate)"
$BIN goal effects --effects-dir "$B" --expect 1 --markers-only
echo "B_MARKER_GATE_EXIT=$? (0 — the OLD gate is green on the same double execution)"

# ---------------------------------------------------------------- C ----------
say "C. LIVE REPRODUCTION: fanout, process-tree kill mid-wave, restart"
C="$ROOT/c"; mkdir -p "$C"
J="$ROOT/fleet.journal"
G="g-b1"

env -u API_KEY -u FLUX_API_KEY $BIN goal open --journal "$J" --goal "$G" \
  --objective "prove exactly-once survives a real kill" \
  --iterations 8 --max-tokens 10000 || exit 1
for i in 0 1 2 3 4 5; do
  env -u API_KEY -u FLUX_API_KEY $BIN goal task --journal "$J" --goal "$G" --task "t0$i" || exit 1
done
for i in 0 1 2 3 4 5; do
  env -u API_KEY -u FLUX_API_KEY $BIN goal task --journal "$J" --goal "$G" \
    --task "t0$((i+6))" --depends-on "t0$i" || exit 1
done

echo "--- RUN 1, own process group, to be killed mid-wave"
setsid env -u API_KEY -u FLUX_API_KEY "$BIN" goal run --journal "$J" --goal "$G" \
  --effects-dir "$C" --worker-command "$ROOT/worker.sh" \
  --width 6 --shard-size 2 --lease 5s > "$ROOT/run1.log" 2>&1 &
RUNPID=$!
sleep 1
PGID=$(ps -o pgid= -p $RUNPID 2>/dev/null | tr -d ' ')
echo "RUN1 pid=$RUNPID pgid=$PGID"

# Kill only once the state we want is REAL: four effects on disk and the two
# long workers still running.
for _ in $(seq 1 200); do
  n=$(ls "$C/observed" 2>/dev/null | wc -l)
  [ "$n" -ge 5 ] && break
  sleep 0.2
done
echo "PRE-KILL observed=$(ls "$C/observed" 2>/dev/null | wc -l) markers=$(ls "$C/effects" 2>/dev/null | wc -l)"
echo "PRE-KILL sleep40_workers=$(pgrep -g "$PGID" -f 'sleep 40' | wc -l) pgid_procs=$(ps -eo pgid=,pid= | awk -v g="$PGID" '$1==g' | wc -l)"
echo "KILL_AT_UTC=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
kill -9 -"$PGID"
sleep 3
echo "POST-KILL pgid_procs=$(ps -eo pgid=,pid= | awk -v g="$PGID" '$1==g' | wc -l) sleep40_workers=$(pgrep -g "$PGID" -f 'sleep 40' | wc -l)"
echo -n "POST-KILL CENSUS: "; $BIN goal effects --effects-dir "$C"
echo "--- run1.log ---"; cat "$ROOT/run1.log"

echo "--- WAIT PAST THE 5s LEASE, THEN RESTART"
sleep 6
echo "RESTART_AT_UTC=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
env -u API_KEY -u FLUX_API_KEY "$BIN" goal run --journal "$J" --goal "$G" \
  --effects-dir "$C" --worker-command "$ROOT/worker.sh" \
  --width 6 --shard-size 2 --lease 60s > "$ROOT/run2.log" 2>&1
echo "RUN2_EXIT=$?"
echo "--- run2.log ---"; cat "$ROOT/run2.log"

say "C. THE GATE: exactly 12 REAL effects"
echo -n "C CENSUS: "; $BIN goal effects --effects-dir "$C"
$BIN goal effects --effects-dir "$C" --expect 12
echo "C_REAL_GATE_EXIT=$?"
$BIN goal effects --effects-dir "$C" --expect 12 --markers-only
echo "C_MARKER_GATE_EXIT=$?   (the OLD gate)"
echo "C per-task execution counts:"
cat "$C"/observed/* 2>/dev/null | sort | uniq -c | sort -rn

say "C. LEDGER STATE"
env -u API_KEY -u FLUX_API_KEY "$BIN" goal status --journal "$J" --goal "$G" > "$ROOT/status.json"
python3 - "$ROOT/status.json" <<'PY'
import json,sys
s=json.load(open(sys.argv[1]))
tasks=s["tasks"]
print("GOAL lifecycle=%s iterations=%s resume_count=%s tasks=%d"%(
  s["lifecycle"], s["iterations_started"], s["resume_count"], len(tasks)))
comp=sum(1 for t in tasks.values() if t.get("completion"))
att=sum(len(t.get("attempts",[])) for t in tasks.values())
unres=sum(1 for t in tasks.values()
          if t.get("attempts") and t["attempts"][-1]["status"]=="unknown")
print("LEDGER completed=%d attempts=%d unresolved=%d"%(comp,att,unres))
for k in sorted(tasks):
    t=tasks[k]
    print("  %s attempts=%d completed=%s"%(k,len(t.get("attempts",[])),bool(t.get("completion"))))
PY
say "DONE"
