#!/usr/bin/env bash
# boot-timing.sh — how long does a first-time user stare at the splash screen?
# Polls a real tmux pty once a second and records the first frame in which the
# onboarding surface is present.
#
# Anti-self-pass: the NEEDLE is proven findable before timing starts (a frame that
# never matches would report "never appeared", which is the free answer). The
# splash marker is used as the known-positive: it MUST be seen at least once,
# otherwise we are not looking at this program's output at all.
set -u
BIN="$1"; HOMEDIR="$2"; OUT="$3"; MAXS="${4:-90}"

[ -x "$BIN" ] || { echo "ASSERT_BIN=FAIL path=$BIN"; exit 92; }
rm -rf "$HOMEDIR"; mkdir -p "$HOMEDIR" "$OUT"
SOCK="boot-$$"

NEEDLE_READY="Connect a provider"
NEEDLE_SPLASH="starting engine"

tmux -L "$SOCK" new-session -d -s b -x 120 -y 40 \
  "env -u FLUX_API_KEY -u OPENAI_API_KEY -u ANTHROPIC_API_KEY -u GEMINI_API_KEY \
       -u GOOGLE_API_KEY -u GROQ_API_KEY -u OPENROUTER_API_KEY HOME=$HOMEDIR $BIN"

T0=$(date +%s)
splash_seen=-1; ready_seen=-1
for i in $(seq 1 "$MAXS"); do
  tmux -L "$SOCK" capture-pane -p -t b > "$OUT/frame.txt" 2>/dev/null
  now=$(( $(date +%s) - T0 ))
  if [ "$splash_seen" -lt 0 ] && grep -qF "$NEEDLE_SPLASH" "$OUT/frame.txt"; then
    splash_seen=$now; echo "SPLASH_FIRST_SEEN_AT=${now}s"
  fi
  if [ "$ready_seen" -lt 0 ] && grep -qF "$NEEDLE_READY" "$OUT/frame.txt"; then
    ready_seen=$now; echo "ONBOARDING_FIRST_SEEN_AT=${now}s"; break
  fi
  sleep 1
done

echo "SPLASH_FIRST_SEEN=${splash_seen}"
echo "ONBOARDING_FIRST_SEEN=${ready_seen}"
# Known-positive: if we never saw the splash we were not watching this program.
if [ "$splash_seen" -lt 0 ] && [ "$ready_seen" -lt 0 ]; then
  echo "INSTRUMENT=DEAD (neither needle ever matched — not measuring this program)"
else
  echo "INSTRUMENT=ALIVE (at least one needle matched, so a miss is a real miss)"
fi
[ "$ready_seen" -lt 0 ] && echo "VERDICT=ONBOARDING_NEVER_APPEARED_WITHIN_${MAXS}s"
tmux -L "$SOCK" kill-server 2>/dev/null
