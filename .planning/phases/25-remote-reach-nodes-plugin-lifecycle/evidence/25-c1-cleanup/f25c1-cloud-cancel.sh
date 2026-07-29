#!/bin/bash
# 25-c1, Gap 1a: CANCEL a task on the hibernating cloud backend, live.
#
# Criterion 1 names cancellation as one of the four properties that must be
# equivalent across the four surfaces. It was proven on local, container and
# ssh; on cloud it was never driven at any commit. This drives it.
#
# Runs ON hetzner-dsm, against the real vendor API, with the credential Sean
# minted on 2026-07-28. The token is sourced from a 0600 file into the
# environment and is NEVER echoed, never placed on an argv, never written to any
# capture: every command that needs it reads it from the environment.
#
# Anti-self-passing design:
#   * the machine census is proved ALIVE by reading 1 while the task is running
#     and 0 only afterwards, from the SAME two instruments;
#   * two INDEPENDENT instruments are used — the product's own
#     `backend orphans` surface, and a raw vendor API call that does not go
#     through the product at all;
#   * the cancellation is issued only after the machine has been observed
#     SUSPENDED and then STARTED again, so what is cancelled is a task on a
#     genuinely hibernated-and-resumed machine, not a task on a booting one.
set -u
set +x

PHASE="${1:?usage: f25c1-cloud-cancel.sh <PHASE> <worktree>}"
TREE="${2:?usage: f25c1-cloud-cancel.sh <PHASE> <worktree>}"
BIN="$TREE/target/release/wayland-core"
OUT=/root/f25c1-evidence
mkdir -p "$OUT"

# shellcheck disable=SC1091
set -a
. /root/.wayland-f25-cloud.env
set +a
# The env file's ORG value is `sean-donahoe`, Sean's PERSONAL org. Every live
# run in this phase scopes to the dedicated app instead — see
# `/root/f25-cloud/live-cloud.sh` and 25-CLOUD-SUMMARY.md §"app-scoped, not
# org-scoped". Sourcing the file without this override probes a nonexistent app
# and the backend answers HTTP 404 `app not found`, which reads exactly like a
# dead credential and is not one.
export WAYLAND_F25_CLOUD_ORG=wayland-f25-test

export WAYLAND_EXEC_BACKEND_STATE_DIR=/root/f25c1-state
mkdir -p "$WAYLAND_EXEC_BACKEND_STATE_DIR"

NONCE=f25c1cancel
TASKID=f25c1cancel
APP="${WAYLAND_F25_CLOUD_ORG:-}"

# An INDEPENDENT census: the vendor's own machine list for this nonce, read
# without going through the product. Prints one line per machine, id + state
# only, so nothing sensitive can reach the capture.
#
# The token reaches curl on STDIN, via `--config -`, never on an argv: a
# `-H "Authorization: Bearer <token>"` would be readable in the host's own
# process table for the life of the call.
vendor_raw() {
  printf 'header = "Authorization: Bearer %s"\n' "${WAYLAND_F25_CLOUD_TOKEN}" \
  | curl -s --max-time 30 --config - \
      "https://api.machines.dev/v1/apps/${APP}/machines?metadata.wayland_task_nonce=${NONCE}"
}
vendor_census() {
  vendor_raw | tr ',' '\n' | grep -E '"(id|state)":' | tr '\n' ' '
  echo
}
vendor_count() {
  vendor_raw | grep -o '"id":"[0-9a-f]*"' | sort -u | wc -l
}
product_cloud_row() { "$BIN" backend orphans --nonce "$NONCE" 2>&1 | grep '^cloud'; }

