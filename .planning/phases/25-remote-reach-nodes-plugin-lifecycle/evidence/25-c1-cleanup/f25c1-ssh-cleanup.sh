#!/bin/bash
# 25-c1, Gap 1b: does the ssh backend leave its task root — and the task's own
# input bytes — on the far end when the task FAILS?
#
# Runs ON hetzner-dsm. The far end is a dedicated container (f25c1-sshd, port
# 2226) reached over a real ssh transport, so /tmp on the far end is a
# filesystem the controller does not share and the count is unambiguous.
#
# Anti-self-passing design, because "zero roots" is the single easiest claim to
# pass without doing any work:
#   * the counter is proved ALIVE in every phase by a planted decoy root, and
#     proved able to return ZERO by removing that decoy and re-reading;
#   * the far-end listing is fenced by LIST-BEGIN/LIST-END markers, so a failed
#     ssh reads as NOT-MEASURED rather than as a clean zero;
#   * the failing task writes a WITNESS on the far end from inside its own body,
#     so a phase reporting zero roots must still prove the task ran there. A fix
#     that "cleans up" by never running would be caught by that line alone.
set -u

PHASE="${1:?usage: f25c1-ssh-cleanup.sh <BASE|FIXED> <worktree>}"
TREE="${2:?usage: f25c1-ssh-cleanup.sh <BASE|FIXED> <worktree>}"
BIN="$TREE/target/release/wayland-core"

ROOT=/root/f25c1
OUT=/root/f25c1-evidence
mkdir -p "$OUT"
SSHCFG="$ROOT/ssh_config"

export WAYLAND_EXEC_SSH_CONFIG="$SSHCFG"
export WAYLAND_EXEC_SSH_TARGET=f25c1-far
export WAYLAND_EXEC_BACKEND_STATE_DIR=/root/f25c1-state
mkdir -p "$WAYLAND_EXEC_BACKEND_STATE_DIR"

FAILNONCE=f25c1fail
OKNONCE=f25c1ok
WITNESS=/tmp/f25c1-task-ran-witness.txt

far() { ssh -F "$SSHCFG" f25c1-far "$@"; }

# Raw far-end listing between two markers. Prints the listing on stdout; a
# missing END marker means the read did not complete and the caller must report
# NOT MEASURED rather than a number.
list_roots_raw() {
  far 'echo F25C1-LIST-BEGIN; ls -1d /tmp/wayland-f25-* 2>/dev/null; echo F25C1-LIST-END' 2>/dev/null
}

# count_roots <substring> -> prints an integer, or the literal NOT-MEASURED
count_roots() {
  local needle="$1" raw
  raw=$(list_roots_raw)
  case "$raw" in
    *F25C1-LIST-END*) : ;;
    *) echo NOT-MEASURED; return ;;
  esac
  printf '%s\n' "$raw" \
    | sed -n '/F25C1-LIST-BEGIN/,/F25C1-LIST-END/p' \
    | grep -v -e F25C1-LIST-BEGIN -e F25C1-LIST-END \
    | grep -c -F -- "$needle"
}

task_json() { # $1 = path, $2 = nonce, $3 = task_id, $4 = argv json
  cat > "$1" <<EOF
{
  "task_id": "$3",
  "nonce": "$2",
  "workspace": [{"path": "README.txt", "bytes": "ZjI1YzEgd29ya3NwYWNlCg=="}],
  "input": "$INPUT_B64",
  "argv": $4,
  "artifact_name": "stdout.bin",
  "resources": {"cpu_millis": 30000, "memory_bytes": 268435456, "wall_time_ms": 60000, "output_bytes": 1048576}
}
EOF
}

INPUT_PLAIN='wayland-f25-c1-INPUT-BYTES-THAT-MUST-NOT-BE-LEFT-BEHIND'
INPUT_B64=$(printf '%s\n' "$INPUT_PLAIN" | base64 -w0)

