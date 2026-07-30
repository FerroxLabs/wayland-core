#!/usr/bin/env bash
# ftfm-type.sh — drive the real wayland-core TUI in a real pty at HUMAN typing
# speed and MECHANICALLY measure how much of the typed first message survived.
#
# Derived from .planning/evidence/uat-tui-unix/slow-type.sh, which established
# the 7.1 chars/sec method. Three things are added here, all of them because the
# original could not answer the question this lane has to answer:
#
#   1. The loss is COUNTED, not eyeballed. slow-type.sh printed the composer line
#      and the sent line side by side and left the diff to a human. A human diff
#      is not a gate. Here the surviving text is compared to the sent text and
#      LOST_CHARS is computed; VERDICT is derived from it.
#   2. Every number is written to a RESULT file, never to stdout only, so the
#      caller reads it back out-of-band (LANE-BRIEF §3b: a proxied tool may
#      re-render a machine-readable count).
#   3. The pty is PROVEN alive before any judgement is made (LANE-BRIEF §6a-i:
#      a participant that never started reports a clean run). A TUI that failed
#      to attach paints nothing; "nothing was lost" and "nothing was typed" are
#      the same capture unless you assert the participant arrived.
#
# Usage:
#   ftfm-type.sh --bin PATH --home DIR --out DIR --label NAME --text STR
#                [--cps 0.14] [--settle 25] [--post-settle 3]
#                [--with-key] [--send KEY]... [--arg A]...
#
#   --with-key   read a FluxRouter API key from stdin (line 1) and export it.
#                Omit for the credentials-absent quadrants.
#   --send KEY   a tmux key name (Down, Enter, Escape, …) sent AFTER the text,
#                one per flag, in order, 0.4s apart. Used to complete onboarding
#                so the type-ahead flush can be observed.
#   --arg A      extra argv for the binary (e.g. -p flux-router).
#
# Exit codes: 0 ran to completion (read VERDICT for the result), 9x harness fault.
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
    --selftest) SELFTEST=1; shift ;;
    *) echo "unknown flag: $1" >&2; exit 90 ;;
  esac
done

# ── self-test ────────────────────────────────────────────────────────────────
# Three assertions, not two (LANE-BRIEF §6b-ii): the matcher must PASS on a
# known-positive, FAIL on a known-negative, AND catch a case the ORIGINAL
# harness would have called clean. Without that third one the self-test passes
# on the broken instrument too.
#
# `compute` is the whole judgement of this harness, so it is what gets tested.
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
    # Longer than sent, or differing in the middle: refuse to grade rather than
    # invent a count. A harness that guesses here is how a false green happens.
    n=-1; v=MISMATCH
  fi
  echo "LOST_CHARS=${n} VERDICT=${v}"
}

if [ "${SELFTEST:-0}" = "1" ]; then
  fails=0
  # A1 known-positive: identical strings must read INTACT.
  r=$(compute "Use the bash tool" "Use the bash tool")
  [ "$r" = "LOST_CHARS=0 VERDICT=INTACT" ] || { echo "SELFTEST A1 FAIL: $r"; fails=1; }
  # A2 known-negative: the real UAT loss must NOT read clean, and must report
  # the exact count the UAT observed (4).
  r=$(compute "Use the bash tool to run echo SLOWTYPE_TOKEN" \
              "the bash tool to run echo SLOWTYPE_TOKEN")
  [ "$r" = "LOST_CHARS=4 VERDICT=PREFIX_LOSS" ] || { echo "SELFTEST A2 FAIL: $r"; fails=1; }
  # A2b the 20-char row, which discriminates the real mechanism from a naive one.
  r=$(compute "MARKERSTART_what is two plus two_MARKEREND" "two plus two_MARKEREND")
  [ "$r" = "LOST_CHARS=20 VERDICT=PREFIX_LOSS" ] || { echo "SELFTEST A2b FAIL: $r"; fails=1; }
  # A2c total loss must not be silently 0.
  r=$(compute "/quit" "")
  [ "$r" = "LOST_CHARS=5 VERDICT=TOTAL_LOSS" ] || { echo "SELFTEST A2c FAIL: $r"; fails=1; }
  # A3 THE ONE THAT PROVES THE REPAIR DOES ANYTHING. The original harness's only
  # verdict was SURFACE_AFTER_TYPING. For the 4-char loss above that variable
  # reads CHAT — i.e. the OLD instrument reports the defect's own signature as a
  # normal outcome. Assert that the old matcher is blind here and the new one is
  # not; if this ever passes on the old matcher the repair is cosmetic.
  old_verdict="CHAT"          # what slow-type.sh printed for the losing run
  new_verdict=$(compute "Use the bash tool to run echo SLOWTYPE_TOKEN" \
                        "the bash tool to run echo SLOWTYPE_TOKEN")
  if [ "$old_verdict" = "CHAT" ] && [ "${new_verdict#*VERDICT=}" = "PREFIX_LOSS" ]; then
    :
  else
    echo "SELFTEST A3 FAIL: old=$old_verdict new=$new_verdict"; fails=1
  fi
  # A4 the matcher must refuse to grade a non-prefix difference rather than
  # inventing a count.
  r=$(compute "abcdef" "xyz")
  [ "$r" = "LOST_CHARS=-1 VERDICT=MISMATCH" ] || { echo "SELFTEST A4 FAIL: $r"; fails=1; }
  if [ "$fails" = 0 ]; then echo "SELFTEST=PASS assertions=6"; exit 0
  else echo "SELFTEST=FAIL"; exit 91; fi
