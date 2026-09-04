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
# actually inspected, that failure/cancelled red, that ANY unrecognised state
# fails closed rather than falling into an `else` that assumed success, and
# -- wayland#1291 c2 -- that a skip is graded on whether the workflow DECLARED
# it, so an accidental skip is no longer indistinguishable from a rationed leg.
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

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# THE FIXTURE WORKFLOW. The gate decides whether a skip was declared by reading
# the job's OWN definition, so these cases need a workflow to read. Job ids are
# split deliberately into the two shapes the rule distinguishes:
#
#   MAY skip  -- `ci`, `c`, `f` carry a job-level `if:`; `cascade` carries a
#                `needs:`. Both are reasons GitHub actually skips a job.
#   MAY NOT   -- `ci-linux`, `build`, `eval-gate-linux`, `a`, `b`, `d`, `e`, `g`
#                carry neither, which is the shape four of `report`'s six real
#                dependencies have.
#
# `ghost` is deliberately ABSENT from this file, for the case where the gate is
# pointed at a workflow that does not define a dependency it was handed.
FIXTURE="$WORK/fixture.yml"
cat > "$FIXTURE" <<'FIXTUREEOF'
name: fixture
on: push
jobs:
  ci:
    if: ${{ github.event_name == 'pull_request' }}
    runs-on: ubuntu-latest
    steps:
      - run: 'true'
  c:
    if: ${{ github.ref_name == 'main' }}
    runs-on: ubuntu-latest
    steps:
      - run: 'true'
  f:
    if: ${{ github.ref_name == 'main' }}
    runs-on: ubuntu-latest
    steps:
      - run: 'true'
  cascade:
    needs: [ci]
    runs-on: ubuntu-latest
    steps:
      - run: 'true'
  ci-linux:
    runs-on: ubuntu-latest
    steps:
      - run: 'true'
  build:
    runs-on: ubuntu-latest
    steps:
      - run: 'true'
  eval-gate-linux:
    runs-on: ubuntu-latest
    steps:
      - run: 'true'
  a:
    runs-on: ubuntu-latest
    steps:
      - run: 'true'
  b:
    runs-on: ubuntu-latest
    steps:
      - run: 'true'
  d:
    runs-on: ubuntu-latest
    steps:
      - run: 'true'
  e:
    runs-on: ubuntu-latest
    steps:
      - run: 'true'
  g:
    runs-on: ubuntu-latest
    steps:
      - run: 'true'
FIXTUREEOF

# ANTI-VACUITY FOR THE FIXTURE ITSELF. If it lost the `if:`-bearing jobs the
# "declared skip passes" cases below would go green for the wrong reason, and if
# it lost the bare ones the "undeclared skip reds" cases could not fire at all.
if python3 - "$FIXTURE" <<'PY'
import sys
import yaml
jobs = yaml.safe_load(open(sys.argv[1]))["jobs"]
with_if = [j for j, s in jobs.items() if s.get("if") is not None]
with_needs = [j for j, s in jobs.items() if s.get("needs") and s.get("if") is None]
bare = [j for j, s in jobs.items() if s.get("if") is None and not s.get("needs")]
sys.exit(0 if (len(with_if) >= 2 and len(with_needs) >= 1 and len(bare) >= 4) else 1)
PY
then
  ok "the fixture workflow carries both shapes the rule distinguishes (anti-vacuity)"
else
  bad "the fixture workflow carries both shapes the rule distinguishes (anti-vacuity)"
fi

# run_gate <json> [workflow]  -> sets OUT and RC
run_gate() {
  set +e
  OUT=$(NEEDS_JSON="$1" WORKFLOW_FILE="${2:-$FIXTURE}" bash "$GATE" 2>&1)
  RC=$?
  set -e
}

want() { # want <label> <expected-rc> <json> [workflow]
  run_gate "$3" "${4:-$FIXTURE}"
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

# A DECLARED SKIP IS NOT A RED. The macOS/Windows legs are rationed by design,
# and `ci` -- the leg most often rationed -- says so with a job-level `if:`.
want "a skip declared by a job-level if: does not red the aggregate" 0 \
  '{"ci":{"result":"skipped"},"ci-linux":{"result":"success"}}'
# ...and the same for the other reason GitHub skips a job -- but only when the
# cascade it claims ACTUALLY happened. `cascade` needs `ci`, and `ci` skipped.
want "a cascade corroborated by its upstream does not red the aggregate" 0 \
  '{"cascade":{"result":"skipped"},"ci":{"result":"skipped"},"ci-linux":{"result":"success"}}'
# A `needs:` says a job CAN cascade, never that it did. With every upstream
# green the skip is unexplained, and "it declares something, so it is fine" is
# the same reasoning this whole change exists to remove, one level down.
want "a cascade NOT corroborated by any upstream reds the aggregate" 1 \
  '{"cascade":{"result":"skipped"},"ci":{"result":"success"},"ci-linux":{"result":"success"}}'
# ...and an upstream that this check does not grade cannot corroborate it
# either: whatever happened to `ci` is invisible here, which is the case that
# must fail closed rather than be given the benefit of the doubt.
want "a cascade whose upstream is not graded here fails closed" 1 \
  '{"cascade":{"result":"skipped"},"ci-linux":{"result":"success"}}'
# The pass must SAY WHY it was allowed, or "skipped" and "skipped for a declared
# reason" are indistinguishable in the log exactly as they were in the code.
run_gate '{"ci":{"result":"skipped"},"ci-linux":{"result":"success"}}'
case "$OUT" in
  *"declared"*"if:"*) ok "an allowed skip names the declaration that allowed it" ;;
  *) bad "an allowed skip names the declaration that allowed it"
     printf '       | %s\n' "$OUT" ;;