{
echo "=== F25C1 CLOUD CANCELLATION — PHASE $PHASE ==="
date -u +%Y-%m-%dT%H:%M:%SZ
echo "controller:   $(hostname)"
echo "tree:         $TREE"
echo "tree HEAD:    $(/usr/bin/git -C "$TREE" rev-parse HEAD 2>&1)"
echo "binary sha:   $(sha256sum "$BIN" | awk '{print $1}')"
echo "credential:   sourced from /root/.wayland-f25-cloud.env (values never printed)"
# WHICH cloud arm is compiled into THIS binary, read out of the binary.
#
# INSTRUMENT REPAIR, 2026-07-29: the first version of this line searched for
# `the cancellation destroyed the machine` — a phrase that appears in NO commit
# of this repository. It therefore printed 0 for the pre-fix binary and 0 for
# the fixed one, i.e. it could not distinguish the thing it existed to
# distinguish. It gated nothing (the live receipt is the real proof), but a dud
# reader left in place is a dud reader the next lane trusts, so it is repaired
# here rather than written up. The needle below is the literal the fixed arm
# actually puts in the binary, and the two controls prove the reader works.
echo "cloud cancel-receipt arm compiled into this binary (1 = present):"
echo "  fixed-arm literal      : $(grep -a -c -F 'this receipt does not claim one' "$BIN")"
echo "  known-positive literal : $(grep -a -c -F 'machine destroy, then the vendor' "$BIN") (in every build)"
echo "  the OLD dud needle     : $(grep -a -c -F 'the cancellation destroyed the machine' "$BIN") (0 in every build — that was the defect)"

echo
echo "--- 0. is the credential live? Read the answer back from the PRODUCT."
"$BIN" backend probe cloud 2>&1 | sed 's/^/  /'
PROBE=$("$BIN" backend probe cloud 2>&1)
if ! printf '%s' "$PROBE" | grep -q "available:     true"; then
  echo "F25C1-CLOUD-CANCEL-$PHASE: BLOCKED — the cloud backend is not available; nothing was driven."
  printf '%s\n' "$PROBE"
  exit 3
fi
printf '%s' "$PROBE" | grep -q "VendorApiCall" \
  && echo "  probe basis read back from the product: VendorApiCall (a real authenticated call, not an env guess)"

echo
echo "--- 1. census BEFORE, both instruments"
echo "  product : $(product_cloud_row)"
echo "  vendor  : count=$(vendor_count) $(vendor_census)"

cat > /root/f25c1-cloud-task.json <<EOF
{
  "task_id": "$TASKID",
  "nonce": "$NONCE",
  "workspace": [{"path": "README.txt", "bytes": "ZjI1YzEgY2FuY2VsCg=="}],
  "input": "d2F5bGFuZC1mMjUtYzEtY2xvdWQtY2FuY2VsLWlucHV0Cg==",
  "argv": ["sleep", "120"],
  "artifact_name": "stdout.bin",
  "resources": {"cpu_millis": 30000, "memory_bytes": 268435456, "wall_time_ms": 60000, "output_bytes": 1048576}
}
EOF
echo
echo "--- 2. the task"
cat /root/f25c1-cloud-task.json

echo
echo "--- 3. start it through the shipped binary, in the background"
rm -f "$OUT/receipt-cloud-cancel-$PHASE.json" "$OUT/run-cloud-cancel-$PHASE.txt"
"$BIN" backend run --backend cloud --task /root/f25c1-cloud-task.json \
  --receipt-out "$OUT/receipt-cloud-cancel-$PHASE.json" > "$OUT/run-cloud-cancel-$PHASE.txt" 2>&1 &
RUNPID=$!
echo "run pid: $RUNPID"

echo
echo "--- 4. watch the vendor's own machine state, and cancel only AFTER a resume"
SEEN_SUSPENDED=0
SEEN_RESUMED=0
MACHINE_SEEN=0
for i in $(seq 1 60); do
  CENSUS=$(vendor_census)
  echo "  t+$((i*2))s  $CENSUS"
  case "$CENSUS" in
    *'"id"'*) MACHINE_SEEN=1 ;;
  esac
  # BOTH spellings. The first run of this script watched only for
  # `"state":"suspended"` and never fired: sampled every 2s, the machine reads
  # `suspending` on the way down and is back to `started` by the next sample.
  # A trigger that cannot observe the transition it waits for is the same
  # self-passing shape as a gate that cannot fail.
  case "$CENSUS" in
    *'"state":"suspended"'* | *'"state":"suspending"'*) SEEN_SUSPENDED=1 ;;
  esac
  if [ "$SEEN_SUSPENDED" = 1 ]; then
    case "$CENSUS" in
      *'"state":"started"'*) SEEN_RESUMED=1 ;;
    esac
  fi
  if [ "$SEEN_RESUMED" = 1 ]; then
    echo "  -> machine has been SUSPENDED and then STARTED again; letting the task settle into the exec, then cancelling"
    sleep 6
    break
  fi
  if ! kill -0 "$RUNPID" 2>/dev/null; then
    echo "  -> the run finished before a resume was observed"
    break
  fi
  sleep 2
