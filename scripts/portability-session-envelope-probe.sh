#!/bin/sh
# portability-session-envelope-probe.sh — F26-03's FIRST clause, measured.
#
# Usage: sh scripts/portability-session-envelope-probe.sh <path-to-wayland-core>
#
# THE QUESTION
#
# F26-03 reads: "Users can CONSUME THE F23 REDACTED SESSION/EVIDENCE ENVELOPE to
# export a portable profile/session corpus and perform authenticated backup,
# restore, and reciprocal migration without executing imported content."
# 26-04 measured zero footprint for the first clause in
# `crates/wcore-cli/src/backup/`, and graded F26-03 OPEN on exactly that.
#
# Deciding whether that clause is worth building or should be retired needs a
# FACT, not a reading of the requirement: does the portable artefact the product
# actually produces today carry raw session transcript bytes, at a moment when a
# purpose-built redacted envelope for those same sessions already exists and is
# not used? This probe answers that by planting a canary in a session and asking
# where the canary ends up.
#
# WHAT MAKES IT NON-VACUOUS
#
# An absence claim is worthless without the paired presence. So the canary is
# first proven PRESENT in the session on disk, and the envelope path is proven
# to actually RUN and produce a document, before either absence is asserted.
# A probe that reported "no canary in the envelope" because the envelope was
# never written would otherwise pass.
#
# Prints a fixed-grammar block. Exits non-zero only if the probe could not be
# CONDUCTED; the findings themselves are reported as fields, because the point
# is to measure the disposition, not to gate on a predetermined answer.

set -u
FAIL() { echo "PROBE-FAIL: $*"; exit 1; }

BIN="${1:-}"
[ -n "$BIN" ] || FAIL "usage: $0 <path-to-wayland-core>"
[ -x "$BIN" ] || FAIL "not executable: $BIN"
"$BIN" session --help >/dev/null 2>&1 || FAIL "binary has no 'session' surface"
"$BIN" backup --help >/dev/null 2>&1 || FAIL "binary has no 'backup' surface"
"$BIN" --build-info 2>/dev/null | sed -n 's/^/BUILD-INFO: /p' | head -2

WORK=$(mktemp -d) || FAIL "no work dir"
trap 'rm -rf "$WORK"' EXIT
HOME_DIR="$WORK/home"
SESSIONS="$HOME_DIR/sessions"
mkdir -p "$SESSIONS" || FAIL "could not build the home"

# A canary shaped like the thing a transcript actually carries: prose a user
# typed. Unique enough that a hit cannot be coincidence.
CANARY="WLC-F26GAPS-TRANSCRIPT-CANARY-8f2b41d9-DO-NOT-USE"
SID="probe-session-0001"

cat > "$SESSIONS/$SID.json" <<JSON
{
  "schema_version": 1,
  "id": "$SID",
  "created_at": "2026-07-28T00:00:00Z",
  "updated_at": "2026-07-28T00:05:00Z",
  "provider": "anthropic",
  "model": "claude-opus-4",
  "cwd": "/tmp/probe",
  "messages": [
    { "role": "user", "content": "my secret plan is $CANARY" },
    { "role": "assistant", "content": "acknowledged" }
  ]
}
JSON
# `summary` is the truncated first user message, so the real index ALREADY
# carries transcript prose. The canary is put there too, deliberately: if the
# archive carries the index it carries user text regardless of the session file.
cat > "$SESSIONS/index.json" <<JSON
{ "sessions": [ { "id": "$SID", "created_at": "2026-07-28T00:00:00Z",
  "updated_at": "2026-07-28T00:05:00Z",
  "model": "claude-opus-4", "summary": "my secret plan is $CANARY",
  "message_count": 2 } ] }
JSON

# --- the fixture must be one the PRODUCT accepts, not merely one we wrote -----
LIST=$(WAYLAND_HOME="$HOME_DIR" "$BIN" session list 2>&1)
LIST_RC=$?
echo "SESSION-LIST-RC: $LIST_RC"
case "$LIST" in
    *"$SID"*) echo "SESSION-FIXTURE-ACCEPTED: yes" ;;
    *) echo "SESSION-FIXTURE-ACCEPTED: no"
       echo "$LIST" | sed -n 's/^/SESSION-LIST: /p' | head -6
       FAIL "the product does not list the fixture session; the probe would measure nothing" ;;
