#!/usr/bin/env bash
# Lane A — one live health-observation run against a real `gateway run`.
#
# The ORDER here is the whole point. An earlier draft of this measurement
# started the sampler AFTER the gateway and recorded healthy 0/46 on the
# UNFIXED binary — a perfect score for a binary that demonstrably flashes
# Healthy, because the flash had already happened and the steady state is
# `unauthenticated`. A negative control that starts late cannot fail, so it
# proves nothing.
#
# So: the sampler starts FIRST and the gateway starts INTO a running sampler.
# The startup window — the only window in which the defect is observable — is
# inside the measurement rather than before it.
#
# The gateway is killed by the PID THIS script recorded, never by `pkill -f`.
# A `pkill -f "wayland-core gateway run"` on this shared build host matches
# other lanes' long-lived gateways (one has been up for two days) and matches
# the invoking ssh command line itself, which kills the shell running the
# measurement. Both were hit while building this.

set -u

BIN=""
HOME_DIR=""
LABEL="unlabelled"
OUTDIR=""
SAMPLES=46
CADENCE=2000

while [ $# -gt 0 ]; do
  case "$1" in
    --bin=*)      BIN="${1#*=}" ;;
    --home=*)     HOME_DIR="${1#*=}" ;;
    --label=*)    LABEL="${1#*=}" ;;
    --outdir=*)   OUTDIR="${1#*=}" ;;
    --samples=*)  SAMPLES="${1#*=}" ;;
    --cadence=*)  CADENCE="${1#*=}" ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ -z "$BIN" ] || [ -z "$HOME_DIR" ] || [ -z "$OUTDIR" ]; then
  echo "usage: --bin=<path> --home=<WAYLAND_HOME> --outdir=<dir> [--label=X] [--samples=N] [--cadence=MS]" >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$OUTDIR"
PIDFILE="$OUTDIR/$LABEL.gw.pid"
GWLOG="$OUTDIR/$LABEL.gateway.log"
SAMPLEOUT="$OUTDIR/$LABEL.sampler.json"

kill_recorded_gateway() {
  if [ -f "$PIDFILE" ]; then
    pid="$(cat "$PIDFILE")"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null
      for _ in 1 2 3 4 5 6 7 8 9 10; do
        kill -0 "$pid" 2>/dev/null || break
        sleep 1
      done
      kill -9 "$pid" 2>/dev/null
    fi
    rm -f "$PIDFILE"
  fi
}

kill_recorded_gateway

# Clear the projections a previous run left behind. A stale
# channel-health.json is read by the transient watcher (which does not check
# the pid lock) and would be scored as a real observation.
rm -f "$HOME_DIR/channel-health.json" "$HOME_DIR/gateway-status.json" "$HOME_DIR/gateway.pid" "$GWLOG"

# 1. Sampler first.
node "$SCRIPT_DIR/lane-a-health-sampler.mjs" \
  --bin="$BIN" --home="$HOME_DIR" --samples="$SAMPLES" --cadence-ms="$CADENCE" \
  --label="$LABEL" --out="$SAMPLEOUT" &
SAMPLER_PID=$!

# 2. Gateway into the running sampler.
sleep 0.3
WAYLAND_HOME="$HOME_DIR" RUST_LOG="${RUST_LOG:-wcore_channel_discord=debug,wcore_channels=debug}" \
  "$BIN" gateway run >"$GWLOG" 2>&1 &
echo $! > "$PIDFILE"

wait "$SAMPLER_PID"
SAMPLER_RC=$?

kill_recorded_gateway

echo "--- gateway log (handshake lines) ---"
grep -aE "sent IDENTIFY|sent RESUME|READY received|RESUMED received|published Connected|rejected the bot token|gateway session ended|Unauthenticated" "$GWLOG" | head -40

exit "$SAMPLER_RC"
