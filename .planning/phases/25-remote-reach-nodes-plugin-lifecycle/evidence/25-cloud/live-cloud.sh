#!/bin/bash
# F25 lane 25-cloud — the live cloud leg, driven through the SHIPPED binary.
#
# The token is sourced from a 0600 env file into the environment and is NEVER
# echoed. There is no `set -x` in this script for exactly that reason.
# Every exit status is captured into a variable IMMEDIATELY after the command,
# before any pipe, because a pipeline reports its LAST command's status.
set -u
cd /root/wayland-25-cloud || exit 1
. /root/.wayland-f25-cloud.env
# Scope to the dedicated APP, not to Sean's personal org. The env file's ORG
# value is `sean-donahoe`, a personal org that will not stay empty; the app is
# the narrowest surface whose emptiness can be asserted meaningfully.
export WAYLAND_F25_CLOUD_ORG=wayland-f25-test
BIN=./target/release/wayland-core
OUT=/root/f25-cloud-evidence
mkdir -p "$OUT"

echo "=== HOST ==="
hostname
date -u +%Y-%m-%dT%H:%M:%SZ
echo "commit: $(git rev-parse HEAD)"
echo "app scoped to: $WAYLAND_F25_CLOUD_ORG (token variable name: WAYLAND_F25_CLOUD_TOKEN, value never printed)"
B=$(stat -c %Y "$BIN"); S=$(stat -c %Y crates/wcore-exec-backend/src/backends/cloud.rs)
if [ "$B" -gt "$S" ]; then echo "BINARY FRESHNESS: FRESH (bin $B > src $S)"; else echo "BINARY FRESHNESS: STALE - THE BUILD DID NOT HAPPEN (bin $B <= src $S)"; exit 1; fi

echo
echo "=== BACKEND LIST (shipped binary) ==="
"$BIN" backend list
LIST_RC=$?
echo "LIST_EXIT=$LIST_RC"

echo
echo "=== PROBE cloud ==="
"$BIN" backend probe cloud
PROBE_RC=$?
echo "PROBE_EXIT=$PROBE_RC"

echo
echo "=== PRE-RUN: the app must be empty before we start ==="
/root/f25-cloud/fly.sh GET "/apps/wayland-f25-test/machines"

echo
echo "=== RUN local (same task, same commit) ==="
"$BIN" backend run --backend local --receipt-out "$OUT/receipt-local.json"
LOCAL_RC=$?
echo "RUN_local_EXIT=$LOCAL_RC"

echo
echo "=== RUN container (same task, same commit) ==="
"$BIN" backend run --backend container --receipt-out "$OUT/receipt-container.json"
CONTAINER_RC=$?
echo "RUN_container_EXIT=$CONTAINER_RC"

echo
echo "=== RUN cloud — THE FOURTH SURFACE ==="
"$BIN" backend run --backend cloud --receipt-out "$OUT/receipt-cloud.json"
CLOUD_RC=$?
echo "RUN_cloud_EXIT=$CLOUD_RC"

echo
echo "=== RECEIPT INTEGRITY (cloud) ==="
"$BIN" backend receipt verify "$OUT/receipt-cloud.json"
VERIFY_RC=$?
echo "RECEIPT_VERIFY_EXIT=$VERIFY_RC"

echo
echo "=== HIBERNATION FIELD, VERBATIM FROM THE CLOUD RECEIPT ==="
python3 -c "
import json,sys
r=json.load(open('$OUT/receipt-cloud.json'))
h=r['body']['hibernation']
print(json.dumps(h,indent=2))
" 2>&1
HIB_RC=$?
echo "HIBERNATION_READ_EXIT=$HIB_RC"

echo
echo "=== NORMALIZED EQUIVALENCE DIFF (local vs container vs cloud) ==="
"$BIN" backend diff "$OUT/receipt-local.json" "$OUT/receipt-container.json" "$OUT/receipt-cloud.json"
DIFF_RC=$?
echo "DIFF_EXIT=$DIFF_RC"

echo
echo "=== POST-RUN: the app must be empty again ==="
/root/f25-cloud/fly.sh GET "/apps/wayland-f25-test/machines"

echo
echo "=== EXIT LEDGER (each captured before any pipe) ==="
echo "LIST_EXIT=$LIST_RC PROBE_EXIT=$PROBE_RC LOCAL_EXIT=$LOCAL_RC CONTAINER_EXIT=$CONTAINER_RC CLOUD_EXIT=$CLOUD_RC VERIFY_EXIT=$VERIFY_RC DIFF_EXIT=$DIFF_RC"
