#!/usr/bin/env bash
# noise-firstpaint.sh — time TUI first paint at 100ms granularity in a real pty,
# and capture the boot trace so the wait can be EXPLAINED, not just measured.
#
# UAT-TUI-UNIX F7 measured at 1s granularity and offered no cause. 1s buckets
# cannot distinguish "one 9s stall" from "ninety 100ms stalls", which are
# different defects with different fixes, so this harness resolves 10x finer and
# pairs every run with the engine's own timestamped boot log.
#
# LANE-BRIEF compliance: WLRC/WLDONE status (§3.2); participant-alive assertion
# before any timing is trusted (§6a-i) — a pane that never attached paints
# nothing and would otherwise be timed as an infinite first paint; lane-unique
# paths (§6a-ii); all numbers land in a file the caller reads (§3b).
#
# usage: noise-firstpaint.sh <label> <binary> <outdir> <homedir> [extra args...]
set -u
LABEL="${1:?label}"; BIN="${2:?binary}"; OUT="${3:?outdir}"; HOMEDIR="${4:?homedir}"; shift 4
EXTRA=("$@")

mkdir -p "$OUT" "$HOMEDIR"
R="$OUT/$LABEL"; : > "$R.result"
say() { echo "$1" >> "$R.result"; }

MAXWAIT=40          # seconds; 3.6x the worst figure the UAT reported
POLL=0.1

say "LABEL=$LABEL"
say "BIN=$BIN"
say "BIN_SHA256=$(sha256sum "$BIN" | cut -d' ' -f1)"
say "BUILD_INFO=$("$BIN" --build-info 2>/dev/null | tr '\n' ' ')"
say "HOME_DIR=$HOMEDIR"
say "HOME_PREEXISTING_CONFIG=$([ -f "$HOMEDIR/.wayland/config.toml" ] && echo YES || echo NO)"
say "EXTRA=[${EXTRA[*]-}]"
say "LOADAVG=$(cut -d' ' -f1-3 /proc/loadavg)"

export WAYLAND_VAULT_PASSPHRASE="uat-throwaway-not-a-real-secret"
SOCK="ftn-$$-$LABEL"

T0=$(date +%s.%N)
tmux -L "$SOCK" new-session -d -s s -x 120 -y 40 \
  "env HOME=$HOMEDIR TERM=xterm-256color $BIN ${EXTRA[*]-}" \
  || { say "ASSERT_TMUX=FAIL"; say "WLRC=94"; say "WLDONE"; exit 94; }

el() { awk -v a="$T0" -v b="$(date +%s.%N)" 'BEGIN{printf "%.2f", b-a}'; }

T_ANY=""; T_SPLASH=""; T_READY=""
STEPS=$(awk -v m="$MAXWAIT" -v p="$POLL" 'BEGIN{print int(m/p)}')
for i in $(seq 1 "$STEPS"); do
  tmux -L "$SOCK" capture-pane -p -t s > "$R.pane" 2>/dev/null || break
  NB=$(awk 'NF{n++} END{print n+0}' "$R.pane")
  if [ -z "$T_ANY" ] && [ "$NB" -ge 1 ]; then T_ANY=$(el); cp "$R.pane" "$R.frame-any.txt"; fi
  if [ -z "$T_SPLASH" ] && /usr/bin/grep -qiE 'starting engine|connecting tools' "$R.pane"; then
    T_SPLASH=$(el); cp "$R.pane" "$R.frame-splash.txt"
  fi
  # "Ready" = the user can act: either the onboarding card or a composer prompt.
  if [ -z "$T_READY" ] && /usr/bin/grep -qE 'Connect a provider|^\s*›' "$R.pane"; then
    T_READY=$(el); cp "$R.pane" "$R.frame-ready.txt"
  fi
  [ -n "$T_READY" ] && break
  sleep "$POLL"
done
T_END=$(el)

# ── participant-alive (§6a-i) ────────────────────────────────────────────────
DEAD=$(tmux -L "$SOCK" list-panes -t s -F '#{pane_dead}' 2>/dev/null | head -1)
FINAL_NB=$(awk 'NF{n++} END{print n+0}' "$R.pane")
say "PANE_DEAD=${DEAD:-unknown}"
say "PANE_NONEMPTY_LINES=$FINAL_NB"
if [ "${DEAD:-1}" != "0" ] || [ "$FINAL_NB" -lt 3 ]; then
  say "ASSERT_PTY=FAIL"
  cp "$R.pane" "$R.frame-fail.txt" 2>/dev/null
  say "T_ANY=${T_ANY:-none} T_SPLASH=${T_SPLASH:-none} T_READY=${T_READY:-none}"
  say "WLRC=95"; say "WLDONE"; tmux -L "$SOCK" kill-server 2>/dev/null; exit 95
fi
say "ASSERT_PTY=OK"

say "T_ANY=${T_ANY:-NEVER}"
say "T_SPLASH=${T_SPLASH:-NEVER}"
say "T_READY=${T_READY:-NEVER}"
say "T_END=$T_END"
say "SURFACE=$(/usr/bin/grep -qF 'Connect a provider' "$R.pane" && echo ONBOARDING || echo WORKSPACE)"

tmux -L "$SOCK" capture-pane -p -t s > "$R.final.txt" 2>/dev/null
tmux -L "$SOCK" kill-server 2>/dev/null
sleep 0.5

# ── the diagnosis half: the engine's own timestamped boot log ────────────────
# In TUI mode wcore-cli routes every trace to $WAYLAND_HOME/logs/wayland-core.log
# with ISO timestamps (main.rs:1167). The largest consecutive delta in that file
# is the stall, named by the line that follows it.
LOG=$(ls -1t "$HOMEDIR"/.wayland/logs/*.log 2>/dev/null | head -1)
if [ -n "$LOG" ]; then
  cp "$LOG" "$R.boot.log"
  say "BOOT_LOG=$R.boot.log"
  say "BOOT_LOG_LINES=$(wc -l < "$LOG" | tr -d ' ')"
  # Emit "<gap_seconds>  <line that ENDED the gap>" sorted widest-first.
  /usr/bin/sed -E 's/\x1b\[[0-9;]*[A-Za-z]//g' "$LOG" \
  | awk '
      match($0, /^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:.]+Z/) {
        ts = substr($0, RSTART, RLENGTH)
        gsub(/[-:TZ]/, " ", ts)
        split(ts, f, " ")
        sec = f[4]*3600 + f[5]*60 + f[6]
        if (prev != "") printf "%8.3f  %s\n", sec - prev, substr($0, 1, 150)
        prev = sec
      }' | sort -rn > "$R.boot.gaps.txt"
  say "BOOT_MAX_GAP_S=$(head -1 "$R.boot.gaps.txt" | awk '{print $1}')"
else
  say "BOOT_LOG=NONE_FOUND"
fi

say "WLRC=0"
say "WLDONE"
exit 0
