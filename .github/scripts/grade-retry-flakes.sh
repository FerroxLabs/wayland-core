#!/usr/bin/env bash
# A TEST THAT NEEDED A RETRY IS A SIGNAL, NOT SILENCE. FerroxLabs/wayland#1169.
#
# ── THE DEFECT THIS CLOSES ─────────────────────────────────────────────────
#
# `.config/nextest.toml` sets `retries = 2` for the `ci` profile, which every
# CI leg runs under. A test that fails and then passes on a retry is reported
# `FLAKY n/3`, counted in the PASSED total, and the run concludes SUCCESS.
# MEASURED (#1155/#1169): a TOCTOU data-loss race failed 13 of 200 runs at
# `--retries 0` — 6.5 % — and 0.065^3 is roughly one visible report in 3,600
# runs. The suite was not passing because the code was correct; it was passing
# because the policy retried until it stopped failing.
#
# It also biases every survey built on run conclusions: a retried failure
# reports the run as SUCCESS, so "green history" is not evidence of absence.
#
# ── WHY THIS SHAPE, AND NOT `retries = 0` ──────────────────────────────────
#
# Retries exist because genuinely flaky infrastructure (shared-runner
# contention, container startup, a 96-core box under load) would otherwise
# block every merge, and that pressure is real: this repo has named,
# load-dependent tests that fail under contention and pass in isolation on a
# byte-identical tree. Turning CI permanently red is a worse outcome than the
# current state. So the retry still RUNS — the suite still finishes, the merge
# is not blocked mid-flight by one transient hiccup — and what changes is that
# the retry is recorded, attributed to a named test, and graded.
#
# Nor is this a fourth copy of the scoped `retries = 0` override pattern
# already in .config/nextest.toml (for #1109, #1101, #1146). That pattern is
# correct and stays; its limit is that YOU HAVE TO ALREADY KNOW a test is
# flaking to write one. Nothing surfaced the first flake, because the evidence
# only ever appeared inside a green run's log and nobody reads a green run's
# log. This is the discovery half. The two compose cleanly: a zero-retry test
# produces a plain failure, never a `<flakyFailure>`, so this gate never sees
# it and cannot double-report it.
#
# ── WHERE IT RUNS ──────────────────────────────────────────────────────────
#
# In the aggregate `report` job (ci.yml), a REQUIRED status context on main,
# invoked from .github/scripts/assert-test-evidence.sh so no workflow wiring
# has to be duplicated or kept in sync. That job downloads EVERY leg's JUnit —
# self-hosted matrix, hosted Windows, linux-containerized — so one grading pass
# covers all of them. It also runs from e2e.yml's report job, where it is a
# no-op by construction: `[profile.e2e] retries = 0`, so that suite cannot
# emit a `<flakyFailure>` at all.
#
# ── WHAT NEXTEST ACTUALLY WRITES ───────────────────────────────────────────
#
# REPRODUCED on hetzner-dsm against a test that panics on attempt 1 and passes
# on attempt 2, under `retries = 2`:
#
#     <testsuites name="nextest-run" tests="2" failures="0" errors="0" ...>
#       <testsuite name="probe" tests="2" failures="0">
#         <testcase name="flaky_on_first_attempt" classname="probe" ...>
#           <flakyFailure ... message="...panicked at src/lib.rs:9:17"
#                         type="test failure with exit code 101">...
#
# Note `failures="0"` at BOTH levels. That is the whole problem in one
# attribute: every downstream consumer of this file — dorny/test-reporter, the
# job conclusion, a human skimming — reads zero. The retried attempt is
# recorded only as a `<flakyFailure>` child, which nothing was reading. One
# `<flakyFailure>` element = one failed attempt.
#
# ── INPUTS (env) ───────────────────────────────────────────────────────────
#
#   EVIDENCE_DIR       directory the JUnit artifacts were downloaded into.
#                      Absent or empty is exit 0, NOT a pass: the absence of
#                      test signal is assert-test-evidence.sh's gate, and
#                      duplicating it here would report one defect as two.
#   FLAKE_ALLOWLIST    path to the allowlist (default: <repo>/.config/flaky-allowlist.txt)
#   FLAKE_GATE_TODAY   YYYY-MM-DD used for expiry comparison. Injectable so the
#                      self-tests can exercise both sides of an expiry boundary
#                      without depending on the day they are run — a test whose
#                      verdict changes with the calendar is a test that will
#                      one day be wrong with nothing having changed.
#
# Exit 0 when every recorded flake is on a live allowlist entry, 1 otherwise.
#
# Self-tests: .github/scripts/tests/assert-test-evidence.test.sh (the flake
# section). Wiring: .github/scripts/tests/report-gate-wiring.test.sh.
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)

