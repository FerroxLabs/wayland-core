#!/usr/bin/env bash
# Self-tests for .github/scripts/grade-failing-set.sh (wayland-core#367).
#
# THE CASE THIS FILE EXISTS FOR is `same_count_different_test`: an evidence set
# with exactly as many failures as the allow-list allows, but a DIFFERENT test
# failing, must go RED. That is the shape that let a never-merge red-arm
# instrument reach `integ/f13` — `1 failed` read as `the known 1 failed` — and a
# gate that cannot distinguish it is the gate we already had.
#
# Every arm asserts an EXIT CODE, and the green arms are as load-bearing as the
# red ones: a gate that reds on everything is not a gate.
#
# Run: bash .github/scripts/tests/failing-set.test.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
GATE="$ROOT/.github/scripts/grade-failing-set.sh"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

PASS=0
FAIL=0
ok()  { PASS=$((PASS + 1)); printf "ok   %s\n" "$1"; }
bad() { FAIL=$((FAIL + 1)); printf "FAIL %s\n" "$1"; }

# A JUnit in the exact shape cargo-nextest writes: multi-line, `classname`
# before `name`, `<failure>` as a child element, passes self-closing.
junit() { # junit <dir> <failing-ids...> -- <passing-ids...>
  local dir="$1"; shift
  mkdir -p "$dir"
  {
    printf '<?xml version="1.0" encoding="UTF-8"?>\n'
    printf '<testsuites name="nextest-run" tests="0" failures="0" errors="0">\n'
    printf '  <testsuite name="probe" tests="0" failures="0">\n'
    local mode=fail
    for id in "$@"; do
      if [ "$id" = "--" ]; then mode=pass; continue; fi
      local cls="${id%::*}" nm="${id##*::}"
      if [ "$mode" = fail ]; then
        printf '    <testcase classname="%s" name="%s" time="0.1">\n' "$cls" "$nm"
        printf '      <failure message="assertion failed" type="test failure with exit code 101">stack</failure>\n'
        printf '    </testcase>\n'
      else
        printf '    <testcase classname="%s" name="%s" time="0.1" />\n' "$cls" "$nm"
      fi
    done
    printf '  </testsuite>\n</testsuites>\n'
  } > "$dir/junit.xml"
}

allow() { # allow <file> <lines...>
  local f="$1"; shift
  : > "$f"
  printf '# fixture allowlist\n' >> "$f"
  for l in "$@"; do printf '%s\n' "$l" >> "$f"; done
}

run_gate() { # run_gate <evidence-dir> <allowlist>  -> sets RC and OUT
  OUT=$(EVIDENCE_DIR="$1" KNOWN_FAILING_LIST="$2" FAILING_SET_TODAY=2026-01-01 bash "$GATE" 2>&1)
  RC=$?
}

expect() { # expect <label> <want-rc> <evidence> <allowlist> [<must-contain>]
  run_gate "$3" "$4"
  if [ "$RC" -ne "$2" ]; then
    bad "$1 (exit $RC, wanted $2)"; printf '%s\n' "$OUT" | sed 's/^/     | /'
    return
  fi
  if [ -n "${5:-}" ] && ! printf '%s' "$OUT" | grep -qF -- "$5"; then
    bad "$1 (exit $RC as wanted, but output never says: $5)"
    printf '%s\n' "$OUT" | sed 's/^/     | /'
    return
  fi
  ok "$1"
}

KNOWN='wcore-exec-backend::conformance_matrix::every_reference_backend_passes_the_same_harness_or_reports_why_it_did_not'
OTHER='wcore-cli::harness_owns_spawned_trees::dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child'
LINE_KNOWN="2027-01-01  $KNOWN  gh#367  fixture entry"

# ── 1. THE CASE. Same COUNT (1), different TEST. Must be RED. ──────────────
D="$TMP/samecount"; A="$TMP/samecount.txt"
junit "$D" "$OTHER"
allow "$A" "$LINE_KNOWN"
expect "same COUNT, different TEST goes RED" 1 "$D" "$A" "UNEXPECTED  $OTHER"

# ── 1b. ...and the arm that proves 1 is not vacuous: the SAME count with the
#        SAME test is GREEN, on the same fixture shape. Without this, arm 1
#        would also pass against a gate that simply always fails.
D="$TMP/samecount_same"; A="$TMP/samecount_same.txt"
junit "$D" "$KNOWN"
allow "$A" "$LINE_KNOWN"
expect "same COUNT, same TEST stays GREEN" 0 "$D" "$A" "KNOWN       $KNOWN"

# ── 2. A clean run is green, and NAMES its (empty) failing set. ────────────
D="$TMP/clean"; A="$TMP/clean.txt"
junit "$D" -- "$KNOWN" "$OTHER"
allow "$A"
expect "no failures, empty allowlist is GREEN" 0 "$D" "$A" "failing tests in this evidence set: 0"

