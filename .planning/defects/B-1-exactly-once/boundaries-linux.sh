#!/bin/bash
# DEFECT B-1 — Linux GREEN proof: kill at every write boundary of the effect
# path, in turn, and show each resumes without duplicating.
#
# Boundaries, in the order the code passes them:
#   W1  intent recorded, worker started, effect NOT yet performed
#   W2  effect performed, worker still running
#   W3  worker exited, commit NOT yet written
#   W4  commit written, intent NOT yet withdrawn      (RECONSTRUCTED — see note)
#   W5  intent withdrawn — the clean path
# Then:
#   P   the instrument's positive control ON THIS BUILD (it must still go red)
#   C   the full fleet reproduction that was red before the fix
set -u
BIN=${BIN:-/root/b1/target/release/wayland-core}
ROOT=${ROOT:-/root/b1-green}
rm -rf "$ROOT"; mkdir -p "$ROOT"
say() { echo; echo "=== $* ==="; }
census() { $BIN goal effects --effects-dir "$1" 2>/dev/null | grep GOAL-EFFECTS; }
state() {
  echo "    on-disk: intents=[$(ls "$1/intents" 2>/dev/null | tr '\n' ' ')] commits=[$(ls "$1/effects" 2>/dev/null | tr '\n' ' ')] observed=$(ls "$1/observed" 2>/dev/null | wc -l)"
}

# effect LAST: killed early means the effect never happened.
cat > "$ROOT/w-late.sh" <<'EOF'
#!/bin/sh
sleep 3
mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
printf '%s\n' "$WAYLAND_GOAL_TASK" > "$WAYLAND_GOAL_EFFECT_SINK/$WAYLAND_GOAL_TASK.$$.$(date +%s%N)"
EOF
# effect FIRST, then keeps working: the ordinary shape of a real job.
cat > "$ROOT/w-early.sh" <<'EOF'
#!/bin/sh
mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
printf '%s\n' "$WAYLAND_GOAL_TASK" > "$WAYLAND_GOAL_EFFECT_SINK/$WAYLAND_GOAL_TASK.$$.$(date +%s%N)"
sleep 30
EOF
# effect, then an uncatchable kill of exec-task itself: dead between the
# worker's exit and the commit.
cat > "$ROOT/w-killparent.sh" <<'EOF'
#!/bin/sh
mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
printf '%s\n' "$WAYLAND_GOAL_TASK" > "$WAYLAND_GOAL_EFFECT_SINK/$WAYLAND_GOAL_TASK.$$.$(date +%s%N)"
kill -9 "$PPID"
sleep 5
EOF
# effect, fast exit: the clean path.
cat > "$ROOT/w-fast.sh" <<'EOF'
#!/bin/sh
mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
printf '%s\n' "$WAYLAND_GOAL_TASK" > "$WAYLAND_GOAL_EFFECT_SINK/$WAYLAND_GOAL_TASK.$$.$(date +%s%N)"
EOF
chmod +x "$ROOT"/w-*.sh

exec_task() { # dir task key worker [extra...]
  local d=$1 t=$2 k=$3 w=$4; shift 4
  env -u API_KEY -u FLUX_API_KEY WAYLAND_GOAL_TASK="$t" WAYLAND_GOAL_IDEMPOTENCY_KEY="$k" \
    $BIN goal exec-task --effects-dir "$d" "$@" -- "$w" 2>&1 | grep -v 'crash sentinel'
  return "${PIPESTATUS[0]}"
}

# Start exec-task in its own process group and kill the WHOLE group after
# `after` seconds. A group kill, not a process kill: the worker is a
# grandchild and killing only the parent would leave it running.
kill_tree_after() { # dir task key worker after
  local d=$1 t=$2 k=$3 w=$4 after=$5
  setsid env -u API_KEY -u FLUX_API_KEY WAYLAND_GOAL_TASK="$t" WAYLAND_GOAL_IDEMPOTENCY_KEY="$k" \
    $BIN goal exec-task --effects-dir "$d" -- "$w" >/dev/null 2>&1 &
  local pid=$! pgid
  sleep 0.4
  pgid=$(ps -o pgid= -p $pid 2>/dev/null | tr -d ' ')
  sleep "$after"
  echo "    KILL -9 on process group $pgid ($(ps -eo pgid= | grep -c "^ *$pgid\$") members)"
  kill -9 -"$pgid" 2>/dev/null
  wait $pid 2>/dev/null
}

