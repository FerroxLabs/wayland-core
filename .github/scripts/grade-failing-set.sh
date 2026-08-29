#!/usr/bin/env bash
# A COUNT OF FAILURES IS NOT A SET OF FAILURES. FerroxLabs/wayland-core#367.
#
# ── THE DEFECT THIS CLOSES ─────────────────────────────────────────────────
#
# `8d6add71 RED ARM (throwaway, never merge)` — a ten-line instrument that
# reduced `OwnedTree::snapshot` to leaf-only ownership behind
# `std::hint::black_box(true)` — was merged into `integ/f13` and survived three
# further commits. A workspace `nextest` on that tree reported ONE failing
# test. This repository has a standing known failure, so `1 failed` was read as
# `the known 1 failed`. It was a different test: the guard's own regression
# case, `dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child`,
# and what shipped was `wayland#1156` reintroduced — a spawned process tree that
# outlives its guard.
#
# Nothing lied. `cargo check` was clean, `clippy -D warnings` was clean (the
# `black_box` was chosen precisely so the dead code below it is not
# `unreachable_code`), and the test run said exactly what was true. The failure
# was that a CARDINAL was compared where an IDENTITY was needed.
#
# This gate compares the failing-test SET, by name, against a named allow-list.
# Two runs with the same failure COUNT and different failure SETS are different
# runs, and this is the thing that says so.
#
# ── WHY NOT A GREP FOR THE INSTRUMENT ──────────────────────────────────────
#
# The ticket's own first suggestion — fail when a shipped source file contains
# `black_box(true)` or a `RED ARM` comment — is WITHDRAWN, and deliberately not
# implemented here. `RED ARM` is a legitimate doc-comment idiom on dozens of
# real tests in this tree, so it produces false positives on honest code; and
# the next instrument spells itself `cfg!(all())`, or a `const`, or an early
# `return` with no marker at all. That is a game of spellings, and a half-guard
# buys false coverage, which is worse than a documented gap. A test IDENTITY is
# exact and has no spellings.
#
# ── WHY NOT scripts/flake-ledger.py, AND NOT grade-retry-flakes.sh ─────────
#
# `scripts/flake-ledger.py` re-measures a NAMED set of tests at `retries = 0`
# and answers whether their failures are load-dependent. It starts from a list
# you already have. This gate answers the prior question: is the failing set the
# expected one at all.
#
# `.github/scripts/grade-retry-flakes.sh` grades `<flakyFailure>` — a test that
# failed and was RETRIED INTO A PASS — against `.config/flaky-allowlist.txt`.
# A test that simply FAILED emits `<failure>`, never `<flakyFailure>`, so that
# gate cannot see this class by construction. The two are complements and share
# no state but the evidence directory.
#
# ── WHERE IT RUNS ──────────────────────────────────────────────────────────
#
# From `.github/scripts/assert-test-evidence.sh`, which both aggregate `report`
# jobs invoke — `report` being a REQUIRED status context on main. It therefore
# sees EVERY leg's uploaded JUnit in one pass. Locally: `just failing-set-gate`
# after any `--profile ci` run.
#
# Self-tests: .github/scripts/tests/failing-set.test.sh (run by lint.yml).
# Wiring:     .github/scripts/tests/report-gate-wiring.test.sh.
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)

EVIDENCE_DIR="${EVIDENCE_DIR:-}"
ALLOWLIST="${KNOWN_FAILING_LIST:-$ROOT/.config/known-failing-tests.txt}"
TODAY="${FAILING_SET_TODAY:-$(date -u +%F)}"

echo "-- failing-set gate (wayland-core#367) --------------------------------"
echo "evidence dir : ${EVIDENCE_DIR:-<unset>}"
echo "allowlist    : $ALLOWLIST"
echo "today (UTC)  : $TODAY"

