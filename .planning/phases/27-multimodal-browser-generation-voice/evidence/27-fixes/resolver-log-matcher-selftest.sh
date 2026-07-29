#!/usr/bin/env bash
# §6b-ii — instrument repair + self-test for the resolver-log matcher.
#
# THE DEFECT (found in this lane, in this lane's own harness — the SECOND such
# defect here, and the same under-detection class as the first):
#   The live driver graded the fix with
#       grep -c 'transcription: using flux-router' positive.stderr.txt
#   The product never emits that string. The tracing call is
#       "transcription: using {} at {} (active OpenAI-wire provider)", model, endpoint
#   so the real line is `transcription: using flux-voice-fast at https://...`.
#   The matcher reported RESOLVER_CHOSE_FLUX=0 against a log that contains the
#   resolver line TWICE — i.e. it reported absence while the evidence was present,
#   which is exactly what a sibling lane did with a wrapped console line.
#
#   Note the matcher was written against an ASSUMED log format rather than an
#   observed one. That is the root cause, and the repair matches on the observed
#   string from the real captured log, kept here as the fixture.
#
# Run: ./resolver-log-matcher-selftest.sh   (no network, no credential, no spend)
set -uo pipefail
cd "$(dirname "$0")"

POS_LOG="live-out/positive.stderr.txt"     # real captured stderr, key-swept
[ -f "$POS_LOG" ] || { echo "FATAL: fixture $POS_LOG missing" >&2; exit 2; }

TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
# Known-negative fixture: the pre-fix observable, verbatim from the defect report.
printf '%s\n' "2026-07-29T00:00:00Z  WARN transcription: no API key found (GROQ_API_KEY or OPENAI_API_KEY) — tool hidden" > "$TMP/hidden.log"

# --- OLD, broken matcher: assumed format, anchored on a string never emitted ---
old_matcher() { grep -c 'transcription: using flux-router' "$1" 2>/dev/null || true; }

# --- REPAIRED matcher --------------------------------------------------------
# Matches the OBSERVED line. Joins wrapped console lines first so a terminal
# newline inside the phrase cannot hide it, and avoids a pipeline stealing status.
resolved_flux() {
  tr -d '\r' < "$1" | tr '\n' ' ' \
    | grep -o -i -- 'transcription: using flux-voice-fast at https://[^ ]*/audio/transcriptions' \
    | wc -l | tr -d ' '
}
said_hidden() {
  tr -d '\r' < "$1" | tr '\n' ' ' \
    | grep -o -i -- 'transcription: no API key found' | wc -l | tr -d ' '
}

fail=0

# ASSERT 1 — known positive: repaired matcher FINDS the resolver line in the
# real captured log (which contains it twice).
n=$(resolved_flux "$POS_LOG")
if [ "$n" -ge 1 ]; then
  echo "ASSERT_1_KNOWN_POSITIVE=PASS (repaired matcher found the resolver line ${n}x in the real log)"
else
  echo "ASSERT_1_KNOWN_POSITIVE=FAIL (found $n)"; fail=1
fi

# ASSERT 2 — known negative: on the pre-fix 'tool hidden' log the repaired
# matcher finds NO flux resolution, and does detect the hidden line.
n_flux=$(resolved_flux "$TMP/hidden.log"); n_hid=$(said_hidden "$TMP/hidden.log")
if [ "$n_flux" -eq 0 ] && [ "$n_hid" -ge 1 ]; then
  echo "ASSERT_2_KNOWN_NEGATIVE=PASS (flux=0, hidden=$n_hid on the pre-fix log)"
else
  echo "ASSERT_2_KNOWN_NEGATIVE=FAIL (flux=$n_flux hidden=$n_hid)"; fail=1
fi

# ASSERT 3 — the one that proves the repair does anything:
# the OLD matcher returns 0 on the SAME real log where the repaired one finds the
# line, i.e. it under-detected and would have graded a working fix as a failure.
old=$(old_matcher "$POS_LOG"); new=$(resolved_flux "$POS_LOG")
if [ "$old" -eq 0 ] && [ "$new" -ge 1 ]; then
  echo "ASSERT_3_OLD_MATCHER_MISSED_IT=PASS (old matcher found 0 in a log where the repaired one found $new)"
else
  echo "ASSERT_3_OLD_MATCHER_MISSED_IT=FAIL (old=$old new=$new)"; fail=1
fi

[ "$fail" -eq 0 ] && echo "SELFTEST_RESULT=PASS" || echo "SELFTEST_RESULT=FAIL"
exit "$fail"
