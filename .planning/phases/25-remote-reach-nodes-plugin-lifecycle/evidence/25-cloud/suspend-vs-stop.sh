#!/bin/bash
# F25 lane 25-cloud — GATE 1: is `suspend` actually distinguishable from `stop`?
#
# Binding condition C1 (25-01-panel-dissent.txt) says the reference hibernation
# transition is `suspend`, and that a `stop`/`start` cycle must NEVER be
# reported as hibernation. This gate establishes that the discriminator the
# backend uses can actually tell them apart, by driving BOTH transitions on the
# SAME machine minutes apart. The only difference between the two halves is the
# transition, so any difference in the readback is caused by the transition.
#
# The discriminator is three RAM-resident facts plus the vendor's own report:
#   - a token written to /dev/shm, which is a tmpfs and so lives only in RAM
#   - /proc/sys/kernel/random/boot_id, which is regenerated on every boot
#   - /proc/uptime, which resets on every boot
#   - the `start` response's `previous_state`
# A RAM-snapshot resume preserves all four. A cold boot destroys all four.
set -u
. /root/.wayland-f25-cloud.env
APP=wayland-f25-test
F=/root/f25-cloud/fly.sh

jsonline() { python3 -c "
import sys
lines=[l for l in sys.stdin.read().splitlines() if l.strip()]
print(lines[1] if len(lines)>1 else '')
"; }

echo "=== HOST / DATE ==="
hostname; date -u +%Y-%m-%dT%H:%M:%SZ
echo "app: $APP  (token variable name WAYLAND_F25_CLOUD_TOKEN; value never printed)"

echo
echo "=== create one machine, used for BOTH halves ==="
cat > /tmp/sm.json <<'JSON'
{"region":"iad","config":{"image":"alpine:3.20","guest":{"cpu_kind":"shared","cpus":1,"memory_mb":256},"init":{"exec":["/bin/sleep","inf"]},"auto_destroy":false,"restart":{"policy":"no"},"metadata":{"wayland_task_nonce":"f25-suspend-vs-stop"}}}
JSON
M=$($F POST "/apps/$APP/machines" /tmp/sm.json | jsonline | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
rm -f /tmp/sm.json
if [ -z "$M" ]; then echo "CREATE FAILED"; exit 1; fi
echo "machine: $M"
$F GET "/apps/$APP/machines/$M/wait?state=started&timeout=60" >/dev/null

plant() {
  cat > /tmp/se.json <<JSON
{"command":["/bin/sh","-c","printf %s $1 > /dev/shm/f25.witness; printf 'WITNESS=%s\\\\n' \$(cat /dev/shm/f25.witness); printf 'BOOT_ID=%s\\\\n' \$(cat /proc/sys/kernel/random/boot_id); printf 'UPTIME=%s\\\\n' \$(cut -d. -f1 /proc/uptime)"],"timeout":20}
JSON
  $F POST "/apps/$APP/machines/$M/exec" /tmp/se.json | jsonline
  rm -f /tmp/se.json
}
readback() {
  cat > /tmp/se.json <<'JSON'
{"command":["/bin/sh","-c","printf 'WITNESS=%s\\n' \"$(cat /dev/shm/f25.witness 2>/dev/null || printf MISSING)\"; printf 'BOOT_ID=%s\\n' \"$(cat /proc/sys/kernel/random/boot_id)\"; printf 'UPTIME=%s\\n' \"$(cut -d. -f1 /proc/uptime)\""],"timeout":20}
JSON
  $F POST "/apps/$APP/machines/$M/exec" /tmp/se.json | jsonline
  rm -f /tmp/se.json
}

echo
echo "############ HALF A — SUSPEND / START (the reference transition) ############"
echo "--- before (state planted in guest RAM) ---"
plant f25-witness-suspend
echo "--- suspend ---"
$F POST "/apps/$APP/machines/$M/suspend"
for i in 1 2 3 4 5 6 7 8 9 10; do
  ST=$($F GET "/apps/$APP/machines/$M" | jsonline | python3 -c "import sys,json; print(json.load(sys.stdin)['state'])")
  [ "$ST" = "suspended" ] && break
  sleep 2
done
echo "state read back from vendor after suspend: $ST"
echo "--- start (resume) ---"
$F POST "/apps/$APP/machines/$M/start" | jsonline
$F GET "/apps/$APP/machines/$M/wait?state=started&timeout=60" >/dev/null
sleep 2
echo "--- after ---"
readback

echo
echo "############ HALF B — STOP / START (the CONTROL, must NOT look like hibernation) ############"
echo "--- before (state planted in guest RAM) ---"
plant f25-witness-stop
echo "--- stop ---"
$F POST "/apps/$APP/machines/$M/stop"
$F GET "/apps/$APP/machines/$M/wait?state=stopped&timeout=60" >/dev/null
echo "--- start ---"
$F POST "/apps/$APP/machines/$M/start" | jsonline
$F GET "/apps/$APP/machines/$M/wait?state=started&timeout=60" >/dev/null
sleep 3
echo "--- after ---"
readback

echo
echo "=== CLEANUP ==="
$F DELETE "/apps/$APP/machines/$M?force=true"
sleep 4
$F GET "/apps/$APP/machines" | python3 -c "
import sys,json
lines=[l for l in sys.stdin.read().splitlines() if l.strip()]
ms=json.loads(lines[1])
print('machines remaining in the app:', len(ms), json.dumps(ms))
sys.exit(0 if len(ms)==0 else 1)
"
echo "CLEANUP_VERIFIED_EXIT=$?"
