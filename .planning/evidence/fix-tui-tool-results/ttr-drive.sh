#!/usr/bin/env bash
# ttr-drive.sh — drive the real wayland-core TUI in a real pty (tmux) until it
# makes a Bash tool call, then MECHANICALLY extract the rendered tool-result
# line so UAT-T3 can be graded from what the product actually painted.
#
# Method borrowed from .planning/evidence/fix-tui-first-message/ftfm-type.sh
# (LANE-BRIEF: reuse the working harness rather than reinventing it). What is
# different here, and why:
#
#   1. The thing under test is a RENDERED LINE, not a keystroke count, so the
#      extraction is a card-line scrape and the grade is a string comparison
#      against the known-bad literal.
#   2. A PARTICIPANT-ALIVE assertion in two stages, not one. Stage 1 is "the
#      TUI attached" (as in ftfm-type.sh). Stage 2 is "a Bash tool card was
#      actually rendered". Without stage 2 this harness is self-passing in the
#      most obvious way available: if the model never calls Bash there is no
#      card, the known-bad literal is absent, and "the defect is gone" and "the
#      experiment did not run" are the same capture (LANE-BRIEF §6a-i).
#   3. Every number and every extracted string goes to a RESULT file that the
#      caller reads out-of-band; nothing load-bearing is trusted from stdout
#      (LANE-BRIEF §3b).
#
# Credentials: the provider key is sourced into the ENVIRONMENT from
# /root/.wayland/.env and never appears in argv, on disk, or in any capture.
# Only its presence is recorded, never its value.
#
# Usage:
#   ttr-drive.sh --bin PATH --home DIR --out DIR --label NAME --text STR
#                [--settle 20] [--poll 12] [--cwd DIR]
#
# Exit codes: 0 ran to completion (read VERDICT), 9x harness fault.
set -u

BIN=""; HOMEDIR=""; OUT=""; LABEL=""; TEXT=""
SETTLE=20; POLL=12; RUNCWD="/root/fixtui-scratch"; SELFTEST=0

while [ $# -gt 0 ]; do
  case "$1" in
    --bin) BIN="$2"; shift 2 ;;
    --home) HOMEDIR="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    --text) TEXT="$2"; shift 2 ;;
    --settle) SETTLE="$2"; shift 2 ;;
    --poll) POLL="$2"; shift 2 ;;
    --cwd) RUNCWD="$2"; shift 2 ;;
    --selftest) SELFTEST=1; shift ;;
    *) echo "unknown flag: $1" >&2; exit 90 ;;
  esac
done

