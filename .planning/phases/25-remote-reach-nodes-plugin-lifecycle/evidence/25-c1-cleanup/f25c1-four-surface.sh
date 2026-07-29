#!/bin/bash
# 25-c1 bonus (verdict gap G3): run the SAME reference task on all FOUR
# surfaces at ONE commit and diff them in ONE invocation.
#
# The verdict accepted the four-surface claim as a composition across two
# commits — local+container+cloud at 5e620ef0, ssh at 25-01's commit — because
# `WAYLAND_EXEC_SSH_TARGET` was unset and the old sshd target was gone. This lane
# already had to stand a real sshd far end up for the cleanup measurement, so
# closing that qualification costs one script.
#
# Every exit status is captured into a variable IMMEDIATELY, before any pipe.
set -u
set +x

TREE="${1:?usage: f25c1-four-surface.sh <worktree>}"
BIN="$TREE/target/release/wayland-core"
OUT=/root/f25c1-evidence
mkdir -p "$OUT"

export WAYLAND_EXEC_SSH_CONFIG=/root/f25c1/ssh_config
export WAYLAND_EXEC_SSH_TARGET=f25c1-far
export WAYLAND_EXEC_BACKEND_STATE_DIR=/root/f25c1-state
mkdir -p "$WAYLAND_EXEC_BACKEND_STATE_DIR"

set -a
. /root/.wayland-f25-cloud.env
set +a
export WAYLAND_F25_CLOUD_ORG=wayland-f25-test   # the APP, not the personal org

{
echo "=== F25C1 FOUR SURFACES, ONE COMMIT ==="
date -u +%Y-%m-%dT%H:%M:%SZ
echo "controller: $(hostname)"
echo "tree HEAD:  $(/usr/bin/git -C "$TREE" rev-parse HEAD)"
echo "binary sha: $(sha256sum "$BIN" | awk '{print $1}')"
echo

echo "--- availability, read back from the product"
"$BIN" backend list
LIST_RC=$?
echo "LIST_EXIT=$LIST_RC"
echo

for B in local container ssh cloud; do
  echo "=== RUN $B (built-in reference task, byte-identical on every surface) ==="
  rm -f "$OUT/four-receipt-$B.json"
  "$BIN" backend run --backend "$B" --receipt-out "$OUT/four-receipt-$B.json"
  RC=$?
  echo "RUN_${B}_EXIT=$RC"
  eval "RC_$B=$RC"
  echo
done

echo "=== NORMALIZED EQUIVALENCE DIFF — all four, in ONE invocation ==="
"$BIN" backend diff \
  "$OUT/four-receipt-local.json" \
  "$OUT/four-receipt-container.json" \
  "$OUT/four-receipt-ssh.json" \
  "$OUT/four-receipt-cloud.json"
DIFF_RC=$?
echo "DIFF_EXIT=$DIFF_RC"

echo
echo "=== orphan scan for the reference nonce, after all four ==="
"$BIN" backend scan --task-id f25-reference --nonce f25-reference-nonce
SCAN_RC=$?
echo "SCAN_EXIT=$SCAN_RC"

echo
echo "=== F25C1-FOUR-SURFACE LEDGER ==="
echo "F25C1-SC1-ONE-COMMIT: $(/usr/bin/git -C "$TREE" rev-parse HEAD)"
echo "F25C1-SC1-LOCAL: exit=$RC_local"
echo "F25C1-SC1-CONTAINER: exit=$RC_container"
echo "F25C1-SC1-SSH: exit=$RC_ssh target=$WAYLAND_EXEC_SSH_TARGET (containerised sshd, separate namespace, same physical host)"
echo "F25C1-SC1-CLOUD: exit=$RC_cloud app=$WAYLAND_F25_CLOUD_ORG"
echo "F25C1-SC1-DIFF: exit=$DIFF_RC (0 = EQUIVALENT)"
echo "F25C1-SC1-ORPHANS: exit=$SCAN_RC (0 = none found and every surface measurable)"
date -u +%Y-%m-%dT%H:%M:%SZ
} 2>&1 | tee "$OUT/four-surface-one-commit.txt"