say "BINARY"
$BIN --version; sha256sum "$BIN"

############################################################### W1 ############
say "W1  killed BEFORE the effect (intent recorded, worker running)"
D="$ROOT/w1"; mkdir -p "$D"
kill_tree_after "$D" t-w1 idem-w1 "$ROOT/w-late.sh" 1
state "$D"; census "$D"
echo "  -- the honest retry:"
exec_task "$D" t-w1 idem-w1 "$ROOT/w-fast.sh"; echo "    RETRY_EXIT=$?  (75 = parked)"
census "$D"
echo "  -- operator checks the sink, finds nothing, resolves as retry:"
exec_task "$D" t-w1 idem-w1 "$ROOT/w-fast.sh" --resolve retry; echo "    EXIT=$?"
state "$D"; census "$D"
$BIN goal effects --effects-dir "$D" --expect 1 >/dev/null 2>&1; echo "  W1_GATE_EXIT=$? (0 = exactly one real effect)"

############################################################### W2 ############
say "W2  killed AFTER the effect, worker still running"
D="$ROOT/w2"; mkdir -p "$D"
kill_tree_after "$D" t-w2 idem-w2 "$ROOT/w-early.sh" 1
state "$D"; census "$D"
echo "  -- the honest retry (this is where it used to duplicate):"
exec_task "$D" t-w2 idem-w2 "$ROOT/w-early.sh"; echo "    RETRY_EXIT=$?  (75 = parked)"
state "$D"; census "$D"
echo "  -- operator checks the sink, finds the effect, resolves as produced:"
exec_task "$D" t-w2 idem-w2 "$ROOT/w-early.sh" --resolve produced; echo "    EXIT=$?"
state "$D"; census "$D"
$BIN goal effects --effects-dir "$D" --expect 1 >/dev/null 2>&1; echo "  W2_GATE_EXIT=$? (0 = exactly one real effect)"

############################################################### W3 ############
say "W3  killed AFTER the worker exited, BEFORE the commit"
D="$ROOT/w3"; mkdir -p "$D"
exec_task "$D" t-w3 idem-w3 "$ROOT/w-killparent.sh"; echo "    FIRST_EXIT=$? (killed)"
state "$D"; census "$D"
echo "  -- the honest retry:"
exec_task "$D" t-w3 idem-w3 "$ROOT/w-fast.sh"; echo "    RETRY_EXIT=$?  (75 = parked)"
state "$D"; census "$D"
exec_task "$D" t-w3 idem-w3 "$ROOT/w-fast.sh" --resolve produced >/dev/null; echo "    RESOLVE_EXIT=$?"
$BIN goal effects --effects-dir "$D" --expect 1 >/dev/null 2>&1; echo "  W3_GATE_EXIT=$? (0 = exactly one real effect)"

############################################################### W3b ###########
say "W3b  worker died AFTER its effect with an UNDECLARED nonzero exit"
echo "  What a killed worker looks like from outside. The first version of this"
echo "  fix read nonzero as 'no effect landed', withdrew the intent, and let the"
echo "  retry duplicate. Windows caught it; it was equally wrong here."
cat > "$ROOT/w-killed.sh" <<'EOF'
#!/bin/sh
mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
printf '%s\n' "$WAYLAND_GOAL_TASK" > "$WAYLAND_GOAL_EFFECT_SINK/$WAYLAND_GOAL_TASK.$$.$(date +%s%N)"
exit 1
EOF
cat > "$ROOT/w-noeffect.sh" <<'EOF'
#!/bin/sh
exit 76
EOF
chmod +x "$ROOT/w-killed.sh" "$ROOT/w-noeffect.sh"
D="$ROOT/w3b"; mkdir -p "$D"
exec_task "$D" t-w3b idem-w3b "$ROOT/w-killed.sh"; echo "    FIRST_EXIT=$?  (75 = parked, NOT retried)"
state "$D"; census "$D"
exec_task "$D" t-w3b idem-w3b "$ROOT/w-fast.sh"; echo "    RETRY_EXIT=$?  (75 = parked)"
state "$D"; census "$D"
$BIN goal effects --effects-dir "$D" --expect 1 >/dev/null 2>&1; echo "  W3b_GATE_EXIT=$? (0 = exactly one real effect)"