# ── the extraction, isolated so it can be self-tested ────────────────────────
# INSTRUMENT DEFECT, found on the first BEFORE run and REPAIRED here rather than
# written up and left (LANE-BRIEF §6b-ii). The first version of this function
# assumed the compact widget's ONE-line shape
# (`<icon> Bash(<args>) · <summary>`) and matched any line containing both
# `Bash(` and `·`. The inline transcript path the TUI actually uses
# (`workspace.rs::push_tool_card_lines`) renders TWO lines:
#
#     ● Bash({ "command": "echo X" }) · done          <- header, HAS `Bash(` and `·`
#       Ran `?` · exit 0 · 0 bytes                    <- body, the formatter summary
#
# The old matcher therefore locked onto the HEADER — whose `· done` is a status
# chip that is CORRECT and never changes — and would have graded the defect
# ABSENT on a completely unfixed build. Verified against the original UAT
# capture `.planning/evidence/uat-tui-unix/l3-tui-turn.log:226-227`.
#
# The body is the 4-space-indented line immediately following the header, so
# take the header and return the NEXT non-blank line with it.
extract_card() {
  # $1 = pane capture file -> prints "<header> || <summary>", or nothing
  awk '
    # The Activity rail is a right-hand pane on the SAME text rows, so a naive
    # full-row scrape drags `│ … │` rail chrome into the extracted string. Cut
    # each row at the first box-drawing vertical before trimming.
    function clean(s) {
      sub(/\xe2\x94\x82.*$/, "", s)
      sub(/^[[:space:]]*/, "", s); sub(/[[:space:]]+$/, "", s)
      return s
    }
    hdr != "" {
      line = clean($0)
      if (line != "") { print hdr " || " line; exit }
      next
    }
    /Bash\(/ && /\xc2\xb7/ { hdr = clean($0) }
  ' "$1"
}

# ── grading, isolated so it can be self-tested and re-run over old captures ──
#
# INSTRUMENT DEFECT #2, found on the AFTER run and repaired rather than noted
# (LANE-BRIEF §6b-ii). The first grader treated the substring "0 bytes" as a
# defect signature. That was right for the BEFORE state, where the literal card
# read `Ran `?` · exit 0 · 0 bytes` and every one of those zeros was invented.
# It is WRONG for the AFTER state, where `ls /no-such-dir` legitimately writes
# nothing to stdout and `0 bytes stdout` is a MEASUREMENT. The grader scored a
# correct card as PARTIAL — a false red, which is as misleading as a false
# green and would have understated the fix.
#
# The signature that actually distinguishes the two states is the FABRICATION,
# not the zero: a `?` standing in for a command the formatter never had, and an
# `exit 0` on a call the engine itself marked as an error.
grade_card() {
  # $1 = header line, $2 = body line
  local hdr="$1" body="$2" hdr_status=unknown fab has_exit0 zero_bytes contradiction
  say "CARD_HEADER=[${hdr}]"
  say "CARD_BODY=[${body}]"
  case "$hdr" in *'\xc2\xb7 error'*) hdr_status=error ;; *'\xc2\xb7 done'*) hdr_status=done ;; esac
  case "$hdr" in *"· error"*) hdr_status=error ;; *"· done"*) hdr_status=done ;; esac
  say "CARD_HEADER_STATUS=${hdr_status}"

  fab=NO;        case "$body" in *'Ran `?`'*) fab=YES ;; esac
  has_exit0=NO;  case "$body" in *"exit 0"*)  has_exit0=YES ;; esac
  zero_bytes=NO; case "$body" in *"0 bytes"*) zero_bytes=YES ;; esac
  say "FABRICATED_CMD_QUESTION_MARK=${fab}"
  say "BODY_SHOWS_EXIT_0=${has_exit0}"
  say "BODY_SHOWS_A_ZERO_BYTE_COUNT=${zero_bytes}  # informational only: after the"
  say "#   fix a zero here can be a real measurement (a command that printed nothing),"
  say "#   so it is NOT a defect signature on its own."

  contradiction=NO
  if [ "$hdr_status" = "error" ] && [ "$has_exit0" = "YES" ]; then contradiction=YES; fi
  say "SELF_CONTRADICTION=${contradiction}"

  # The defect is the FABRICATION. Either symptom alone is conclusive:
  #   * a `?` rendered where the command should be, or
  #   * `exit 0` on a card the engine marked `error`.
  if [ "$fab" = "YES" ] || [ "$contradiction" = "YES" ]; then
    say "VERDICT=DEFECT_PRESENT"
  else
    say "VERDICT=DEFECT_ABSENT"
  fi
}

# ── re-grade an existing capture, so one grader can score every run ──────────
if [ -n "${GRADE_FILE:-}" ]; then
  RESULT="${GRADE_FILE}.regrade"
  : > "$RESULT"
  say() { echo "$*"; echo "$*" >> "$RESULT"; }
  say "REGRADED_FILE=${GRADE_FILE}"
  C=$(extract_card "$GRADE_FILE")
  if [ -z "$C" ]; then
    say "ASSERT_TOOLCARD=FAIL reason=no-bash-tool-card-in-capture"
    say "VERDICT=EXPERIMENT_DID_NOT_RUN"; exit 0
  fi
  grade_card "${C%% || *}" "${C#* || }"
  exit 0
fi

