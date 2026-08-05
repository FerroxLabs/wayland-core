#!/usr/bin/env bash
# measure-both-directions.sh — the quiet-by-default proof AND its inverse.
#
# LANE-BRIEF §3b-iii: a gate that cannot pass is as worthless as one that cannot
# fail. "The logs are gone" is trivially satisfiable by breaking logging, so this
# harness refuses to report the quiet number without also proving, in the same
# run, that the record is still obtainable — twice over:
#
#   direction A  RUST_LOG unset  -> stderr must be quiet AND the log FILE must
#                                   hold the full INFO record
#   direction B  RUST_LOG=info   -> stderr must carry the full record again,
#                                   i.e. the previous behaviour is one env var
#                                   away
#
# usage: measure-both-directions.sh <prefix> <binary> <outdir> <lanehome>
set -u
PREFIX="${1:?}"; BIN="${2:?}"; OUT="${3:?}"; LANEHOME="${4:?}"
mkdir -p "$OUT"; R="$OUT/$PREFIX"; : > "$R.result"
say() { echo "$1" >> "$R.result"; }

KEY=$(/usr/bin/grep -m1 '^ANTHROPIC_API_KEY=' /root/.wayland/.env | cut -d= -f2-)
[ -z "$KEY" ] && { say "NO_KEY"; say "WLRC=97"; say "WLDONE"; exit 97; }
export ANTHROPIC_API_KEY="$KEY"; unset KEY
export HOME="$LANEHOME" PROVIDER=anthropic MODEL=claude-sonnet-4-5-20250929
LOGFILE="$LANEHOME/.wayland/logs/wayland-core.log"
PROMPT='What is 17 times 23? Reply with just the number.'

say "BIN_SHA256=$(sha256sum "$BIN" | cut -d' ' -f1)"
say "BUILD_INFO=$("$BIN" --build-info 2>/dev/null | tr '\n' ' ')"

run() { # run <tag> <rustlog-or-empty>
  local tag="$1" rl="$2"
  rm -f "$LOGFILE"
  if [ -n "$rl" ]; then
    RUST_LOG="$rl" "$BIN" --no-tui "$PROMPT" > "$R.$tag.stdout" 2> "$R.$tag.stderr"
  else
    env -u RUST_LOG "$BIN" --no-tui "$PROMPT" > "$R.$tag.stdout" 2> "$R.$tag.stderr"
  fi
  local rc=$?
  /usr/bin/sed -E 's/\x1b\[[0-9;]*[A-Za-z]//g' "$R.$tag.stderr" > "$R.$tag.stderr.plain"
  say "${tag}_RC=$rc"
  say "${tag}_RUST_LOG=${rl:-<unset>}"
  # Participant-alive: a turn that failed emits fewer logs than one that worked,
  # so a "quiet" reading from a broken run is meaningless.
  say "${tag}_ANSWER_HAS_391=$(/usr/bin/grep -c 391 "$R.$tag.stdout" || true)"
  if [ "$rc" -ne 0 ]; then
    say "${tag}_ASSERT_TURN=FAIL"
    say "${tag}_FIRST_STDERR=$(head -1 "$R.$tag.stderr" | cut -c1-160)"
  else
    say "${tag}_ASSERT_TURN=OK"
  fi
  say "${tag}_STDERR_LINES=$(wc -l < "$R.$tag.stderr" | tr -d ' ')"
  say "${tag}_STDERR_INFO=$(/usr/bin/grep -c ' INFO ' "$R.$tag.stderr.plain" || true)"
  say "${tag}_STDERR_WARN=$(/usr/bin/grep -c ' WARN ' "$R.$tag.stderr.plain" || true)"
  say "${tag}_STDERR_ERROR=$(/usr/bin/grep -c ' ERROR ' "$R.$tag.stderr.plain" || true)"
  say "${tag}_STDOUT_BYTES=$(wc -c < "$R.$tag.stdout" | tr -d ' ')"
  say "${tag}_STDOUT_HEX=$(od -An -tx1 < "$R.$tag.stdout" | tr -s ' ' | tr -d '\n' | cut -c1-60)"
  local last; last=$(tail -c 1 "$R.$tag.stdout" | od -An -tx1 | tr -d ' \n')
  say "${tag}_STDOUT_ENDS_NEWLINE=$([ "$last" = '0a' ] && echo YES || echo NO)"
  say "${tag}_STDOUT_STARTS_STAR=$([ "$(head -c 2 "$R.$tag.stdout")" = '* ' ] && echo YES || echo NO)"
  if [ -f "$LOGFILE" ]; then
    cp "$LOGFILE" "$R.$tag.logfile"
    say "${tag}_LOGFILE_LINES=$(wc -l < "$LOGFILE" | tr -d ' ')"
    say "${tag}_LOGFILE_INFO=$(/usr/bin/sed -E 's/\x1b\[[0-9;]*[A-Za-z]//g' "$LOGFILE" | /usr/bin/grep -c ' INFO ' || true)"
    say "${tag}_LOGFILE_WARN=$(/usr/bin/sed -E 's/\x1b\[[0-9;]*[A-Za-z]//g' "$LOGFILE" | /usr/bin/grep -c ' WARN ' || true)"
    # A named line that MUST be in the record if the record is real.
    say "${tag}_LOGFILE_HAS_SPOTIFY=$(/usr/bin/grep -c 'spotify_playback' "$LOGFILE" || true)"
  else
    say "${tag}_LOGFILE_LINES=NO_FILE"
  fi
}

run quiet ""
run verbose "info"

# ── controls in the same capture (§3b-i) ─────────────────────────────────────
say "CTRL_POS=$(/usr/bin/grep -c 'BIN_SHA256=' "$R.result" || true)"
say "CTRL_NEG=$(/usr/bin/grep -c 'ZZQQ_NOT_PRESENT_9F3A' "$R.quiet.stderr.plain" || true)"

say "WLRC=0"
say "WLDONE"
exit 0
