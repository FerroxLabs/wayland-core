#!/bin/bash
# 25-c1: can the new cleanup test actually FAIL?
# Reverts the fix IN PLACE (never with git — the file is restored from a copy
# this script makes), runs the test, then restores and re-runs. A gate that has
# never been observed red is not a gate.
set -u
F=/root/wayland-25c1/crates/wcore-exec-backend/src/backends/ssh.rs
BAK=/root/f25c1-ssh.rs.fixed.bak
cd /root/wayland-25c1 || exit 1
{
echo "=== F25C1 GATE-CAN-FAIL ==="
date -u +%Y-%m-%dT%H:%M:%SZ
echo "tree HEAD: $(/usr/bin/git rev-parse HEAD)"
cp "$F" "$BAK"
python3 - "$F" <<"PY"
import sys
p=sys.argv[1]; s=open(p).read()
new="""status=0
wait "$child" || status=$?
cd /
rm -rf "$root"
"""
old="""wait "$child"
status=$?
rm -rf "$root"
"""
assert s.count(new)==1, ("fixed shape not found exactly once", s.count(new))
open(p,"w").write(s.replace(new,old,1))
print("REVERTED-THE-FIX-IN-PLACE (the pre-2026-07-29 runner is now compiled)")
PY
echo
echo "--- the test, against the PRE-FIX runner (must be RED)"
/root/.cargo/bin/cargo test --release -p wcore-exec-backend --test ssh_remote_runner_cleanup 2>&1 | tail -16
echo "TESTRC_ON_UNFIXED=${PIPESTATUS[0]}"
cp "$BAK" "$F"
echo
echo "--- restored; working tree must be clean:"
/usr/bin/git status --porcelain | sed "s/^/  DIRTY: /"
echo "  (no DIRTY line above = the revert left nothing behind)"
echo
echo "--- the same test, against the SHIPPED runner (must be GREEN, with a real count)"
/root/.cargo/bin/cargo test --release -p wcore-exec-backend --test ssh_remote_runner_cleanup 2>&1 | tail -10
echo "TESTRC_ON_FIXED=${PIPESTATUS[0]}"
date -u +%Y-%m-%dT%H:%M:%SZ
} 2>&1 | tee /root/f25c1-evidence/gate-can-fail.txt
