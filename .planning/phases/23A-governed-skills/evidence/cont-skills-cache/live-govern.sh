#!/usr/bin/env bash
# Live governed-skills journey against the REAL shipped binary.
# Lane cont-skills-cache. Lane-unique paths only (LANE-BRIEF 6a-ii).
set -u -o pipefail

BIN=/root/wayland-cont-skills-cache/target/debug/wayland-core
LIVE=/root/cont-skills-cache-live
rm -rf "$LIVE"; mkdir -p "$LIVE/home/skills" "$LIVE/proj"
export WAYLAND_HOME="$LIVE/home"
export HOME="$LIVE/fakehome"; mkdir -p "$HOME"

echo "== BUILD IDENTITY (asserted before any measurement) =="
"$BIN" --version
echo

# --- fixture: a generated draft, exactly as the auto-draft loop writes one ---
mk() {
  d="$WAYLAND_HOME/skills/$1"; mkdir -p "$d"
  printf -- '---\nname: %s\ndescription: live subject %s\n---\n\nbody of %s\n' "$1" "$1" "$1" > "$d/SKILL.md"
  printf '{"auto_drafted":true,"signature":"sig-%s"}\n' "$1" > "$d/manifest.json"
}
mk wl-subject
mk wl-control          # never touched: proves an absence below is the subject's, not a collapse

echo "== STEP 1: baseline --skills-govern (expect both status=installed) =="
"$BIN" --skills-govern; echo "RC=$?"
echo

echo "== STEP 2 (CAN-PASS direction): promote the subject =="
"$BIN" --skills-promote wl-subject; echo "RC=$?"
echo

echo "== STEP 3: --skills-govern (expect subject status=promoted, control still installed) =="
"$BIN" --skills-govern; echo "RC=$?"
echo

echo "== STEP 4 (CAN-FAIL direction): promote a name that does not exist =="
"$BIN" --skills-promote wl-no-such-skill; echo "RC=$?"
echo

echo "== STEP 5: revoke the subject =="
"$BIN" --skills-revoke wl-subject > "$LIVE/revoke.out" 2>&1; echo "RC=$?"
cat "$LIVE/revoke.out"
REVID=$(/usr/bin/grep -o 'revocation id: .*' "$LIVE/revoke.out" | /usr/bin/sed 's/revocation id: //' | tr -d ' \r')
echo "PARSED_REVID=$REVID"
echo

echo "== STEP 6: --skills-govern (expect subject GONE from INSTALLED, present under REVOKED, control intact) =="
"$BIN" --skills-govern; echo "RC=$?"
echo

echo "== STEP 7 (CAN-FAIL direction): rollback a bogus id =="
"$BIN" --skills-rollback not-a-real-revocation-id; echo "RC=$?"
echo

echo "== STEP 8: rollback the real id =="
"$BIN" --skills-rollback "$REVID"; echo "RC=$?"
echo

echo "== STEP 9: --skills-govern (expect subject back, status=installed i.e. re-quarantined NOT promoted) =="
"$BIN" --skills-govern; echo "RC=$?"
echo

echo "== STEP 10: promote appears in --help (advertised) =="
"$BIN" --help 2>&1 | /usr/bin/grep -E 'skills-(promote|revoke|rollback|govern)'
echo "GREP_RC=$?"
echo "== END =="
