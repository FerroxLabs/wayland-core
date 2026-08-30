#!/usr/bin/env bash
# Self-tests for .github/scripts/assert-no-dependency-failed.sh -- the step that
# makes ci.yml's required `report` check grade what it aggregates.
#
# WHY THIS FILE EXISTS. The body it replaced was six hand-written `check <job>`
# lines beside a six-entry `needs:` list. A verifier deleted ONE of those lines
# (`check ci-linux`), the workflow still parsed, and all five self-test suites
# on the branch stayed green -- the required check would have passed silently
# over a failed `ci-linux`, which is the same class as `ci-linux` not being in
# `needs:` at all (run 33262552890: `report` GREEN over a RED `ci-linux`).
#
# The script under test takes `${{ toJSON(needs) }}` and iterates it, so the set
# graded IS the set depended on and there is no second list to drift. These
# cases grade the part that is still decidable per-run: that every dependency is
# actually inspected, that failure/cancelled red, and that ANY unrecognised
# state fails closed rather than falling into an `else` that assumed success.
#
# Run: bash .github/scripts/tests/dependency-aggregate.test.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
GATE="$ROOT/.github/scripts/assert-no-dependency-failed.sh"
PASS=0
FAIL=0

ok()  { PASS=$((PASS + 1)); printf "ok   %s\n" "$1"; }
bad() { FAIL=$((FAIL + 1)); printf "FAIL %s\n" "$1"; }

if [ -f "$GATE" ]; then ok "the aggregate gate exists"; else bad "the aggregate gate exists ($GATE)"; exit 1; fi

# run_gate <json>  -> sets OUT and RC
run_gate() {
  set +e
  OUT=$(NEEDS_JSON="$1" bash "$GATE" 2>&1)
  RC=$?
  set -e
}

want() { # want <label> <expected-rc> <json>
  run_gate "$3"
  if [ "$RC" -eq "$2" ]; then
    ok "$1"
  else
    bad "$1 (want exit=$2, got exit=$RC)"
    printf '       | %s\n' "$OUT"
  fi
}

ALL_GREEN='{"ci":{"result":"success"},"ci-linux":{"result":"success"},"build":{"result":"success"}}'

want "an all-green needs object passes" 0 "$ALL_GREEN"

# ANTI-VACUITY, and it is the whole point of the file: a gate that exits 0
# without looking at anything passes the case above for free. Every dependency
# must appear in the output, so "graded" is observable rather than assumed.
run_gate "$ALL_GREEN"
missing=""
for job in ci ci-linux build; do
  case "$OUT" in *"$job"*) ;; *) missing="${missing}${job} " ;; esac
done
if [ -z "$missing" ] && case "$OUT" in *"3 graded"*) true ;; *) false ;; esac; then
  ok "every dependency is named in the pass output, and the count matches"
else
  bad "every dependency is named in the pass output, and the count matches (missing: ${missing:-none})"
  printf '       | %s\n' "$OUT"
fi

# A SKIPPED LEG IS NOT A RED. The macOS/Windows legs are rationed by design.
want "a skipped dependency does not red the aggregate" 0 \
  '{"ci":{"result":"skipped"},"ci-linux":{"result":"success"}}'

# ...and the counterpart, or the rule above degenerates into "nothing reds".
want "a failed dependency reds the aggregate" 1 \
  '{"ci":{"result":"success"},"ci-linux":{"result":"failure"}}'
want "a cancelled dependency reds the aggregate" 1 \
  '{"ci":{"result":"success"},"ci-linux":{"result":"cancelled"}}'

# The failure must SAY WHICH job, or the required check's red is unactionable.
run_gate '{"ci":{"result":"success"},"eval-gate-linux":{"result":"failure"}}'
case "$OUT" in
  *"::error"*"eval-gate-linux"*"failure"*)
    ok "the red names the dependency and its conclusion" ;;
  *)
    bad "the red names the dependency and its conclusion"
    printf '       | %s\n' "$OUT" ;;
esac
# ...while the healthy sibling in the same call is still reported, so the red is
# not produced by the gate giving up at the first job it reads.
case "$OUT" in
  *"ok   ci"*) ok "a healthy sibling is still graded alongside a failing one" ;;
  *) bad "a healthy sibling is still graded alongside a failing one"
     printf '       | %s\n' "$OUT" ;;
esac

# FAIL CLOSED ON EVERYTHING ELSE. GitHub can add a conclusion string; an `else`
# branch that assumed the unknown was fine is how a required check passes over a
# state nobody has thought about yet.
want "an unrecognised result fails closed" 1 \
  '{"ci":{"result":"neutral"}}'
want "a dependency with no result at all fails closed" 1 \
  '{"ci":{}}'

# A GATE WITH NO DEPENDENCIES CERTIFIES NOTHING. This is the `needs:`-side of
# the same defect: `report` was required while depending only on the matrix.
want "an empty needs object fails closed" 1 '{}'
want "unparseable input fails closed" 1 'not json at all'

set +e
OUT=$(env -u NEEDS_JSON bash "$GATE" 2>&1)
RC=$?
set -e
if [ "$RC" -eq 1 ]; then
  ok "an absent NEEDS_JSON fails closed"
else
  bad "an absent NEEDS_JSON fails closed (got exit=$RC)"
  printf '       | %s\n' "$OUT"
fi

# THE ENUMERATION CANNOT COME BACK THROUGH THIS FILE EITHER: the gate must not
# contain executable job names. Comments are excluded -- the header quotes the
# enumeration it replaced, deliberately, and a comment cannot grade anything.
CODE=$(grep -v '^[[:space:]]*#' "$GATE")
if printf '%s' "$CODE" | grep -qE 'ci-linux|eval-gate-linux|all-features-check'; then
  bad "the aggregate gate hard-codes no job name"
  printf '%s' "$CODE" | grep -nE 'ci-linux|eval-gate-linux|all-features-check' | sed 's/^/       | /'
else
  ok "the aggregate gate hard-codes no job name"
fi
# ...control for that grep, which is otherwise a search for absence and passes
# for free against an empty extraction.
if printf '%s' "$CODE" | grep -q 'NEEDS_JSON'; then
  ok "control: the extracted code body is non-empty and is the gate's"
else
  bad "control: the extracted code body is non-empty and is the gate's"
fi

# Scaling: seven dependencies grade as seven, so adding a producing leg needs no
# edit here and no edit in the workflow beyond `needs:`.
want "a seventh dependency needs no edit anywhere" 0 \
  '{"a":{"result":"success"},"b":{"result":"success"},"c":{"result":"skipped"},"d":{"result":"success"},"e":{"result":"success"},"f":{"result":"skipped"},"g":{"result":"success"}}'
run_gate '{"a":{"result":"success"},"b":{"result":"success"},"c":{"result":"skipped"},"d":{"result":"success"},"e":{"result":"success"},"f":{"result":"skipped"},"g":{"result":"success"}}'
case "$OUT" in
  *"7 graded"*) ok "all seven are graded, not the first six" ;;
  *) bad "all seven are graded, not the first six"; printf '       | %s\n' "$OUT" ;;
esac

echo ""
echo "dependency-aggregate: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