esac

# THE COUNTERPART, and it is wayland#1291 c2. `build` carries no `if:` and no
# `needs:`, so it has no reason to skip; if it did, the workflow changed under
# the gate. Before this rule existed this payload exited 0.
want "a skip with NO declared reason reds the aggregate" 1 \
  '{"ci-linux":{"result":"success"},"build":{"result":"skipped"}}'
run_gate '{"ci-linux":{"result":"success"},"build":{"result":"skipped"}}'
case "$OUT" in
  *"::error"*"build"*"declares no condition"*)
    ok "the red for an undeclared skip names the job and the reason" ;;
  *)
    bad "the red for an undeclared skip names the job and the reason"
    printf '       | %s\n' "$OUT" ;;
esac
# CONTROL for the two cases above: the SAME job, same fixture, concluding
# `success` passes. Without this the red could be the fixture, not the skip.
want "control: the same undeclared job passes when it actually ran" 0 \
  '{"ci-linux":{"result":"success"},"build":{"result":"success"}}'

# A dependency the workflow does not define cannot have its skip explained, and
# a gate pointed at the wrong file must say so rather than wave it through.
want "a skipped dependency absent from the workflow fails closed" 1 \
  '{"ci-linux":{"result":"success"},"ghost":{"result":"skipped"}}'

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
OUT=$(env -u NEEDS_JSON WORKFLOW_FILE="$FIXTURE" bash "$GATE" 2>&1)
RC=$?
set -e
if [ "$RC" -eq 1 ]; then
  ok "an absent NEEDS_JSON fails closed"
else
  bad "an absent NEEDS_JSON fails closed (got exit=$RC)"
  printf '       | %s\n' "$OUT"
fi

# THE WORKFLOW INPUT FAILS CLOSED THE SAME WAY, AND ON EVERY RUN -- not only on
# runs where something skipped. A gate that discovers it cannot answer the
# question on the one run the answer matters has failed open for every run
# before it, so an all-green payload is used here on purpose.
set +e
OUT=$(env -u WORKFLOW_FILE NEEDS_JSON="$ALL_GREEN" bash "$GATE" 2>&1)
RC=$?
set -e
if [ "$RC" -eq 1 ]; then
  ok "an absent WORKFLOW_FILE fails closed even when nothing skipped"
else
  bad "an absent WORKFLOW_FILE fails closed even when nothing skipped (got exit=$RC)"
  printf '       | %s\n' "$OUT"
fi
want "a WORKFLOW_FILE that does not exist fails closed" 1 "$ALL_GREEN" "$WORK/absent.yml"
printf 'jobs: [this is\n  not: valid\n' > "$WORK/broken.yml"
want "a WORKFLOW_FILE that does not parse fails closed" 1 "$ALL_GREEN" "$WORK/broken.yml"
printf 'name: empty\non: push\n' > "$WORK/nojobs.yml"
want "a WORKFLOW_FILE declaring no jobs fails closed" 1 "$ALL_GREEN" "$WORK/nojobs.yml"

# THE REAL WORKFLOW IS THE ONE THAT SHIPS. Everything above runs against a
# fixture; this runs the gate against ci.yml itself with the payload a healthy
# run produces, so a rule that only holds on the fixture cannot pass here.
CI_WF="$ROOT/.github/workflows/ci.yml"
CI_NEEDS=$(python3 - "$CI_WF" <<'PY'
import json
import sys
import yaml
needs = yaml.safe_load(open(sys.argv[1]))["jobs"]["report"]["needs"]
print(json.dumps({job: {"result": "success"} for job in needs}))
PY
)
want "ci.yml's own dependency set passes when every leg succeeded" 0 "$CI_NEEDS" "$CI_WF"
# ...and the rationed leg really is declared in ci.yml, not just in the fixture.
CI_RATIONED=$(python3 - "$CI_WF" <<'PY'
import json
import sys
import yaml
doc = yaml.safe_load(open(sys.argv[1]))
jobs = doc["jobs"]
needs = jobs["report"]["needs"]
out = {}
for job in needs:
    spec = jobs[job]
    out[job] = {"result": "skipped" if spec.get("if") is not None else "success"}
print(json.dumps(out))
PY
)
want "ci.yml's rationed legs may skip without redding the required check" 0 "$CI_RATIONED"  "$CI_WF"
# ...while a leg that declares nothing may not, on the real file.
CI_ACCIDENT=$(python3 - "$CI_WF" <<'PY'
import json
import sys
import yaml
doc = yaml.safe_load(open(sys.argv[1]))
jobs = doc["jobs"]
needs = jobs["report"]["needs"]
victim = next(j for j in needs
              if jobs[j].get("if") is None and not jobs[j].get("needs"))
out = {job: {"result": "success"} for job in needs}
out[victim] = {"result": "skipped"}
print(json.dumps(out))
PY
)
want "a ci.yml leg with no declared condition may not skip silently" 1 "$CI_ACCIDENT" "$CI_WF"

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
SEVEN='{"a":{"result":"success"},"b":{"result":"success"},"c":{"result":"skipped"},"d":{"result":"success"},"e":{"result":"success"},"f":{"result":"skipped"},"g":{"result":"success"}}'
want "a seventh dependency needs no edit anywhere" 0 "$SEVEN"
run_gate "$SEVEN"
case "$OUT" in
  *"7 graded"*) ok "all seven are graded, not the first six" ;;
  *) bad "all seven are graded, not the first six"; printf '       | %s\n' "$OUT" ;;
esac

echo ""
echo "dependency-aggregate: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
