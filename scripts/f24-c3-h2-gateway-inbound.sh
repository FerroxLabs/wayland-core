#!/usr/bin/env bash
# F24-C3-H2 regression guard — can the PERSISTENT GATEWAY RUNTIME receive an
# inbound message?
#
# The gateway is the surface Phase 24 installs as a systemd unit, a launchd
# plist and a scheduled task. It is what an operator runs. Before this fix,
# `run_gateway` registered its channel adapters and called `start_all()`, so
# they polled — and constructed no `InboundSubscriber` on the manager broadcast
# and spawned no inbound webhook host. Both lived only in `AgentBootstrap`,
# which the gateway does not use.
#
# WHAT THIS GUARD ASSERTS, AND WHY IT TAKES THREE RUNS
#
# The observable is a real message arriving at a real endpoint in another
# process, derived from that sink's own journal — never a status line the
# product prints about itself, and never the config key that started this whole
# finding by saying `enabled = true` over a socket nobody served.
#
#   A  PRE-FIX binary, --runtime json-stream   must be GREEN 15/15
#   B  PRE-FIX binary, --runtime gateway       must be RED with NO listener
#   C  POST-FIX binary, --runtime gateway      must be GREEN 15/15
#
# A is not decoration. Without it, B's RED is worth nothing: a driver that
# failed for its own reasons — a broken fixture, a port already held, a config
# the binary rejects — produces exactly the same RED as the defect does. A runs
# the IDENTICAL driver, fixtures, config and legs against the IDENTICAL binary
# and only the runtime surface differs, so a GREEN A and a RED B isolate the
# fault to the runtime and nothing else. This is the same class of mistake the
# 24-C3 lane caught in its own `access` leg, which passed at the pre-fix binary
# because every message was denied: a green produced by universal denial, and a
# red produced by universal breakage, are the same error wearing two colours.
#
# THIS GUARD CAN FAIL IN EVERY DIRECTION.
#   * C red   -> the fix does not work. Reported red.
#   * B green -> the finding was wrong, or the pre-fix binary is not pre-fix.
#                Reported as a contradiction to be resolved, not swallowed.
#   * A red   -> the instrument is broken; B's red proves nothing and the whole
#                run is graded INCOMPLETE rather than passed.
#
# Every exit status is captured on the line AFTER its command, never through a
# pipe: `cmd | tee` reports tee's status, not cmd's.
#
# usage: f24-c3-h2-gateway-inbound.sh <prefix-binary> <postfix-binary> <run-root>

set -uo pipefail

PREFIX_BIN="${1:?usage: f24-c3-h2-gateway-inbound.sh <prefix-binary> <postfix-binary> <run-root>}"
POSTFIX_BIN="${2:?postfix binary}"
ROOT="${3:?run root}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

mkdir -p "$ROOT"
OUT="$ROOT/f24-c3-h2-guard.txt"
: > "$OUT"
say() { echo "$*" | tee -a "$OUT"; }

say "=== F24-C3-H2 gateway inbound guard ==="
say "prefix  binary: $PREFIX_BIN"
say "postfix binary: $POSTFIX_BIN"

# Binary identity, read out of each binary rather than assumed from a path.
# Two runs attributed to the same binary would make A's control vacuous.
say "--- binary identity ---"
"$PREFIX_BIN" --build-info > "$ROOT/prefix.build-info" 2>&1
rc=$?
say "prefix --build-info rc=${rc}: $(cat "$ROOT/prefix.build-info" | head -1)"
[ "$rc" -ne 0 ] && { say "GUARD INCOMPLETE: prefix --build-info failed"; exit 3; }

"$POSTFIX_BIN" --build-info > "$ROOT/postfix.build-info" 2>&1
rc=$?
say "postfix --build-info rc=${rc}: $(cat "$ROOT/postfix.build-info" | head -1)"
[ "$rc" -ne 0 ] && { say "GUARD INCOMPLETE: postfix --build-info failed"; exit 3; }

PREFIX_SHA=$(sha256sum "$PREFIX_BIN" | cut -d' ' -f1)
POSTFIX_SHA=$(sha256sum "$POSTFIX_BIN" | cut -d' ' -f1)
say "prefix  sha256 ${PREFIX_SHA}"
say "postfix sha256 ${POSTFIX_SHA}"
if [ "$PREFIX_SHA" = "$POSTFIX_SHA" ]; then
  say "GUARD INCOMPLETE: the two binaries are byte-identical, so A/B/C cannot"
  say "                  distinguish the fix from its absence."
  exit 3
fi

# One run of the matrix. Emits progress continuously — a silent subprocess is
# indistinguishable from a hung one to anything watching this script.
run_leg() {
  local name="$1" bin="$2" runtime="$3"
  local dir="$ROOT/$name"
  rm -rf "$dir"; mkdir -p "$dir"
  say "--- leg ${name}: runtime=${runtime} ---"
  node "$HERE/f24-inbound.mjs" --binary "$bin" --run-dir "$dir" --runtime "$runtime" \
    > "$dir/driver.log" 2>&1
  local drc=$?
  echo "$drc" > "$dir/driver.rc"
  say "leg ${name} driver rc=${drc}"
  grep -E '^INBOUND MATRIX' "$dir/driver.log" | tee -a "$OUT"
  return 0
}

