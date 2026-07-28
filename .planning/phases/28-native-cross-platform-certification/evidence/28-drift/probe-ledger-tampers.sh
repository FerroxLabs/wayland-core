#!/usr/bin/env bash
# Lane 28-drift — prove f28-ledger.py --validate still bites on the two rows THIS lane added.
#
# A gate that was already green at base proves nothing was done. F-28-ADJ-001 and -002 are new
# rows; if the validator does not reject a broken version of them, adding them was decorative.
# Four tampers, each targeting a different rule, plus the pristine control.
set -uo pipefail

ROOT=$(/usr/bin/git rev-parse --show-toplevel)
cd "$ROOT" || exit 2
L=.planning/phases/28-native-cross-platform-certification/evidence/28-04/findings.tsv
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

python3 - "$L" "$WORK" <<'PY'
import pathlib, sys
src, work = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
lines = src.read_text(encoding="utf-8").split("\n")
assert any(l.startswith("F-28-ADJ-001\t") for l in lines), "target row absent — nothing to tamper"

def mutate(name, fn):
    out = []
    for l in lines:
        if l.startswith("F-28-ADJ-001\t"):
            f = l.split("\t")
            fn(f)
            l = "\t".join(f)
        out.append(l)
    (work / name).write_text("\n".join(out), encoding="utf-8")

mutate("open.tsv",   lambda f: f.__setitem__(6, "OPEN"))               # F28L-002 non-terminal
mutate("noexec.tsv", lambda f: f.__setitem__(10, ""))                  # F28L-008 FIXED w/o check
mutate("accept.tsv", lambda f: (f.__setitem__(6, "ACCEPTED"),
                                f.__setitem__(4, "2")))                # F28L-004/005/007 A2
mutate("badsev.tsv", lambda f: f.__setitem__(3, "SEVERE"))             # F28L-011 unknown severity
PY

fail=0
probe () {  # probe <fixture> <expected-code>
  local out; out=$(mktemp)
  python3 .planning/scripts/f28-ledger.py --validate "$WORK/$1" >"$out" 2>&1
  local rc=$?
  local codes; codes=$(grep -o 'F28L-[0-9]*' "$out" | sort -u | tr '\n' ' ')
  echo "  tamper F-28-ADJ-001:${1%.tsv}  rc=$rc  codes=[$codes]"
  { [ "$rc" -ne 0 ] && grep -q "$2" "$out"; } || { echo "    FAILED: expected $2 and rc!=0"; fail=1; }
  rm -f "$out"
}

probe open.tsv   F28L-002
probe noexec.tsv F28L-008
probe accept.tsv F28L-007
probe badsev.tsv F28L-011

python3 .planning/scripts/f28-ledger.py --validate "$L" >/dev/null 2>&1
ctl=$?
echo "  pristine control                rc=$ctl"
[ "$ctl" -eq 0 ] || { echo "    FAILED: the real ledger does not validate"; fail=1; }

echo "  LEDGER_TAMPER_RESULT=$([ $fail -eq 0 ] && echo PASS || echo FAIL)"
exit $fail
