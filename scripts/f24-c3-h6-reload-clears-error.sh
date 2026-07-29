#!/usr/bin/env bash
# F24-C3-H6 — driven proof that `channel reload` erases the record of a dead
# inbound path.
#
# THE DEFECT. `gateway.rs`'s reload success branch sets `registration_error =
# None` unconditionally. That field is the ONLY thing `channel health` fails on
# once `registered >= configured`, and at startup it carries facts a reload does
# not re-evaluate and cannot fix — most concretely that this gateway LOST the
# single-owner inbound polling lease and therefore "will send but not poll".
#
# So: gateway starts degraded, `channel health` correctly exits non-zero, the
# operator runs the documented `channel reload`, and health starts exiting 0
# while the inbound path is exactly as dead as it was a second earlier.
#
# THE ONE VARIABLE IS THE BINARY. Everything else — the home, the channel
# config, the lock holder, the ordering, the waits — is generated here and is
# identical across runs. Invoke as:
#
#   f24-c3-h6-reload-clears-error.sh <path-to-wayland-core> <run-dir>
#
# EXIT: 0 iff every leg passes. Any leg failing exits 1. A leg that could not be
# RUN exits 2 (instrument fault), which is deliberately distinct from a product
# failure — a driver that reports "defect absent" because it never drove
# anything is the self-passing shape LANE-BRIEF 3b-i warns about.
set -u -o pipefail

BIN="${1:?usage: $0 <binary> <run-dir>}"
RUN="${2:?usage: $0 <binary> <run-dir>}"

[ -x "$BIN" ] || { echo "INSTRUMENT-FAULT: $BIN is not executable"; exit 2; }

HOME_DIR="$RUN/home"
CHANNELS="$HOME_DIR/channels"
LOCK="$CHANNELS/channel-poll.lock"
GWLOG="$RUN/gateway.log"
RESULT="$RUN/result.json"

rm -rf "$RUN"
mkdir -p "$CHANNELS"

PASS=0
FAIL=0
LEGS=""
leg() { # leg <name> <ok|no> <detail>
  if [ "$2" = ok ]; then PASS=$((PASS + 1)); echo "PASS  $1 — $3"
  else FAIL=$((FAIL + 1)); echo "FAIL  $1 — $3"; fi
  LEGS="${LEGS}$(printf '\n    {"leg":"%s","pass":%s,"detail":"%s"}' \
    "$1" "$([ "$2" = ok ] && echo true || echo false)" "$(echo "$3" | tr -d '"' | tr '\n' ' ')"),"
}

# ---------------------------------------------------------------------------
# The channel. `platform = "slack"` because its factory constructs OFFLINE, so
# `registered` reaches 1 without a network or a real credential and the
# `registered >= configured` half of `is_complete()` is satisfied — which is
# required, or health would fail on the COUNT and the run would prove nothing
# about `registration_error`.
# ---------------------------------------------------------------------------
cat > "$CHANNELS/f24h6.toml" <<'TOML'
name = "f24h6"
platform = "slack"
enabled = true

[options]
workspace_name = "f24h6"
default_channel_id = "DF24H6"
credential_handle_bot_token = "slack.f24h6.bot_token"
credential_handle_signing_secret = "slack.f24h6.signing_secret"
max_retry_attempts = 1

[inbound]
dm = "allowlist"
dm_allowlist = ["U-F24H6"]
group = "disabled"
TOML

# ---------------------------------------------------------------------------
# Take the inbound polling lease with a foreign holder, exactly as a second
# wayland process (an ordinary session, or `cron daemon`) does. It is a plain
# exclusive flock over a one-byte sentinel, so `flock(1)` is the same lock the
# product takes -- not a simulation of it.
#
# Pre-created at exactly one byte: the product rewrites a sentinel of any other
# length BEFORE locking, and a rewrite is not what is under test.
# ---------------------------------------------------------------------------
printf '\0' > "$LOCK"
flock -x "$LOCK" -c 'echo LOCKHELD; sleep 900' > "$RUN/lockholder.log" 2>&1 &
LOCK_PID=$!

# LANE-BRIEF 6a-i: assert the PARTICIPANT STARTED. A lock holder that never
# acquired makes this a one-actor run, and the contention this whole test is
# about cannot appear with one actor -- it would read as a clean pass.
for _ in $(seq 1 50); do
  grep -q LOCKHELD "$RUN/lockholder.log" 2>/dev/null && break
  sleep 0.2
done
if ! grep -q LOCKHELD "$RUN/lockholder.log" 2>/dev/null; then
  echo "INSTRUMENT-FAULT: the foreign lock holder never acquired the lease."
  kill "$LOCK_PID" 2>/dev/null
  exit 2
fi
echo "setup: foreign lock holder live (pid $LOCK_PID) on $LOCK"