# ── self-test: THREE assertions, not two (LANE-BRIEF §6b-ii) ─────────────────
#   (1) known-positive: a real card line is extracted verbatim.
#   (2) known-negative: a pane with no card yields the empty string.
#   (3) the NAIVE matcher this replaced (grep -F 'Ran `') would MISS a card
#       whose summary no longer starts with "Ran `" — i.e. exactly the AFTER
#       state this harness has to be able to read. Without (3) the self-test
#       passes on a matcher that can only see the broken output, which would
#       make the whole AFTER capture unreadable and look like a regression.
if [ "$SELFTEST" = "1" ]; then
  T=$(mktemp -d); rc=0
  # Known-positive: the REAL two-line shape, copied byte-for-byte from the
  # original UAT capture l3-tui-turn.log:226-227.
  printf '  some transcript text\n     \xe2\x97\x8f Bash({ "command": "echo LINUX_UAT_TOKEN" }) \xc2\xb7 done\n       Ran `?` \xc2\xb7 exit 0 \xc2\xb7 0 bytes\n' > "$T/pos"
  printf '  some transcript text\n  no tool cards here at all\n' > "$T/neg"
  # A plausible FIXED render, to prove the matcher can still read the after-state.
  printf '     \xe2\x97\x8f Bash({ "command": "echo X" }) \xc2\xb7 done\n       exit 0 \xc2\xb7 11 bytes stdout\n' > "$T/after"

  # (1) known-positive: the SUMMARY (not just the header) must come back.
  got=$(extract_card "$T/pos")
  case "$got" in
    *'Ran `?`'*'0 bytes'*) echo "SELFTEST_1_KNOWN_POSITIVE=PASS got=[$got]" ;;
    *) echo "SELFTEST_1_KNOWN_POSITIVE=FAIL got=[$got]"; rc=1 ;;
  esac

  # (2) known-negative: a pane with no card yields nothing.
  got=$(extract_card "$T/neg")
  if [ -z "$got" ]; then echo "SELFTEST_2_KNOWN_NEGATIVE=PASS"
  else echo "SELFTEST_2_KNOWN_NEGATIVE=FAIL got=[$got]"; rc=1; fi

  # (3) THE ONE THAT PROVES THE REPAIR DID SOMETHING. The OLD matcher
  #     (first line containing both `Bash(` and `·`) locks onto the HEADER,
  #     whose `· done` chip is correct on a broken build. Show that the old
  #     matcher returns a line with NO defect signature while the new one
  #     returns the line that carries it. Without this the self-test passes
  #     on the broken instrument too.
  old=$(awk '/Bash\(/ && /\xc2\xb7/ { sub(/^[[:space:]]*/,""); print; exit }' "$T/pos")
  new=$(extract_card "$T/pos")
  old_sees=NO; case "$old" in *'Ran `?`'*) old_sees=YES ;; esac
  new_sees=NO; case "$new" in *'Ran `?`'*) new_sees=YES ;; esac
  if [ "$old_sees" = "NO" ] && [ "$new_sees" = "YES" ]; then
    echo "SELFTEST_3_OLD_MATCHER_WOULD_MISS=PASS old=[$old] new=[$new]"
  else
    echo "SELFTEST_3_OLD_MATCHER_WOULD_MISS=FAIL old=[$old] new=[$new]"; rc=1
  fi

  # (4) can the matcher READ a fixed build? A gate with no reachable pass
  #     state proves nothing (LANE-BRIEF §3b-iii).
  got=$(extract_card "$T/after")
  if [ -n "$got" ]; then echo "SELFTEST_4_CAN_READ_FIXED_BUILD=PASS got=[$got]"
  else echo "SELFTEST_4_CAN_READ_FIXED_BUILD=FAIL"; rc=1; fi

  rm -rf "$T"; exit "$rc"
fi

[ -n "$BIN" ] && [ -n "$HOMEDIR" ] && [ -n "$OUT" ] && [ -n "$LABEL" ] && [ -n "$TEXT" ] \
  || { echo "missing required flag" >&2; exit 90; }
[ -x "$BIN" ] || { echo "ASSERT_BIN=FAIL"; exit 91; }

mkdir -p "$OUT" "$RUNCWD"
RESULT="$OUT/${LABEL}.result"
: > "$RESULT"
say() { echo "$*"; echo "$*" >> "$RESULT"; }

