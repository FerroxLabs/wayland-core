#!/bin/bash
# PROVE THE SUPERSESSION BINDING CAN SAY NO.
#
# The supersession introduces new surface: a posture statement naming the superseded
# body_sha256, and an artifact binding over the superseded receipt's exact bytes. A checker
# that has only ever been seen ACCEPTING a good receipt is not evidence that surface is
# checked at all -- and "the instrument that hunts a defect class carries it" has been
# measured eight times on this program.
#
# So: four tampers that MUST be rejected, and one pristine control that MUST pass. A run where
# the control fails is as much a failure as a tamper slipping through.
#
# Every case runs in a symlink farm so the REAL phase directory is never written to. Case 4
# tampers the superseded receipt itself, which is the whole point of binding its bytes, and
# that is only safe against a copy.
#
# Exit 0 only if all five cases behave. Usage: probe-supersession-tamper.sh <phase-dir>
set -u

PHASE=$(cd "$1" && pwd) || exit 2
V="$PHASE/../../scripts/f28-verify-bindings.py"
NEW="28-04-CERTIFICATION-RECEIPT-SUPERSEDING-001.json"
OLD="28-04-CERTIFICATION-RECEIPT.json"
FAILS=0

farm() { # $1 = dir to build
  mkdir -p "$1"
  for e in "$PHASE"/*; do ln -sfn "$e" "$1/$(basename "$e")"; done
  # The receipt under test must be a REAL FILE, never a symlink. `f28-verify-bindings.py`
  # sets `base = receipt.resolve().parent`, and resolve() FOLLOWS SYMLINKS -- so a symlinked
  # receipt silently redirects the whole verification at the real phase directory and every
  # tamper planted in this farm becomes invisible. That is not hypothetical: the first run of
  # this probe scored case 4 as a pass for exactly that reason, which is the same
  # self-passing-gate shape this phase exists to catch, occurring inside the probe built to
  # catch it.
  rm -f "$1/$NEW"; cp "$PHASE/$NEW" "$1/$NEW"
}

# $1 label  $2 expected-rc  $3 expected-substring  $4 mode  $5 python-mutator (on the farm dir)
case_run() {
  local label=$1 exp_rc=$2 needle=$3 mode=$4 mutator=$5
  local d; d=$(mktemp -d)
  farm "$d"
  python3 - "$d" <<PY
import json, sys, pathlib
d = pathlib.Path(sys.argv[1])
$mutator
PY
  local out rc
  out=$(python3 "$V" "$mode" "$d/$NEW" 2>&1); rc=$?
  local verdict="ok"
  if [ "$rc" != "$exp_rc" ]; then verdict="FAIL (rc=$rc, wanted $exp_rc)"; FAILS=$((FAILS+1));
  elif ! printf '%s' "$out" | grep -qF -- "$needle"; then
    verdict="FAIL (rc ok but no '$needle')"; FAILS=$((FAILS+1)); fi
  printf '%-46s rc=%-3s %s\n' "$label" "$rc" "$verdict"
  printf '%s\n' "$out" | grep -F -- "$needle" | head -2 | sed 's/^/        /'
  rm -rf "$d"
}

echo "== PROBE: can the supersession binding reject? =="
echo "phase dir: $PHASE"
echo

# 0 -- CONTROL. Untampered, must PASS. Without this the four rejections below could all be
#      coming from a farm that is broken rather than from a tamper being caught.
case_run "0 CONTROL untampered (must PASS)" 0 "--verify: OK" --verify \
  'pass'

# 1 -- the superseded digest inside the posture prose is altered. Nothing recomputes prose, so
#      this must be caught by the BODY DIGEST, not by a binding rule.
case_run "1 posture supersession sha altered" 1 "F28V-DIGEST" --check-tamper-detection \
'p = d / "'"$NEW"'"
r = json.loads(p.read_text())
for e in r["body"]["bindings"]["posture"]:
    if "supersedes" in e["name"]:
        e["description"] = e["description"].replace("2037352c", "deadbeef")
p.unlink(); p.write_text(json.dumps(r, indent=2) + "\n")'

# 2 -- the artifact binding over the superseded receipt is altered.
case_run "2 bound superseded-receipt sha altered" 1 "F28V-ARTIFACT" --verify \
'p = d / "'"$NEW"'"
r = json.loads(p.read_text())
for a in r["body"]["bindings"]["artifacts"]:
    if a["path"] == "'"$OLD"'":
        a["sha256"] = "0" * 64
p.unlink(); p.write_text(json.dumps(r, indent=2) + "\n")'

# 3 -- a claim is flipped away from what the raw ledger says.
case_run "3 claim flipped vs raw ledger" 1 "F28V-CLAIM" --verify \
'p = d / "'"$NEW"'"
r = json.loads(p.read_text())
r["body"]["claims"]["zero_unresolved_critical_or_high"] = False
p.unlink(); p.write_text(json.dumps(r, indent=2) + "\n")'

# 4 -- THE ONE THAT MATTERS. Somebody edits the receipt we claim to supersede. The supersession
#      must stop being verifiable, because it named those exact bytes.
case_run "4 the SUPERSEDED receipt itself edited" 1 "F28V-ARTIFACT" --verify \
'p = d / "'"$OLD"'"
t = p.resolve().read_text().replace("28-native-cross-platform-certification",
                                    "28-native-cross-platform-certificatioN", 1)
p.unlink(); p.write_text(t)'

echo
if [ "$FAILS" -eq 0 ]; then
  echo "PROBE RESULT: PASS -- 4 tampers rejected, 1 pristine control accepted"; exit 0
fi
echo "PROBE RESULT: FAIL -- $FAILS case(s) misbehaved"; exit 1