cleanup() {
  [ -n "${GW_PID:-}" ] && kill "$GW_PID" 2>/dev/null
  kill "$LOCK_PID" 2>/dev/null
  wait 2>/dev/null
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
export WAYLAND_HOME="$HOME_DIR"
"$BIN" gateway run > "$GWLOG" 2>&1 &
GW_PID=$!

for _ in $(seq 1 100); do
  grep -q "channels registered=" "$GWLOG" 2>/dev/null && break
  sleep 0.2
done
if ! grep -q "channels registered=" "$GWLOG" 2>/dev/null; then
  echo "INSTRUMENT-FAULT: gateway never reported channel registration. Log:"
  sed -n '1,40p' "$GWLOG"
  exit 2
fi

# LANE-BRIEF 3b-ii: read the state back from the PRODUCT'S OWN OUTPUT rather
# than inferring it from the setup. If the gateway won the lease, my lock holder
# did not do what I think and every later number is about a different world.
if grep -q "inbound polling is owned by another process" "$GWLOG"; then
  leg "gateway-observed-the-lost-lease" ok \
    "$(grep -m1 'inbound polling is owned' "$GWLOG" | tr -d '\r')"
else
  echo "INSTRUMENT-FAULT: the gateway did NOT report a lost lease, so the"
  echo "precondition this test needs does not hold. Log:"
  sed -n '1,40p' "$GWLOG"
  exit 2
fi

health() { WAYLAND_HOME="$HOME_DIR" "$BIN" channel health > "$1" 2>&1; echo $?; }

# --- Leg: health fails BEFORE the reload. -----------------------------------
# NOTE this leg passes on the BROKEN binary too. It is the "old shape", kept
# deliberately and labelled, because it is the control that proves the health
# surface can fail at all -- and on its own it is worth nothing.
for _ in $(seq 1 30); do
  [ -f "$HOME_DIR/channel-health.json" ] && break
  sleep 0.2
done
RC_BEFORE=$(health "$RUN/health-before.txt")
if [ "$RC_BEFORE" != 0 ]; then
  leg "health-fails-while-the-lease-is-lost--OLD-SHAPE" ok \
    "rc=$RC_BEFORE: $(tr -d '\r' < "$RUN/health-before.txt" | tail -1)"
else
  leg "health-fails-while-the-lease-is-lost--OLD-SHAPE" no \
    "rc=0 — health reported complete on a gateway that is not polling"
fi

# --- Drive the reload, and prove it actually ran. ---------------------------
WAYLAND_HOME="$HOME_DIR" "$BIN" channel reload > "$RUN/reload.txt" 2>&1
RC_RELOAD=$?
for _ in $(seq 1 40); do
  grep -q "channel reload: added=" "$GWLOG" 2>/dev/null && break
  sleep 0.25
done
if grep -q "channel reload: added=" "$GWLOG"; then
  leg "the-reload-actually-ran" ok \
    "$(grep -m1 'channel reload: added=' "$GWLOG" | tr -d '\r')"
else
  echo "INSTRUMENT-FAULT: the reload never reached the gateway (cli rc=$RC_RELOAD),"
  echo "so the post-reload health reading is not a measurement of anything."
  sed -n '1,60p' "$GWLOG"
  exit 2
fi
# The health document is republished on the tick AFTER the reload block.
sleep 2

# --- THE LEG THAT SEPARATES THE TWO BINARIES. ------------------------------
# Nothing about the dead inbound path changed: the foreign holder still holds
# the lease, and the gateway never re-attempts it. So health MUST still fail.
RC_AFTER=$(health "$RUN/health-after.txt")
if [ "$RC_AFTER" != 0 ]; then
  leg "health-STILL-fails-after-a-successful-reload" ok \
    "rc=$RC_AFTER: $(tr -d '\r' < "$RUN/health-after.txt" | tail -1)"
else
  leg "health-STILL-fails-after-a-successful-reload" no \
    "rc=0 — a reload that fixed NOTHING erased the degradation report"
fi

# --- Leg: the lock holder is still alive, so the path really is still dead. --
# Without this the run could be graded on a world where the degradation had
# genuinely been resolved, and clearing the error would have been CORRECT.
if kill -0 "$LOCK_PID" 2>/dev/null; then
  leg "the-degradation-was-still-real-at-the-final-reading" ok \
    "foreign lock holder pid $LOCK_PID still alive"
else
  leg "the-degradation-was-still-real-at-the-final-reading" no \
    "the lock holder died mid-run — the final reading is not interpretable"
fi

cat > "$RESULT" <<JSON
{
  "finding": "F24-C3-H6",
  "binary": "$BIN",
  "pass": $PASS,
  "fail": $FAIL,
  "rc_health_before_reload": $RC_BEFORE,
  "rc_health_after_reload": $RC_AFTER,
  "legs": [${LEGS%,}
  ]
}
JSON

echo
echo "legs: $PASS passed / $FAIL failed   (result: $RESULT)"
[ "$FAIL" -eq 0 ]