EVIDENCE_DIR="${EVIDENCE_DIR:-}"
ALLOWLIST="${FLAKE_ALLOWLIST:-$ROOT/.config/flaky-allowlist.txt}"
TODAY="${FLAKE_GATE_TODAY:-$(date -u +%F)}"

echo "-- retry-flake gate (wayland#1169) ------------------------------------"
echo "evidence dir : ${EVIDENCE_DIR:-<unset>}"
echo "allowlist    : $ALLOWLIST"
echo "today (UTC)  : $TODAY"

if [ -z "$EVIDENCE_DIR" ] || [ ! -d "$EVIDENCE_DIR" ]; then
  echo "no evidence directory - nothing to grade (the absence of test signal is assert-test-evidence.sh's gate, not this one)."
  exit 0
fi

# ── 1. Extract every retried test from every leg's JUnit ────────────────────
#
# awk, not grep: a `<flakyFailure>` has to be attributed to its ENCLOSING
# `<testcase>`, and a bare `grep -c '<flakyFailure'` gives a count with no test
# name attached — which is the current state of affairs, only in a different
# file. The key is `classname::name`, which is what nextest prints on its own
# `FLAKY` line, so a name from this gate can be pasted straight into a
# `-E 'test(=...)'` filter or into the allowlist.
#
# THE LEADING SPACE IN THE ATTRIBUTE MATCHER IS LOAD-BEARING: `name="[^"]*"`
# also matches inside `classname="..."`, so without it a `<testcase>` whose
# attributes appeared in the other order would be keyed by its classname twice.
#
# A self-closing `<testcase .../>` cannot contain a child, so its state is
# dropped immediately rather than left open for the next element to inherit.
# `FNR==1` resets between files for the same reason. Test names and binary ids
# are Rust identifiers plus `::`, so XML escaping inside them is not a concern;
# a panic `message` can contain anything, which is why nothing here parses it.
FLAKES=$(
  find "$EVIDENCE_DIR" -type f -name "*.xml" -print0 2>/dev/null |
  xargs -0 -r awk '
    function attr(line, key,   s) {
      if (match(line, "[ \t]" key "=\"[^\"]*\"")) {
        s = substr(line, RSTART, RLENGTH)
        return substr(s, length(key) + 4, length(s) - length(key) - 4)
      }
      return ""
    }
    FNR == 1 { name = ""; cls = ""; flaky = 0 }
    /<testcase[ \t>]/ {
      name = attr($0, "name"); cls = attr($0, "classname"); flaky = 0
      if ($0 ~ /\/>[ \t]*$/) { name = ""; cls = "" }
      next
    }
    /<flakyFailure/ { if (name != "") flaky++ ; next }
    /<\/testcase>/ {
      if (flaky > 0 && name != "") printf "%s::%s\t%d\n", cls, name, flaky
      name = ""; cls = ""; flaky = 0
      next
    }
  ' |
  awk -F'\t' '{ n[$1] += $2 } END { for (k in n) printf "%s\t%d\n", k, n[k] }' |
  sort
)

# ── 2. Read and VALIDATE the allowlist ─────────────────────────────────────
#
# Validated, not merely read. An allowlist that silently tolerates a malformed
# line is an allowlist that allows everything on the day someone fat-fingers a
# date, and it would fail in the safe-looking direction (green) — the exact
# class of defect this gate exists to close.
ALLOW_KEYS=""
BAD_ENTRIES=0
EXPIRED_ENTRIES=0

