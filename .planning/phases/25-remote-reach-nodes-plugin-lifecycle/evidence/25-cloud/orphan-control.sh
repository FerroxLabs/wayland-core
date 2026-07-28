#!/bin/bash
# F25 lane 25-cloud — GATE 3: the cloud orphan surface, with a positive control.
#
# THE CONTROL IS NOT SYNTHETIC. While running the provenance gate, a helper in
# this lane's own script mis-parsed the API response and issued its destroy
# against an empty machine id, leaving machine 82d1d97b062338 running in
# wayland-f25-test tagged wayland_task_nonce=f25-provenance-oracle.
#
# That is a REAL leaked cloud machine, produced by a real defect, and it is the
# best possible positive control: the scanner is asked to find an orphan that
# nobody planted for its benefit. If the scan cannot see this, a clean scan
# means nothing.
set -u
cd /root/wayland-25-cloud || exit 1
. /root/.wayland-f25-cloud.env
export WAYLAND_F25_CLOUD_ORG=wayland-f25-test
BIN=./target/release/wayland-core
LEAKED=82d1d97b062338
NONCE=f25-provenance-oracle

echo "=== HOST / COMMIT ==="
hostname; date -u +%Y-%m-%dT%H:%M:%SZ; echo "commit: $(git rev-parse HEAD)"

echo
echo "=== the leak, as the vendor sees it (independent instrument) ==="
/root/f25-cloud/fly.sh GET "/apps/wayland-f25-test/machines" | python3 -c "
import sys,json
lines=[l for l in sys.stdin.read().splitlines() if l.strip()]
print('http status:', lines[0])
ms=json.loads(lines[1])
print('machine count:', len(ms))
for m in ms:
    print(' ', m['id'], m['state'], m['config'].get('metadata'))
"

echo
echo "########## POSITIVE CONTROL: the scan MUST find the leaked machine ##########"
"$BIN" backend scan --task-id "$NONCE"
SCAN_LEAKED_RC=$?
echo "SCAN_WITH_LEAK_EXIT=$SCAN_LEAKED_RC   (nonzero is REQUIRED here: an orphan exists)"

echo
echo "########## NEGATIVE CONTROL: a nonce nothing carries must measure 0, not fail ##########"
"$BIN" backend scan --task-id f25-nonce-that-never-existed
SCAN_CLEAN_RC=$?
echo "SCAN_UNUSED_NONCE_EXIT=$SCAN_CLEAN_RC"

echo
echo "########## CLEANUP: destroy the leaked machine ##########"
/root/f25-cloud/fly.sh DELETE "/apps/wayland-f25-test/machines/$LEAKED?force=true"
echo "DESTROY_ISSUED_FOR=$LEAKED"
sleep 5

echo
echo "########## POST-CLEANUP: the same scan must now measure ZERO ##########"
"$BIN" backend scan --task-id "$NONCE"
SCAN_AFTER_RC=$?
echo "SCAN_AFTER_CLEANUP_EXIT=$SCAN_AFTER_RC   (zero is REQUIRED here)"

echo
echo "########## EMPTINESS PROOF: the app holds no machines at all ##########"
/root/f25-cloud/fly.sh GET "/apps/wayland-f25-test/machines" | python3 -c "
import sys,json
lines=[l for l in sys.stdin.read().splitlines() if l.strip()]
print('http status:', lines[0])
ms=json.loads(lines[1])
print('machine count:', len(ms))
print('raw:', json.dumps(ms))
import sys as s
s.exit(0 if len(ms)==0 else 1)
"
EMPTY_RC=$?
echo "APP_EMPTY_EXIT=$EMPTY_RC   (zero means the app genuinely holds no machines)"

echo
echo "=== EXIT LEDGER (each captured before any pipe) ==="
echo "SCAN_WITH_LEAK_EXIT=$SCAN_LEAKED_RC SCAN_UNUSED_NONCE_EXIT=$SCAN_CLEAN_RC SCAN_AFTER_CLEANUP_EXIT=$SCAN_AFTER_RC APP_EMPTY_EXIT=$EMPTY_RC"