if [ -z "$EVIDENCE_DIR" ] || [ ! -d "$EVIDENCE_DIR" ]; then
  echo "no evidence directory - nothing to grade (the absence of test signal is assert-test-evidence.sh's gate, not this one)."
  exit 0
fi

# ── 1. Extract the failing SET, and the passing set, from every leg ────────
#
# `outer-attempt-*.xml` is EXCLUDED. Those files are attempts that an outer
# `nick-fields/retry` discarded; `grade-retry-flakes.sh` already grades them
# against the retry allowlist, and re-grading them here would report one
# failure twice under two different policies with two different remedies.
#
# awk, not grep: a `<failure>` has to be attributed to its ENCLOSING
# `<testcase>`. `grep -c '<failure'` gives a COUNT WITH NO NAME ATTACHED, which
# is the state of affairs this gate exists to end, only in a different file.
#
# The key is `classname::name`, exactly what nextest prints on its own FAIL
# line, so a name from this gate pastes straight into `-E 'test(=...)'`.
#
# THE LEADING SPACE IN THE ATTRIBUTE MATCHER IS LOAD-BEARING: `name="[^"]*"`
# also matches inside `classname="..."`. And the matcher is applied to the
# `<testcase ...>` TAG, never to the whole line: on the compact single-line XML
# that arm 9 of the self-test feeds it, `<testsuite name="probe">` sits on the
# same line and a whole-line matcher keys every case in the file by the SUITE's
# name. That is not hypothetical — it is what this reader did on its first run.
#
# A self-closing `<testcase .../>` is a PASS with no children; it is recorded as
# such immediately rather than left open for the next element to inherit.
# `<flakyFailure>` is NOT counted as a failure here — it is a retried pass, and
# it is the other gate's subject.
extract() { # extract <want: fail|pass>
  find "$EVIDENCE_DIR" -type f -name "*.xml" ! -name "outer-attempt-*.xml" -print0 2>/dev/null |
  xargs -0 -r awk -v want="$1" '
    function tag(line,   i, s, j) {
      i = index(line, "<testcase")
      if (i == 0) return ""
      s = substr(line, i)
      j = index(s, ">")
      return (j > 0) ? substr(s, 1, j) : s
    }
    function attr(t, key,   s) {
      if (match(t, "[ \t]" key "=\"[^\"]*\"")) {
        s = substr(t, RSTART, RLENGTH)
        return substr(s, length(key) + 4, length(s) - length(key) - 4)
      }
      return ""
    }
    function emit(  verdict) {
      if (name == "") return
      verdict = (failed > 0) ? "fail" : "pass"
      if (verdict == want) printf "%s::%s\n", cls, name
      name = ""; cls = ""; failed = 0
    }
    FNR == 1 { name = ""; cls = ""; failed = 0 }
    {
      line = $0
      if (line ~ /<testcase[ \t>]/) {
        emit()
        t = tag(line)
        name = attr(t, "name"); cls = attr(t, "classname"); failed = 0
        # A self-closing `<testcase .../>` is a PASS with no children. Detected
        # on the TAG, not at end of line: a compact report puts the next
        # element after it.
        if (t ~ /\/>$/) { emit(); next }
      }
      # Counted on the SAME line as the <testcase> too: nextest writes multi-line
      # XML, but a compact single-line report must not grade as clean. A rule
      # that skips the rest of the line is a grader that fails open on
      # whitespace. `<flakyFailure` does not contain `<failure`, so the two
      # matchers cannot collide.
      if (name != "") failed += gsub(/<failure|<error/, "&", line)
      if (line ~ /<\/testcase>/) emit()
    }
    END { emit() }
  ' | sort -u
}

FAILING=$(extract fail)
PASSING=$(extract pass)

FAIL_COUNT=$(printf '%s' "$FAILING" | grep -c . || true)
PASS_COUNT=$(printf '%s' "$PASSING" | grep -c . || true)