say "W3c  worker DECLARED no effect (exit 76): plainly retryable, not parked"
D="$ROOT/w3c"; mkdir -p "$D"
exec_task "$D" t-w3c idem-w3c "$ROOT/w-noeffect.sh"; echo "    FIRST_EXIT=$?  (1 = a plain failure)"
state "$D"
exec_task "$D" t-w3c idem-w3c "$ROOT/w-fast.sh"; echo "    RETRY_EXIT=$?  (0 = it ran, as it must)"
state "$D"; census "$D"
$BIN goal effects --effects-dir "$D" --expect 1 >/dev/null 2>&1; echo "  W3c_GATE_EXIT=$? (0 = exactly one real effect)"

############################################################### W4 ############
say "W4  commit written, intent NOT yet withdrawn  [RECONSTRUCTED STATE]"
echo "  A kill in the two-syscall gap between the commit's fsync and the"
echo "  intent's unlink cannot be aimed at from outside the process, so this"
echo "  leg RE-CREATES that on-disk state rather than racing for it. The"
echo "  decision under test is 'commit beats leftover intent'."
D="$ROOT/w4"; mkdir -p "$D"
exec_task "$D" t-w4 idem-w4 "$ROOT/w-fast.sh" >/dev/null
printf 'task=t-w4 pid=1\n' > "$D/intents/idem-w4"
state "$D"; census "$D"
exec_task "$D" t-w4 idem-w4 "$ROOT/w-fast.sh"; echo "    RETRY_EXIT=$?"
state "$D"; census "$D"
$BIN goal effects --effects-dir "$D" --expect 1 >/dev/null 2>&1; echo "  W4_GATE_EXIT=$? (0 = exactly one real effect)"

############################################################### W5 ############
say "W5  the clean path: commit written, intent withdrawn"
D="$ROOT/w5"; mkdir -p "$D"
exec_task "$D" t-w5 idem-w5 "$ROOT/w-fast.sh"
exec_task "$D" t-w5 idem-w5 "$ROOT/w-fast.sh"
state "$D"; census "$D"
$BIN goal effects --effects-dir "$D" --expect 1 >/dev/null 2>&1; echo "  W5_GATE_EXIT=$? (0 = exactly one real effect)"

############################################################### P #############
say "P  INSTRUMENT POSITIVE CONTROL ON THIS BUILD"
echo "  A fixed product must not make the instrument unfalsifiable. An operator"
echo "  who resolves as 'retry' when the effect DID land produces a real second"
echo "  execution; the counter must report it."
D="$ROOT/p"; mkdir -p "$D"
exec_task "$D" t-p idem-p "$ROOT/w-killparent.sh" >/dev/null 2>&1
exec_task "$D" t-p idem-p "$ROOT/w-fast.sh" --resolve retry >/dev/null
census "$D"
$BIN goal effects --effects-dir "$D" --expect 1 >/dev/null 2>&1
echo "  P_GATE_EXIT=$? (NONZERO is the PASS: the instrument still catches a duplicate)"

############################################################### C #############
say "C  FULL FLEET REPRODUCTION: process-tree kill mid-wave, then restart"
C="$ROOT/c"; mkdir -p "$C"
J="$ROOT/fleet.journal"; G="g-b1"
cat > "$ROOT/worker.sh" <<'EOF'
#!/bin/sh
mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
printf '%s\n' "$WAYLAND_GOAL_TASK" > "$WAYLAND_GOAL_EFFECT_SINK/$WAYLAND_GOAL_TASK.$$.$(date +%s%N)"
case "$WAYLAND_GOAL_TASK" in
  t00|t01) sleep 1 ;; t02|t03) sleep 2 ;; t04|t05) sleep 40 ;; *) sleep 1 ;;