# ── 3. An unlisted failure with an EMPTY allowlist is red (fail-closed). ───
D="$TMP/unlisted"; A="$TMP/unlisted.txt"
junit "$D" "$OTHER"
allow "$A"
expect "unlisted failure with empty allowlist is RED" 1 "$D" "$A" "Unexpected failing test"

# ── 4. Two failures where one is allowed: still red, and only for the other.
D="$TMP/mixed"; A="$TMP/mixed.txt"
junit "$D" "$KNOWN" "$OTHER"
allow "$A" "$LINE_KNOWN"
expect "one known + one unexpected is RED" 1 "$D" "$A" "expected 1, UNEXPECTED 1"

# ── 5. STALE: allow-listed, ran, PASSED. The list must shrink. ─────────────
D="$TMP/stale"; A="$TMP/stale.txt"
junit "$D" -- "$KNOWN"
allow "$A" "$LINE_KNOWN"
expect "an allowlisted test that PASSED is RED (stale entry)" 1 "$D" "$A" "STALE       $KNOWN"

# ── 6. NOT COLLECTED is not stale: a platform-gated test absent from this
#       leg's report must not red the run.
D="$TMP/absent"; A="$TMP/absent.txt"
junit "$D" -- "$OTHER"
allow "$A" "$LINE_KNOWN"
expect "an allowlisted test absent from the report is GREEN" 0 "$D" "$A" "not-collected $KNOWN"

# ── 7. A retried pass (<flakyFailure>) is NOT this gate's failure. ─────────
#       grade-retry-flakes.sh owns it; double-reporting one failure under two
#       policies with two remedies is how a reader learns to ignore both.
D="$TMP/flaky"; A="$TMP/flaky.txt"; mkdir -p "$D"
cat > "$D/junit.xml" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="1" failures="0" errors="0">
  <testsuite name="probe" tests="1" failures="0">
    <testcase classname="wcore-cli::probe" name="flaky_on_first_attempt" time="0.3">
      <flakyFailure message="panicked" type="test failure with exit code 101">stack</flakyFailure>
    </testcase>
  </testsuite>
</testsuites>
XML
allow "$A"
expect "a retried pass is not graded here" 0 "$D" "$A" "failing tests in this evidence set: 0"

# ── 8. Malformed / expired / unowned allow-list entries are red. An
#       allowlist nobody can parse is an allowlist that allows everything.
D="$TMP/bad"; junit "$D" -- "$KNOWN"
A="$TMP/bad1.txt"; allow "$A" "nodate  $KNOWN  gh#367  reason"
expect "a missing expiry date is RED" 1 "$D" "$A" "Malformed known-failure allowlist"
A="$TMP/bad2.txt"; allow "$A" "2027-01-01  notakey  gh#367  reason"
expect "a key with no :: is RED" 1 "$D" "$A" "has no <binary-id>::<test-name> key"
A="$TMP/bad3.txt"; allow "$A" "2027-01-01  $KNOWN  nobody  reason"
expect "an unowned entry is RED" 1 "$D" "$A" "Unowned known-failure entry"
A="$TMP/bad4.txt"; allow "$A" "2027-01-01  $KNOWN  gh#367"
expect "an entry with no reason is RED" 1 "$D" "$A" "Unjustified known-failure entry"
A="$TMP/bad5.txt"; allow "$A" "2020-01-01  $KNOWN  gh#367  long expired"
expect "an expired entry is RED" 1 "$D" "$A" "Expired known-failure entry"

# ── 9. Compact single-line XML must not grade as clean. A rule that reads
#       `<failure>` only on its own line is a grader that fails open on
#       whitespace.
D="$TMP/compact"; A="$TMP/compact.txt"; mkdir -p "$D"
printf '<testsuites><testsuite name="probe"><testcase classname="p" name="t"><failure message="x">y</failure></testcase></testsuite></testsuites>\n' > "$D/junit.xml"
allow "$A"
expect "a single-line <testcase><failure> is seen" 1 "$D" "$A" "UNEXPECTED  p::t"

# ── 10. POSITIVE CONTROL FOR THE READER ITSELF. An evidence dir the gate
#        cannot read must not be reported as a clean run by this gate; and a
#        missing dir is explicitly deferred to assert-test-evidence.sh, which
#        is the gate that owns "the suite never ran".
D="$TMP/nonexistent-dir"; A="$TMP/pc.txt"; allow "$A"
expect "a missing evidence dir defers, and says so" 0 "$D" "$A" "nothing to grade"

# ── 11. outer-attempt-*.xml is excluded (grade-retry-flakes.sh owns it). ───
D="$TMP/outer"; A="$TMP/outer.txt"; mkdir -p "$D"
junit "$D" -- "$KNOWN"
cp "$D/junit.xml" "$D/tmp.xml"
junit "$TMP/outer-src" "$OTHER"
mv "$TMP/outer-src/junit.xml" "$D/outer-attempt-1.xml"
mv "$D/tmp.xml" "$D/junit.xml"
allow "$A"
expect "a discarded outer attempt is not graded here" 0 "$D" "$A" "failing tests in this evidence set: 0"

echo ""
echo "passed: $PASS   failed: $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