# NAME THEM. Even on a clean run. The whole ticket is that a number was printed
# where names were needed, and a gate that only speaks when it is angry trains
# the reader to accept the number again on the next run.
echo ""
echo "failing tests in this evidence set: ${FAIL_COUNT}"
if [ "$FAIL_COUNT" -gt 0 ]; then
  printf '  %s\n' $FAILING
fi

# ── 2. Read and VALIDATE the allow-list ────────────────────────────────────
#
# Validated, not merely read. An allow-list that tolerates a malformed line
# allows everything on the day someone fat-fingers a date, and it fails in the
# safe-looking direction (green) — the exact class of defect this gate closes.
#
# Format, identical to .config/flaky-allowlist.txt so there is one shape to
# learn:  <YYYY-MM-DD expiry>  <binary-id>::<test-name>  <gh#NNNN>  <reason...>
ALLOWED=""
BAD_ENTRIES=0
EXPIRED_ENTRIES=0

if [ ! -f "$ALLOWLIST" ]; then
  echo ""
  echo "allowlist file absent - grading with an EMPTY allowlist (fail-closed: every failure is unexpected)."
else
  lineno=0
  while IFS= read -r raw || [ -n "$raw" ]; do
    lineno=$((lineno + 1))
    line=$(printf '%s' "$raw" | tr -d '\r' | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')
    case "$line" in ''|'#'*) continue ;; esac

    expiry=$(printf '%s\n' "$line" | awk '{print $1}')
    key=$(printf '%s\n' "$line" | awk '{print $2}')
    issue=$(printf '%s\n' "$line" | awk '{print $3}')
    reason=$(printf '%s\n' "$line" | awk '{ $1=""; $2=""; $3=""; sub(/^[ \t]+/, ""); print }')

    if ! printf '%s' "$expiry" | grep -qE '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'; then
      echo "::error title=Malformed known-failure allowlist::${ALLOWLIST}:${lineno} does not start with a YYYY-MM-DD expiry (got '${expiry}'). Format: <expiry> <binary-id>::<test-name> <gh#NNNN> <reason>."
      BAD_ENTRIES=$((BAD_ENTRIES + 1)); continue
    fi
    if [ -z "$key" ] || ! printf '%s' "$key" | grep -q '::'; then
      echo "::error title=Malformed known-failure allowlist::${ALLOWLIST}:${lineno} has no <binary-id>::<test-name> key (got '${key}'). Copy it verbatim from the line nextest printed."
      BAD_ENTRIES=$((BAD_ENTRIES + 1)); continue
    fi
    if ! printf '%s' "$issue" | grep -qE '^(gh)?#[0-9]+$'; then
      echo "::error title=Unowned known-failure entry::${ALLOWLIST}:${lineno} names no owning issue (got '${issue}'). A failure nobody owns is a failure nobody will ever fix."
      BAD_ENTRIES=$((BAD_ENTRIES + 1)); continue
    fi
    if [ -z "$(printf '%s' "$reason" | tr -d '[:space:]')" ]; then
      echo "::error title=Unjustified known-failure entry::${ALLOWLIST}:${lineno} states no reason. The justification is the point of the file."
      BAD_ENTRIES=$((BAD_ENTRIES + 1)); continue
    fi

    # String comparison, correct for zero-padded ISO-8601 and needing no `date`
    # binary — BSD and GNU `date` disagree on everything except this format.
    if [ "$expiry" \< "$TODAY" ]; then
      echo "::error title=Expired known-failure entry::${ALLOWLIST}:${lineno} expired on ${expiry} (today is ${TODAY}): ${key} (${issue}). Fix the test, or renew the entry with a fresh date and a reason that is still true. An entry that never expires is a permanent exemption."
      EXPIRED_ENTRIES=$((EXPIRED_ENTRIES + 1)); continue
    fi

    ALLOWED="${ALLOWED}${key}"$'\n'
  done < "$ALLOWLIST"
fi

