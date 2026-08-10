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
# Every effect namespace is GOAL-scoped now: `effects/<goal>/<key>`,
# `intents/<goal>/<key>`, `observed/<goal>/<task>/<invocation>`. Two Goals
# sharing one --effects-dir no longer share one namespace, so the boundary legs
# below have to name the Goal they run under and count with `find`, not `ls`.
GOAL=${GOAL:-g-b1-bnd}
rm -rf "$ROOT"; mkdir -p "$ROOT"
say() { echo; echo "=== $* ==="; }
census() { $BIN goal effects --effects-dir "$1" 2>/dev/null | grep GOAL-EFFECTS; }
state() {
  echo "    on-disk: intents=[$(find "$1/intents" -type f -printf '%f ' 2>/dev/null)] commits=[$(find "$1/effects" -type f -printf '%f ' 2>/dev/null)] observed=$(find "$1/observed" -type f 2>/dev/null | wc -l)"
}
# Where the scoping put the intent for a key under $GOAL. Discovered rather than
# recomputed: the scope component carries a digest so two different goal ids can
# never share a directory, and a script that guessed it would be guessing.
scoped_intent() { # dir key
  local scope
  scope=$(ls "$1/intents" 2>/dev/null | head -1)
  [ -z "$scope" ] && scope=$(ls "$1/effects" 2>/dev/null | head -1)
  echo "$1/intents/$scope/$2"
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
  env -u API_KEY -u FLUX_API_KEY WAYLAND_GOAL_ID="$GOAL" WAYLAND_GOAL_TASK="$t" WAYLAND_GOAL_IDEMPOTENCY_KEY="$k" \
    $BIN goal exec-task --effects-dir "$d" "$@" -- "$w" 2>&1 | grep -v 'crash sentinel'
  return "${PIPESTATUS[0]}"
}

