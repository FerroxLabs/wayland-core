#!/bin/sh
# v3 — v2's exec fixture used ```!shell, which the detector's block regex
# (r"(?s)```!\s*\n") does NOT match. That was MY error, not the product's.
# v3 uses the committed fixture's real syntax (```! alone) so the containment
# path gets a fair positive control, and asks the two questions separately:
#   Q1 does an EXECUTABLE skill in a scanned root reach quarantine?   (containment)
#   Q2 does a DATA skill in a scanned root reach the Wayland home?    (import)
set -u
BIN="$1"
FIXTURE="$2"
W="$(mktemp -d)"
PEER="$W/peer"
HOME_DIR="$W/wayland-home"
mkdir -p "$HOME_DIR"
cp -a "$FIXTURE" "$PEER"
mkdir -p "$PEER/skills/probe-data" "$PEER/skills/probe-exec"

cat >"$PEER/skills/probe-data/SKILL.md" <<'MD'
---
name: probe-data
description: prose only, no directive
---
DATASKILL-SENTINEL-1a2b there is no executable content here.
MD

# REAL directive syntax, copied from tests/fixtures/portability-exec/skills/repo-status
printf '%s\n' '---' 'name: probe-exec' 'description: carries a real directive' '---' \
    'EXECSKILL-SENTINEL-3c4d' '' '```!' 'touch __SENTINEL__' '```' \
    >"$PEER/skills/probe-exec/SKILL.md"

echo "BINARY: $("$BIN" --version)"
echo "=== APPLY ==="
WAYLAND_HOME="$HOME_DIR" "$BIN" migrate hermes --home "$PEER" --yes 2>&1 | tail -12
echo "=== FILES IN WAYLAND HOME ==="
/usr/bin/find "$HOME_DIR" -type f | /usr/bin/sed "s|$HOME_DIR|HOME|" | /usr/bin/sort
echo "TOTAL: $(/usr/bin/find "$HOME_DIR" -type f | /usr/bin/wc -l | tr -d ' ')"

echo "=== Q1 CONTAINMENT (positive control for the whole probe) ==="
NE=$(/usr/bin/grep -rlF "EXECSKILL-SENTINEL-3c4d" "$HOME_DIR" 2>/dev/null | /usr/bin/wc -l | tr -d ' ')
if [ "$NE" -ge 1 ]; then
    echo "Q1: PASS — the executable skill IS in the home (quarantined):"
    /usr/bin/grep -rlF "EXECSKILL-SENTINEL-3c4d" "$HOME_DIR" 2>/dev/null | /usr/bin/sed "s|$HOME_DIR|HOME|"
else
    echo "Q1: FAIL — executable skill nowhere in the home; the probe cannot see skill writes, so Q2 proves nothing"
fi

echo "=== Q2 IMPORT: does the DATA skill land anywhere in the home? ==="
ND=$(/usr/bin/grep -rlF "DATASKILL-SENTINEL-1a2b" "$HOME_DIR" 2>/dev/null | /usr/bin/wc -l | tr -d ' ')
echo "data-skill body files_containing=$ND   (product reported it as Outcome::Imported)"

echo "=== product's quarantine surface ==="
WAYLAND_HOME="$HOME_DIR" "$BIN" migrate quarantined 2>&1 | head -20
rm -rf "$W"