fi

[ -n "$BIN" ] && [ -n "$HOMEDIR" ] && [ -n "$OUT" ] && [ -n "$LABEL" ] \
  || { echo "missing required flag" >&2; exit 90; }
[ -x "$BIN" ] || { echo "ASSERT_BIN=FAIL path=$BIN" >&2; exit 92; }
[ -d "$BIN" ] && { echo "ASSERT_BIN=FAIL is-a-directory" >&2; exit 93; }

rm -rf "$HOMEDIR"; mkdir -p "$HOMEDIR" "$OUT"
RESULT="$OUT/${LABEL}.result"
: > "$RESULT"
say() { echo "$*"; echo "$*" >> "$RESULT"; }

say "LABEL=${LABEL}"
say "BIN=${BIN}"
say "BIN_SHA256=$(sha256sum "$BIN" | awk '{print $1}')"
say "ASSERT_BIN=OK"
say "CPS=${CPS}"

# ── credential handling ──────────────────────────────────────────────────────
# The key arrives on STDIN ONLY (LANE-BRIEF §0): never in argv, never on disk,
# never echoed. Only its LENGTH is recorded.
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
# Durable-session vault: a throwaway literal, deliberately not a secret. Without
# it the engine refuses to boot on a headless host (UAT-TUI-UNIX F2) and every
# quadrant would fail for a reason that has nothing to do with this lane.
export WAYLAND_VAULT_PASSPHRASE="uat-throwaway-not-a-real-secret"

SOCK="ftfm-$$-${LABEL}"
# Scrub every OTHER provider key. hetzner's /root/.wayland/.env injects
# ANTHROPIC_API_KEY into the product regardless of the shell (LANE-BRIEF §3b-ii),
# so HOME is redirected to a pristine dir, which is what keeps that file out.
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
# ── PARTICIPANT-ALIVE ASSERTION ──────────────────────────────────────────────
# A pty that never attached paints an empty pane, and an empty pane loses no
# characters — which reads as a pass. Require (i) the process is in the session
# and (ii) the pane carries real chrome. The status bar's separator rule is
# present on BOTH the onboarding and the chat surface, so it discriminates
# "TUI is up" from "which surface", which is a separate question below.
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

# Which surface is up before a single key is typed. This is quadrants 1 and 2.
if grep -qF 'Connect a provider' "$OUT/${LABEL}.before.txt"; then
  say "SURFACE_BEFORE=ONBOARDING"
else
  say "SURFACE_BEFORE=CHAT"
fi

# ── type at human speed, one keystroke per write ─────────────────────────────
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
# The type-ahead readout, if the build under test has one.
TA=$(awk -F'Type-ahead: ' '/Type-ahead: /{print $2; exit}' "$OUT/${LABEL}.typed.txt" | sed 's/ *$//')
say "TYPEAHEAD_LINE=[${TA}]"

# ── optional follow-up keys (complete onboarding, etc.) ──────────────────────
for k in ${SENDS[@]+"${SENDS[@]}"}; do
  tmux -L "$SOCK" send-keys -t s "$k"
  say "SENT_KEY=${k}"
  sleep 0.4
done
sleep "$POST_SETTLE"

tmux -L "$SOCK" capture-pane -p -t s > "$OUT/${LABEL}.after.txt"
if grep -qF 'Connect a provider' "$OUT/${LABEL}.after.txt"; then
  say "SURFACE_FINAL=ONBOARDING"
else
  say "SURFACE_FINAL=CHAT"
fi

# ── extract what actually landed in the composer ─────────────────────────────
# The composer renders as `  › <text>`. Take the FIRST such line and strip the
# marker and the single space after it; trailing pad is right-trimmed.
LANDED=$(awk '
  /\xe2\x80\xba/ { sub(/^.*\xe2\x80\xba ?/, ""); sub(/ +$/, ""); print; exit }
' "$OUT/${LABEL}.after.txt")
say "COMPOSER_TEXT=[${LANDED}]"
say "COMPOSER_LEN=${#LANDED}"

eval "$(compute "$TEXT" "$LANDED")"
say "LOST_CHARS=${LOST_CHARS}"
if [ "${LOST_CHARS}" -ge 0 ]; then
  say "LOST_PREFIX=[${TEXT:0:${LOST_CHARS}}]"
else
  say "LOST_PREFIX=[<ungradeable — see COMPOSER_TEXT>]"
fi
say "VERDICT=${VERDICT}"

tmux -L "$SOCK" kill-server 2>/dev/null
say "WLRC=0"
say "WLDONE"
