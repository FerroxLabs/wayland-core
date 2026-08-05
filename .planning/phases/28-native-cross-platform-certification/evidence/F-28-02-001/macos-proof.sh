#!/bin/bash
# F-28-02-001 — macOS activeness proof against the CI-built lane binary.
#
# Obtains the containment differential that 28-02 could not obtain, using the
# new `wayland-core sandbox exec` surface, and runs the negative control that
# proves the detector discriminates rather than firing unconditionally.
#
# Usage: macos-proof.sh <run-id> <expected-commit>
set -uo pipefail

RUN_ID="${1:?run id}"
COMMIT="${2:?expected commit}"
ROOT=/private/tmp/claude-501/-Users-seandonahoe-dev-waylandcore/11929102-d58a-47e9-9644-0e9d530b58c4/scratchpad/f28-macos
ART="$ROOT/artifact"
WS="$ROOT/ws"
ESCAPE_DIR="$HOME/.f28-escape"

rm -rf "$ROOT"; mkdir -p "$ART" "$WS" "$ESCAPE_DIR"

echo "===== 1. DOWNLOAD the CI artifact for $COMMIT (run $RUN_ID) ====="
gh run download "$RUN_ID" -R FerroxLabs/wayland-core \
  -n wayland-core-aarch64-apple-darwin -D "$ART" || { echo "DOWNLOAD FAILED"; exit 2; }
BIN="$ART/wayland-core"
chmod +x "$BIN"

echo "===== 2. DIGEST-ASSERT and identify the binary ====="
shasum -a 256 "$BIN"
file "$BIN"
"$BIN" --version || { echo "BINARY DID NOT RUN"; exit 3; }
echo "expected commit: $COMMIT"

echo "===== 3. sandbox status (the surface must exist in THIS binary) ====="
"$BIN" sandbox status --json || { echo "STATUS FAILED — surface absent from this build"; exit 4; }

echo "===== 4. build the probe ====="
# The escape target is an absolute path outside the workspace and outside the
# temp scratch root the contained policy grants.
cat > "$WS/probe.sh" <<EOF
TAG=\$1
echo F28RAN
( (dscacheutil -q host -a name github.com 2>/dev/null | grep -q ip_address) \
  || nslookup github.com >/dev/null 2>&1 ) && echo F28_DNS=RESOLVES || echo F28_DNS=NO_DNS
(touch "$ESCAPE_DIR/marker-\$TAG" 2>/dev/null && echo F28_ESCAPE=WROTE) || echo F28_ESCAPE=DENIED
(head -c 10 /etc/hosts >/dev/null 2>&1 && echo F28_ETC=READ) || echo F28_ETC=DENIED
echo F28_ROOTLS=\$(ls / 2>/dev/null | tr "\n" ",")
EOF

rm -f "$ESCAPE_DIR"/marker-*

echo "===== 5. OUTSIDE — the uncontained baseline ====="
( cd "$WS" && /bin/sh probe.sh OUTSIDE ) | tee "$ROOT/outside.txt"
echo "host escape marker OUTSIDE: $(test -e "$ESCAPE_DIR/marker-OUTSIDE" && echo PRESENT || echo ABSENT)"

echo "===== 6. INSIDE — through the product's own containment path ====="
"$BIN" sandbox exec --workspace "$WS" "sh probe.sh INSIDE" 2>&1 | tee "$ROOT/inside.txt"
echo "EXIT=$?"
echo "host escape marker INSIDE: $(test -e "$ESCAPE_DIR/marker-INSIDE" && echo PRESENT || echo ABSENT)"

echo "===== 7. DIFFERENTIAL ====="
python3 - "$ROOT/outside.txt" "$ROOT/inside.txt" "$ESCAPE_DIR" <<'PY'
import re, sys, os
out = open(sys.argv[1]).read(); ins = open(sys.argv[2]).read(); esc = sys.argv[3]
def g(t, k):
    m = re.search(rf'{k}=(\S*)', t)
    return m.group(1) if m else None
diffs = []
if 'F28RAN' not in ins:
    print('ACTIVENESS: observed=false — the child never ran inside the product'); sys.exit(0)
if g(out,'F28_DNS')=='RESOLVES' and g(ins,'F28_DNS')=='NO_DNS':
    diffs.append('DNS resolves outside and does not inside (network denied by the sandbox profile)')
o_esc = os.path.exists(os.path.join(esc,'marker-OUTSIDE'))
i_esc = os.path.exists(os.path.join(esc,'marker-INSIDE'))
if o_esc and not i_esc:
    diffs.append('a write outside the workspace lands on the host uncontained and is NOT visible on the host from inside')
if g(out,'F28_ETC')=='READ' and g(ins,'F28_ETC')=='DENIED':
    diffs.append('/etc/hosts is readable outside and denied inside (filesystem read confined)')
ro, ri = g(out,'F28_ROOTLS'), g(ins,'F28_ROOTLS')
if ro and ri and ro != ri:
    diffs.append(f'filesystem root listing differs ({len([x for x in ro.split(",") if x])} entries outside, {len([x for x in ri.split(",") if x])} inside)')
print('ACTIVENESS: observed=' + ('true' if diffs else 'false'))
for d in diffs: print('  - ' + d)
PY

echo "===== 8. NEGATIVE CONTROL — the detector must NOT fire on outside-vs-outside ====="
python3 - "$ROOT/outside.txt" <<'PY'
import re, sys
t = open(sys.argv[1]).read()
def g(k):
    m = re.search(rf'{k}=(\S*)', t)
    return m.group(1) if m else None
# Same reading on both sides of the comparison: a detector that fires here is
# firing unconditionally rather than discriminating.
diffs = []
if g('F28_DNS')=='RESOLVES' and g('F28_DNS')=='NO_DNS': diffs.append('dns')
if g('F28_ETC')=='READ' and g('F28_ETC')=='DENIED': diffs.append('etc')
if g('F28_ROOTLS') != g('F28_ROOTLS'): diffs.append('root')
print('NEGATIVE CONTROL activeness observed=' + ('true — DETECTOR IS BROKEN' if diffs else 'false — detector discriminates'))
PY
rm -f "$ESCAPE_DIR"/marker-*