esac

# Presence half: the canary really is in the session on disk.
if grep -qF "$CANARY" "$SESSIONS/$SID.json"; then
    echo "CANARY-IN-SESSION-FILE: yes"
else
    FAIL "the canary is not in the session file; the fixture is wrong"
fi

# --- the F23 envelope ---------------------------------------------------------
ENV_OUT="$WORK/envelope.json"
WAYLAND_HOME="$HOME_DIR" "$BIN" session export "$SID" --out "$ENV_OUT" \
    > "$WORK/export.log" 2>&1
EXPORT_RC=$?
echo "SESSION-EXPORT-RC: $EXPORT_RC"
if [ "$EXPORT_RC" -ne 0 ]; then
    sed -n 's/^/EXPORT-LOG: /p' "$WORK/export.log" | head -6
    FAIL "session export failed; the envelope half cannot be measured"
fi
[ -s "$ENV_OUT" ] || FAIL "session export wrote an empty file"
echo "ENVELOPE-BYTES: $(wc -c < "$ENV_OUT" | tr -d ' ')"
if grep -qF "$CANARY" "$ENV_OUT"; then
    echo "CANARY-IN-ENVELOPE: yes"
else
    echo "CANARY-IN-ENVELOPE: no"
fi
# The envelope must be a REAL document about this session, or "no canary" is
# just "no content".
if grep -qF "$SID" "$ENV_OUT"; then
    echo "ENVELOPE-NAMES-SESSION: yes"
else
    echo "ENVELOPE-NAMES-SESSION: no"
fi

# --- the portable artefact the product actually produces ----------------------
ARCHIVE="$WORK/backup.tar.gz"
WAYLAND_HOME="$HOME_DIR" "$BIN" backup create --home "$HOME_DIR" --out "$ARCHIVE" \
    > "$WORK/create.log" 2>&1
CREATE_RC=$?
echo "BACKUP-CREATE-RC: $CREATE_RC"
[ "$CREATE_RC" -eq 0 ] || { sed -n 's/^/CREATE-LOG: /p' "$WORK/create.log" | head -8; FAIL "backup create failed"; }
[ -s "$ARCHIVE" ] || FAIL "backup create wrote an empty archive"
echo "ARCHIVE-BYTES: $(wc -c < "$ARCHIVE" | tr -d ' ')"

# Search the DECOMPRESSED bytes: a gzip stream will not match a plaintext grep,
# so grepping the archive file directly would report a comforting absence.
EXTRACT="$WORK/extract"
mkdir -p "$EXTRACT"
tar xzf "$ARCHIVE" -C "$EXTRACT" 2>/dev/null || FAIL "could not extract the archive"
echo "ARCHIVE-ENTRIES: $(find "$EXTRACT" -type f | wc -l | tr -d ' ')"
HITS=$(grep -rlF "$CANARY" "$EXTRACT" 2>/dev/null | wc -l | tr -d ' ')
echo "CANARY-IN-ARCHIVE-FILES: $HITS"
if [ "$HITS" -gt 0 ]; then
    echo "CANARY-IN-ARCHIVE: yes"
    grep -rlF "$CANARY" "$EXTRACT" 2>/dev/null | sed "s|$EXTRACT/||" | sed -n 's/^/CANARY-AT: /p' | head -5
else
    echo "CANARY-IN-ARCHIVE: no"
fi

# Does anything in the archive resemble the envelope shape? If the backup path
# consumed the envelope, a consumer would expect to find it named.
if grep -rlF "envelope_version" "$EXTRACT" >/dev/null 2>&1; then
    echo "ENVELOPE-IN-ARCHIVE: yes"
else
    echo "ENVELOPE-IN-ARCHIVE: no"
fi

echo "PROBE: COMPLETE"