say "LABEL=${LABEL}"
say "BIN=${BIN}"
say "BIN_SHA256=$(sha256sum "$BIN" | awk '{print $1}')"
say "WAYLAND_HOME=${HOMEDIR}"
say "RUNCWD=${RUNCWD}"

# ── credential: environment only, value never recorded ───────────────────────
set -a; . /root/.wayland/.env 2>/dev/null; set +a
if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
  say "CREDENTIALS=PRESENT len=${#ANTHROPIC_API_KEY}"
else
  say "CREDENTIALS=ABSENT"; say "WLRC=97"; say "WLDONE"; exit 97
fi

SOCK="ttr-$$-${LABEL}"
STDERR_LOG="$OUT/${LABEL}.stderr"
: > "$STDERR_LOG"
# Redirect the product's stderr to a file: it is the ONLY place the per-turn
# external-edit injection is observable (the engine deliberately suppresses it
# from the transcript, engine.rs:5124). Without this the harness cannot tell
# "the model ignored the prompt" from "the engine overwrote the prompt".
# NOTE: deliberately WITHOUT --dangerously-skip-permissions.
#
# MEASURED, after five failed configurations: the tool card only appeared once
# the run went through the real APPROVAL flow — which is also the exact
# condition the original UAT describes ("After approving a shell command the
# user is shown…"). Across four runs with `--dangerously-skip-permissions` the
# model called Bash repeatedly (the status line read `running Bash`) and NOT ONE
# tool card was ever painted, in any of ~90 captured frames. Whether that is a
# second defect or a consequence of those turns never settling is NOT
# established here and is reported as an open question, not as a finding.
tmux -L "$SOCK" new-session -d -s s -x 200 -y 50 \
  "cd $RUNCWD && env WAYLAND_HOME=$HOMEDIR $BIN 2>>$STDERR_LOG" \
  || { say "ASSERT_TMUX=FAIL"; exit 94; }

say "SETTLE=${SETTLE}"
sleep "$SETTLE"

tmux -L "$SOCK" capture-pane -p -t s > "$OUT/${LABEL}.before.txt" 2>/dev/null
PANE_LINES=$(awk 'NF{n++} END{print n+0}' "$OUT/${LABEL}.before.txt")
PANE_DEAD=$(tmux -L "$SOCK" list-panes -t s -F '#{pane_dead}' 2>/dev/null | head -1)
say "PANE_NONEMPTY_LINES=${PANE_LINES}"
say "PANE_DEAD=${PANE_DEAD:-unknown}"
if [ "${PANE_LINES:-0}" -lt 3 ] || [ "${PANE_DEAD:-1}" != "0" ]; then
  say "ASSERT_PTY=FAIL"; say "WLRC=95"; say "WLDONE"
  tmux -L "$SOCK" kill-server 2>/dev/null; exit 95
fi
say "ASSERT_PTY=OK"

# ── warm-up turn ─────────────────────────────────────────────────────────────
# MEASURED OBSTACLE, not a superstition. The engine mounts a FileWatcher on cwd
# (bootstrap.rs:3139, unconditional, no config knob) and drains it per turn,
# bundling a synthetic "User edited <path> while I was thinking — re-read it
# before proceeding" message into the user's turn. On a freshly-created cwd the
# watcher fires for paths that existed BEFORE it armed, so TURN ONE always
# carries the notice — and the model answers the notice instead of the prompt.
# Measured headless in three separate configurations, including a chmod 555
# read-only cwd with memory and durable sessions both disabled: 8, 2 and 3
# injections respectively, and in all three the model replied about the
# directory and never called Bash.
#
# The drain is per-turn, so a throwaway first turn absorbs the backlog and the
# real prompt lands on a clean turn. This is a HARNESS workaround for a product
# defect that is NOT in this lane's scope — it is reported separately.
# ── send the prompt ──────────────────────────────────────────────────────────
say "SENT_TEXT=[${TEXT}]"
tmux -L "$SOCK" send-keys -t s -l "$TEXT"
sleep 1
tmux -L "$SOCK" send-keys -t s Enter

