#!/usr/bin/env bash
# slow-type.sh — type into the TUI at HUMAN speed, one character at a time.
#
# Why this exists: `tmux send-keys -l "<whole string>"` delivers an entire line in
# one write. If the TUI drops characters under that, it might be an artifact of
# robot-speed input that no human would ever hit — which would make it an unfair
# finding. This types at ~7 chars/sec (a brisk human) so the result is attributable
# to the product, not to the harness.
set -u
BIN="$1"; HOMEDIR="$2"; OUT="$3"; TEXT="$4"; SETTLE="${5:-25}"; CPS="${6:-0.14}"

[ -x "$BIN" ] || { echo "ASSERT_BIN=FAIL path=$BIN"; exit 92; }
rm -rf "$HOMEDIR"; mkdir -p "$HOMEDIR" "$OUT"
SOCK="slow-$$"

# key arrives on stdin only
IFS= read -r FLUX_API_KEY
export FLUX_API_KEY
[ -n "${FLUX_API_KEY:-}" ] || { echo "ASSERT_KEY=FAIL"; exit 97; }
export WAYLAND_VAULT_PASSPHRASE="uat-throwaway-not-a-real-secret"
echo "ASSERT_KEY=OK len=${#FLUX_API_KEY}"

tmux -L "$SOCK" new-session -d -s s -x 120 -y 40 \
  "env -u OPENAI_API_KEY -u ANTHROPIC_API_KEY -u GEMINI_API_KEY -u GOOGLE_API_KEY \
       -u GROQ_API_KEY -u OPENROUTER_API_KEY HOME=$HOMEDIR $BIN -p flux-router -m flux-auto"

echo "SETTLING ${SETTLE}s"
sleep "$SETTLE"
tmux -L "$SOCK" capture-pane -p -t s > "$OUT/before-typing.txt"
echo "SURFACE_BEFORE_TYPING=$(grep -qF 'Connect a provider' "$OUT/before-typing.txt" && echo ONBOARDING || echo CHAT)"

echo "TYPING ${#TEXT} chars at ${CPS}s/char (~$(awk -v c="$CPS" 'BEGIN{printf "%.1f", 1/c}') chars/sec)"
i=0
while [ "$i" -lt "${#TEXT}" ]; do
  ch="${TEXT:$i:1}"
  tmux -L "$SOCK" send-keys -t s -l "$ch"
  i=$((i+1))
  sleep "$CPS"
done

sleep 2
tmux -L "$SOCK" capture-pane -p -t s > "$OUT/after-typing.txt"
echo "SURFACE_AFTER_TYPING=$(grep -qF 'Connect a provider' "$OUT/after-typing.txt" && echo ONBOARDING || echo CHAT)"
echo "----- COMPOSER / INPUT LINE AFTER TYPING -----"
grep -n '›' "$OUT/after-typing.txt" || echo "(no input line found)"
echo "----- SENT -----"
echo "SENT_TEXT=[$TEXT]"
echo "SENT_LEN=${#TEXT}"

tmux -L "$SOCK" kill-server 2>/dev/null