if [ ! -f "$ALLOWLIST" ]; then
  echo "allowlist file absent - grading with an EMPTY allowlist (fail-closed)."
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
      echo "::error title=Malformed flake allowlist::${ALLOWLIST}:${lineno} does not start with a YYYY-MM-DD expiry (got '${expiry}'). Format: <expiry> <binary-id>::<test-name> <gh#NNNN> <reason>."
      BAD_ENTRIES=$((BAD_ENTRIES + 1)); continue
    fi
    if [ -z "$key" ] || ! printf '%s' "$key" | grep -q '::'; then
      echo "::error title=Malformed flake allowlist::${ALLOWLIST}:${lineno} has no <binary-id>::<test-name> key (got '${key}'). Copy it verbatim from the FLAKY line nextest printed."
      BAD_ENTRIES=$((BAD_ENTRIES + 1)); continue
    fi
    if ! printf '%s' "$issue" | grep -qE '^(gh)?#[0-9]+$'; then
      echo "::error title=Unowned flake allowlist entry::${ALLOWLIST}:${lineno} names no owning issue (got '${issue}'). An entry nobody owns is a retry nobody will ever remove."
      BAD_ENTRIES=$((BAD_ENTRIES + 1)); continue
    fi
    if [ -z "$(printf '%s' "$reason" | tr -d '[:space:]')" ]; then
      echo "::error title=Unjustified flake allowlist entry::${ALLOWLIST}:${lineno} states no reason. The justification is the point of the file."
      BAD_ENTRIES=$((BAD_ENTRIES + 1)); continue
    fi

    # String comparison, correct for zero-padded ISO-8601 and needing no `date`
    # binary — BSD and GNU `date` disagree on everything except this format.
    if [ "$expiry" \< "$TODAY" ]; then
      echo "::error title=Expired flake allowlist entry::${ALLOWLIST}:${lineno} expired on ${expiry} (today is ${TODAY}): ${key} (${issue}). Fix the test, or renew the entry with a fresh date and a reason that is still true. An entry that never expires is a permanent exemption, which is what this gate exists to prevent."
      EXPIRED_ENTRIES=$((EXPIRED_ENTRIES + 1)); continue
    fi

    ALLOW_KEYS="${ALLOW_KEYS}${key}"$'\n'
  done < "$ALLOWLIST"
fi

# ── 3. Grade ───────────────────────────────────────────────────────────────
UNLISTED=0
LISTED=0
TOTAL_ATTEMPTS=0

if [ -n "$FLAKES" ]; then
  echo ""
  echo "tests that needed a retry:"
  while IFS=$'\t' read -r key attempts; do
    if [ -z "$key" ]; then continue; fi
    TOTAL_ATTEMPTS=$((TOTAL_ATTEMPTS + attempts))
    if printf '%s' "$ALLOW_KEYS" | grep -qxF -- "$key"; then
      LISTED=$((LISTED + 1))
      printf "  ALLOWED  %-3s failed attempt(s)  %s\n" "$attempts" "$key"
      echo "::warning title=Known-flaky test retried::${key} needed ${attempts} retry attempt(s). It is on ${ALLOWLIST} with an expiry, so it does not fail this run - but it is still a failing test, and the entry is debt with a date on it."
    else
      UNLISTED=$((UNLISTED + 1))
      printf "  RED      %-3s failed attempt(s)  %s\n" "$attempts" "$key"
      echo "::error title=Retried failure (wayland#1169)::${key} FAILED ${attempts} time(s) and was retried into a pass, so the run conclusion would have said SUCCESS. Reproduce with: cargo nextest run --retries 0 -E 'test(=${key##*::})' - repeated, because an intermittent failure needs n runs and not one. If it is a real defect, fix it or give it a scoped 'retries = 0' override in .config/nextest.toml so the failure reaches the conclusion. If it is genuinely infrastructure noise, add a dated, owned, justified line to ${ALLOWLIST}."
    fi
  done <<< "$FLAKES"
else
  echo ""
  echo "tests that needed a retry: none"
fi

echo ""
echo "flaky tests    : $((LISTED + UNLISTED))  (allowlisted: ${LISTED}, unlisted: ${UNLISTED})"
echo "failed attempts: ${TOTAL_ATTEMPTS}"
echo "allowlist      : bad entries ${BAD_ENTRIES}, expired ${EXPIRED_ENTRIES}"

if [ "$UNLISTED" -gt 0 ] || [ "$BAD_ENTRIES" -gt 0 ] || [ "$EXPIRED_ENTRIES" -gt 0 ]; then
  echo "::error title=Retry-flake gate FAILED::${UNLISTED} test(s) were retried into a pass without an allowlist entry, ${BAD_ENTRIES} allowlist entr(ies) are malformed and ${EXPIRED_ENTRIES} have expired. A retried failure is a signal, not silence (wayland#1169)."
  exit 1
fi
exit 0
