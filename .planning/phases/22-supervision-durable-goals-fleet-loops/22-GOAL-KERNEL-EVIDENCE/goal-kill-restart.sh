#!/usr/bin/env bash
# Live durability exercise for the Phase 22 Goal kernel.
#
# Creates a real durable Goal in a real process, kills that process with an
# UNCATCHABLE SIGKILL while it holds the journal writer lease, and then proves a
# fresh process resumes the Goal from the ledger alone.
#
# Every verdict below is derived from process state and program output, never
# from a file this script itself wrote as its own evidence.
set -uo pipefail

BIN="${1:?usage: goal-kill-restart.sh <path-to-p22_goal_live>}"
WORK="$(mktemp -d)"
J="$WORK/session.journal"
GOAL="g-live-1"
FAIL=0

note() { printf '%s\n' "$*"; }
check() { # check <label> <condition-result>
  if [ "$2" -eq 0 ]; then note "PASS: $1"; else note "FAIL: $1"; FAIL=$((FAIL+1)); fi
}

note "=== leg 1: create a durable Goal, then SIGKILL the process holding it ==="
"$BIN" open "$J" "$GOAL" parent-v1 > "$WORK/open.log" 2>&1 &
OPEN_PID=$!

# Wait for the process to commit its transitions and signal readiness.
for _ in $(seq 1 100); do
  [ -f "$J.ready" ] && break
  sleep 0.1
done
if [ ! -f "$J.ready" ]; then
  note "FAIL: the writer never reached a committed state"; cat "$WORK/open.log"; exit 1
fi
KILLED_PID="$(cat "$J.ready")"
note "writer pid=$KILLED_PID committed its transitions"
grep -E '^GOAL-LIVE: (opened|pre_kill)' "$WORK/open.log"

KILL_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
kill -9 "$KILLED_PID"
note "kill -9 $KILLED_PID at $KILL_AT"
wait "$OPEN_PID" 2>/dev/null
sleep 0.5

# The process must actually be gone, and must not have exited cooperatively.
kill -0 "$KILLED_PID" 2>/dev/null; PROC_ALIVE=$?
check "the killed process is gone (uncatchable, no cooperative shutdown ran)" \
  "$([ "$PROC_ALIVE" -ne 0 ] && echo 0 || echo 1)"
grep -q 'GOAL-LIVE: ready_for_kill' "$WORK/open.log"; check "the writer was mid-flight when killed" $?

note ""
note "=== leg 2: a FRESH process resumes the Goal from the ledger ==="
"$BIN" resume "$J" "$GOAL" parent-v1 > "$WORK/resume.log" 2>&1
RESUME_RC=$?
cat "$WORK/resume.log"
check "the fresh process exited zero after a kill -9 left the writer lease behind" \
  "$([ "$RESUME_RC" -eq 0 ] && echo 0 || echo 1)"
grep -q 'GOAL-LIVE: RESUMED ' "$WORK/resume.log"; check "the Goal RESUMED rather than parking or vanishing" $?
grep -q 'GOAL-LIVE: RESUMED iterations=1 resumes=1 ' "$WORK/resume.log"
check "the iteration committed before the kill survived, and the resume was counted" $?
grep -q 'max_tokens=Some(500) max_cost_cents=Some(25)' "$WORK/resume.log"
check "the authority envelope was restored from the record, narrowed not widened" $?
grep -q 'objective="survive an uncatchable kill"' "$WORK/resume.log"
check "the objective replayed from the chain" $?

note ""
note "=== leg 3: kill it again — durability is not a one-shot property ==="
# A SECOND real SIGKILL against a process that genuinely holds the writer lease.
rm -f "$J.ready"
"$BIN" hold "$J" "$GOAL" parent-v1 > "$WORK/hold.log" 2>&1 &
HOLD_PID=$!
for _ in $(seq 1 100); do [ -f "$J.ready" ] && break; sleep 0.1; done
if [ ! -f "$J.ready" ]; then note "FAIL: the holder never took the lease"; cat "$WORK/hold.log"; exit 1; fi
HOLD_KILLED="$(cat "$J.ready")"
grep -q 'GOAL-LIVE: holding_lease' "$WORK/hold.log"; check "a second process really held the journal writer lease" $?
kill -9 "$HOLD_KILLED"
note "kill -9 $HOLD_KILLED at $(date -u +%Y-%m-%dT%H:%M:%SZ)"
wait "$HOLD_PID" 2>/dev/null
sleep 0.5
kill -0 "$HOLD_KILLED" 2>/dev/null; HOLD_ALIVE=$?
check "the second killed process is gone" "$([ "$HOLD_ALIVE" -ne 0 ] && echo 0 || echo 1)"

"$BIN" resume "$J" "$GOAL" parent-v1 > "$WORK/resume2.log" 2>&1
cat "$WORK/resume2.log"
grep -q 'GOAL-LIVE: RESUMED iterations=1 resumes=2 ' "$WORK/resume2.log"
check "a second crash-resume cycle increments the durable resume count to 2" $?

note ""
note "=== leg 4: a moved parent envelope PARKS instead of resuming permissively ==="
"$BIN" resume "$J" "$GOAL" parent-MOVED > "$WORK/parked.log" 2>&1
cat "$WORK/parked.log"
grep -q 'GOAL-LIVE: PARKED terminal=AuthorityUnreconstructable' "$WORK/parked.log"
check "a Goal whose parent envelope moved is parked, not resumed under a re-derived one" $?
# And the park is durable, not advisory: the next process sees it too.
"$BIN" resume "$J" "$GOAL" parent-v1 > "$WORK/after-park.log" 2>&1
grep -q 'GOAL-LIVE: ALREADY-TERMINAL terminal=AuthorityUnreconstructable' "$WORK/after-park.log"
check "the park survived into a fresh process, so it is durable not in-memory" $?

note ""
note "journal bytes on disk: $(wc -c < "$J")"
note "WORKDIR=$WORK"
note "GOAL-KERNEL-LIVE: failures=$FAIL"
[ "$FAIL" -eq 0 ]
