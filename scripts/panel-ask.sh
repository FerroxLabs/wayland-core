#!/bin/sh
# Hand ONE evidence bundle, byte for byte, to each external panel member.
#
# Why a wrapper instead of three ad-hoc invocations: "every panelist saw the
# same evidence" has to be a property of the MECHANISM, not of the executor's
# discipline. This reads the question file once and passes those identical bytes
# to each member, then stamps every capture with the SHA-256 of that file, so
# the claim becomes checkable after the fact by panel-decision-check.sh.
#
# It exits non-zero, loudly, if any member errors or returns an empty body.
# Writing a stub for a member that did not answer would give you a three-member
# panel wearing a fourth's name.
#
# Usage: panel-ask.sh <question-file> <out-dir>

set -u

Q="${1:-}"
OUT="${2:-}"
if [ -z "$Q" ] || [ -z "$OUT" ]; then
    echo "usage: $0 <question-file> <out-dir>" >&2
    exit 2
fi
[ -s "$Q" ] || {
    echo "FAIL: question file '$Q' is missing or empty" >&2
    exit 1
}
mkdir -p "$OUT" || exit 1

DIGEST=$(shasum -a 256 "$Q" | /usr/bin/awk '{print $1}')
BODY=$(cat "$Q")
RC=0

# Each member is invoked with the SAME $BODY. stdout only: gemini emits a
# GOOGLE_API_KEY notice on stderr that is NOT a failure, and letting stderr into
# the capture would corrupt the verdict line.
ask() {
    NAME=$1
    FILE="$OUT/$NAME.txt"
    shift
    TMP=$(mktemp)
    "$@" >"$TMP" 2>"$OUT/$NAME.stderr"
    STATUS=$?
    if [ "$STATUS" -ne 0 ]; then
        echo "FAIL: panel member '$NAME' exited $STATUS" >&2
        /usr/bin/sed 's/^/    /' "$OUT/$NAME.stderr" >&2
        rm -f "$TMP"
        RC=1
        return
    fi
    if [ ! -s "$TMP" ]; then
        echo "FAIL: panel member '$NAME' returned an EMPTY body" >&2
        rm -f "$TMP"
        RC=1
        return
    fi
    {
        echo "PANEL-MEMBER: $NAME"
        echo "PANEL-QUESTION-SHA256: $DIGEST"
        echo "---"
        cat "$TMP"
    } >"$FILE"
    rm -f "$TMP"
    echo "captured $NAME ($(/usr/bin/wc -c <"$FILE" | tr -d ' ') bytes)"
}

ask codex codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check "$BODY"
ask gemini gemini -p "$BODY" -m gemini-3.1-pro-preview -o text --skip-trust
# ABSOLUTE path required: a non-interactive shell here predates the PATH entry.
ask kimi /Users/seandonahoe/.kimi-code/bin/kimi -p "$BODY" --output-format text

if [ "$RC" -ne 0 ]; then
    echo "panel-ask: one or more members failed; the panel is INCOMPLETE" >&2
else
    echo "panel-ask: 3 external captures written, all stamped $DIGEST"
fi
exit "$RC"