# Read a field out of a leg's result JSON. Absent file or absent field yields
# the literal string ABSENT, which no comparison below accepts as a pass.
field() {
  local dir="$1" key="$2"
  local f
  f=$(ls "$dir"/*-inbound-result.json 2>/dev/null | head -1)
  [ -z "$f" ] && { echo ABSENT; return; }
  node -e "
    const r = require('$f');
    const v = r['$key'];
    process.stdout.write(v === undefined || v === null ? 'ABSENT' : String(v));
  " 2>/dev/null || echo ABSENT
}

# Count failed legs from the result JSON, not from the banner: a banner is a
# string the driver prints about itself.
failed_legs() {
  local dir="$1"
  local f
  f=$(ls "$dir"/*-inbound-result.json 2>/dev/null | head -1)
  [ -z "$f" ] && { echo ABSENT; return; }
  node -e "
    const r = require('$f');
    process.stdout.write(String(r.results.filter((x) => !x.ok).length));
  " 2>/dev/null || echo ABSENT
}

total_legs() {
  local dir="$1"
  local f
  f=$(ls "$dir"/*-inbound-result.json 2>/dev/null | head -1)
  [ -z "$f" ] && { echo ABSENT; return; }
  node -e "const r=require('$f');process.stdout.write(String(r.results.length));" 2>/dev/null || echo ABSENT
}

run_leg A "$PREFIX_BIN"  json-stream
run_leg B "$PREFIX_BIN"  gateway
run_leg C "$POSTFIX_BIN" gateway

say "=== observations ==="
A_FAILED=$(failed_legs "$ROOT/A"); A_TOTAL=$(total_legs "$ROOT/A")
A_ARR=$(field "$ROOT/A" arrivals_total); A_BOUND=$(field "$ROOT/A" webhook_host_bound)
B_FAILED=$(failed_legs "$ROOT/B"); B_TOTAL=$(total_legs "$ROOT/B")
B_ARR=$(field "$ROOT/B" arrivals_total); B_BOUND=$(field "$ROOT/B" webhook_host_bound)
C_FAILED=$(failed_legs "$ROOT/C"); C_TOTAL=$(total_legs "$ROOT/C")
C_ARR=$(field "$ROOT/C" arrivals_total); C_BOUND=$(field "$ROOT/C" webhook_host_bound)
C_TURNS=$(field "$ROOT/C" turns_total)

say "A prefix/json-stream : legs=${A_TOTAL} failed=${A_FAILED} arrivals=${A_ARR} webhook_bound=${A_BOUND}"
say "B prefix/gateway     : legs=${B_TOTAL} failed=${B_FAILED} arrivals=${B_ARR} webhook_bound=${B_BOUND}"
say "C postfix/gateway    : legs=${C_TOTAL} failed=${C_FAILED} arrivals=${C_ARR} webhook_bound=${C_BOUND} turns=${C_TURNS}"

VERDICT=PASS
note_fail() { say "  !! $*"; VERDICT=FAIL; }
note_incomplete() { say "  ?? $*"; VERDICT=INCOMPLETE; }

# --- A: the control. -------------------------------------------------------
# A red A means the instrument is broken and B's red is uninterpretable.
if [ "$A_TOTAL" != "15" ]; then
  note_incomplete "A ran ${A_TOTAL} legs, expected 15 — a run that measured nothing cannot control anything"
elif [ "$A_FAILED" != "0" ]; then
  note_incomplete "A failed ${A_FAILED} legs at the PRE-FIX binary on json-stream. The instrument, \
the fixtures or the host is at fault, NOT the gateway; B's red proves nothing until this is green."
else
  say "  ok A: the instrument, fixtures and pre-fix binary produce 15/15 on json-stream"
fi

# --- B: the falsifier. -----------------------------------------------------
if [ "$B_BOUND" = "true" ]; then
  note_fail "B: the PRE-FIX gateway BOUND an inbound webhook host. F24-C3-H2 as written is \
wrong, or \$PREFIX_BIN is not actually pre-fix. Resolve before trusting C."
elif [ "$B_BOUND" != "false" ]; then
  note_incomplete "B: webhook_host_bound=${B_BOUND}; B produced no readable result"
elif [ "$B_ARR" != "0" ]; then
  note_fail "B: no listener bound yet ${B_ARR} arrivals were recorded — the journal is being \
written by something other than this run"
else
  say "  ok B: the pre-fix gateway bound NO inbound host and received 0 messages"
fi

# --- C: the fix. -----------------------------------------------------------
if [ "$C_TOTAL" != "15" ]; then
  note_fail "C ran ${C_TOTAL} legs, expected 15"
elif [ "$C_BOUND" != "true" ]; then
  note_fail "C: the fixed gateway bound no inbound webhook host"
elif [ "$C_FAILED" != "0" ]; then
  note_fail "C: ${C_FAILED}/15 legs failed at the fixed binary — the gateway still does not receive"
elif [ "$C_ARR" = "0" ] || [ "$C_ARR" = "ABSENT" ]; then
  note_fail "C: 15 legs passed with ${C_ARR} arrivals — a green with no arrival is a green over a \
dead path, which is the exact defect class this guard exists for"
elif [ "$C_TURNS" = "0" ] || [ "$C_TURNS" = "ABSENT" ]; then
  note_fail "C: ${C_ARR} arrivals but ${C_TURNS} turns — replies without model turns behind them"
else
  say "  ok C: the fixed gateway bound a host, took ${C_TURNS} turns and delivered ${C_ARR} replies"
fi

say "=== VERDICT: ${VERDICT} ==="
case "$VERDICT" in
  PASS) exit 0 ;;
  FAIL) exit 1 ;;
  *)    exit 3 ;;
esac