# Start exec-task in its own process group and kill the WHOLE group after
# `after` seconds. A group kill, not a process kill: the worker is a
# grandchild and killing only the parent would leave it running.
kill_tree_after() { # dir task key worker after
  local d=$1 t=$2 k=$3 w=$4 after=$5
  setsid env -u API_KEY -u FLUX_API_KEY WAYLAND_GOAL_ID="$GOAL" WAYLAND_GOAL_TASK="$t" WAYLAND_GOAL_IDEMPOTENCY_KEY="$k" \
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
exec_task "$D" t-w1 idem-w1 "$ROOT/w-fast.sh"; echo "    RETRY_EXIT=$?  (90 = parked)"
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
exec_task "$D" t-w2 idem-w2 "$ROOT/w-early.sh"; echo "    RETRY_EXIT=$?  (90 = parked)"
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
exec_task "$D" t-w3 idem-w3 "$ROOT/w-fast.sh"; echo "    RETRY_EXIT=$?  (90 = parked)"
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
# A worker that DECLARES no effect landed. Both halves are required now: the
# out-of-band receipt AND the exit code. An exit code alone cannot withdraw an
# intent, because sysexits.h values are emitted by other tools by accident.
cat > "$ROOT/w-noeffect.sh" <<'EOF'
#!/bin/sh
printf 'no-effect\n' > "$WAYLAND_GOAL_NO_EFFECT_RECEIPT"
exit 91
EOF
# The collision case: the SAME exit code with no receipt, which is what a
# sendmail-style EX_PROTOCOL in the worker's pipeline looks like from outside.
# It must park, never withdraw.
cat > "$ROOT/w-collide.sh" <<'EOF'
#!/bin/sh
mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
printf '%s\n' "$WAYLAND_GOAL_TASK" > "$WAYLAND_GOAL_EFFECT_SINK/$WAYLAND_GOAL_TASK.$$.$(date +%s%N)"
exit 91
EOF
chmod +x "$ROOT/w-collide.sh"
chmod +x "$ROOT/w-killed.sh" "$ROOT/w-noeffect.sh"
D="$ROOT/w3b"; mkdir -p "$D"
exec_task "$D" t-w3b idem-w3b "$ROOT/w-killed.sh"; echo "    FIRST_EXIT=$?  (90 = parked, NOT retried)"
state "$D"; census "$D"
exec_task "$D" t-w3b idem-w3b "$ROOT/w-fast.sh"; echo "    RETRY_EXIT=$?  (90 = parked)"
state "$D"; census "$D"
$BIN goal effects --effects-dir "$D" --expect 1 >/dev/null 2>&1; echo "  W3b_GATE_EXIT=$? (0 = exactly one real effect)"

say "W3c  worker DECLARED no effect (receipt + exit 91): plainly retryable, not parked"
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
printf 'task=t-w4 pid=1\n' > "$(scoped_intent "$D" idem-w4)"
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
J="$ROOT/fleet.journal"; G="g-b1"   # the fleet leg runs under its OWN goal id
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
  [ "$(find "$C/observed" -type f 2>/dev/null | wc -l)" -ge 6 ] && break; sleep 0.2
done
echo "PRE-KILL observed=$(find "$C/observed" -type f 2>/dev/null|wc -l) commits=$(find "$C/effects" -type f 2>/dev/null|wc -l) intents=$(find "$C/intents" -type f 2>/dev/null|wc -l) sleep40=$(pgrep -g "$PGID" -f 'sleep 40'|wc -l)"
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
echo "per-task execution counts (one directory per task, one file per invocation):"
find "$C/observed" -mindepth 2 -maxdepth 2 -type d 2>/dev/null | while read -r d; do
  echo "  $(find "$d" -type f | wc -l)  $(basename "$d")"
done | sort -rn
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

############################################################### X1 ############
say "X1  FINDING 3: the SAME exit code with NO receipt must park, not withdraw"
echo "  91 is this product's 'no effect landed' code. A worker that emits it by"
echo "  accident -- which is what any sysexits-speaking tool in the pipeline does"
echo "  -- must NOT have its intent withdrawn, or the retry duplicates the effect."
echo "  That is exactly what 76 (=EX_PROTOCOL) did before this fix."
D="$ROOT/x1"; mkdir -p "$D"
exec_task "$D" t-x1 idem-x1 "$ROOT/w-collide.sh"; echo "    FIRST_EXIT=$?  (90 = parked)"
state "$D"
INTENTS=$(find "$D/intents" -type f 2>/dev/null | wc -l)
echo "  X1_INTENT_HELD=$INTENTS  (1 = the intent survived a bare exit code)"
exec_task "$D" t-x1 idem-x1 "$ROOT/w-fast.sh"; echo "    RETRY_EXIT=$?  (90 = parked, NOT re-run)"
census "$D"
$BIN goal effects --effects-dir "$D" --expect 1 >/dev/null 2>&1; echo "  X1_GATE_EXIT=$? (0 = exactly one real effect)"

say "X1b  and the DECLARED case still withdraws: receipt AND code together"
D="$ROOT/x1b"; mkdir -p "$D"
exec_task "$D" t-x1b idem-x1b "$ROOT/w-noeffect.sh"; echo "    FIRST_EXIT=$?  (1 = a plain, retryable failure)"
echo "  X1b_INTENT_HELD=$(find "$D/intents" -type f 2>/dev/null | wc -l)  (0 = withdrawn, as declared)"
exec_task "$D" t-x1b idem-x1b "$ROOT/w-fast.sh"; echo "    RETRY_EXIT=$?  (0 = it ran, as it must)"
$BIN goal effects --effects-dir "$D" --expect 1 >/dev/null 2>&1; echo "  X1b_GATE_EXIT=$? (0 = exactly one real effect)"

############################################################### X2 ############
say "X2  FINDING 1+2: two Goals, ONE --effects-dir, the same task names"
echo "  Before the scoping: goal B declined every task as already-committed and"
echo "  reported success having executed nothing, and a stale intent left by a"
echo "  killed goal A permanently parked a brand-new goal B."
D="$ROOT/x2"; mkdir -p "$D"
( GOAL=goal-a; exec_task "$D" deploy idem-deploy "$ROOT/w-fast.sh" ); echo "    GOAL_A_EXIT=$?"
( GOAL=goal-b; exec_task "$D" deploy idem-deploy "$ROOT/w-fast.sh" ); echo "    GOAL_B_EXIT=$?  (0 with produced=yes = B did its own work)"
census "$D"
$BIN goal effects --effects-dir "$D" --expect 2 >/dev/null 2>&1
echo "  X2_GATE_EXIT=$? (0 = TWO real effects, one per goal, no duplicate)"
echo "  -- and the intent half, the denial of service the fix itself opened:"
D="$ROOT/x2b"; mkdir -p "$D"
( GOAL=goal-a; exec_task "$D" deploy idem-deploy "$ROOT/w-killparent.sh" ) >/dev/null 2>&1
echo "    goal A killed mid-window; intents on disk: $(find "$D/intents" -type f 2>/dev/null | wc -l)"
( GOAL=goal-b; exec_task "$D" deploy idem-deploy "$ROOT/w-fast.sh" ); echo "    GOAL_B_EXIT=$?  (0 = B is not parked by A's corpse)"

############################################################### X3 ############
say "X3  FINDING 4: the instrument counts INVOCATIONS, not record contents"
echo "  A worker whose record carries an invocation identity -- task=... msg_id=..."
echo "  as any real effect log does -- used to make two executions of ONE task"
echo "  read as two DISTINCT effects and zero duplicates. The instrument went"
echo "  green on the very failure it exists to catch."
cat > "$ROOT/w-ident.sh" <<'EOF'
#!/bin/sh
mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
printf 'task=%s msg_id=%s\n' "$WAYLAND_GOAL_TASK" "$$-$(date +%s%N)" \
  > "$WAYLAND_GOAL_EFFECT_SINK/r.$$.$(date +%s%N)"
EOF
cat > "$ROOT/w-ident-kill.sh" <<'EOF'
#!/bin/sh
mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
printf 'task=%s msg_id=%s\n' "$WAYLAND_GOAL_TASK" "$$-$(date +%s%N)" \
  > "$WAYLAND_GOAL_EFFECT_SINK/r.$$.$(date +%s%N)"
kill -9 "$PPID"
sleep 5
EOF
chmod +x "$ROOT/w-ident.sh" "$ROOT/w-ident-kill.sh"
D="$ROOT/x3"; mkdir -p "$D"
exec_task "$D" t-p idem-p "$ROOT/w-ident-kill.sh" >/dev/null 2>&1
exec_task "$D" t-p idem-p "$ROOT/w-ident.sh" --resolve retry >/dev/null
echo "  the two records, byte-different by construction:"
find "$D/observed" -type f -exec cat {} \; | sed 's/^/    /'
echo "  distinct CONTENTS = $(find "$D/observed" -type f -exec cat {} \; | sort -u | wc -l)  (2 -- which is why content cannot be the identity)"
census "$D"
DUP=$($BIN goal effects --effects-dir "$D" 2>/dev/null | grep -o 'duplicates=[0-9]*' | cut -d= -f2)
echo "  X3_DUPLICATES=$DUP  (1 is the PASS: the instrument saw the duplicate)"
$BIN goal effects --effects-dir "$D" --expect 1 >/dev/null 2>&1
echo "  X3_GATE_EXIT=$? (NONZERO is the PASS: the gate went red on a real duplicate)"

############################################################### X4 ############
say "X4  FINDING 5: goal run --terminate over PARKED tasks"
echo "  The canonical terminal transition used to report"
echo "  PartiallyCompleted { completed: 0, failed: 0 } over four parked tasks --"
echo "  a terminal that structurally could not see the one state needing a human."
X4="$ROOT/x4"; mkdir -p "$X4"
XJ="$ROOT/x4.journal"; XG="g-b1-x4"
cat > "$ROOT/w-park.sh" <<'EOF'
#!/bin/sh
mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
printf '%s\n' "$WAYLAND_GOAL_TASK" > "$WAYLAND_GOAL_EFFECT_SINK/$WAYLAND_GOAL_TASK.$$.$(date +%s%N)"
exit 1
EOF
chmod +x "$ROOT/w-park.sh"
env -u API_KEY -u FLUX_API_KEY $BIN goal open --journal "$XJ" --goal "$XG" \
  --objective "park every task, then terminate" --iterations 4 --max-tokens 10000 >/dev/null
for i in 0 1 2 3; do
  env -u API_KEY -u FLUX_API_KEY $BIN goal task --journal "$XJ" --goal "$XG" --task "p0$i" >/dev/null
done
env -u API_KEY -u FLUX_API_KEY $BIN goal run --journal "$XJ" --goal "$XG" \
  --effects-dir "$X4" --worker-command "$ROOT/w-park.sh" --width 4 --shard-size 2 \
  --lease 60s --terminate 2>&1 | grep -E 'GOAL: (wave|unresolved|canonical|run_complete)'
echo "  X4_TERMINAL: the line above must NOT be PartiallyCompleted { completed: 0, failed: 0 }"
env -u API_KEY -u FLUX_API_KEY $BIN goal status --journal "$XJ" --goal "$XG" 2>/dev/null \
  | python3 -c 'import json,sys; s=json.load(sys.stdin); print("  X4_LIFECYCLE:", json.dumps(s["lifecycle"])[:400])'

say "DONE"
