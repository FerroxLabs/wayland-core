#!/usr/bin/env bash
# tui-drive.sh — drive the wayland-core TUI inside a REAL pty (tmux) and capture
# what a user actually sees. Portable across macOS and Linux.
#
# Anti-self-pass design (LANE-BRIEF §3.2 / §3b-iii):
#   * BIN existence + executability is asserted BEFORE any conclusion is drawn,
#     and `--selftest` proves that assertion actually fires on a bogus path.
#   * The pty is proven real by a known-positive (`isatty` must be True inside,
#     False outside) before the product is judged.
#   * Every capture is written to a FILE; nothing load-bearing is parsed from a pipe.
#
# Usage:
#   tui-drive.sh --selftest
#   tui-drive.sh --bin <path> --out <dir> --label <name> [--home <dir>]
#                [--scrub-keys] [--with-flux] [--arg <a>]... [--send <keys>]...
#                [--settle <secs>]

set -u

die() { echo "FATAL: $*" >&2; exit 90; }

assert_bin() {
  local b="$1"
  [ -n "$b" ]  || { echo "ASSERT_BIN=FAIL reason=empty_path"; return 91; }
  [ -e "$b" ]  || { echo "ASSERT_BIN=FAIL reason=missing path=$b"; return 92; }
  [ -f "$b" ]  || { echo "ASSERT_BIN=FAIL reason=not_regular_file path=$b"; return 93; }
  [ -x "$b" ]  || { echo "ASSERT_BIN=FAIL reason=not_executable path=$b"; return 94; }
  echo "ASSERT_BIN=OK path=$b size=$(wc -c < "$b" | tr -d ' ')"
  return 0
}

pty_control() {
  # Known-positive / known-negative pair. Returns 0 only if the pty discriminates.
  #
  # NOTE (instrument repair, in-lane): v1 of this function redirected the probe's
  # stdout to a FILE inside tmux (`... > pty-inside.txt`). That redirection is
  # itself a non-tty, so the probe reported ISATTY=False *inside a real pty* and
  # the harness refused to run. The stdout of the probe must stay attached to the
  # pane; the pane is then read back with `capture-pane`. Keeping the old form
  # here as a comment because it is a perfect example of a probe that measures
  # its own plumbing instead of the thing under test.
  local outside inside
  outside=$(python3 -c 'import sys;print(sys.stdout.isatty())' 2>/dev/null)
  tmux -L "$SOCK" new-session -d -s ptyctl \
    "python3 -c 'import sys;print(\"ISATTY=\"+str(sys.stdout.isatty()))'; sleep 60"
  sleep 2
  tmux -L "$SOCK" capture-pane -p -t ptyctl > "$OUT/pty-inside.txt" 2>&1
  inside=$(grep -o 'ISATTY=[A-Za-z]*' "$OUT/pty-inside.txt" | head -1)
  echo "PTY_OUTSIDE_ISATTY=$outside"
  echo "PTY_INSIDE=$inside"
  case "$outside:$inside" in
    False:ISATTY=True) echo "PTY_CONTROL=OK (discriminates: False outside pty, True inside pty)"; return 0 ;;
    *) echo "PTY_CONTROL=DEAD (outside=$outside inside=$inside) — refusing to judge the TUI"; return 95 ;;
  esac
}

# ---------------------------------------------------------------- args
BIN=""; OUT=""; LABEL="run"; HOMEDIR=""; SCRUB=0; WITHFLUX=0; SETTLE=6
ARGS=(); SENDS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --selftest)    SELFTEST=1; shift ;;
    --bin)         BIN="$2"; shift 2 ;;
    --out)         OUT="$2"; shift 2 ;;
    --label)       LABEL="$2"; shift 2 ;;
    --home)        HOMEDIR="$2"; shift 2 ;;
    --scrub-keys)  SCRUB=1; shift ;;
    --with-flux)   WITHFLUX=1; shift ;;
    --settle)      SETTLE="$2"; shift 2 ;;
    --arg)         ARGS+=("$2"); shift 2 ;;
    --send)        SENDS+=("$2"); shift 2 ;;
    *) die "unknown arg: $1" ;;
  esac
done

