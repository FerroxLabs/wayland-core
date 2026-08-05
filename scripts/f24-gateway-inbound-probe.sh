#!/usr/bin/env bash
# F24-C3 probe: can the PERSISTENT GATEWAY RUNTIME receive an inbound message?
#
# The gateway is the surface Phase 24 installs as a systemd unit, a launchd
# plist and a scheduled task. It is what an operator runs. `run_gateway` in
# `crates/wcore-cli/src/gateway.rs` registers the channel adapters and calls
# `start_all()`, so the adapters poll — but it constructs no
# `InboundSubscriber` on the `ChannelManager` broadcast and spawns no inbound
# webhook host, both of which live only in `AgentBootstrap`.
#
# This probe asks the running gateway the question directly rather than reading
# the source: with `[inbound_webhook] enabled = true` in its own config, is
# anything listening on the configured bind?
#
# It is a real gate. It fails if the port ANSWERS (the claim would be wrong),
# it fails if the gateway did not come up (nothing was measured), and it fails
# if the control does not hold. `set -euo pipefail` is not enough on its own —
# every exit status below is captured on the line after its command, never
# through a pipe, because a pipeline reports only its last stage.

set -uo pipefail

BINARY="${1:?usage: f24-gateway-inbound-probe.sh <binary> <home> <port>}"
HOME_DIR="${2:?home}"
PORT="${3:?port}"
OUT="${HOME_DIR}/../gateway-probe.txt"

: > "$OUT"
say() { echo "$*" | tee -a "$OUT"; }

say "=== binary identity ==="
"$BINARY" --build-info >> "$OUT" 2>&1
rc=$?
say "build-info rc=${rc}"
[ "$rc" -ne 0 ] && { say "PROBE INCOMPLETE: --build-info failed"; exit 3; }

say "=== gateway config in force ==="
grep -A3 '^\[inbound_webhook\]' "${HOME_DIR}/config.toml" >> "$OUT" 2>&1
rc=$?
say "config grep rc=${rc}"
[ "$rc" -ne 0 ] && { say "PROBE INCOMPLETE: no [inbound_webhook] section in the gateway's own config"; exit 3; }

say "=== start gateway run ==="
WAYLAND_HOME="$HOME_DIR" "$BINARY" gateway run > "${HOME_DIR}/../gateway-run.log" 2>&1 &
GW_PID=$!
say "gateway pid=${GW_PID}"

# Wait for the gateway to announce itself. A probe that fired before the
# gateway was up would report "nothing listening" for the wrong reason.
up=0
for i in $(seq 1 30); do
  if grep -q '^\[gateway\] started pid=' "${HOME_DIR}/../gateway-run.log" 2>/dev/null; then
    up=1
    say "gateway announced itself after ${i}s"
    break
  fi
  echo "waiting for gateway: ${i}s $(date -u +%H:%M:%S)"
  sleep 1
done
if [ "$up" -ne 1 ]; then
  kill "$GW_PID" 2>/dev/null
  say "PROBE INCOMPLETE: gateway never announced itself"
  exit 3
fi

say "=== control: is the gateway process actually alive? ==="
kill -0 "$GW_PID" 2>/dev/null
alive=$?
say "kill -0 rc=${alive} (0 = alive)"
if [ "$alive" -ne 0 ]; then
  say "PROBE INCOMPLETE: gateway exited before the probe"
  exit 3
fi

say "=== the question: is anything listening on 127.0.0.1:${PORT}/healthz ? ==="
node -e "
  fetch('http://127.0.0.1:${PORT}/healthz')
    .then(async r => { process.stdout.write('ANSWERED ' + r.status + ' ' + (await r.text()) + '\n'); process.exit(0); })
    .catch(e => { process.stdout.write('NO_LISTENER ' + e.cause?.code + '\n'); process.exit(7); });
" >> "$OUT" 2>&1
probe_rc=$?
say "webhook probe rc=${probe_rc} (0 = something answered, 7 = nothing listening)"

say "=== gateway's own account of itself ==="
grep -E '^\[gateway\]' "${HOME_DIR}/../gateway-run.log" | head -10 >> "$OUT" 2>&1

kill "$GW_PID" 2>/dev/null
wait "$GW_PID" 2>/dev/null

if [ "$probe_rc" -eq 7 ]; then
  say "RESULT: the running gateway binds NO inbound webhook host despite [inbound_webhook] enabled = true."
  say "        Inbound cannot reach the persistent runtime. F24-C3-H2 stands."
  exit 0
fi
if [ "$probe_rc" -eq 0 ]; then
  say "RESULT: something ANSWERED — the gateway does host inbound. F24-C3-H2 is WRONG and must be withdrawn."
  exit 1
fi
say "PROBE INCOMPLETE: probe rc=${probe_rc} is neither 0 nor 7"
exit 3
