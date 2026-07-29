#!/bin/sh
# F24-C4-H1 redaction proof driver.
#
# A hit count of 0 is the SUCCESS value of this check, and every failure mode
# -- bundle not generated, unwritable out path, errored grep, shell-mangled
# variable, nothing collected -- also produces 0. So the sweep alone proves
# nothing. Four arms guard it, each with its own exit code:
#
#   71  the planted secret was NOT in the input      (redaction of something
#                                                      never present is free)
#   72  the bundle is missing or empty               (absent-because-nothing)
#   73  the sweep cannot find the needle it hunts    (dead instrument)
#   74  the bundle collected no log                  (absent-because-nothing)
#    1  LEAK -- the secret survived into the bundle
#    0  redacted, with all four arms green
#
# The planted values are synthetic canaries, not real credentials. They are
# meant to be legible.

set -u
BIN=/root/wayland-support-bundle/target/debug/wayland-core
SWEEP=/root/wayland-support-bundle/scripts/f24-secret-sweep.sh
GREP=/usr/bin/grep
WORK=/root/sb-proof
rm -rf "$WORK"; mkdir -p "$WORK"
HOME_DIR="$WORK/wlhome"; mkdir -p "$HOME_DIR"
BUNDLE="$WORK/bundle"

CFG_SECRET='sk-cfgplant-A1b2C3d4E5f6G7h8'
ENV_SECRET='tok-envplant-Z9y8X7w6V5u4T3s2'
MARKER='COLLECTION-MARKER-8fa31c'

echo "############ SETUP ############"
cat > "$HOME_DIR/config.toml" <<EOF
[providers.anthropic]
api_key = "$CFG_SECRET"
base_url = "https://api.anthropic.com"
EOF

# Free text quoting BOTH secrets, plus a NON-SECRET marker that MUST survive.
# The marker is arm 4: it discriminates "absent because redacted" from "absent
# because the log was never collected".
cat > "$HOME_DIR/gateway.log" <<EOF
2026-07-29T00:00:01Z INFO  gateway starting $MARKER
2026-07-29T00:00:02Z ERROR auth rejected for key $CFG_SECRET (401)
2026-07-29T00:00:03Z ERROR upstream refused bearer $ENV_SECRET
2026-07-29T00:00:04Z WARN  delivery abandoned id=d-17
EOF
echo "input home: $HOME_DIR"
ls -la "$HOME_DIR"