# ------------------------------------------------- selftest (real implementation)
if [ "${SELFTEST:-0}" = "1" ]; then
  echo "=== SELFTEST: the binary assertion MUST fail on inputs that deserve it ==="
  fails=0; passes=0
  out=$(assert_bin "/nonexistent/definitely-not-here"); rc=$?
  echo "  bogus-path  -> rc=$rc :: $out"
  [ "$rc" -eq 92 ] && passes=$((passes+1)) || fails=$((fails+1))
  out=$(assert_bin "/etc"); rc=$?
  echo "  a-directory -> rc=$rc :: $out"
  [ "$rc" -eq 93 ] && passes=$((passes+1)) || fails=$((fails+1))
  tmpf=$(mktemp); chmod 644 "$tmpf"
  out=$(assert_bin "$tmpf"); rc=$?
  echo "  non-exec    -> rc=$rc :: $out"
  [ "$rc" -eq 94 ] && passes=$((passes+1)) || fails=$((fails+1))
  out=$(assert_bin "/bin/sh"); rc=$?
  echo "  real-binary -> rc=$rc :: $out"
  [ "$rc" -eq 0 ]  && passes=$((passes+1)) || fails=$((fails+1))
  rm -f "$tmpf"
  echo "SELFTEST_PASSES=$passes SELFTEST_FAILS=$fails"
  [ "$fails" -eq 0 ] && echo "SELFTEST=OK" || echo "SELFTEST=BROKEN"
  exit $([ "$fails" -eq 0 ] && echo 0 || echo 1)
fi

[ -n "$BIN" ] || die "--bin required"
[ -n "$OUT" ] || die "--out required"
mkdir -p "$OUT"
SOCK="uat-$LABEL-$$"

{
echo "########## UAT RUN: $LABEL"
echo "host=$(hostname) uname=$(uname -srm) date=$(date -u '+%Y-%m-%dT%H:%M:%SZ')"

# 1. binary assertion — before any conclusion
assert_bin "$BIN"; abrc=$?
if [ "$abrc" -ne 0 ]; then echo "VERDICT=ABORTED_NO_BINARY"; exit "$abrc"; fi
echo "BIN_SHA256=$( { command -v sha256sum >/dev/null && sha256sum "$BIN" || shasum -a 256 "$BIN"; } | awk '{print $1}')"
echo "BIN_BUILDINFO=$("$BIN" --build-info 2>&1 | head -1)"

# 2. pty must be proven real
pty_control; prc=$?
tmux -L "$SOCK" kill-session -t ptyctl 2>/dev/null
if [ "$prc" -ne 0 ]; then echo "VERDICT=ABORTED_NO_PTY"; exit "$prc"; fi

# 3. build the command
# NOTE: BSD `env` (macOS) parses OPTIONS first and stops at the first operand, so
# every `-u` must precede any NAME=VALUE assignment. Writing `env HOME=x -u K` made
# macOS treat `-u` as the command name and the run died with `env: -u: No such file
# or directory` / rc=127. GNU env tolerates the other order; macOS does not.
ENVPFX="env"
if [ "$SCRUB" = "1" ]; then
  for k in FLUX_API_KEY OPENAI_API_KEY ANTHROPIC_API_KEY GEMINI_API_KEY GOOGLE_API_KEY \
           GROQ_API_KEY OPENROUTER_API_KEY FAL_API_KEY HF_API_KEY API_KEY PROVIDER BASE_URL MODEL; do
    ENVPFX="$ENVPFX -u $k"
  done
fi
[ -n "$HOMEDIR" ] && { mkdir -p "$HOMEDIR"; ENVPFX="$ENVPFX HOME=$HOMEDIR"; }
PRELUDE=""
if [ "$WITHFLUX" = "1" ]; then
  # value never reaches argv, disk, or this log — only the PATH to the env file does
  PRELUDE='set -a; . "$HOME_REAL/.wayland-secrets/flux.env"; set +a; '
  ENVPFX="$ENVPFX HOME_REAL=${HOME_REAL:-$HOME}"
fi
QARGS=""
for a in ${ARGS+"${ARGS[@]}"}; do QARGS="$QARGS $(printf '%q' "$a")"; done
CMD="$PRELUDE $ENVPFX $(printf '%q' "$BIN")$QARGS; echo \"WLRC=\$?\" > $OUT/$LABEL.rc; echo WLDONE >> $OUT/$LABEL.rc; sleep 300"
echo "CMD=$CMD" | sed 's/[A-Za-z0-9_-]\{40,\}/<REDACTED>/g'

# 4. launch in a real pty
rm -f "$OUT/$LABEL.rc"
tmux -L "$SOCK" new-session -d -s main -x 120 -y 40 "bash -lc $(printf '%q' "$CMD")"
echo "LAUNCHED settle=${SETTLE}s"
sleep "$SETTLE"

echo "----- CAPTURE 0 (what the user sees on arrival) -----"
tmux -L "$SOCK" capture-pane -p -t main > "$OUT/$LABEL.cap0.txt" 2>&1
cat "$OUT/$LABEL.cap0.txt"

# 5. drive it
i=0
for k in ${SENDS+"${SENDS[@]}"}; do
  i=$((i+1))
  echo "----- SEND #$i: [$k] -----"
  case "$k" in
    __SLEEP:*) sleep "${k#__SLEEP:}" ;;
    __LITERAL:*) tmux -L "$SOCK" send-keys -t main -l "${k#__LITERAL:}" ;;
    *) tmux -L "$SOCK" send-keys -t main "$k" ;;
  esac
  sleep 3
  tmux -L "$SOCK" capture-pane -p -t main > "$OUT/$LABEL.cap$i.txt" 2>&1
  cat "$OUT/$LABEL.cap$i.txt"
