#!/bin/sh
# D3 REPAIRED. The first version used `chmod 500` and ran as root, and root
# bypasses directory permissions via CAP_DAC_OVERRIDE -- so the write SUCCEEDED
# and the test reported a product defect that did not exist. The instrument was
# measuring the wrong thing, not the product failing.
#
# Repaired in two independent ways, because "unwritable" has two causes and
# root can bypass only one of them:
#   D3a  the out path's parent is a regular FILE -> ENOTDIR, which nothing bypasses
#   D3b  a genuinely unprivileged user against a root-owned 700 dir -> EACCES
#
# Plus a CONTROL proving the original D3 was bypassed rather than ignored.
set -u
BIN=/root/wayland-support-bundle/target/debug/wayland-core
GREP=/usr/bin/grep
W=/root/sb-d3
rm -rf "$W"; mkdir -p "$W"
fails=0
ok()  { printf 'PASS  %s\n' "$1"; }
bad() { printf 'FAIL  %s\n' "$1"; fails=$((fails+1)); }

echo "######## CONTROL: was the original D3 a root bypass, or a real defect? ########"
CTL="$W/ctl"; mkdir -p "$CTL"; chmod 500 "$CTL"
echo "id = $(id -u) ($(id -un))"
if touch "$CTL/probe" 2>/dev/null; then
  ok "CONTROL: a plain 'touch' ALSO succeeds in the chmod-500 dir as root --"
  echo "      so the original D3 measured root's CAP_DAC_OVERRIDE, not the product."
else
  bad "CONTROL: touch was refused, so chmod 500 DOES bind here and D3 was a real defect"
fi
chmod 700 "$CTL"

echo
echo "######## D3a: out path's PARENT IS A REGULAR FILE (ENOTDIR) ########"
H="$W/h"; mkdir -p "$H"
printf 'i am a file, not a directory\n' > "$W/afile"
env WAYLAND_HOME="$H" "$BIN" gateway support-bundle --out "$W/afile/bundle" > "$W/d3a.out" 2>&1
RC=$?
echo "exit=$RC"; sed 's/^/    /' "$W/d3a.out"
if [ "$RC" -ne 0 ]; then ok "D3a FAILS (rc=$RC) rather than claiming success"
else bad "D3a claimed success against an impossible path"; fi
if $GREP -qi 'not a directory\|cannot write a support bundle' "$W/d3a.out"; then
  ok "D3a the error names the cause"
else bad "D3a the error does not name the cause"; fi
if $GREP -qi 'support bundle written' "$W/d3a.out"; then
  bad "D3a printed a success banner anyway"
else ok "D3a printed NO success banner"; fi
# The file must be untouched.
if [ "$(cat "$W/afile")" = "i am a file, not a directory" ]; then
  ok "D3a the blocking file is untouched"
else bad "D3a clobbered the blocking file"; fi

echo
echo "######## D3b: UNPRIVILEGED USER vs a root-owned 700 dir (EACCES) ########"
if ! command -v setpriv >/dev/null 2>&1; then
  echo "SKIP: setpriv unavailable"
else
  RO="$W/rootonly"; mkdir -p "$RO"; chown root:root "$RO"; chmod 700 "$RO"
  UH="$W/uh"; mkdir -p "$UH"; chmod 777 "$UH"
  # Prove the unprivileged user really is unprivileged, in the same run.
  if setpriv --reuid=65534 --regid=65534 --clear-groups touch "$RO/probe" 2>/dev/null; then
    bad "D3b CONTROL: nobody CAN write the 700 dir -- the fixture is not restrictive"
  else
    ok "D3b CONTROL: nobody genuinely cannot write $RO (so this fixture binds)"
  fi
  setpriv --reuid=65534 --regid=65534 --clear-groups \
    env WAYLAND_HOME="$UH" HOME="$UH" "$BIN" gateway support-bundle \
    --out "$RO/bundle" > "$W/d3b.out" 2>&1
  RC=$?
  echo "exit=$RC"; sed 's/^/    /' "$W/d3b.out"
  if [ "$RC" -ne 0 ]; then ok "D3b FAILS (rc=$RC) for a genuinely unprivileged writer"
  else bad "D3b claimed success"; fi
  if $GREP -qi 'permission denied' "$W/d3b.out"; then ok "D3b names permission denied"
  else bad "D3b does not name the cause"; fi
  if [ -d "$RO/bundle" ]; then bad "D3b left a partial bundle"
  else ok "D3b left no partial bundle"; fi
fi

echo
printf 'd3_repaired all_pass=%s failures=%s\n' "$([ $fails -eq 0 ] && echo true || echo false)" "$fails"
[ $fails -eq 0 ] || exit 1
exit 0