# ── poll + approve, emitting every iteration (§6b: silence looks like a hang) ─
# The run is gated on a real approval modal, so the loop must both WAIT and
# ANSWER. The model typically makes several Bash calls; approve each one as it
# appears and keep the FIRST card seen, which is the command under test.
CARD=""
APPROVALS=0
for i in $(seq 1 "$POLL"); do
  sleep 5
  tmux -L "$SOCK" capture-pane -p -t s > "$OUT/${LABEL}.poll${i}.txt"
  if [ -z "$CARD" ]; then
    CARD=$(extract_card "$OUT/${LABEL}.poll${i}.txt")
    [ -n "$CARD" ] && say "CARD_SEEN_AT_ITERATION=${i} (~$((i*5))s)"
  fi
  if grep -qF '[enter/y] approve once' "$OUT/${LABEL}.poll${i}.txt"; then
    tmux -L "$SOCK" send-keys -t s "y"
    APPROVALS=$((APPROVALS+1))
    echo "approving: iteration $i, approval #$APPROVALS, $(date +%H:%M:%S)"
  else
    echo "waiting: iteration $i, $(date +%H:%M:%S)"
  fi
  [ -n "$CARD" ] && [ "$i" -ge 4 ] && break
done
say "APPROVALS_GIVEN=${APPROVALS}"

sleep 5
tmux -L "$SOCK" capture-pane -p -t s   > "$OUT/${LABEL}.after.txt"
[ -z "$CARD" ] && CARD=$(extract_card "$OUT/${LABEL}.after.txt")

# The TUI runs on tmux's ALTERNATE screen, so `capture-pane -S -<n>` returns
# nothing older than the visible pane — there is no tmux scrollback to read.
# The transcript scrolls INSIDE the application, so a card that has moved off
# the viewport is only reachable by driving the app's own PgUp. Found the hard
# way on the first BEFORE run, which reported "no card" for a session that had
# demonstrably made tool calls. Scan upward and keep every frame as evidence.
if [ -z "$CARD" ]; then
  for p in $(seq 1 12); do
    tmux -L "$SOCK" send-keys -t s PageUp
    sleep 0.6
    tmux -L "$SOCK" capture-pane -p -t s > "$OUT/${LABEL}.pgup${p}.txt"
    CARD=$(extract_card "$OUT/${LABEL}.pgup${p}.txt")
    if [ -n "$CARD" ]; then say "CARD_FOUND_AFTER_PAGEUPS=${p}"; break; fi
    echo "scrolling: pageup $p"
  done
fi

# ── PARTICIPANT-ALIVE stage 2 ────────────────────────────────────────────────
# No card == the experiment did not run. Grade it as such; never as a pass.
if [ -z "$CARD" ]; then
  say "ASSERT_TOOLCARD=FAIL reason=no-bash-tool-card-rendered"
  say "TOOLCARD_LINE=[]"
  # See the note at the other removal site: the TUI does not route tracing to
  # stderr, so this count is 0 on a build where the notice IS firing. Not
  # asserted (LANE-BRIEF §3b-i).
  say "VERDICT=EXPERIMENT_DID_NOT_RUN"
  say "WLRC=96"; say "WLDONE"
  tmux -L "$SOCK" kill-server 2>/dev/null; exit 0
fi
say "ASSERT_TOOLCARD=OK"
say "TOOLCARD_LINE=[${CARD}]"
# NOT MEASURED: the TUI does not route tracing to stderr, so grepping the stderr
# log for the external-edit notice returns 0 on a build where it IS firing. That
# is a dead instrument, and reporting its 0 would be a self-passing absence
# claim (LANE-BRIEF §3b-i). Deliberately not asserted.

# ── grade ────────────────────────────────────────────────────────────────────
# The defect signature is the formatter's fabricated triple. Report each
# component separately so a PARTIAL fix cannot read as a full one.
HDR="${CARD%% || *}"; BODY="${CARD#* || }"
grade_card "$HDR" "$BODY"

say "WLRC=0"; say "WLDONE"
tmux -L "$SOCK" kill-server 2>/dev/null
exit 0
