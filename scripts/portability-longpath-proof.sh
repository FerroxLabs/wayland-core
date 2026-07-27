#!/usr/bin/env bash
# F26-03-D, the Linux half.
#
# Two things are proved here, and the second is the reason this script exists on
# Linux at all:
#
#   1. A deep tree — reconstructed well past Windows' MAX_PATH — round-trips
#      byte-identically. On Linux this is not a hard case (PATH_MAX is 4096), so
#      it is a CONTROL: it establishes that the fixture is well formed and that
#      the archive is good, so a Windows failure against the same archive is a
#      Windows fact rather than a broken fixture.
#
#   2. An archive is BUILT here carrying filenames that are ordinary on Linux
#      and impossible on Windows: a reserved DOS device name (`aux.txt`) and a
#      forbidden character (`report:final.md`). It restores cleanly HERE. That
#      is the positive control for the Windows leg, where the same archive must
#      be refused by name. Without it, a Windows refusal would be consistent
#      with an archive that was simply broken.
#
# Usage: portability-longpath-proof.sh <wayland-core-binary> <workdir>
set -euo pipefail

BIN=${1:?usage: portability-longpath-proof.sh <binary> <workdir>}
WORK=${2:?usage: portability-longpath-proof.sh <binary> <workdir>}

# A gate whose binary is missing must go RED, not silently pass. This is the
# same shape as the missing-PowerShell-script trap recorded in F26-03-C.
if [ ! -x "$BIN" ]; then
  echo "PROOF-FAIL: binary not executable: $BIN" >&2
  exit 1
fi

rm -rf "$WORK"
mkdir -p "$WORK"
SRC="$WORK/src"
mkdir -p "$SRC"

echo "LONGPATH-PLATFORM: $(uname -s)"
echo "BINARY: $BIN"

# --- fixture 1: the deep tree ------------------------------------------------
# 8 segments of 40 chars = 328 characters of relative path, so the reconstructed
# destination is past 260 under any plausible target root.
DEEP_REL="skills"
for i in $(seq 1 8); do
  DEEP_REL="$DEEP_REL/deeply-nested-directory-segment-$i-padding"
done
mkdir -p "$SRC/$DEEP_REL"
printf 'CANARY-DEEP-PAYLOAD' > "$SRC/$DEEP_REL/deep-canary.md"
printf '[storage.credentials]\nbackend = "plaintext"\n' > "$SRC/config.toml"
mkdir -p "$SRC/memory"
printf 'CANARY-MEMORY' > "$SRC/memory/notes.md"
echo "DEEP-REL-LEN: ${#DEEP_REL}"

DEEP_ARCHIVE="$WORK/deep.tar.gz"
# `set -e` would abort before any `echo $?`, so an exit status printed that way
# is always 0 and measures nothing. Every status below is captured explicitly.
set +e; "$BIN" backup create --home "$SRC" --out "$DEEP_ARCHIVE"; DEEP_CREATE_EXIT=$?; set -e
echo "DEEP-CREATE-EXIT: $DEEP_CREATE_EXIT"
[ "$DEEP_CREATE_EXIT" -eq 0 ] || { echo "PROOF-FAIL: deep create failed" >&2; exit 2; }

SRC_DIGEST=$("$BIN" backup digest --home "$SRC" | sed -n 's/^DIGEST: //p')
TARGET="$WORK/target"
set +e; "$BIN" backup restore "$DEEP_ARCHIVE" --home "$TARGET"; DEEP_RESTORE_EXIT=$?; set -e
echo "DEEP-RESTORE-EXIT: $DEEP_RESTORE_EXIT"
[ "$DEEP_RESTORE_EXIT" -eq 0 ] || { echo "PROOF-FAIL: deep restore failed" >&2; exit 2; }
TARGET_DIGEST=$("$BIN" backup digest --home "$TARGET" | sed -n 's/^DIGEST: //p')
echo "DEEP-SRC-DIGEST:    $SRC_DIGEST"
echo "DEEP-TARGET-DIGEST: $TARGET_DIGEST"
if [ "$SRC_DIGEST" != "$TARGET_DIGEST" ]; then
  echo "PROOF-FAIL: deep tree did not round-trip byte-identically" >&2
  exit 2
fi
DEEP_ABS="$TARGET/$DEEP_REL/deep-canary.md"
echo "DEEP-RESTORED-ABS-LEN: ${#DEEP_ABS}"
[ -f "$DEEP_ABS" ] || { echo "PROOF-FAIL: deep canary absent after restore" >&2; exit 2; }

# --- fixture 2: names Windows cannot represent -------------------------------
# Ordinary Linux filenames. `aux` is a reserved DOS device name; `:` is
# forbidden in a Windows filename. Both are created here without complaint,
# which is exactly the portability hazard.
HOSTILE="$WORK/hostile-src"
mkdir -p "$HOSTILE/reports"
printf '[storage.credentials]\nbackend = "plaintext"\n' > "$HOSTILE/config.toml"
printf 'CANARY-AUX' > "$HOSTILE/aux.txt"
printf 'CANARY-COLON' > "$HOSTILE/reports/report:final.md"
CREATED=$(find "$HOSTILE" -type f | wc -l | tr -d ' ')
echo "HOSTILE-NAMES-CREATED: $CREATED"
if [ "$CREATED" -ne 3 ]; then
  echo "PROOF-FAIL: hostile fixture not built ($CREATED files) -- the Windows leg would be vacuous" >&2
  exit 3
fi

HOSTILE_ARCHIVE="$WORK/hostile.tar.gz"
set +e; CREATE_OUT=$("$BIN" backup create --home "$HOSTILE" --out "$HOSTILE_ARCHIVE" 2>&1); HOSTILE_CREATE_EXIT=$?; set -e
echo "HOSTILE-CREATE-EXIT: $HOSTILE_CREATE_EXIT"
[ "$HOSTILE_CREATE_EXIT" -eq 0 ] || { echo "PROOF-FAIL: create must WARN about Windows-impossible names, not refuse" >&2; exit 3; }
echo "$CREATE_OUT" | sed -n 's/^/HOSTILE-CREATE| /p'
# The create-time WARNING must name both, and must not refuse.
WARNED=$(echo "$CREATE_OUT" | grep -c 'will not restore on Windows' || true)
echo "HOSTILE-CREATE-WARNINGS: $WARNED"
if [ "$WARNED" -lt 2 ]; then
  echo "PROOF-FAIL: create did not warn about both unrestorable names" >&2
  exit 3
fi

# ...and it restores cleanly HERE. This is the positive control for Windows.
HOSTILE_TARGET="$WORK/hostile-target"
set +e; "$BIN" backup restore "$HOSTILE_ARCHIVE" --home "$HOSTILE_TARGET"; HOSTILE_RESTORE_EXIT=$?; set -e
echo "HOSTILE-RESTORE-EXIT-LINUX: $HOSTILE_RESTORE_EXIT"
[ "$HOSTILE_RESTORE_EXIT" -eq 0 ] || { echo "PROOF-FAIL: names legal on this platform must restore here" >&2; exit 3; }
[ -f "$HOSTILE_TARGET/aux.txt" ] || { echo "PROOF-FAIL: aux.txt not restored on Linux" >&2; exit 3; }
[ -f "$HOSTILE_TARGET/reports/report:final.md" ] || { echo "PROOF-FAIL: colon file not restored on Linux" >&2; exit 3; }
echo "HOSTILE-RESTORED-ON-LINUX: yes"

echo "PROOF-OK: deep tree round-trips exactly; an archive carrying Windows-impossible names is built, warned about at create, and restores on the platform where those names are legal"
