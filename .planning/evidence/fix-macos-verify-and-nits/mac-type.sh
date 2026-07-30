#!/usr/bin/env bash
# mac-type.sh — the fix-tui-first-message harness, ported to run on macOS.
#
# Derived from .planning/evidence/fix-tui-first-message/ftfm-type.sh, which is
# reused rather than reinvented (it already carries nine instrument repairs).
# THREE portability defects were found in it before first use and repaired here
# rather than written up and left (LANE-BRIEF §6b-ii):
#
#   P1. THE COMPOSER SCRAPER IS DEAD ON macOS, and it fails as a FALSE RED.
#       ftfm-type.sh extracts the composer with
#           awk '/\xe2\x80\xba/ { sub(/^.*\xe2\x80\xba ?/, ""); ... }'
#       BSD/onetrue awk (macOS ships "awk version 20200816") does NOT interpret
#       \xNN escapes in a regex. MEASURED on this Mac against a line that
#       plainly contains the marker:
#           printf '  \xe2\x80\xba hello world\n' | <that awk>   ->   ""
#       The `grep -q` guard above it DOES match (grep handles the raw bytes), so
#       the harness would take the COMPOSER_PRESENT=YES branch, scrape nothing,
#       fall through to GRADED_ON=nothing-on-screen and report TOTAL_LOSS.
#       i.e. running the Linux harness unmodified on the Mac would have reported
#       all three merged fixes BROKEN. Repaired with index()/substr and the
#       marker passed in as a variable, which is verified working on BOTH awks.
#
#   P2. `sha256sum` is not universally present on macOS. Wrapped.
#
#   P3. The self-test covered only `compute`, the pure-bash grader — so it went
#       GREEN on a platform where the extractor was dead. The whole judgement
#       of the harness is grader(extractor(pane)), and only the grader was
#       tested. Self-test now covers the EXTRACTOR too, against REAL captured
#       panes from the previous lane, with a known-positive AND a known-negative
#       AND the proof that the old matcher would have missed it here.
#
# Usage: identical to ftfm-type.sh.
#   mac-type.sh --bin PATH --home DIR --out DIR --label NAME --text STR
#               [--cps 0.14] [--settle 25] [--post-settle 3]
#               [--with-key] [--send KEY]... [--arg A]... [--seed-config F]
#   mac-type.sh --selftest
#
# Exit codes: 0 ran to completion (read VERDICT), 9x harness fault.
set -u

BIN=""; HOMEDIR=""; OUT=""; LABEL=""; TEXT=""
CPS="0.14"; SETTLE="25"; POST_SETTLE="3"; WITH_KEY=0
declare -a SENDS=(); declare -a ARGS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --bin) BIN="$2"; shift 2 ;;
    --home) HOMEDIR="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    --text) TEXT="$2"; shift 2 ;;
    --cps) CPS="$2"; shift 2 ;;
    --settle) SETTLE="$2"; shift 2 ;;
    --post-settle) POST_SETTLE="$2"; shift 2 ;;
    --with-key) WITH_KEY=1; shift ;;
    --send) SENDS+=("$2"); shift 2 ;;
    --arg) ARGS+=("$2"); shift 2 ;;
    --seed-config) SEED="$2"; shift 2 ;;
    --selftest) SELFTEST=1; shift ;;
    *) echo "unknown flag: $1" >&2; exit 90 ;;
  esac
done

# The composer marker, U+203A. Held as a variable and passed to awk with -v so
# no regex hex escape is ever needed. This is repair P1.
MARKER=$(printf '\xe2\x80\xba')

# Portable sha256 (repair P2).
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  elif command -v shasum   >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}'
  else echo "UNAVAILABLE"; fi
}

# THE EXTRACTOR (repair P1). Prints the composer text on the first row carrying
# the marker, or nothing. Portable across BSD awk and gawk.
extract_composer() {
  awk -v m="$MARKER" '
    index($0, m) {
      s = substr($0, index($0, m) + length(m))
      sub(/^ /, "", s); sub(/ +$/, "", s)
      print s; exit
    }' "$1"
}