done
echo "F25C1-MACHINE-OBSERVED-$PHASE: seen=$MACHINE_SEEN suspended=$SEEN_SUSPENDED resumed=$SEEN_RESUMED"

echo
echo "--- 5. census WHILE THE TASK IS LIVE — this is the census instruments' known-positive"
LIVE_PRODUCT=$(product_cloud_row)
LIVE_VENDOR=$(vendor_count)
echo "  product : $LIVE_PRODUCT"
echo "  vendor  : count=$LIVE_VENDOR"
echo "F25C1-CENSUS-ALIVE-$PHASE: vendor_count_while_running=$LIVE_VENDOR (0 here would mean both later zeros prove nothing)"
echo "  the machine's own event types, read from the vendor BEFORE the cancel destroys the record:"
vendor_raw | grep -o '"type":"[a-z_]*"' | sed 's/^/    /'
vendor_raw > "$OUT/vendor-machine-record-$PHASE.json"
echo "  full record saved to $OUT/vendor-machine-record-$PHASE.json"

echo
echo "--- 6. CANCEL, from a SECOND process"
"$BIN" backend cancel --task-id "$TASKID" --backend cloud 2>&1 | sed 's/^/  /'
echo "CANCEL_EXIT=${PIPESTATUS[0]}"

echo
echo "--- 7. what the run process did"
for i in $(seq 1 40); do
  if ! kill -0 "$RUNPID" 2>/dev/null; then echo "  run exited after ~$((i*3))s of waiting"; break; fi
  echo "  waiting for the run to return: $i"
  sleep 3
done
wait "$RUNPID" 2>/dev/null
echo "RUN_AFTER_CANCEL_EXIT=$?"
echo "  --- run output ---"
sed 's/^/  /' "$OUT/run-cloud-cancel-$PHASE.txt" 2>/dev/null

echo
echo "--- 8. did a RECEIPT survive the cancellation?"
if [ -f "$OUT/receipt-cloud-cancel-$PHASE.json" ]; then
  "$BIN" backend receipt verify "$OUT/receipt-cloud-cancel-$PHASE.json" 2>&1 | sed 's/^/  /'
  grep -o '"terminal":{[^}]*}' "$OUT/receipt-cloud-cancel-$PHASE.json" | sed 's/^/  /'
  echo "F25C1-CLOUD-CANCEL-RECEIPT-$PHASE: WRITTEN"
else
  echo "  no receipt file was written"
  echo "F25C1-CLOUD-CANCEL-RECEIPT-$PHASE: ABSENT"
fi

echo
echo "--- 9. census AFTER, both instruments"
sleep 5
POST_PRODUCT=$(product_cloud_row)
POST_VENDOR=$(vendor_count)
echo "  product : $POST_PRODUCT"
echo "  vendor  : count=$POST_VENDOR $(vendor_census)"
echo
"$BIN" backend scan --task-id "$TASKID" --nonce "$NONCE" 2>&1 | sed -n '/backend    cloud/,/rows/p' | sed 's/^/  /'
echo "SCAN_EXIT=${PIPESTATUS[0]}"

echo
echo "=== F25C1-CLOUD-SUMMARY-$PHASE ==="
echo "F25C1-$PHASE-MACHINE-SEEN: $MACHINE_SEEN   SUSPENDED: $SEEN_SUSPENDED   RESUMED: $SEEN_RESUMED"
echo "F25C1-$PHASE-VENDOR-COUNT-WHILE-RUNNING: $LIVE_VENDOR"
echo "F25C1-$PHASE-VENDOR-COUNT-AFTER-CANCEL: $POST_VENDOR"
echo "F25C1-$PHASE-PRODUCT-ROW-AFTER-CANCEL: $POST_PRODUCT"
date -u +%Y-%m-%dT%H:%M:%SZ
} 2>&1 | tee "$OUT/cloud-cancel-$PHASE.txt"
