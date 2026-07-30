#!/usr/bin/env bash
# noise-help.sh — measure --help / -h size and internal-identifier leakage.
#
# LANE-BRIEF compliance: numbers go to a file the caller reads (§3b); a
# known-POSITIVE and a known-NEGATIVE run in the same capture (§3b-i); status is
# WLRC/WLDONE (§3.2). The identifier matcher is the SAME regex the Rust
# regression test uses, so the shell measurement and the test cannot drift.
#
# usage: noise-help.sh <label> <binary> <outdir>
set -u
LABEL="${1:?label}"; BIN="${2:?binary}"; OUT="${3:?outdir}"
mkdir -p "$OUT"; R="$OUT/$LABEL"; : > "$R.result"
say() { echo "$1" >> "$R.result"; }

# Internal-identifier patterns. Deliberately anchored on shapes that ONLY occur
# in our own sprint vocabulary, so a legitimate word cannot trip them:
#   F-089 / F-092        dash-numbered finding ids
#   F23-02 / F24-B       phase-dash ids
#   W9.1 / W7-N / W5     wave ids
#   T4 / T11             task ids (word-bounded)
#   23A-C1 / 22-C3       phase-criterion ids
#   Phase 23B            literal phase references
#   M3.4 / M5.2          milestone ids
#   v0.6.4 Task 2.4      release-task references
IDRE='(\bF-[0-9]{3}\b|\bF[0-9]{2}-[0-9A-Z]+\b|\bW[0-9]+(\.[0-9]+)?(-[A-Z])?\b|\bT[0-9]{1,2}\b|\b[0-9]{2}[A-Z]?-C[0-9]+\b|\bPhase [0-9]+[A-Z]?\b|\bM[0-9]\.[0-9]\b|Task [0-9]+\.[0-9]+|\bA[0-9]+[a-z]?\b)'

say "LABEL=$LABEL"
say "BIN=$BIN"
say "BIN_SHA256=$(sha256sum "$BIN" | cut -d' ' -f1)"
say "BUILD_INFO=$("$BIN" --build-info 2>/dev/null | tr '\n' ' ')"

"$BIN" --help > "$R.help.txt" 2>&1; say "HELP_RC=$?"
"$BIN" -h     > "$R.h.txt"    2>&1; say "H_RC=$?"

HELP_LINES=$(wc -l < "$R.help.txt" | tr -d ' ')
H_LINES=$(wc -l < "$R.h.txt" | tr -d ' ')
say "HELP_LINES=$HELP_LINES"
say "H_LINES=$H_LINES"

# ── participant-alive: --help that printed nothing has zero ids, trivially ────
if [ "$HELP_LINES" -lt 5 ]; then
  say "ASSERT_HELP_RAN=FAIL_help_under_5_lines"
  say "WLRC=95"; say "WLDONE"; exit 95
fi
say "ASSERT_HELP_RAN=OK"

# ── control pair (§3b-i): the matcher must be alive in BOTH directions ───────
# POSITIVE: a seeded line containing a known internal id MUST match.
# NEGATIVE: a line of ordinary help prose MUST NOT match.
printf 'F-089: model catalog commands\n' > "$R.ctrlpos.txt"
printf 'Print config file path and exit\n' > "$R.ctrlneg.txt"
say "CTRL_POS_HITS=$(/usr/bin/grep -cE "$IDRE" "$R.ctrlpos.txt" || true)"
say "CTRL_NEG_HITS=$(/usr/bin/grep -cE "$IDRE" "$R.ctrlneg.txt" || true)"
# A third control the LANE-BRIEF §6b-ii repair rule demands: prove --help itself
# is greppable at all, with a string that must be there.
say "CTRL_USAGE_HITS=$(/usr/bin/grep -c 'Usage' "$R.help.txt" || true)"

say "HELP_ID_LINES=$(/usr/bin/grep -cE "$IDRE" "$R.help.txt" || true)"
say "H_ID_LINES=$(/usr/bin/grep -cE "$IDRE" "$R.h.txt" || true)"
/usr/bin/grep -nE "$IDRE" "$R.help.txt" > "$R.help.ids.txt" 2>&1 || true
/usr/bin/grep -noE "$IDRE" "$R.help.txt" | cut -d: -f2- | sort | uniq -c | sort -rn > "$R.help.ids.tally.txt" 2>&1 || true
say "HELP_ID_TOKENS=$(wc -l < "$R.help.ids.tally.txt" | tr -d ' ')"

# ── subcommand help surfaces too — --help is not the only user-facing text ───
for sub in plugin session workflow; do
  "$BIN" "$sub" --help > "$R.sub-$sub.txt" 2>&1 || true
  say "SUB_${sub}_LINES=$(wc -l < "$R.sub-$sub.txt" | tr -d ' ')"
  say "SUB_${sub}_ID_LINES=$(/usr/bin/grep -cE "$IDRE" "$R.sub-$sub.txt" || true)"
done

say "WLRC=0"
say "WLDONE"
exit 0