# ── 3. Grade the SET, in both directions ───────────────────────────────────
#
# UNEXPECTED — failed, not on the list. This is the #367 defect: it is exactly
# the case a count cannot distinguish, because swapping one failure for another
# leaves the count unchanged.
#
# STALE — on the list, RAN, and PASSED. An allow-list that outlives its failure
# is a licence for the next unexpected failure to hide behind it, so it is red
# too, and the remedy is one deleted line.
#
# NOT COLLECTED — on the list, absent from this evidence set. NOT an error: a
# platform-gated or feature-gated test legitimately does not appear on every
# leg, and reddening that would make the gate unpassable on a partial run.
# Reported, so the reader can see the list is not being exercised.
UNEXPECTED=0
EXPECTED=0
STALE=0
NOTCOLLECTED=0

if [ "$FAIL_COUNT" -gt 0 ]; then
  echo ""
  echo "grading the failing set:"
  while IFS= read -r key; do
    [ -z "$key" ] && continue
    if printf '%s' "$ALLOWED" | grep -qxF -- "$key"; then
      EXPECTED=$((EXPECTED + 1))
      printf "  KNOWN       %s\n" "$key"
      echo "::warning title=Known failing test::${key} failed and is named on ${ALLOWLIST} with an expiry, so it does not fail this run - but it is still a failing test and the entry is debt with a date on it."
    else
      UNEXPECTED=$((UNEXPECTED + 1))
      printf "  UNEXPECTED  %s\n" "$key"
      echo "::error title=Unexpected failing test (wayland-core#367)::${key} FAILED and is NOT on ${ALLOWLIST}. Do not read this run's failure count and stop: this is a DIFFERENT test from the known ones, and that difference is invisible to a count - it is how a never-merge red-arm instrument reached integ/f13 and shipped a leaking process tree. Reproduce with: cargo nextest run --retries 0 -E 'test(=${key##*::})'. Fix it, or - only if it is genuinely known and owned - add a dated, owned, justified line to ${ALLOWLIST}."
    fi
  done <<< "$FAILING"
fi

if [ -n "$(printf '%s' "$ALLOWED" | tr -d '[:space:]')" ]; then
  echo ""
  echo "grading the allowlist against this evidence set:"
  while IFS= read -r key; do
    [ -z "$key" ] && continue
    if printf '%s' "$FAILING" | grep -qxF -- "$key"; then
      continue
    elif printf '%s' "$PASSING" | grep -qxF -- "$key"; then
      STALE=$((STALE + 1))
      printf "  STALE       %s\n" "$key"
      echo "::error title=Stale known-failure entry (wayland-core#367)::${key} is named on ${ALLOWLIST} but it RAN AND PASSED in this evidence set. Delete the line. An allowlist entry that outlives its failure is a hole the next unexpected failure fits through."
    else
      NOTCOLLECTED=$((NOTCOLLECTED + 1))
      printf "  not-collected %s\n" "$key"
    fi
  done <<< "$ALLOWED"
fi

echo ""
echo "test cases seen: $((FAIL_COUNT + PASS_COUNT))  (failing: ${FAIL_COUNT}, passing: ${PASS_COUNT})"
echo "failing set    : expected ${EXPECTED}, UNEXPECTED ${UNEXPECTED}"
echo "allowlist      : stale ${STALE}, not-collected ${NOTCOLLECTED}, bad ${BAD_ENTRIES}, expired ${EXPIRED_ENTRIES}"

if [ "$UNEXPECTED" -gt 0 ] || [ "$STALE" -gt 0 ] || [ "$BAD_ENTRIES" -gt 0 ] || [ "$EXPIRED_ENTRIES" -gt 0 ]; then
  echo "::error title=Failing-set gate FAILED::${UNEXPECTED} unexpected failing test(s), ${STALE} stale allowlist entr(ies), ${BAD_ENTRIES} malformed and ${EXPIRED_ENTRIES} expired. The failing SET differs from the allowed SET; a matching COUNT is not a matching run (wayland-core#367)."
  exit 1
fi
exit 0
