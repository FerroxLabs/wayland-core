#!/bin/bash
# F25 lane 25-cloud — GATE 2b: complete the provenance verdict.
#
# The first attempt at this gate leaked a machine because it parsed fly.sh's
# output with `tail -1`, which returns the trailing blank line rather than the
# JSON body, so the machine id was empty and the destroy was a no-op. That
# defect is recorded rather than quietly fixed: it is the same class as the
# pipeline-steals-exit-status trap, and it produced the real orphan that GATE 3
# used as its positive control.
#
# Parsing is now done in python over the WHOLE response, and the destroy is
# verified by re-reading the app's machine list rather than trusting the call.
set -u
cd /root/wayland-25-cloud || exit 1
. /root/.wayland-f25-cloud.env
export WAYLAND_F25_CLOUD_ORG=wayland-f25-test

echo "=== HOST / COMMIT ==="
hostname; date -u +%Y-%m-%dT%H:%M:%SZ; echo "commit: $(git rev-parse HEAD)"

jsonline() { python3 -c "
import sys
lines=[l for l in sys.stdin.read().splitlines() if l.strip()]
print(lines[1] if len(lines)>1 else '')
"; }

cat > /tmp/pm.json <<'JSON'
{"region":"iad","config":{"image":"alpine:3.20","guest":{"cpu_kind":"shared","cpus":1,"memory_mb":256},"init":{"exec":["/bin/sleep","inf"]},"auto_destroy":false,"restart":{"policy":"no"},"metadata":{"wayland_task_nonce":"f25-provenance-oracle2"}}}
JSON
CREATE=$(/root/f25-cloud/fly.sh POST "/apps/wayland-f25-test/machines" /tmp/pm.json | jsonline)
rm -f /tmp/pm.json
OM=$(printf '%s' "$CREATE" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
if [ -z "$OM" ]; then echo "ORACLE CREATE FAILED - refusing to continue"; exit 1; fi
echo "oracle machine: $OM"

/root/f25-cloud/fly.sh GET "/apps/wayland-f25-test/machines/$OM/wait?state=started&timeout=60" >/dev/null
cat > /tmp/pe.json <<'JSON'
{"command":["/bin/sh","-c","cat /etc/alpine-release"],"timeout":20}
JSON
ORACLE=$(/root/f25-cloud/fly.sh POST "/apps/wayland-f25-test/machines/$OM/exec" /tmp/pe.json | jsonline)
rm -f /tmp/pe.json
echo "oracle response: $ORACLE"

echo
echo "=== DESTROY the oracle machine, then VERIFY by re-reading ==="
/root/f25-cloud/fly.sh DELETE "/apps/wayland-f25-test/machines/$OM?force=true"
sleep 4
/root/f25-cloud/fly.sh GET "/apps/wayland-f25-test/machines" | python3 -c "
import sys,json
lines=[l for l in sys.stdin.read().splitlines() if l.strip()]
ms=json.loads(lines[1])
print('machines remaining in wayland-f25-test:', len(ms), json.dumps(ms))
sys.exit(0 if len(ms)==0 else 1)
"
EMPTY_RC=$?
echo "ORACLE_CLEANUP_VERIFIED_EXIT=$EMPTY_RC"

echo
echo "=== PROVENANCE VERDICT ==="
python3 - "$ORACLE" <<'PY'
import json, sys, hashlib
oracle = json.loads(sys.argv[1])
guest = oracle["stdout"].encode()
guest_sha = hashlib.sha256(guest).hexdigest()
echo_sha  = hashlib.sha256(b"this-input-must-NOT-be-the-output\n").hexdigest()
r = json.load(open("/root/f25-cloud-evidence/receipt-prov-cloud.json"))
got = r["body"]["artifact"]["sha256"]
lo = json.load(open("/root/f25-cloud-evidence/receipt-prov-local.json"))
print("guest /etc/alpine-release, read by an INDEPENDENT instrument:", repr(oracle["stdout"]))
print("  sha256(guest file)              =", guest_sha, f"({len(guest)} bytes)")
print("  sha256(submitted input)         =", echo_sha, "  <- the echo-defect digest")
print("  cloud receipt artifact sha256   =", got, f"({r['body']['artifact']['bytes']} bytes)")
print("  cloud receipt terminal          =", json.dumps(r["body"]["terminal"]))
print("  LOCAL receipt terminal          =", json.dumps(lo["body"]["terminal"]), " <- negative control")
print("  local produced an artifact?     =", lo["body"]["artifact"] is not None)
ok = True
if got != guest_sha:
    ok = False
    print("PROVENANCE: FAIL - artifact does not match the guest file")
    if got == echo_sha:
        print("  and it DOES match the submitted input: the backend echoed, nothing ran in the cloud")
if lo["body"]["terminal"] == "success" or lo["body"].get("artifact"):
    ok = False
    print("NEGATIVE CONTROL FAIL - the local backend succeeded at a task only the guest can do")
if ok:
    print("PROVENANCE: PASS")
    print("  The cloud artifact is the GUEST's file, which the controller does not have.")
    print("  The same task on the local backend FAILED with exit-1 and published no artifact.")
    print("  The task therefore ran ON the cloud machine and was not echoed by the controller.")
sys.exit(0 if ok else 1)
PY
PROV_RC=$?
echo "PROVENANCE_EXIT=$PROV_RC"

echo
echo "=== EXIT LEDGER ==="
echo "ORACLE_CLEANUP_VERIFIED_EXIT=$EMPTY_RC PROVENANCE_EXIT=$PROV_RC"
