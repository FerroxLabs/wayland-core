#!/bin/sh
# Degraded cases for `gateway support-bundle`, OBSERVED rather than asserted.
# This is diagnostic machinery: it runs AFTER something has already failed.
set -u
BIN=/root/wayland-support-bundle/target/debug/wayland-core
GREP=/usr/bin/grep
W=/root/sb-degraded
rm -rf "$W"; mkdir -p "$W"
fails=0
ok()  { printf 'PASS  %s\n' "$1"; }
bad() { printf 'FAIL  %s\n' "$1"; fails=$((fails+1)); }

echo "################ D1: STALE STATUS FILE, DEAD GATEWAY ################"
echo "The trap the fix exists for. A crashed gateway leaves gateway-status.json"
echo "behind claiming Running. A bundle that copies it ships a confident lie."
H1="$W/h1"; mkdir -p "$H1"
DEADPID=999999
kill -0 "$DEADPID" 2>/dev/null && { echo "pid $DEADPID is alive, pick another"; exit 9; }
cat > "$H1/gateway.pid" <<EOF
{"pid":$DEADPID,"home":"$H1","started_at":"2026-07-29T00:00:00Z","binary_path":"/opt/x/wayland-core"}
EOF
cat > "$H1/gateway-status.json" <<EOF
{"state":"running","pid":$DEADPID,"uptime_secs":98765,"profile":"default","turns_in_flight":7,"deliveries_pending":42,"binary_path":"/opt/x/wayland-core","binary_version":"9.9.9"}
EOF
echo "--- the LIE planted on disk ---"; cat "$H1/gateway-status.json"; echo
env WAYLAND_HOME="$H1" "$BIN" gateway support-bundle --out "$W/b1" > "$W/b1.out" 2>&1
echo "exit=$?"; sed 's/^/    /' "$W/b1.out"
echo "--- what the BUNDLE says ---"; cat "$W/b1/gateway-status.json"; echo
if $GREP -q '"running": false' "$W/b1/gateway-status.json"; then
  ok "D1 the bundle reports running=false for a dead pid"
else bad "D1 the bundle repeated the stale Running claim"; fi
# The discriminator: the planted lie's own field values must NOT appear.
if $GREP -q '98765' "$W/b1/gateway-status.json"; then
  bad "D1 the stale uptime 98765 was copied verbatim into the bundle"
else ok "D1 the stale uptime (98765) did NOT survive into the bundle"; fi
if $GREP -q '"turns_in_flight": 7' "$W/b1/gateway-status.json"; then
  bad "D1 the stale turns_in_flight=7 was copied verbatim"
else ok "D1 the stale turns_in_flight (7) did NOT survive"; fi
# Known-positive for that pair of negatives: prove the values ARE findable by
# this grep in the file where they really live. Two negatives above are free
# on a dead instrument; this is the liveness control.
if $GREP -q '98765' "$H1/gateway-status.json" && $GREP -q '"turns_in_flight":7' "$H1/gateway-status.json"; then
  ok "D1 LIVENESS: the same grep DOES find 98765 and turns_in_flight in the planted file"
else bad "D1 LIVENESS: grep cannot find the planted values even where they are -- dead instrument"; fi

echo
echo "################ D2: COMPLETELY EMPTY HOME, NO CONFIG, NO LOG ################"
H2="$W/h2"; mkdir -p "$H2"
env WAYLAND_HOME="$H2" "$BIN" gateway support-bundle --out "$W/b2" > "$W/b2.out" 2>&1
RC2=$?
echo "exit=$RC2"; sed 's/^/    /' "$W/b2.out"
if [ "$RC2" -eq 0 ] && [ -s "$W/b2/manifest.json" ]; then
  ok "D2 a bundle is still produced with nothing on disk"
else bad "D2 no bundle produced (rc=$RC2)"; fi
echo "--- manifest absent_sources ---"
$GREP -A6 'absent_sources' "$W/b2/manifest.json"
NABS=$($GREP -c 'config:\|log:\|credentials:' "$W/b2/manifest.json" 2>/dev/null)
echo "named absent sources = ${NABS:-0}"
if [ "${NABS:-0}" -ge 2 ]; then ok "D2 missing sources are NAMED, not silently skipped"
else bad "D2 absences were skipped rather than named"; fi
if [ -f "$W/b2/recent-log.txt" ]; then bad "D2 invented an empty log member"
else ok "D2 no log member invented for a log that does not exist"; fi

echo
echo "################ D3: UNWRITABLE OUTPUT PATH ################"
H3="$W/h3"; mkdir -p "$H3"
RO="$W/readonly"; mkdir -p "$RO"; chmod 500 "$RO"
env WAYLAND_HOME="$H3" "$BIN" gateway support-bundle --out "$RO/nested/bundle" > "$W/b3.out" 2>&1
RC3=$?
echo "exit=$RC3"; sed 's/^/    /' "$W/b3.out"
if [ "$RC3" -ne 0 ]; then ok "D3 an unwritable out path FAILS (rc=$RC3) rather than claiming success"
else bad "D3 reported success against an unwritable path"; fi
if $GREP -qi 'permission denied\|cannot write' "$W/b3.out"; then
  ok "D3 the error names the cause"
else bad "D3 the error does not name the cause"; fi
if [ -d "$RO/nested" ]; then bad "D3 left a partial bundle behind"
else ok "D3 no partial bundle left behind"; fi
chmod 700 "$RO"

echo
echo "################ D4: NON-EMPTY OUTPUT DIRECTORY ################"
echo "(otherwise whatever was already there ships inside the ticket attachment)"
H4="$W/h4"; mkdir -p "$H4"
OUT4="$W/b4"; mkdir -p "$OUT4"
echo "somebody-elses-private-data" > "$OUT4/private.txt"
env WAYLAND_HOME="$H4" "$BIN" gateway support-bundle --out "$OUT4" > "$W/b4.out" 2>&1
RC4=$?
echo "exit=$RC4"; sed 's/^/    /' "$W/b4.out"
if [ "$RC4" -ne 0 ]; then ok "D4 a populated out dir is REFUSED (rc=$RC4)"
else bad "D4 merged a bundle into a populated directory"; fi
if [ "$(cat "$OUT4/private.txt")" = "somebody-elses-private-data" ]; then
  ok "D4 the pre-existing file is untouched"
else bad "D4 clobbered pre-existing content"; fi
if [ -f "$OUT4/manifest.json" ]; then bad "D4 wrote bundle members anyway"
else ok "D4 no bundle members written"; fi

echo
printf 'degraded_cases all_pass=%s failures=%s\n' "$([ $fails -eq 0 ] && echo true || echo false)" "$fails"
[ $fails -eq 0 ] || exit 1
exit 0
