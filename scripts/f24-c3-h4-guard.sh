#!/usr/bin/env bash
# F24-C3-H4 regression guard — does `gateway run` poll each account ONCE, and
# does an inbound message on a POLLING adapter still arrive?
#
# WHAT IT ASSERTS, AND WHY IT TAKES TWO RUNS
#
#   A  PRE-FIX binary   -> TWO pollers, and inbound LOST
#   B  POST-FIX binary  -> ONE poller,  and inbound ARRIVES (turns + replies),
#                          AND the cron schedule still fires through the same
#                          adapter
#
# The poller count is measured by the fixture, in another OS process, from
# overlapping open `getUpdates` requests. It is not a log line the binary prints
# about itself.
#
# THIS GUARD CAN FAIL IN EVERY DIRECTION, INCLUDING THE ONE THAT MATTERS MOST:
#
#   * B pollers == 0  -> FAIL. "Nothing polls" also satisfies "no duplicate
#                        registration". A fix that works by making nothing start
#                        is the universal-denial green, and it is failed here
#                        explicitly rather than passed by omission.
#   * B lost != 0     -> FAIL. One manager that receives nothing is not a fix.
#   * B turns != submitted -> FAIL. Replies with no model turn behind them.
#   * B cron_fires == 0 -> FAIL. The cron handler no longer owns a manager; if
#                        the scheduler lost its send path, this lane traded a
#                        message-loss risk for a dead scheduler.
#   * A pollers != 2  -> the double start did NOT reproduce on this tree. Graded
#                        INCOMPLETE and reported as a contradiction to resolve,
#                        NOT swallowed, and NOT converted into a pass for B.
#   * A lost == 0     -> the double start is real but produced no loss in this
#                        run. Reported as UNPROVEN-RACE — an honest distinct
#                        state, not a FAIL and not a PASS.
#   * either run instrument_fault -> INCOMPLETE. The driver could not read the
#                        product's own output, so no loss claim may be made.
#   * identical binaries -> INCOMPLETE before any leg runs.
#
# Every exit status is captured on the line AFTER its command, never through a
# pipe: `cmd | tee` reports tee's status, not cmd's.
#
# usage: f24-c3-h4-guard.sh <prefix-binary> <postfix-binary> <run-root>

set -uo pipefail

PREFIX_BIN="${1:?usage: f24-c3-h4-guard.sh <prefix-binary> <postfix-binary> <run-root>}"
POSTFIX_BIN="${2:?postfix binary}"
ROOT="${3:?run root}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

mkdir -p "$ROOT"
OUT="$ROOT/f24-c3-h4-guard.txt"
: > "$OUT"
say() { echo "$*" | tee -a "$OUT"; }

say "=== F24-C3-H4 gateway polling-race guard ==="
say "prefix  binary: $PREFIX_BIN"
say "postfix binary: $POSTFIX_BIN"

# Binary identity. NOTE: `--build-info` reports the git HEAD sha, which for a
# mutation built from a modified working tree is the SAME string for both
# binaries. So identity here is decided by sha256 of the file, never by the
# version banner — using the banner would let two different binaries look
# identical, or two identical ones look different.
PREFIX_SHA=$(sha256sum "$PREFIX_BIN" | cut -d' ' -f1)
POSTFIX_SHA=$(sha256sum "$POSTFIX_BIN" | cut -d' ' -f1)
say "prefix  sha256 ${PREFIX_SHA}"
say "postfix sha256 ${POSTFIX_SHA}"
if [ "$PREFIX_SHA" = "$POSTFIX_SHA" ]; then
  say "GUARD INCOMPLETE: the two binaries are byte-identical, so A/B cannot"
  say "                  distinguish the fix from its absence."
  exit 3
fi

run_leg() {
  local name="$1" bin="$2"; shift 2
  local dir="$ROOT/$name"
  rm -rf "$dir"; mkdir -p "$dir"
  say "--- leg ${name} ---"
  node "$HERE/f24-c3-h4-polling-race.mjs" --binary "$bin" --run-dir "$dir" "$@" \
    > "$dir/driver.log" 2>&1
  local drc=$?
  echo "$drc" > "$dir/driver.rc"
  say "leg ${name} driver rc=${drc}"
  grep -E '^F24C3H4 RACE' "$dir/driver.log" | tee -a "$OUT"
  return 0
}

field() {
  local dir="$1" key="$2" f="$1/f24-c3-h4-race-result.json"
  [ -f "$f" ] || { echo ABSENT; return; }
  node -e "
    const r = require('$f');
    const v = r['$key'];
    process.stdout.write(v === undefined || v === null ? 'ABSENT' : String(v));
  " 2>/dev/null || echo ABSENT
}

run_leg A "$PREFIX_BIN"  --preload 4 --live 4 --budget-ms 90000
run_leg B "$POSTFIX_BIN" --preload 4 --live 4 --budget-ms 180000 --cron

