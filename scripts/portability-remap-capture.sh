#!/bin/sh
# portability-remap-capture.sh — F26-04: what a cross-machine restore TELLS an
# operator, and what it actually DOES, captured per credential backend.
#
# Usage:  sh scripts/portability-remap-capture.sh <path-to-wayland-core> <outdir>
#
# WHY PER BACKEND
#
# Where a secret lives differs per backend, and that difference IS the remap:
#
#   plaintext       credentials* in the tree            portable if carried
#   auto            keyring first, plaintext fallback   partly
#   keyring         OS keychain, not in the tree        not portable
#   encrypted-file  files at ABSOLUTE machine paths     only if inside the home
#
# So one record per variant, and the record is written by this script from what
# the binary actually printed and did — never from what anyone believed.
#
# TWO THINGS ARE MEASURED, NOT INFERRED
#
#   REMAP-TARGET-WRITTEN is derived by digesting the target BEFORE and AFTER the
#   default restore attempt. That is how a "warn and continue" is caught: a
#   refusal that writes the target anyway is not a refusal, and its own message
#   would never say so.
#
#   REMAP-CARRIES-SOURCE-ABSOLUTE-PATH is checked against the tree produced by
#   the ACKNOWLEDGED restore (--accept-missing-secrets), because a refusal writes
#   nothing and searching an unwritten tree for a leaked path would pass
#   vacuously.
#
# Every home built here is synthetic and canary-seeded. No real home is read.

set -u

FAIL() { echo "REMAP-CAPTURE-FAIL: $*" >&2; exit 1; }

BIN="${1:-}"
OUT="${2:-}"
[ -n "$BIN" ] || FAIL "usage: $0 <path-to-wayland-core> <outdir>"
[ -n "$OUT" ] || FAIL "usage: $0 <path-to-wayland-core> <outdir>"
[ -x "$BIN" ] || FAIL "binary does not exist or is not executable: $BIN"
"$BIN" backup --help >/dev/null 2>&1 || FAIL "binary does not support 'backup': $BIN"

mkdir -p "$OUT" || FAIL "could not create $OUT"
REC="$OUT/remap-records.txt"
: > "$REC"

WORK=$(mktemp -d) || FAIL "could not create a work directory"
trap 'rm -rf "$WORK"' EXIT

digest_of() { "$BIN" backup digest --home "$1" 2>/dev/null | sed -n 's/^DIGEST: //p'; }

capture_one() {
    LABEL="$1"          # the record label
    CFG_BODY="$2"       # config.toml contents

    SRC="$WORK/$LABEL-src"
    TGT="$WORK/$LABEL-tgt"
    TGT2="$WORK/$LABEL-tgt-ack"
    ARC="$WORK/$LABEL.tar.gz"
    mkdir -p "$SRC/skills" "$TGT" "$TGT2"

    printf '%s' "$CFG_BODY" > "$SRC/config.toml"
    printf 'CANARY-SKILL\n' > "$SRC/skills/a.md"
    # A secret the DEFAULT (redacted) archive will omit, so every backend has a
    # credential source that can go absent.
    printf 'api_key = "CANARY-REMAP-SECRET"\n' > "$SRC/credentials.toml"

    "$BIN" backup create --home "$SRC" --out "$ARC" >"$WORK/$LABEL-create.log" 2>&1 \
        || FAIL "$LABEL: archive creation failed"

    # The cross-machine case: restore onto a DIFFERENT home, default policy.
    PRE=$(digest_of "$TGT")
    "$BIN" backup restore "$ARC" --home "$TGT" >"$WORK/$LABEL-restore.log" 2>&1
    RC=$?
    POST=$(digest_of "$TGT")

    WRITTEN=no
    [ "$PRE" != "$POST" ] && WRITTEN=yes

    DISPOSITION=remapped
    [ "$RC" -ne 0 ] && DISPOSITION=refused

    MSG=$(cat "$WORK/$LABEL-restore.log")

    # Derived from the captured text, by this script.
    NAMES_BACKEND=no
    NAMES_COUNT=no
    NAMES_ACTION=no
    echo "$MSG" | grep -q 'backend `' && NAMES_BACKEND=yes
    echo "$MSG" | grep -qE 'credential source\(s\)' && NAMES_COUNT=yes
    echo "$MSG" | grep -q 'action:' && NAMES_ACTION=yes

    # The acknowledged restore actually writes a tree, so the leaked-path search
    # has something to search.
    "$BIN" backup restore "$ARC" --home "$TGT2" --accept-missing-secrets \
        >"$WORK/$LABEL-ack.log" 2>&1
    # Take every absolute path the SOURCE config names and require none of them
    # to appear in the RESTORED config. Searching for a fixed string would only
    # have covered the paths this script happened to predict.
    ABS=no
    if [ -f "$TGT2/config.toml" ]; then
        for p in $(grep -oE '"/[^"]+"' "$SRC/config.toml" 2>/dev/null | tr -d '"'); do
            if grep -qF -- "$p" "$TGT2/config.toml" 2>/dev/null; then
                ABS=yes
            fi
        done
    fi

    {
        echo "REMAP-BACKEND: $LABEL"
        echo "REMAP-EXIT: $RC"
        echo "REMAP-DISPOSITION: $DISPOSITION"
        echo "REMAP-TARGET-WRITTEN: $WRITTEN"
        echo "REMAP-MESSAGE-BEGIN"
        echo "$MSG"
        echo "REMAP-MESSAGE-END"
        echo "REMAP-NAMES-BACKEND: $NAMES_BACKEND"
        echo "REMAP-NAMES-COUNT: $NAMES_COUNT"
        echo "REMAP-NAMES-ACTION: $NAMES_ACTION"
        echo "REMAP-CARRIES-SOURCE-ABSOLUTE-PATH: $ABS"
    } >> "$REC"
}

capture_one auto      '[storage.credentials]
backend = "auto"
'
capture_one plaintext '[storage.credentials]
backend = "plaintext"
'
capture_one keyring   '[storage.credentials]
backend = "keyring"
'

# The struct variant carries ABSOLUTE machine-specific paths, which is the case
# the rewrite exists for. Pointed OUTSIDE the home so it is genuinely
# unportable, exactly as a real second machine would find it.
capture_one encrypted-file "[storage.credentials.backend.encrypted_file]
cipher_path = \"$WORK/machine-only/credentials.enc\"
key_params_path = \"$WORK/machine-only/credentials.kdf.json\"
"

cat "$REC"
echo
echo "REMAP-CAPTURE-OK: 4 backend records written to $REC"
exit 0
