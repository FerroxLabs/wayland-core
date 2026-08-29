#!/usr/bin/env bash
# WHOLE-RUN TRUTH FOR THE NIGHTLY WINDOWS SOAK TRACKER. FerroxLabs/wayland-core#325.
#
# ── THE DEFECT THIS CLOSES ─────────────────────────────────────────────────
#
# `.github/workflows/nightly-windows-soak.yml` used to report AND close its
# tracker issue from steps inside the `windows-soak` job:
#
#     - name: Close the failure issue on a green soak
#       if: success()                # <-- JOB-scoped, not RUN-scoped
#
# `success()` in a step condition means "this job has not failed so far". It
# cannot see any other job. The workflow has two more test jobs
# (`keyring-blob-size`, `windows-live-acceptance`), and neither fed the
# tracker. Measured consequences, both verified on real runs:
#
#   1. Reds that produced no issue. Runs 32103885715 / 32220451402 /
#      32336640709 each show `success :: Windows soak` +
#      `failure :: Windows live-acceptance`. Three consecutive reds, zero
#      issues opened.
#   2. Laundering. Run 33053333326 (2026-08-27) had run conclusion FAILURE and
#      still CLOSED issue #319 at 08:56:03Z, because the reporting job itself
#      was green. A tracker that closes on a signal it does not measure does
#      not track — it launders.
#
# ── THE RULE ───────────────────────────────────────────────────────────────
#
# An issue may be closed ONLY when the WHOLE run genuinely passed, and a
# decision that cannot see the whole run closes NOTHING. That is a fail-closed
# rule in the close direction and a fail-open rule in the report direction:
# opening an issue we did not strictly need is cheap, closing one we should not
# have closed is exactly the harm above.
#
# ── INPUTS (env) ───────────────────────────────────────────────────────────
#
#   JOB_RESULTS     newline-separated `<job-id>=<result>` pairs, one per gating
#                   job, built from `needs.<job>.result` by the caller.
#   REQUIRED_JOBS   whitespace-separated job ids that MUST appear in
#                   JOB_RESULTS. This is the guard against the original bug
#                   returning by omission: adding a fourth test job and
#                   forgetting to add it to `needs:` would otherwise silently
#                   restore a partial view. `.github/scripts/tests/soak-tracker-truth.test.sh`
#                   additionally lints this list against the workflow's own
#                   job list, so the two cannot drift.
#
# ── OUTPUTS ────────────────────────────────────────────────────────────────
#
# `action=<close|report|none>` and `reason=<slug>` on stdout, and appended to
# $GITHUB_OUTPUT when set. Exit 0 for a conclusive decision (close/report) and
# for an honestly inconclusive one (a cancelled or skipped run). Exit 1 — loud,
# because it is a WIRING defect and not a product one — when the roster is
# empty, incomplete, or contains a result this script cannot interpret.
#
# Self-tests: .github/scripts/tests/soak-tracker-truth.test.sh
set -uo pipefail

JOB_RESULTS="${JOB_RESULTS:-}"
REQUIRED_JOBS="${REQUIRED_JOBS:-}"

emit() { # emit <action> <reason>
  printf 'action=%s\n' "$1"
  printf 'reason=%s\n' "$2"
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    {
      printf 'action=%s\n' "$1"
      printf 'reason=%s\n' "$2"
    } >>"$GITHUB_OUTPUT"
  fi
}

echo "-- soak tracker decision (core#325) -----------------------------------"

SEEN_NAMES=""
COUNT=0
FAILED=0
UNREADABLE=0
NONSUCCESS=0

while IFS= read -r raw || [ -n "$raw" ]; do
  line=$(printf '%s' "$raw" | tr -d '\r' | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')
  case "$line" in '') continue ;; esac

  name=${line%%=*}
  result=${line#*=}

  if [ "$name" = "$line" ] || [ -z "$name" ]; then
    echo "::error title=Unreadable soak roster::'${line}' is not a <job-id>=<result> pair."
    UNREADABLE=$((UNREADABLE + 1))
    continue
  fi

  COUNT=$((COUNT + 1))
  SEEN_NAMES="${SEEN_NAMES}${name}"$'\n'

  case "$result" in
    success)
      printf '  %-28s %s\n' "$name" "$result"
      ;;
    failure)
      printf '  %-28s %s\n' "$name" "$result"
      FAILED=$((FAILED + 1))
      NONSUCCESS=$((NONSUCCESS + 1))
      ;;
    cancelled | skipped)
      printf '  %-28s %s\n' "$name" "$result"
      NONSUCCESS=$((NONSUCCESS + 1))
      ;;
    *)
      # An empty string is what `needs.<job>.result` expands to when the job id
      # does not exist — a typo in `needs:` reads exactly like a green job to
      # anything doing a naive `!= 'failure'` test. It must never close.
      printf '  %-28s %s  <-- uninterpretable\n' "$name" "${result:-<empty>}"
      echo "::error title=Uninterpretable job result::job '${name}' reported '${result:-<empty>}', which is not one of success/failure/cancelled/skipped. A result this script cannot read is a result it cannot certify."
      UNREADABLE=$((UNREADABLE + 1))
      NONSUCCESS=$((NONSUCCESS + 1))
      ;;
  esac
done <<EOF
${JOB_RESULTS}
EOF

MISSING=""
for want in $REQUIRED_JOBS; do
  if ! printf '%s' "$SEEN_NAMES" | grep -qxF -- "$want"; then
    MISSING="${MISSING}${want} "
  fi
done

echo ""
echo "jobs read      : ${COUNT}"
echo "failed         : ${FAILED}"
echo "uninterpretable: ${UNREADABLE}"
echo "required       : ${REQUIRED_JOBS:-<none declared>}"

if [ "$COUNT" -eq 0 ]; then
  echo "::error title=Empty soak roster::No job results were supplied, so this run's outcome is unknown. Closing nothing (core#325)."
  emit none empty-roster
  exit 1
fi

if [ -n "$MISSING" ]; then
  echo "::error title=Incomplete soak roster::required job(s) missing from needs/JOB_RESULTS: ${MISSING%% }. A tracker that cannot see every test job is the core#325 defect returning by omission. Closing nothing."
  emit none missing-required
  exit 1
fi

# Order matters. A definite failure outranks an uninterpretable sibling: we
# know at least one thing went red, so the tracker must say so.
if [ "$FAILED" -gt 0 ]; then
  emit report job-failed
  exit 0
fi

if [ "$UNREADABLE" -gt 0 ]; then
  emit none unreadable
  exit 1
fi

if [ "$NONSUCCESS" -eq 0 ]; then
  emit close all-green
  exit 0
fi

echo "no failures, but not every job succeeded (cancelled/skipped): closing nothing, reporting nothing."
emit none not-conclusive
exit 0