say "=== observations ==="
A_POLLERS=$(field "$ROOT/A" max_concurrent_getupdates)
A_LOST=$(field "$ROOT/A" lost_total)
A_TURNS=$(field "$ROOT/A" turns_total)
A_SUB=$(field "$ROOT/A" submitted_total)
A_RAW=$(field "$ROOT/A" raw_replies_total)
A_IF=$(field "$ROOT/A" instrument_fault)

B_POLLERS=$(field "$ROOT/B" max_concurrent_getupdates)
B_LOST=$(field "$ROOT/B" lost_total)
B_TURNS=$(field "$ROOT/B" turns_total)
B_SUB=$(field "$ROOT/B" submitted_total)
B_REPLIED=$(field "$ROOT/B" replied_total)
B_DUP=$(field "$ROOT/B" duplicated_total)
B_CRON=$(field "$ROOT/B" cron_fires)
B_IF=$(field "$ROOT/B" instrument_fault)

say "A prefix : pollers=${A_POLLERS} submitted=${A_SUB} turns=${A_TURNS} lost=${A_LOST} raw_replies=${A_RAW}"
say "B postfix: pollers=${B_POLLERS} submitted=${B_SUB} turns=${B_TURNS} replied=${B_REPLIED} lost=${B_LOST} dup=${B_DUP} cron_fires=${B_CRON}"

VERDICT=PASS
note_fail() { say "  !! $*"; VERDICT=FAIL; }
note_incomplete() { say "  ?? $*"; [ "$VERDICT" = FAIL ] || VERDICT=INCOMPLETE; }

# --- instrument first. A driver that cannot read the product's output makes
#     every other number in this file meaningless.
if [ "$A_IF" != "ABSENT" ]; then
  note_incomplete "A: instrument fault — ${A_IF}"
fi
if [ "$B_IF" != "ABSENT" ]; then
  note_incomplete "B: instrument fault — ${B_IF}"
fi

# --- A: the reproduction. --------------------------------------------------
if [ "$A_POLLERS" = "2" ]; then
  say "  ok A: the PRE-FIX gateway polled the one account with TWO managers"
elif [ "$A_POLLERS" = "ABSENT" ]; then
  note_incomplete "A produced no readable result"
else
  note_incomplete "A saw ${A_POLLERS} concurrent pollers, expected 2. F24-C3-H4 does NOT \
reproduce on this tree — resolve that before reading B as a fix for anything."
fi
if [ "$A_LOST" = "0" ]; then
  say "  ?? A: the double start reproduced but NO message was lost in this run — \
UNPROVEN-RACE. The duplication is real; the consumption race is not demonstrated here."
elif [ "$A_LOST" = "ABSENT" ]; then
  note_incomplete "A produced no loss figure"
else
  say "  ok A: ${A_LOST}/${A_SUB} inbound messages LOST, with ${A_TURNS} agent turns run"
fi

# --- B: the fix. -----------------------------------------------------------
if [ "$B_POLLERS" = "0" ]; then
  note_fail "B: the fixed gateway polled the account ZERO times. 'nothing starts' also \
satisfies 'no duplicate registration' — this is the universal-denial green, not a fix."
elif [ "$B_POLLERS" != "1" ]; then
  note_fail "B: ${B_POLLERS} concurrent pollers after the fix, expected exactly 1"
else
  say "  ok B: exactly ONE manager polled the account"
fi

if [ "$B_LOST" != "0" ]; then
  note_fail "B: ${B_LOST}/${B_SUB} inbound messages still lost after the fix"
elif [ "$B_REPLIED" != "$B_SUB" ]; then
  note_fail "B: replied=${B_REPLIED} of submitted=${B_SUB}"
elif [ "$B_TURNS" != "$B_SUB" ]; then
  note_fail "B: ${B_REPLIED} replies but ${B_TURNS} model turns — a reply with no turn behind it"
elif [ "$B_DUP" != "0" ]; then
  note_fail "B: ${B_DUP} messages answered MORE than once — loss traded for duplication is not a fix"
else
  say "  ok B: ${B_SUB} submitted, ${B_TURNS} turns, ${B_REPLIED} replies, 0 lost, 0 duplicated"
fi

if [ "$B_CRON" = "0" ] || [ "$B_CRON" = "ABSENT" ]; then
  note_fail "B: cron_fires=${B_CRON}. The cron handler no longer owns a manager; if the \
scheduler cannot reach a channel, this fix traded message loss for a dead scheduler."
else
  say "  ok B: the cron schedule fired ${B_CRON} time(s) through the SAME shared manager"
fi

say "=== VERDICT: ${VERDICT} ==="
case "$VERDICT" in
  PASS) exit 0 ;;
  FAIL) exit 1 ;;
  *)    exit 3 ;;
esac
