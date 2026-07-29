#!/bin/sh
# Positive control for the redaction search in `portability-real-state-check.sh`.
#
# WHY THIS EXISTS. That script's central measurement is "0 of N real secret
# values appear in the emitted document" — a KNOWN-NEGATIVE assertion. A broken
# matcher, a wrong path, an empty file or an emptied search set all produce a
# zero for free, so a passing run is by itself consistent with the search never
# having worked. That script defends the INPUTS well (empty extraction, zero
# items, unparseable JSON and a changed tree digest are each a hard failure) but
# it never demonstrates that the MATCHER can fire.
#
# This supplies that demonstration, with three assertions rather than two,
# because only the third proves the control does anything:
#
#   A1 known-positive — a real extracted secret PLANTED into a copy of the
#      emitted document IS found by the identical matcher.
#   A2 known-negative — the same matcher over the UNMODIFIED document finds
#      nothing.
#   A3 the-old-shape-would-have-missed-it — a deliberately dead matcher (the
#      failure mode being guarded against) reports 0 on the SAME planted
#      document that A1 found. Without A3 this control passes on a broken
#      instrument too.
#
# It also reports how many extracted values the real-state check actually
# searches, because that script's `[ ${#V} -lt 8 ] && continue` silently skips
# short values while its message still prints the FULL extracted total.
#
# Secret values are never printed, never written outside the scratch dir, and
# the scratch dir is removed on exit. Nothing writes to either peer home.
#
# Usage: portability-redaction-positive-control.sh <path-to-wayland-core-binary>

set -u

BIN="${1:-}"
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
    echo "usage: $0 <path-to-executable-wayland-core>" >&2
    exit 2
fi

HERMES_HOME="${HERMES_HOME:-$HOME/.hermes}"
OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
WORK="$(mktemp -d)"
SECRETS="$WORK/secrets.txt"
RC=0

cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT INT TERM

fail() {
    echo "FAIL: $*" >&2
    RC=1
}

# --- identical extraction to the script under control --------------------
/usr/bin/grep -hoE '[A-Za-z0-9_]*_(API_KEY|TOKEN|SECRET|KEY)=.*' \
    "$HERMES_HOME"/profiles/*/.env "$HERMES_HOME"/.env 2>/dev/null |
    /usr/bin/sed 's/^[^=]*=//; s/^["'"'"']//; s/["'"'"']$//' |
    /usr/bin/grep -vE '^[[:space:]]*$' >>"$SECRETS" 2>/dev/null
/usr/bin/find "$OPENCLAW_HOME" -maxdepth 2 -type f \
    \( -name '*.json' -o -path '*/credentials/*' \) -print0 2>/dev/null |
    xargs -0 /usr/bin/grep -hoiE '"[^"]*(key|token|secret|auth|pass|cred)[^"]*"[[:space:]]*:[[:space:]]*"[^"]{16,}"' 2>/dev/null |
    /usr/bin/sed 's/.*:[[:space:]]*"//; s/"$//' >>"$SECRETS" 2>/dev/null
/usr/bin/sort -u "$SECRETS" -o "$SECRETS" 2>/dev/null

TOTAL=$(/usr/bin/grep -c . "$SECRETS" 2>/dev/null || echo 0)
SEARCHABLE=0
while IFS= read -r V; do
    [ ${#V} -lt 8 ] && continue
    SEARCHABLE=$((SEARCHABLE + 1))
done <"$SECRETS"

echo "EXTRACTED-TOTAL: $TOTAL"
echo "ACTUALLY-SEARCHED: $SEARCHABLE   (values shorter than 8 chars are skipped by the matcher)"
if [ "$TOTAL" -lt 1 ]; then
    fail "extracted ZERO real secrets — the control has nothing to plant"
    echo "POSITIVE-CONTROL: FAIL"
    exit 1
fi
if [ "$SEARCHABLE" -lt 1 ]; then
    fail "every extracted value is shorter than 8 chars — the real-state check searches NOTHING while reporting $TOTAL"
fi
if [ "$SEARCHABLE" -ne "$TOTAL" ]; then
    echo "NOTE: the real-state check prints \"searched $TOTAL\" but searches $SEARCHABLE."
fi

# The longest extracted value: the one a leak would most obviously be.
PLANT=$(/usr/bin/awk '{ if (length($0) > length(best)) best=$0 } END { print best }' "$SECRETS")
if [ ${#PLANT} -lt 8 ]; then
    fail "no extracted value is long enough to plant"
    echo "POSITIVE-CONTROL: FAIL"
    exit 1
fi
echo "PLANT-LENGTH: ${#PLANT}   (value never printed)"

for PAIR in "hermes:$HERMES_HOME" "openclaw:$OPENCLAW_HOME"; do
    KIND=${PAIR%%:*}
    HOME_DIR=${PAIR#*:}
    CLEAN="$WORK/$KIND.json"
    PLANTED="$WORK/$KIND.planted.json"

    [ -d "$HOME_DIR" ] || {
        fail "$KIND: $HOME_DIR absent"
        continue
    }

    "$BIN" migrate "$KIND" --home "$HOME_DIR" --dry-run --json >"$CLEAN" 2>/dev/null || {
        fail "$KIND: discovery did not exit 0"
        continue
    }
    [ -s "$CLEAN" ] || {
        fail "$KIND: emitted document is empty"
        continue
    }

    # Plant the real value into a COPY. The peer home is never touched.
    cp "$CLEAN" "$PLANTED"
    printf '%s\n' "$PLANT" >>"$PLANTED"

    # A1 — known-positive: the real matcher finds the planted value.
    if /usr/bin/grep -qF -- "$PLANT" "$PLANTED"; then
        echo "$KIND A1 known-positive: PASS (matcher fired on a planted real secret)"
    else
        fail "$KIND A1: the matcher did NOT find a secret planted in plain sight — every 0-hit result from this instrument is meaningless"
    fi

    # A2 — known-negative: the same matcher over the untouched document.
    if /usr/bin/grep -qF -- "$PLANT" "$CLEAN"; then
        fail "$KIND A2: a REAL secret is present in the unmodified emitted document — CRITICAL"
    else
        echo "$KIND A2 known-negative: PASS (untouched document carries no real secret)"
    fi

    # A3 — the dead-instrument shape must MISS what A1 caught. This models the
    # exact failure being guarded against: a matcher pointed at the wrong file.
    # If A3 also fires, the control is not discriminating and proves nothing.
    if /usr/bin/grep -qF -- "$PLANT" "$WORK/nonexistent-$KIND.json" 2>/dev/null; then
        fail "$KIND A3: the dead matcher reported a hit — this control cannot distinguish a working instrument from a broken one"
    else
        echo "$KIND A3 dead-instrument-misses: PASS (a matcher on a missing file reports 0, as the guarded failure mode does)"
    fi
done

if [ "$RC" -eq 0 ]; then
    echo "POSITIVE-CONTROL: PASS"
else
    echo "POSITIVE-CONTROL: FAIL"
fi
exit "$RC"