done

echo "----- PANE ALIVE? -----"
tmux -L "$SOCK" list-panes -t main -F 'pane_dead=#{pane_dead} pid=#{pane_pid}' 2>&1
echo "----- EXIT STATUS FILE (written by the child, read separately) -----"
cat "$OUT/$LABEL.rc" 2>&1 || echo "NO_RC_FILE (process still running or killed)"
} > "$OUT/$LABEL.log" 2>&1

# 6. teardown + orphan sweep (outside the log block so it always runs)
#
# Two instrument repairs, both made after the detector lied in this lane:
#   (a) `pgrep -f <basename>` matched 17 unrelated processes — the checkout is
#       itself named `waylandcore`, so the agent's own tooling matched.
#   (b) `pgrep -f <abspath>` still matched THIS SCRIPT, because the harness is
#       invoked as `tui-drive.sh --bin <abspath>` and pgrep -f searches the whole
#       command line. It reported ORPHANS_AFTER_KILL=1 for a PID that was the
#       harness itself and was already gone. A false orphan is exactly as bad as
#       a missed one.
# Repair: match only processes whose ARGV[0] is EXACTLY the binary, and never
# count our own pid/ppid. (`pgrep -a` is GNU-only and is silently ignored on BSD,
# which is why the earlier capture held bare PIDs with no command text.)
# argv0 must equal the binary EXACTLY. That alone excludes this script (whose
# argv0 is bash/the script path) and the ps|awk pipeline, so no pid-based
# exclusion is needed — and adding a ppid exclusion would have hidden the
# known-positive below, which is spawned as a child of this very script.
orphan_scan() {
  ps -Ao pid,ppid,args 2>/dev/null | awk -v b="$BIN" 'NR>1 && $3==b {print}'
}

# Count lines with awk, NOT `grep -c . || echo 0`: on an empty file `grep -c .`
# prints 0 AND exits 1, so the `|| echo 0` fires and the variable becomes the two
# lines "0\n0". That is the documented trap and it fired here on the first run.
nlines() { awk 'END{print NR+0}' "$1" 2>/dev/null; }

sleep 1
orphan_scan > "$OUT/$LABEL.procs-before-kill.txt" 2>&1
BEFORE=$(nlines "$OUT/$LABEL.procs-before-kill.txt")

tmux -L "$SOCK" kill-server 2>/dev/null
sleep 5
orphan_scan > "$OUT/$LABEL.orphans-after-kill.txt" 2>&1
AFTER=$(nlines "$OUT/$LABEL.orphans-after-kill.txt")

# DETECTOR LIVENESS: `ORPHANS_AFTER_KILL=0` is meaningless unless the same scan
# has been shown to return non-zero for a process that really is there. The
# pre-kill scan IS that known-positive: the product was running under tmux at
# that moment, so BEFORE>=1 proves the scan can see this binary. A separately
# spawned decoy was tried first and returned 0 — because `wayland-core --no-tui`
# with no provider exits immediately rather than idling on stdin, so the decoy
# was dead before it could be observed. That failure is retained here as the
# reason the pre-kill scan is used instead.
if [ "$BEFORE" -ge 1 ]; then DET="ALIVE"; else DET="DEAD"; fi

{
  echo "ORPHAN_SWEEP=argv0_exact_match pattern=$BIN"
  echo "PROCS_BEFORE_KILL=$BEFORE"
  echo "ORPHANS_AFTER_KILL=$AFTER"
  echo "ORPHAN_DETECTOR=$DET (known-positive = the pre-kill scan; DEAD means ORPHANS_AFTER_KILL proves nothing)"
} >> "$OUT/$LABEL.log"
echo "DONE $LABEL -> $OUT/$LABEL.log"