echo
echo "############ ARM 3 -- the secret really IS in the input ############"
echo "(redaction of something never present is free; exit 71 if absent)"
for pair in "config:$CFG_SECRET" "env-value-in-log:$ENV_SECRET"; do
  label=${pair%%:*}; needle=${pair#*:}
  n=$(printf '%s\n' "$needle" | $GREP -rIl -F -f - "$HOME_DIR" 2>/dev/null | wc -l | tr -d ' ')
  echo "  input files containing the $label plant = $n"
  [ "$n" -ge 1 ] || { echo "ARM3 FAIL: $label plant is not in the input"; exit 71; }
done

echo
echo "############ DRIVE THE REAL BINARY ############"
env WAYLAND_HOME="$HOME_DIR" WL_SUPPORT_TOKEN="$ENV_SECRET" \
    "$BIN" gateway support-bundle --out "$BUNDLE"
RC=$?
echo "support-bundle exit = $RC"
[ "$RC" -eq 0 ] || { echo "the verb itself failed"; exit 72; }

echo
echo "############ ARM 1 -- the bundle exists and is non-empty ############"
echo "(two independent byte-count methods: wc -c returned 0 for a 72-byte file"
echo " on this programme, so one method is not enough)"
[ -d "$BUNDLE" ] || { echo "ARM1 FAIL: no bundle directory"; exit 72; }
FILES=$(find "$BUNDLE" -type f | wc -l | tr -d ' ')
BYTES_WC=$(cat "$BUNDLE"/* 2>/dev/null | wc -c | tr -d ' ')
BYTES_DU=$(du -sb "$BUNDLE" 2>/dev/null | cut -f1)
BYTES_STAT=$(find "$BUNDLE" -type f -exec stat -c%s {} + 2>/dev/null | awk '{s+=$1} END {print s+0}')
echo "  files in bundle          = $FILES"
echo "  bytes (cat|wc -c)        = $BYTES_WC"
echo "  bytes (du -sb)           = $BYTES_DU"
echo "  bytes (stat -c%s summed) = $BYTES_STAT"
[ "$FILES" -ge 5 ] || { echo "ARM1 FAIL: expected >=5 members, got $FILES"; exit 72; }
[ "$BYTES_WC" -gt 0 ] || { echo "ARM1 FAIL: wc says empty"; exit 72; }
[ "$BYTES_STAT" -gt 0 ] || { echo "ARM1 FAIL: stat says empty"; exit 72; }

echo
echo "############ ARM 4 -- the bundle actually COLLECTED things ############"
echo "(a bundle that silently collects nothing passes every redaction test)"
for m in manifest.json environment-keys.txt config-keys.txt recent-log.txt gateway-status.json; do
  if [ -s "$BUNDLE/$m" ]; then echo "  present and non-empty: $m"
  else echo "ARM4 FAIL: missing or empty member: $m"; exit 74; fi
done
MARKS=$($GREP -c -F "$MARKER" "$BUNDLE/recent-log.txt" 2>/dev/null)
echo "  non-secret marker occurrences in recent-log.txt = ${MARKS:-0}"
[ "${MARKS:-0}" -ge 1 ] || { echo "ARM4 FAIL: the log was not collected"; exit 74; }
CFGKEYS=$($GREP -c "api_key" "$BUNDLE/config-keys.txt" 2>/dev/null)
echo "  api_key NAME present in config-keys.txt = ${CFGKEYS:-0} (structural elision keeps names)"
[ "${CFGKEYS:-0}" -ge 1 ] || { echo "ARM4 FAIL: config was not collected"; exit 74; }

echo
echo "############ ARM 2 -- the sweep can FIND this needle ############"
echo "(known-positive over a control dir, in the same run as the real sweep;"
echo " a prior lane reported '0 hits, clean' from a grep that had errored)"
CTRL="$WORK/control"; mkdir -p "$CTRL"
printf 'this control file deliberately contains %s\n' "$CFG_SECRET" > "$CTRL/positive.txt"
printf 'this control file deliberately contains %s\n' "$ENV_SECRET" >> "$CTRL/positive.txt"

for pair in "config-plant:$CFG_SECRET" "env-plant:$ENV_SECRET"; do
  label=${pair%%:*}; needle=${pair#*:}
  echo "--- known-positive: sweeping the CONTROL dir for the $label ---"
  printf '%s\n' "$needle" | sh "$SWEEP" "$CTRL" > "$WORK/kp.out" 2>&1
  KPRC=$?
  sed 's/^/      /' "$WORK/kp.out"
  echo "      known-positive rc = $KPRC (1 = found, which is what proves the sweep works)"
  [ "$KPRC" -eq 1 ] || { echo "ARM2 FAIL: the sweep cannot find $label -- instrument dead"; exit 73; }

  echo "--- real sweep: the same needle over the BUNDLE ---"
  printf '%s\n' "$needle" | sh "$SWEEP" "$BUNDLE" > "$WORK/real.out" 2>&1
  REALRC=$?
  sed 's/^/      /' "$WORK/real.out"
  echo "      real-sweep rc = $REALRC (0 = clean)"
  [ "$REALRC" -eq 0 ] || { echo "LEAK: $label survived into the bundle"; exit 1; }
done

echo
echo "############ WHAT THE REDACTION ACTUALLY DID ############"
echo "--- manifest.json ---"
cat "$BUNDLE/manifest.json"
echo
echo "--- recent-log.txt (the scrubbed free text) ---"
cat "$BUNDLE/recent-log.txt"
echo
echo "--- config-keys.txt ---"
cat "$BUNDLE/config-keys.txt"
echo
echo "--- gateway-status.json ---"
cat "$BUNDLE/gateway-status.json"
echo
echo "--- the env plant's NAME in environment-keys.txt ---"
$GREP "WL_SUPPORT_TOKEN" "$BUNDLE/environment-keys.txt"
echo
echo "ALL FOUR ARMS GREEN -- redacted, and the check could have failed."
exit 0
