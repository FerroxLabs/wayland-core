#!/usr/bin/env bash
# Lane 28-drift — prove every F28D-* rejection code in f28-check-drift.py can actually fire.
#
# A gate never shown red is a gate of unknown value. --self-test covers F28D-003 and the
# known-negative; this covers the four remaining codes, each with a fixture built to trip
# exactly it, plus a pristine control that must come back clean.
#
# Run from the repository root. Uses /usr/bin/git only (rtk filters git output).
set -uo pipefail

ROOT=$(/usr/bin/git rev-parse --show-toplevel)
cd "$ROOT" || exit 2
PHASE=.planning/phases/28-native-cross-platform-certification
RECEIPT=$PHASE/28-04-CERTIFICATION-RECEIPT-SUPERSEDING-001.json
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

python3 - "$ROOT" "$RECEIPT" "$WORK" <<'PY'
import json, pathlib, subprocess, sys
root, receipt, work = (pathlib.Path(a) for a in sys.argv[1:4])
r = json.loads((root / receipt).read_text())

def variant(name, mutate):
    v = json.loads(json.dumps(r))
    mutate(v)
    (work / name).write_text(json.dumps(v))

def set_commit(v, sha):
    for c in v["body"]["bindings"]["candidate"]:
        c["commit"] = sha

# F28D-001: a certified commit this repository does not contain.
variant("unknown.json", lambda v: set_commit(v, "0" * 40))

# F28D-002: a certified commit that is NOT an ancestor of the ref — diverged, not merely aged.
head = subprocess.run(["/usr/bin/git", "-C", str(root), "rev-parse", "HEAD"],
                      capture_output=True, text=True).stdout.strip()
refs = subprocess.run(["/usr/bin/git", "-C", str(root), "for-each-ref",
                       "--format=%(objectname) %(refname)", "refs/heads", "refs/remotes"],
                      capture_output=True, text=True).stdout.splitlines()
pick = None
for line in refs:
    sha, _, name = line.partition(" ")
    if subprocess.run(["/usr/bin/git", "-C", str(root), "merge-base", "--is-ancestor", sha, head],
                      capture_output=True).returncode != 0:
        pick = (sha, name)
        break
if pick:
    print(f"non-ancestor ref used for F28D-002: {pick[1]} {pick[0][:8]}")
    variant("diverged.json", lambda v: set_commit(v, pick[0]))
else:
    print("NO non-ancestor ref available; F28D-002 cannot be probed here", file=sys.stderr)

# F28D-004: a receipt binding no candidate at all.
variant("nocand.json", lambda v: v["body"]["bindings"].__setitem__("candidate", []))
PY

fail=0
probe () {  # probe <fixture-or-receipt> <ref> <expected-code>
  local target=$1 ref=$2 want=$3
  local log="$WORK/out.txt"
  python3 .planning/scripts/f28-check-drift.py --receipt "$target" --ref "$ref" >"$log" 2>&1
  local rc=$?
  local got
  got=$(grep -o 'F28D-[0-9]*' "$log" | sort -u | tr '\n' ' ')
  echo "PROBE want=$want rc=$rc got=[${got}] bytes=$(wc -c <"$log")"
  if [ "$rc" -eq 0 ] || ! grep -q "$want" "$log"; then
    echo "  FAILED: expected $want and a non-zero rc"
    fail=1
  fi
}

probe "$WORK/unknown.json"  HEAD                        F28D-001
[ -f "$WORK/diverged.json" ] && probe "$WORK/diverged.json" HEAD F28D-002
probe "$WORK/nocand.json"   HEAD                        F28D-004
probe "$RECEIPT"            refs/heads/does-not-exist   F28D-000

# Pristine control: the same script, the real receipt, measured against the commit it
# certifies. A checker that rejects everything would pass all four probes above and be useless.
CAND=$(python3 -c "import json,sys;print(json.load(open('$RECEIPT'))['body']['bindings']['candidate'][0]['commit'])")
python3 .planning/scripts/f28-check-drift.py --receipt "$RECEIPT" --ref "$CAND" >"$WORK/ctl.txt" 2>&1
ctl_rc=$?
echo "CONTROL (ref = the certified commit itself) rc=$ctl_rc bytes=$(wc -c <"$WORK/ctl.txt")"
cat "$WORK/ctl.txt"
# The macOS candidate is NOT an ancestor of the linux/windows one, so the control is scoped:
# candidate[0] must come back CURRENT. That is the assertion.
grep -q "matrix-linux-windows: CURRENT" "$WORK/ctl.txt" || { echo "  FAILED: control did not report candidate[0] CURRENT"; fail=1; }

echo "PROBE_RESULT=$([ $fail -eq 0 ] && echo PASS || echo FAIL)"
exit $fail
