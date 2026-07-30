#!/usr/bin/env bash
# noise-headless.sh — measure headless startup spew + stdout shape.
#
# LANE-BRIEF compliance:
#  * every number is written to a FILE and read back by the caller with the Read
#    tool; nothing load-bearing is printed through a proxied tool (§3b).
#  * status is written as WLRC=<code> first and WLDONE last (§3.2) so a caller
#    never trusts an ssh exit code.
#  * every absence claim ships a known-POSITIVE and a known-NEGATIVE control in
#    the same capture (§3b-i), and the run asserts the participant actually
#    started (§6a-i) — a binary that never launched emits zero log lines, which
#    would read as a perfect pass.
#
# usage: noise-headless.sh <label> <binary> <outdir> [extra args...]
set -u

LABEL="${1:?label}"; BIN="${2:?binary}"; OUT="${3:?outdir}"; shift 3
EXTRA=("$@")

mkdir -p "$OUT"
R="$OUT/$LABEL"
: > "$R.result"
say() { echo "$1" >> "$R.result"; }

PROMPT='What is 17 times 23? Reply with just the number.'

# Pristine HOME. hetzner's /root/.wayland/.env injects ANTHROPIC_API_KEY into the
# product regardless of the shell (LANE-BRIEF §3b-ii). We deliberately DO want a
# live provider for this measurement, so HOME stays real and the arm is read back
# out of the product's own output below rather than assumed.
export WAYLAND_VAULT_PASSPHRASE="uat-throwaway-not-a-real-secret"

say "LABEL=$LABEL"
say "BIN=$BIN"
say "BIN_SHA256=$(sha256sum "$BIN" | cut -d' ' -f1)"
say "BUILD_INFO=$("$BIN" --build-info 2>/dev/null | tr '\n' ' ')"
say "EXTRA=[${EXTRA[*]-}]"
say "RUST_LOG_SET=${RUST_LOG-<unset>}"

# The prompt is a TRAILING POSITIONAL, not `-p`. `-p` is the short form of
# `--provider` (main.rs:269). Harness defect #2, found on the first run: passing
# the prompt as `-p` made the binary exit 1 at argv parsing with 0 INFO lines —
# i.e. a PERFECT "no spew" reading produced by never starting the engine. That
# is exactly the self-passing shape LANE-BRIEF §3b-i describes, so the fix ships
# with the hard assertion below rather than only with the corrected argv.
START=$(date +%s.%N)
"$BIN" "${EXTRA[@]}" "$PROMPT" > "$R.stdout" 2> "$R.stderr"
RC=$?
END=$(date +%s.%N)
say "PROC_RC=$RC"
say "WALL_S=$(awk -v a="$START" -v b="$END" 'BEGIN{printf "%.2f", b-a}')"

# ── participant-alive assertion ──────────────────────────────────────────────
# A binary that failed to exec writes nothing to either stream. Zero log lines
# is the headline number this harness reports, so "it never ran" and "it is
# beautifully quiet" are the same observation unless we discriminate them.
OUT_BYTES=$(wc -c < "$R.stdout" | tr -d ' ')
ERR_LINES=$(wc -l < "$R.stderr" | tr -d ' ')
say "STDOUT_BYTES=$OUT_BYTES"
say "STDERR_LINES=$ERR_LINES"
if [ "$OUT_BYTES" -eq 0 ] && [ "$ERR_LINES" -eq 0 ]; then
  say "ASSERT_RAN=FAIL_no_output_on_either_stream"
  say "WLRC=95"; say "WLDONE"; exit 95
fi
say "ASSERT_RAN=OK"

# Harness defect #2 repair (§6b-ii). A run that died in argv parsing, in config
# resolution, or on a refused provider produces FEWER log lines than a healthy
# one — so every "the spew is gone" number this harness reports is meaningless
# unless the turn actually completed. Grade the run before grading its noise.
# `ALLOW_FAIL=1` is for the deliberate negative controls only.
if [ "$RC" -ne 0 ] && [ "${ALLOW_FAIL:-0}" != "1" ]; then
  say "ASSERT_TURN=FAIL_rc_$RC"
  say "FIRST_STDERR=$(head -1 "$R.stderr" | cut -c1-200)"
  say "WLRC=96"; say "WLDONE"; exit 96
fi
say "ASSERT_TURN=OK"

# ── the numbers ──────────────────────────────────────────────────────────────
# tracing's fmt layer emits ANSI colour; strip it before matching levels so the
# matcher cannot be defeated by an escape sequence splitting the level token.
/usr/bin/sed -E 's/\x1b\[[0-9;]*[A-Za-z]//g' "$R.stderr" > "$R.stderr.plain"
say "INFO_LINES=$(/usr/bin/grep -c ' INFO ' "$R.stderr.plain" || true)"
say "WARN_LINES=$(/usr/bin/grep -c ' WARN ' "$R.stderr.plain" || true)"
say "ERROR_LINES=$(/usr/bin/grep -c ' ERROR ' "$R.stderr.plain" || true)"
say "DEBUG_LINES=$(/usr/bin/grep -c ' DEBUG ' "$R.stderr.plain" || true)"
say "TRACE_LINES=$(/usr/bin/grep -c ' TRACE ' "$R.stderr.plain" || true)"
say "STDERR_NONBLANK=$(awk 'NF{n++} END{print n+0}' "$R.stderr.plain")"

# ── control pair, same invocation (§3b-i) ────────────────────────────────────
# POSITIVE: the harness's own result file always contains "LABEL=", so a grep
# that cannot find it is dead and every count above is worthless.
# NEGATIVE: a string that cannot occur must return 0.
say "CTRL_POS=$(/usr/bin/grep -c 'LABEL=' "$R.result" || true)"
say "CTRL_NEG=$(/usr/bin/grep -c 'ZZQQ_NOT_PRESENT_9F3A' "$R.stderr.plain" || true)"

# ── stdout shape (UAT-TUI-WINDOWS F4/F5) ─────────────────────────────────────
if [ "$OUT_BYTES" -gt 0 ]; then
  LASTBYTE=$(tail -c 1 "$R.stdout" | od -An -tx1 | tr -d ' \n')
  say "STDOUT_LAST_BYTE_HEX=$LASTBYTE"
  say "STDOUT_ENDS_NEWLINE=$([ "$LASTBYTE" = "0a" ] && echo YES || echo NO)"
  say "STDOUT_FIRST8_HEX=$(head -c 8 "$R.stdout" | od -An -tx1 | tr -d '\n' | tr -s ' ')"
  say "STDOUT_STARTS_STAR_SPACE=$([ "$(head -c 2 "$R.stdout")" = '* ' ] && echo YES || echo NO)"
else
  say "STDOUT_LAST_BYTE_HEX=<empty>"
  say "STDOUT_ENDS_NEWLINE=NA"
  say "STDOUT_STARTS_STAR_SPACE=NA"
fi

# ── provider arm read back from the product, not from the environment (§3b-ii) ─
say "ARM_LINES=$(/usr/bin/grep -iE 'provider|model' "$R.stderr.plain" | head -3 | tr '\n' '|')"

# ── did the answer actually arrive? a refused turn is not a measurement ──────
say "ANSWER_HAS_391=$(/usr/bin/grep -c '391' "$R.stdout" || true)"

say "WLRC=0"
say "WLDONE"
exit 0