# The OLD extractor, kept verbatim ONLY so the self-test can prove whether the
# repair is load-bearing on this platform. Never used for grading.
extract_composer_old() {
  awk '/\xe2\x80\xba/ { sub(/^.*\xe2\x80\xba ?/, ""); sub(/ +$/, ""); print; exit }' "$1"
}

# `compute` is the grader, carried over unchanged from ftfm-type.sh.
compute() {
  # $1 = sent, $2 = landed  ->  prints "LOST_CHARS=<n> VERDICT=<v>"
  local sent="$1" got="$2" n v
  if [ "$sent" = "$got" ]; then
    n=0; v=INTACT
  elif [ -z "$got" ]; then
    n=${#sent}; v=TOTAL_LOSS
  elif [ "${#got}" -lt "${#sent}" ] && [ "${sent: -${#got}}" = "$got" ]; then
    n=$(( ${#sent} - ${#got} )); v=PREFIX_LOSS
  else
    n=-1; v=MISMATCH
  fi
  echo "LOST_CHARS=${n} VERDICT=${v}"
}

# ── self-test ────────────────────────────────────────────────────────────────
if [ "${SELFTEST:-0}" = "1" ]; then
  fails=0; n_assert=0
  a() { n_assert=$((n_assert+1)); }

  # ── Part A: the grader (carried over from ftfm-type.sh) ──
  a; r=$(compute "Use the bash tool" "Use the bash tool")
  [ "$r" = "LOST_CHARS=0 VERDICT=INTACT" ] || { echo "SELFTEST A1 FAIL: $r"; fails=1; }
  a; r=$(compute "Use the bash tool to run echo SLOWTYPE_TOKEN" \
                 "the bash tool to run echo SLOWTYPE_TOKEN")
  [ "$r" = "LOST_CHARS=4 VERDICT=PREFIX_LOSS" ] || { echo "SELFTEST A2 FAIL: $r"; fails=1; }
  a; r=$(compute "MARKERSTART_what is two plus two_MARKEREND" "two plus two_MARKEREND")
  [ "$r" = "LOST_CHARS=20 VERDICT=PREFIX_LOSS" ] || { echo "SELFTEST A2b FAIL: $r"; fails=1; }
  a; r=$(compute "/quit" "")
  [ "$r" = "LOST_CHARS=5 VERDICT=TOTAL_LOSS" ] || { echo "SELFTEST A2c FAIL: $r"; fails=1; }
  a; old_verdict="CHAT"
  new_verdict=$(compute "Use the bash tool to run echo SLOWTYPE_TOKEN" \
                        "the bash tool to run echo SLOWTYPE_TOKEN")
  [ "$old_verdict" = "CHAT" ] && [ "${new_verdict#*VERDICT=}" = "PREFIX_LOSS" ] \
    || { echo "SELFTEST A3 FAIL: old=$old_verdict new=$new_verdict"; fails=1; }
  a; r=$(compute "abcdef" "xyz")
  [ "$r" = "LOST_CHARS=-1 VERDICT=MISMATCH" ] || { echo "SELFTEST A4 FAIL: $r"; fails=1; }
  a; ta="Use the bash tool   (kept for your first message · ⌫ edit · ⎋ discard)"
  h="${ta%%   (kept for your first message*}"; [ "$h" = "$ta" ] && h=""
  [ "$h" = "Use the bash tool" ] || { echo "SELFTEST A5 FAIL: [$h]"; fails=1; }
  a; ta="no readout on this build"
  h="${ta%%   (kept for your first message*}"; [ "$h" = "$ta" ] && h=""
  [ -z "$h" ] || { echo "SELFTEST A5b FAIL: [$h]"; fails=1; }
  a; r=$(compute "Use the bash tool to run echo Q3TOKEN" "Use the bash tool to run echo Q3TOKEN")
  [ "${r#*VERDICT=}" = "INTACT" ] \
    || { echo "SELFTEST A6 FAIL: readout must be gradeable: $r"; fails=1; }

  # ── Part B: THE EXTRACTOR (new — this is repair P3) ──
  # Graded against REAL captured panes from lane/fix-tui-first-message, so the
  # fixture is a genuine product render, not something this script wrote.
  FIX_DIR="$(cd "$(dirname "$0")/../fix-tui-first-message" 2>/dev/null && pwd)"
  POS="$FIX_DIR/after/Q1-keys-present.after.txt"
  NEG="$FIX_DIR/before/BEFORE-q23-nokeys.after.txt"
  EXPECT="Use the bash tool to run echo SLOWTYPE_TOKEN"

  a
  if [ ! -f "$POS" ] || [ ! -f "$NEG" ]; then
    echo "SELFTEST B0 FAIL: fixtures missing (POS=$POS NEG=$NEG)"; fails=1
  fi

  # B1 KNOWN-POSITIVE: a real pane with a composer must yield its exact text.
  a; got=$(extract_composer "$POS" 2>/dev/null)
  [ "$got" = "$EXPECT" ] || { echo "SELFTEST B1 FAIL: extractor got [$got] want [$EXPECT]"; fails=1; }

  # B2 KNOWN-NEGATIVE: a real pane with NO composer must yield nothing. Without
  # this, an extractor that returns the whole line always would pass B1.
  a; got=$(extract_composer "$NEG" 2>/dev/null)
  [ -z "$got" ] || { echo "SELFTEST B2 FAIL: extractor invented [$got] on a composer-less pane"; fails=1; }

  # B3 THE ONE THAT PROVES THE REPAIR DOES ANYTHING ON THIS PLATFORM.
  # Compare against the OLD matcher on the same known-positive fixture.
  #   - old blind  -> this platform needed the repair; assert the new one sees it.
  #   - old works  -> this platform was unaffected; assert the new one AGREES,
  #                   i.e. the repair introduced no regression.
  # Both branches assert something falsifiable, and the branch taken is
  # reported so a reader knows which platform they are looking at.
  a; old_got=$(extract_composer_old "$POS" 2>/dev/null)
  if [ "$old_got" != "$EXPECT" ]; then
    echo "SELFTEST B3 branch=OLD_MATCHER_BLIND_HERE old=[$old_got]"
    new_got=$(extract_composer "$POS" 2>/dev/null)
    [ "$new_got" = "$EXPECT" ] \
      || { echo "SELFTEST B3 FAIL: repair does not fix the platform it was written for"; fails=1; }
  else
    echo "SELFTEST B3 branch=OLD_MATCHER_WORKS_HERE"
    new_got=$(extract_composer "$POS" 2>/dev/null)
    [ "$new_got" = "$old_got" ] \
      || { echo "SELFTEST B3 FAIL: repair regressed a platform that was fine: [$new_got] vs [$old_got]"; fails=1; }
  fi

  # B4 END-TO-END: extractor feeding the grader on the real positive fixture
  # must produce INTACT. This is the composed judgement the harness actually
  # makes, and neither Part A nor B1 alone exercises it.
  a; got=$(extract_composer "$POS" 2>/dev/null)
  r=$(compute "$EXPECT" "$got")
  [ "$r" = "LOST_CHARS=0 VERDICT=INTACT" ] || { echo "SELFTEST B4 FAIL: $r"; fails=1; }

  # B5 END-TO-END the other way: the composer-less pane must grade as a loss,
  # not as a pass. This is the CAN-IT-FAIL control for the composed judgement.
  a; got=$(extract_composer "$NEG" 2>/dev/null)
  r=$(compute "$EXPECT" "$got")
  [ "${r#*VERDICT=}" = "TOTAL_LOSS" ] || { echo "SELFTEST B5 FAIL: $r"; fails=1; }

  if [ "$fails" = 0 ]; then echo "SELFTEST=PASS assertions=${n_assert}"; exit 0
  else echo "SELFTEST=FAIL assertions=${n_assert}"; exit 91; fi
fi

[ -n "$BIN" ] && [ -n "$HOMEDIR" ] && [ -n "$OUT" ] && [ -n "$LABEL" ] \
  || { echo "missing required flag" >&2; exit 90; }
[ -x "$BIN" ] || { echo "ASSERT_BIN=FAIL path=$BIN" >&2; exit 92; }
[ -d "$BIN" ] && { echo "ASSERT_BIN=FAIL is-a-directory" >&2; exit 93; }

rm -rf "$HOMEDIR"; mkdir -p "$HOMEDIR" "$OUT"
if [ -n "${SEED:-}" ]; then
  CFG=$(HOME="$HOMEDIR" "$BIN" --config-path 2>/dev/null | head -1)
  [ -n "$CFG" ] || { echo "SEED_PATH=FAIL"; exit 96; }
  mkdir -p "$(dirname "$CFG")"
  cp "$SEED" "$CFG"
  echo "SEEDED_CONFIG=$CFG"
  [ -s "$CFG" ] || { echo "SEED_WRITE=FAIL"; exit 96; }
fi
RESULT="$OUT/${LABEL}.result"
: > "$RESULT"
say() { echo "$*"; echo "$*" >> "$RESULT"; }

say "LABEL=${LABEL}"
say "PLATFORM=$(uname -s) $(uname -m)"
say "BIN=${BIN}"
say "BIN_SHA256=$(sha256_of "$BIN")"
# Provenance from the binary's OWN self-report, not from what we think we built
# (LANE-BRIEF: UAT-TUI-UNIX hit a binary that predated its own checkout).
say "BUILD_INFO=[$("$BIN" --build-info 2>&1 | tr '\n' ' ' | sed 's/  */ /g')]"
say "ASSERT_BIN=OK"
say "CPS=${CPS}"

if [ "$WITH_KEY" = "1" ]; then
  IFS= read -r FLUX_API_KEY
  [ -n "${FLUX_API_KEY:-}" ] || { say "ASSERT_KEY=FAIL"; exit 97; }
  export FLUX_API_KEY
  say "ASSERT_KEY=OK len=${#FLUX_API_KEY}"
  say "CREDENTIALS=PRESENT"
else
  unset FLUX_API_KEY
  say "CREDENTIALS=ABSENT"
fi
export WAYLAND_VAULT_PASSPHRASE="uat-throwaway-not-a-real-secret"

SOCK="mtype-$$-${LABEL}"
EXTRA=""
if [ "${#ARGS[@]}" -gt 0 ]; then EXTRA="${ARGS[*]}"; fi
say "BIN_ARGS=[${EXTRA}]"
tmux -L "$SOCK" new-session -d -s s -x 120 -y 40 \
  "env -u OPENAI_API_KEY -u ANTHROPIC_API_KEY -u GEMINI_API_KEY -u GOOGLE_API_KEY \
       -u GROQ_API_KEY -u OPENROUTER_API_KEY -u DEEPSEEK_API_KEY -u XAI_API_KEY \
       -u MOONSHOT_API_KEY HOME=$HOMEDIR $BIN $EXTRA" \
  || { say "ASSERT_TMUX=FAIL"; exit 94; }

say "SETTLE=${SETTLE}"
sleep "$SETTLE"

tmux -L "$SOCK" capture-pane -p -t s > "$OUT/${LABEL}.before.txt" 2>/dev/null
PANE_LINES=$(awk 'NF{n++} END{print n+0}' "$OUT/${LABEL}.before.txt")
PANES=$(tmux -L "$SOCK" list-panes -t s -F '#{pane_dead}' 2>/dev/null | head -1)
say "PANE_NONEMPTY_LINES=${PANE_LINES}"
say "PANE_DEAD=${PANES:-unknown}"
if [ "${PANE_LINES:-0}" -lt 3 ] || [ "${PANES:-1}" != "0" ]; then
  say "ASSERT_PTY=FAIL"
  say "WLRC=95"; say "WLDONE"
  tmux -L "$SOCK" kill-server 2>/dev/null; exit 95
fi
say "ASSERT_PTY=OK"

if grep -qF 'Connect a provider' "$OUT/${LABEL}.before.txt"; then
  say "SURFACE_BEFORE=ONBOARDING"
else
  say "SURFACE_BEFORE=CHAT"
fi

say "SENT_TEXT=[${TEXT}]"
say "SENT_LEN=${#TEXT}"
i=0
while [ "$i" -lt "${#TEXT}" ]; do
  tmux -L "$SOCK" send-keys -t s -l "${TEXT:$i:1}"
  i=$((i+1))
  sleep "$CPS"
done
sleep 2
tmux -L "$SOCK" capture-pane -p -t s > "$OUT/${LABEL}.typed.txt"
if grep -qF 'Connect a provider' "$OUT/${LABEL}.typed.txt"; then
  say "SURFACE_AFTER_TYPING=ONBOARDING"
else
  say "SURFACE_AFTER_TYPING=CHAT"
fi
TA=$(awk -F'Type-ahead: ' '/Type-ahead: /{print $2; exit}' "$OUT/${LABEL}.typed.txt" | sed 's/ *$//')
say "TYPEAHEAD_LINE=[${TA}]"
HELD="${TA%%   (kept for your first message*}"
[ "$HELD" = "$TA" ] && HELD=""
say "TYPEAHEAD_HELD=[${HELD}]"
say "TYPEAHEAD_HELD_LEN=${#HELD}"

for k in ${SENDS[@]+"${SENDS[@]}"}; do
  tmux -L "$SOCK" send-keys -t s "$k"
  say "SENT_KEY=${k}"
  sleep 0.4
done
sleep "$POST_SETTLE"

tmux -L "$SOCK" capture-pane -p -t s > "$OUT/${LABEL}.after.txt"
FINAL=CHAT
grep -qF 'Connect a provider' "$OUT/${LABEL}.after.txt" && FINAL=ONBOARDING
say "SURFACE_FINAL=${FINAL}"

if [ "$FINAL" = "ONBOARDING" ]; then
  LANDED=""
  say "COMPOSER_PRESENT=NO reason=onboarding-modal-still-up"
elif ! grep -qF "$MARKER" "$OUT/${LABEL}.after.txt"; then
  LANDED=""
  say "COMPOSER_PRESENT=NO reason=no-composer-on-workspace"
else
  say "COMPOSER_PRESENT=YES"
  LANDED=$(extract_composer "$OUT/${LABEL}.after.txt")
  [ "$LANDED" = "type / for commands" ] && LANDED=""
fi
say "COMPOSER_TEXT=[${LANDED}]"
say "COMPOSER_LEN=${#LANDED}"

if [ -n "$LANDED" ]; then
  GRADED_ON=composer; GOT="$LANDED"
elif [ -n "$HELD" ]; then
  GRADED_ON=typeahead-readout; GOT="$HELD"
else
  GRADED_ON=nothing-on-screen; GOT=""
fi
say "GRADED_ON=${GRADED_ON}"
eval "$(compute "$TEXT" "$GOT")"
say "LOST_CHARS=${LOST_CHARS}"
if [ "${LOST_CHARS}" -ge 0 ]; then
  say "LOST_PREFIX=[${TEXT:0:${LOST_CHARS}}]"
else
  say "LOST_PREFIX=[<ungradeable — see COMPOSER_TEXT / TYPEAHEAD_HELD>]"
fi
if [ "$VERDICT" = "INTACT" ] && [ "$GRADED_ON" = "typeahead-readout" ]; then
  VERDICT=INTACT_HELD
elif [ "$VERDICT" = "INTACT" ]; then
  VERDICT=INTACT_DELIVERED
fi
say "VERDICT=${VERDICT}"

tmux -L "$SOCK" kill-server 2>/dev/null
say "WLRC=0"
say "WLDONE"