{
echo "=== F25C1 SSH CLEANUP — PHASE $PHASE ==="
date -u +%Y-%m-%dT%H:%M:%SZ
echo "controller: $(hostname)"
echo "tree:       $TREE"
echo "tree HEAD:  $(/usr/bin/git -C "$TREE" rev-parse HEAD 2>&1)"
echo "binary:     $BIN"
echo "binary sha: $(sha256sum "$BIN" 2>&1 | awk '{print $1}')"
echo "binary mtime: $(stat -c %y "$BIN" 2>&1)"
echo "far end:    $(far 'hostname; uname -s' 2>&1 | tr '\n' ' ')"
# WHICH runner is compiled into THIS binary, read out of the binary itself
# rather than assumed from the directory name. `|| status=$?` exists only in
# the fixed runner; `rm -rf "$root"` exists in both, so it is the known-positive
# that proves this reader can find the script at all.
echo "runner shape compiled into this binary (1 = present, 0 = absent):"
echo "  fixed-shape  '|| status=\$?'  : $(grep -a -c -F 'wait "$child" || status=$?' "$BIN")"
echo "  known-positive 'rm -rf \$root': $(grep -a -c -F 'rm -rf "$root"' "$BIN")"
echo

echo "--- 0. start from a clean far end"
far 'rm -rf /tmp/wayland-f25-* ; rm -f '"$WITNESS" 2>/dev/null
echo "roots carrying wayland-f25 after purge: $(count_roots wayland-f25)"

echo
echo "--- 1. INSTRUMENT ALIVENESS (a counter that cannot say 1 cannot say 0 either)"
far 'mkdir -p /tmp/wayland-f25-DECOY && printf decoy > /tmp/wayland-f25-DECOY/input.bin'
DECOY_COUNT=$(count_roots wayland-f25-DECOY)
echo "F25C1-INSTRUMENT-POSITIVE: decoy roots counted = $DECOY_COUNT (expect 1)"
far 'rm -rf /tmp/wayland-f25-DECOY'
DECOY_GONE=$(count_roots wayland-f25-DECOY)
echo "F25C1-INSTRUMENT-NEGATIVE: decoy roots after removal = $DECOY_GONE (expect 0)"
if [ "$DECOY_COUNT" != "1" ] || [ "$DECOY_GONE" != "0" ]; then
  echo "F25C1-VERDICT-$PHASE: NOT MEASURED — the far-end counter is not alive in both directions"
  exit 2
fi

echo
echo "--- 2. THE FAILING TASK (exit 7), through the shipped binary"
task_json /root/f25c1-fail-task.json "$FAILNONCE" f25c1fail \
  '["sh","-c","echo \"$WAYLAND_TASK_NONCE ran on $(hostname) at $(date -u +%FT%TZ)\" > '"$WITNESS"'; exit 7"]'
cat /root/f25c1-fail-task.json
rm -f "$OUT/receipt-fail-$PHASE.json"
"$BIN" backend run --backend ssh --task /root/f25c1-fail-task.json \
  --receipt-out "$OUT/receipt-fail-$PHASE.json" 2>&1 | sed 's/^/  /'
echo "RUN_EXIT=${PIPESTATUS[0]}"

echo
echo "--- 3. DID THE TASK ACTUALLY RUN ON THE FAR END?"
echo "    (a zero root count means nothing if the task never ran there)"
far "cat $WITNESS 2>&1" | sed 's/^/  witness: /'
WITNESS_LINES=$(far "cat $WITNESS 2>/dev/null | grep -c -F $FAILNONCE" 2>/dev/null)
echo "F25C1-TASK-RAN-ON-FAR-END: witness lines carrying the nonce = ${WITNESS_LINES:-NOT-MEASURED}"

echo
echo "--- 4. WHAT THE FAILING TASK LEFT BEHIND"
far 'echo F25C1-LIST-BEGIN; ls -1d /tmp/wayland-f25-* 2>/dev/null; echo F25C1-LIST-END' 2>&1 | sed 's/^/  /'
LEAK=$(count_roots "wayland-f25-$FAILNONCE")
echo "F25C1-LEAKED-ROOTS-$PHASE: $LEAK"
echo "  contents of any leaked root:"
far "ls -la /tmp/wayland-f25-$FAILNONCE 2>&1; echo '--- input.bin ---'; cat /tmp/wayland-f25-$FAILNONCE/input.bin 2>&1" | sed 's/^/  /'
LEAKED_INPUT=$(far "cat /tmp/wayland-f25-$FAILNONCE/input.bin 2>/dev/null | grep -c -F -- '$INPUT_PLAIN'" 2>/dev/null)
echo "F25C1-LEAKED-INPUT-BYTES-$PHASE: ${LEAKED_INPUT:-0} (1 = the task's own input is readable on the far end)"

echo
echo "--- 5. the receipt the product wrote for that failing task"
if [ -f "$OUT/receipt-fail-$PHASE.json" ]; then
  "$BIN" backend receipt verify "$OUT/receipt-fail-$PHASE.json" 2>&1 | sed 's/^/  /'
  grep -o '"terminal":{[^}]*}' "$OUT/receipt-fail-$PHASE.json" | sed 's/^/  /'
else
  echo "  no receipt written"
fi

echo
echo "--- 6. THE SUCCEEDING TASK (exit 0) — the same counter must read zero"
task_json /root/f25c1-ok-task.json "$OKNONCE" f25c1ok '["sh","-c","exit 0"]'
rm -f "$OUT/receipt-ok-$PHASE.json"
"$BIN" backend run --backend ssh --task /root/f25c1-ok-task.json \
  --receipt-out "$OUT/receipt-ok-$PHASE.json" 2>&1 | sed 's/^/  /'
OKLEAK=$(count_roots "wayland-f25-$OKNONCE")
echo "F25C1-SUCCESS-PATH-ROOTS-$PHASE: $OKLEAK (expect 0 in every phase)"

echo
echo "--- 7. post-run orphan scan through the product's own surface"
"$BIN" backend scan --task-id f25c1fail --nonce "$FAILNONCE" 2>&1 | sed 's/^/  /'
echo "SCAN_EXIT=${PIPESTATUS[0]}"

echo
echo "=== F25C1-SUMMARY-$PHASE ==="
echo "F25C1-$PHASE-INSTRUMENT: positive=$DECOY_COUNT negative=$DECOY_GONE"
echo "F25C1-$PHASE-TASK-RAN-ON-FAR-END: ${WITNESS_LINES:-NOT-MEASURED}"
echo "F25C1-$PHASE-LEAKED-ROOTS-AFTER-FAILING-TASK: $LEAK"
echo "F25C1-$PHASE-LEAKED-INPUT-BYTES: ${LEAKED_INPUT:-0}"
echo "F25C1-$PHASE-ROOTS-AFTER-SUCCEEDING-TASK: $OKLEAK"
date -u +%Y-%m-%dT%H:%M:%SZ
} 2>&1 | tee "$OUT/ssh-cleanup-$PHASE.txt"
