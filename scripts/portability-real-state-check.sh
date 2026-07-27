#!/bin/sh
# Real-state, real-secret check for migrate discovery (F26-01, Task 4 leg).
#
# Runs BOTH discoveries against the REAL peer homes on this machine with the
# machine-readable + dry-run flags, extracts the REAL secret values from the
# real sources, and requires zero of them to appear in either emitted document.
#
# Why this exists: the Linux gate proves redaction against CANARY values in a
# committed corpus. That is necessary and not sufficient — it cannot prove the
# behaviour holds against live credentials at real scale (12 profiles, 540 skill
# directories, real provider keys). Only this leg can, and it can only run where
# the real homes live.
#
# Its own failure modes are loud, because a check that cannot go red would make
# the whole leg decorative:
#   - an EMPTY secret extraction fails (nothing to search for ⇒ vacuous pass)
#   - an unparseable emitted document fails
#   - a zero item count fails
#   - a non-zero exit from either discovery fails
#   - a changed tree digest fails (discovery mutated what it previewed)
#
# The scratch extraction never leaves this machine and is deleted before exit.
#
# Usage: portability-real-state-check.sh <path-to-wayland-core-binary>

set -u

BIN="${1:-}"
if [ -z "$BIN" ]; then
    echo "usage: $0 <path-to-wayland-core-binary>" >&2
    exit 2
fi
if [ ! -x "$BIN" ]; then
    echo "FAIL: '$BIN' is not an executable file" >&2
    exit 1
fi

HERMES_HOME="${HERMES_HOME:-$HOME/.hermes}"
OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
WORK="$(mktemp -d)"
SECRETS="$WORK/secrets.txt"
RC=0

cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT INT TERM

fail() { echo "FAIL: $*" >&2; RC=1; }

digest_tree() {
    # Deterministic content digest over a tree; used before and after.
    /usr/bin/find "$1" -type f -print0 2>/dev/null | /usr/bin/sort -z |
        xargs -0 shasum -a 256 2>/dev/null | shasum -a 256 | /usr/bin/awk '{print $1}'
}

# --- extract the REAL secret values -------------------------------------
# R1: provider keys from every Hermes dotenv, root and per-profile.
/usr/bin/grep -hoE '[A-Za-z0-9_]*_(API_KEY|TOKEN|SECRET|KEY)=.*' \
    "$HERMES_HOME"/profiles/*/.env "$HERMES_HOME"/.env 2>/dev/null |
    /usr/bin/sed 's/^[^=]*=//; s/^["'"'"']//; s/["'"'"']$//' |
    /usr/bin/grep -vE '^[[:space:]]*$' >>"$SECRETS" 2>/dev/null
# R2/R3: secret-shaped JSON values from the OpenClaw config and credentials.
/usr/bin/find "$OPENCLAW_HOME" -maxdepth 2 -type f \
    \( -name '*.json' -o -path '*/credentials/*' \) -print0 2>/dev/null |
    xargs -0 /usr/bin/grep -hoiE '"[^"]*(key|token|secret|auth|pass|cred)[^"]*"[[:space:]]*:[[:space:]]*"[^"]{16,}"' 2>/dev/null |
    /usr/bin/sed 's/.*:[[:space:]]*"//; s/"$//' >>"$SECRETS" 2>/dev/null
/usr/bin/sort -u "$SECRETS" -o "$SECRETS" 2>/dev/null

TOTAL=$(/usr/bin/grep -c . "$SECRETS" 2>/dev/null || echo 0)
if [ "$TOTAL" -lt 1 ]; then
    echo "FAIL: extracted ZERO real secret values from $HERMES_HOME and $OPENCLAW_HOME." >&2
    echo "      A search with nothing to search for would pass vacuously, so this is a" >&2
    echo "      hard failure rather than a clean result." >&2
    exit 1
fi
echo "extracted $TOTAL real secret values to search for (values never printed)"

# --- run both discoveries ------------------------------------------------
for PAIR in "hermes:$HERMES_HOME" "openclaw:$OPENCLAW_HOME"; do
    KIND=${PAIR%%:*}
    HOME_DIR=${PAIR#*:}
    OUT="$WORK/$KIND.json"

    if [ ! -d "$HOME_DIR" ]; then
        fail "$KIND: real home $HOME_DIR does not exist"
        continue
    fi

    D_BEFORE=$(digest_tree "$HOME_DIR")
    "$BIN" migrate "$KIND" --home "$HOME_DIR" --dry-run --json >"$OUT" 2>"$WORK/$KIND.err"
    EXIT=$?
    D_AFTER=$(digest_tree "$HOME_DIR")

    if [ "$EXIT" -ne 0 ]; then
        fail "$KIND: discovery exited $EXIT"
        /usr/bin/sed 's/^/    /' "$WORK/$KIND.err" >&2
        continue
    fi

    # Positive assertions FIRST — an empty or malformed document must go red
    # here rather than sail through the secret search below.
    COUNT=$(python3 -c "
import json,sys
try:
    d=json.load(open('$OUT'))
except Exception as e:
    print('PARSE_ERROR', e); sys.exit(0)
print(len(d.get('items', [])))
" 2>/dev/null)
    case "$COUNT" in
    PARSE_ERROR*)
        fail "$KIND: emitted document is not parseable JSON ($COUNT)"
        continue
        ;;
    '' | 0)
        fail "$KIND: emitted document declares ZERO items — that reads as success while proving nothing"
        continue
        ;;
    esac
    echo "$KIND: exit 0, well-formed JSON, items=$COUNT"

    if [ "$D_BEFORE" != "$D_AFTER" ]; then
        fail "$KIND: the real home CHANGED across a dry-run (digest $D_BEFORE -> $D_AFTER)"
    else
        echo "$KIND: non-mutation confirmed (tree digest unchanged)"
    fi

    # --- the secret search ------------------------------------------------
    HITS=0
    while IFS= read -r V; do
        [ ${#V} -lt 8 ] && continue
        if /usr/bin/grep -qF -- "$V" "$OUT" 2>/dev/null; then
            HITS=$((HITS + 1))
            echo "    LEAK: a real secret of length ${#V} appears in the $KIND document" >&2
        fi
    done <"$SECRETS"
    if [ "$HITS" -ne 0 ]; then
        fail "$KIND: $HITS real secret value(s) present in the emitted document — CRITICAL"
    else
        echo "$KIND: searched $TOTAL real values, 0 hits"
    fi
done

rm -f "$SECRETS"
if [ "$RC" -eq 0 ]; then
    echo "REAL-STATE CHECK PASSED"
else
    echo "REAL-STATE CHECK FAILED" >&2
fi
exit "$RC"
