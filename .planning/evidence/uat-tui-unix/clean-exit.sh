#!/usr/bin/env bash
# clean-exit.sh — does Ctrl-C leave anything behind?
#
# This project has a documented history of process-reaping defects, so "0 orphans"
# is a headline claim and must not be free. The scan is proven alive by running it
# WHILE the TUI is up (must be >=1) before it is trusted to report 0 afterwards.
# The generic harness reported ORPHAN_DETECTOR=DEAD for this scenario, correctly:
# by the time it scanned, the process had already exited, so its known-positive
# window had closed. This script puts the known-positive inside the live window.
set -u
BIN="$1"; HOMEDIR="$2"; OUT="$3"; SETTLE="${4:-25}"

[ -x "$BIN" ] || { echo "ASSERT_BIN=FAIL path=$BIN"; exit 92; }
rm -rf "$HOMEDIR"; mkdir -p "$HOMEDIR" "$OUT"
SOCK="ce-$$"; RC="$OUT/child.rc"; rm -f "$RC"

IFS= read -r FLUX_API_KEY; export FLUX_API_KEY
[ -n "${FLUX_API_KEY:-}" ] || { echo "ASSERT_KEY=FAIL"; exit 97; }
export WAYLAND_VAULT_PASSPHRASE="uat-throwaway-not-a-real-secret"

scan() { ps -Ao pid,ppid,args 2>/dev/null | awk -v b="$BIN" 'NR>1 && $3==b {print}'; }
n()    { awk 'END{print NR+0}' "$1"; }

tmux -L "$SOCK" new-session -d -s c -x 120 -y 40 \
  "env -u OPENAI_API_KEY -u ANTHROPIC_API_KEY -u GEMINI_API_KEY -u GOOGLE_API_KEY \
       -u GROQ_API_KEY -u OPENROUTER_API_KEY HOME=$HOMEDIR $BIN -p flux-router -m flux-auto; \
   echo WLRC=\$? > $RC; echo WLDONE >> $RC; sleep 120"
sleep "$SETTLE"

scan > "$OUT/while-running.txt"; LIVE=$(n "$OUT/while-running.txt")
echo "SCAN_WHILE_RUNNING=$LIVE  (this is the known-positive; must be >=1)"
if [ "$LIVE" -lt 1 ]; then
  echo "DETECTOR=DEAD — refusing to report an orphan count"; tmux -L "$SOCK" kill-server 2>/dev/null; exit 95
fi
echo "DETECTOR=ALIVE"

# Record the whole process tree the product owns, so a surviving CHILD is visible
# too (not just a surviving main process).
PID=$(awk 'NR==1{print $1}' "$OUT/while-running.txt")
echo "MAIN_PID=$PID"
ps -Ao pid,ppid,args 2>/dev/null | awk -v p="$PID" 'NR>1 && $2==p {print}' > "$OUT/children-while-running.txt"
echo "DIRECT_CHILDREN_WHILE_RUNNING=$(n "$OUT/children-while-running.txt")"

echo "SENDING Ctrl-C"
tmux -L "$SOCK" send-keys -t c C-c
sleep 3
tmux -L "$SOCK" send-keys -t c C-c 2>/dev/null
sleep 8

echo "----- child-written exit status, read separately (never trust the pipe) -----"
if [ -f "$RC" ]; then cat "$RC"; else echo "NO_RC_FILE=the TUI did not exit"; fi

scan > "$OUT/after-exit.txt"; AFTER=$(n "$OUT/after-exit.txt")
echo "ORPHANS_AFTER_CTRLC=$AFTER  (meaningful because the same scan returned $LIVE while running)"
[ "$AFTER" -gt 0 ] && { echo "SURVIVORS:"; cat "$OUT/after-exit.txt"; }

# Any leftover child of the old main pid?
ps -Ao pid,ppid,args 2>/dev/null | awk -v p="$PID" 'NR>1 && $2==p {print}' > "$OUT/children-after.txt"
echo "ORPHANED_DIRECT_CHILDREN=$(n "$OUT/children-after.txt")"

tmux -L "$SOCK" kill-server 2>/dev/null