esac
EOF
chmod +x "$ROOT/worker.sh"

env -u API_KEY -u FLUX_API_KEY $BIN goal open --journal "$J" --goal "$G" \
  --objective "prove exactly-once survives a real kill" --iterations 8 --max-tokens 10000 >/dev/null || exit 1
for i in 0 1 2 3 4 5; do
  env -u API_KEY -u FLUX_API_KEY $BIN goal task --journal "$J" --goal "$G" --task "t0$i" >/dev/null || exit 1
done
for i in 0 1 2 3 4 5; do
  env -u API_KEY -u FLUX_API_KEY $BIN goal task --journal "$J" --goal "$G" \
    --task "t0$((i+6))" --depends-on "t0$i" >/dev/null || exit 1
done

setsid env -u API_KEY -u FLUX_API_KEY "$BIN" goal run --journal "$J" --goal "$G" \
  --effects-dir "$C" --worker-command "$ROOT/worker.sh" \
  --width 6 --shard-size 2 --lease 5s > "$ROOT/run1.log" 2>&1 &
RUNPID=$!; sleep 1
PGID=$(ps -o pgid= -p $RUNPID 2>/dev/null | tr -d ' ')
for _ in $(seq 1 200); do
  [ "$(ls "$C/observed" 2>/dev/null | wc -l)" -ge 6 ] && break; sleep 0.2
done
echo "PRE-KILL observed=$(ls "$C/observed" 2>/dev/null|wc -l) commits=$(ls "$C/effects" 2>/dev/null|wc -l) intents=$(ls "$C/intents" 2>/dev/null|wc -l) sleep40=$(pgrep -g "$PGID" -f 'sleep 40'|wc -l)"
echo "KILL_AT_UTC=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
kill -9 -"$PGID"; sleep 3
echo "POST-KILL pgid_procs=$(ps -eo pgid=,pid= | awk -v g="$PGID" '$1==g'|wc -l)"
echo -n "POST-KILL "; census "$C"
sleep 6
env -u API_KEY -u FLUX_API_KEY "$BIN" goal run --journal "$J" --goal "$G" \
  --effects-dir "$C" --worker-command "$ROOT/worker.sh" \
  --width 6 --shard-size 2 --lease 60s > "$ROOT/run2.log" 2>&1
echo "RUN2_EXIT=$?"
echo "--- run2.log ---"; grep -v 'crash sentinel' "$ROOT/run2.log"

say "C  RESULT"
census "$C"
echo "per-task execution counts:"; cat "$C"/observed/* 2>/dev/null | sort | uniq -c | sort -rn
DUP=$($BIN goal effects --effects-dir "$C" 2>/dev/null | grep -o 'duplicates=[0-9]*' | cut -d= -f2)
echo "C_DUPLICATES=$DUP   (0 is the PASS; it was 4 before the fix)"
env -u API_KEY -u FLUX_API_KEY "$BIN" goal status --journal "$J" --goal "$G" > "$ROOT/status.json" 2>/dev/null
python3 - "$ROOT/status.json" <<'PY'
import json,sys
s=json.load(open(sys.argv[1])); tasks=s["tasks"]
comp=sum(1 for t in tasks.values() if t.get("completion"))
# NOTE: attempts[-1]["status"] is the tagged enum OBJECT, not a string. The
# 22-03 evidence script compared it to "unknown" and could therefore never
# report a parked task -- a third vacuity in the same proof.
unres=[k for k,t in sorted(tasks.items())
       if t.get("attempts") and t["attempts"][-1]["status"]["status"]=="unknown"]
print("LEDGER completed=%d unresolved=%d %s"%(comp,len(unres),unres))
print("RESIDUAL: those %d tasks are parked. The ledger has no transition that"%len(unres))
print("un-parks them, so they and their dependents cannot be driven to completion")
print("through the product today. That is the named open half of this defect.")
PY
say "DONE"
