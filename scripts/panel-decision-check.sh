#!/bin/sh
# Mechanical enforcement of the phase-26 panel procedure.
#
# This is why a converted decision still has a gate that can go RED. Its
# load-bearing assertions read the CAPTURED panel files, never prose the
# executor typed — in particular the dissent set is COMPUTED from the captured
# verdicts, so a losing argument cannot be disposed of by leaving it out.
#
# Usage:
#   panel-decision-check.sh <panel-dir> <option-id>...
#   panel-decision-check.sh --self-test

set -u

# --- helpers -------------------------------------------------------------

# The single verdict line of a capture, or empty. Deliberately UNANCHORED:
# kimi bullet-prefixes and indents its output, so an anchored '^PANEL-VERDICT:'
# regex silently loses that member's vote — the same defect class as a gate that
# cannot fail.
verdict_of() {
    /usr/bin/grep -oE 'PANEL-VERDICT:[[:space:]]*[A-Za-z0-9_-]+' "$1" 2>/dev/null |
        /usr/bin/sed -E 's/.*PANEL-VERDICT:[[:space:]]*//' | /usr/bin/tail -1
}
verdict_count() {
    /usr/bin/grep -coE 'PANEL-VERDICT:[[:space:]]*[A-Za-z0-9_-]+' "$1" 2>/dev/null || echo 0
}

check_panel() {
    DIR="$1"
    shift
    OPTIONS="$*"
    RC=0
    fail() {
        echo "  REJECT: $*" >&2
        RC=1
    }

    [ -d "$DIR" ] || {
        echo "  REJECT: no panel directory '$DIR'" >&2
        return 1
    }

    Q="$DIR/question.md"
    if [ ! -s "$Q" ]; then
        fail "question.md is missing or empty"
    elif [ "$(/usr/bin/wc -c <"$Q" | tr -d ' ')" -lt 200 ]; then
        fail "question.md is too thin to be a real evidence bundle"
    fi

    QSHA=""
    [ -s "$Q" ] && QSHA=$(shasum -a 256 "$Q" | /usr/bin/awk '{print $1}')

    # --- the four captures ---
    for M in codex gemini kimi internal; do
        F="$DIR/$M.txt"
        [ "$M" = internal ] && F="$DIR/internal.md"
        if [ ! -s "$F" ]; then
            fail "capture for member '$M' is missing or empty"
            continue
        fi
        if [ "$(/usr/bin/wc -c <"$F" | tr -d ' ')" -lt 80 ]; then
            fail "capture for '$M' is trivially short — a stub, not an answer"
        fi
        N=$(verdict_count "$F")
        if [ "$N" -ne 1 ]; then
            fail "capture for '$M' carries $N PANEL-VERDICT lines; exactly 1 required"
            continue
        fi
        V=$(verdict_of "$F")
        FOUND=0
        for O in $OPTIONS; do [ "$V" = "$O" ] && FOUND=1; done
        [ "$FOUND" -eq 1 ] || fail "'$M' voted '$V', which is not in the option set: $OPTIONS"

        # The digest stamp binds a capture to the bundle it answered.
        if [ "$M" != internal ] && [ -n "$QSHA" ]; then
            /usr/bin/grep -qF "PANEL-QUESTION-SHA256: $QSHA" "$F" ||
                fail "'$M' does not carry the digest of question.md — it may have answered a different bundle"
        fi
    done

    # --- pairwise distinct external captures (one answer copied into several) ---
    for A in codex gemini kimi; do
        for B in codex gemini kimi; do
            [ "$A" = "$B" ] && continue
            FA="$DIR/$A.txt"
            FB="$DIR/$B.txt"
            if [ -s "$FA" ] && [ -s "$FB" ]; then
                # Compare bodies, ignoring the member/digest header.
                if /usr/bin/tail -n +4 "$FA" 2>/dev/null | /usr/bin/diff -q - \
                    "$(mktemp -u)" >/dev/null 2>&1; then :; fi
                BA=$(/usr/bin/tail -n +4 "$FA" | shasum -a 256 | /usr/bin/awk '{print $1}')
                BB=$(/usr/bin/tail -n +4 "$FB" | shasum -a 256 | /usr/bin/awk '{print $1}')
                [ "$BA" = "$BB" ] && fail "captures '$A' and '$B' are byte-identical — one answer in two files"
            fi
        done
    done

    # --- the decision record ---
    D="$DIR/DECISION.md"
    if [ ! -s "$D" ]; then
        fail "DECISION.md is missing or empty"
        [ "$RC" -eq 0 ] && RC=1
        return "$RC"
    fi

    NC=$(/usr/bin/grep -cE '^CHOSEN:[[:space:]]*[A-Za-z0-9_-]+[[:space:]]*$' "$D")
    [ "$NC" -eq 1 ] || fail "DECISION.md must carry exactly one CHOSEN: line (found $NC)"
    CHOSEN=$(/usr/bin/grep -E '^CHOSEN:' "$D" | /usr/bin/sed -E 's/^CHOSEN:[[:space:]]*//; s/[[:space:]]*$//' | /usr/bin/head -1)
    FOUND=0
    for O in $OPTIONS; do [ "$CHOSEN" = "$O" ] && FOUND=1; done
    [ "$FOUND" -eq 1 ] || fail "CHOSEN '$CHOSEN' is not in the option set: $OPTIONS"

    BASIS=$(/usr/bin/grep -E '^BASIS:' "$D" | /usr/bin/sed -E 's/^BASIS:[[:space:]]*//; s/[[:space:]]*$//' | /usr/bin/head -1)
    case "$BASIS" in
    majority | minority-with-evidence) ;;
    *) fail "BASIS must be 'majority' or 'minority-with-evidence' (got '$BASIS')" ;;
    esac

    RAT=$(/usr/bin/sed -n 's/^RATIONALE:[[:space:]]*//p' "$D" | /usr/bin/head -1)
    [ "${#RAT}" -ge 120 ] || fail "RATIONALE is ${#RAT} chars; at least 120 required"

    # --- majority computed from the CAPTURED verdicts ---
    TALLY=""
    for M in codex gemini kimi internal; do
        F="$DIR/$M.txt"
        [ "$M" = internal ] && F="$DIR/internal.md"
        [ -s "$F" ] || continue
        V=$(verdict_of "$F")
        [ -n "$V" ] && TALLY="$TALLY$V
"
    done
    TOP=$(printf '%s' "$TALLY" | /usr/bin/grep -v '^$' | /usr/bin/sort | /usr/bin/uniq -c | /usr/bin/sort -rn | /usr/bin/head -1 | /usr/bin/awk '{print $2}')

    if [ "$BASIS" = "minority-with-evidence" ]; then
        EV=$(/usr/bin/sed -n 's/^EVIDENCE:[[:space:]]*//p' "$D" | /usr/bin/head -1)
        if [ -z "$EV" ]; then
            fail "BASIS is minority-with-evidence but no EVIDENCE: line names an artifact"
        elif [ ! -s "$DIR/$EV" ] && [ ! -s "$EV" ]; then
            fail "EVIDENCE names '$EV', which is not a real non-empty artifact"
        fi
    elif [ "$CHOSEN" != "$TOP" ]; then
        fail "BASIS is 'majority' but CHOSEN '$CHOSEN' is not the plurality verdict ('$TOP')"
    fi

    # --- dissent, derived from the captures rather than read from prose ---
    /usr/bin/grep -qE '^##[[:space:]]+DISSENT' "$D" || fail "DECISION.md has no '## DISSENT' section"
    DISSENT_BODY=$(/usr/bin/sed -n '/^##[[:space:]]*DISSENT/,$p' "$D")
    DIFFERING=$(printf '%s' "$TALLY" | /usr/bin/grep -v '^$' | /usr/bin/sort -u | /usr/bin/grep -vx "$CHOSEN" || true)
    if [ -z "$DIFFERING" ]; then
        printf '%s' "$DISSENT_BODY" | /usr/bin/grep -qiE 'unanimous|no dissent|all four agree' ||
            fail "all verdicts agree, so DISSENT must say so explicitly rather than be empty"
    else
        for V in $DIFFERING; do
            printf '%s' "$DISSENT_BODY" | /usr/bin/grep -qF "$V" ||
                fail "verdict '$V' differs from CHOSEN but is absent from the DISSENT section"
        done
    fi

    return "$RC"
}

# --- self-test -----------------------------------------------------------
#
# Negative controls ALONE would be satisfied by a checker that rejects
# everything, so the ACCEPTING case is mandatory.

mk_capture() {
    # $1=file $2=member $3=digest $4=verdict $5=filler
    {
        echo "PANEL-MEMBER: $2"
        echo "PANEL-QUESTION-SHA256: $3"
        echo "---"
        echo "This is a substantive answer body with enough length to be real. $5"
        echo "PANEL-VERDICT: $4"
        echo "PANEL-BASIS: because $5"
    } >"$1"
}

build_good() {
    D="$1"
    mkdir -p "$D"
    {
        echo "# Question"
        echo "Does the emitted type make a credential value unrepresentable?"
        printf 'evidence line %s\n' 1 2 3 4 5 6 7 8 9 10 11 12
    } >"$D/question.md"
    G=$(shasum -a 256 "$D/question.md" | /usr/bin/awk '{print $1}')
    mk_capture "$D/codex.txt" codex "$G" contract-holds "alpha reasoning"
    mk_capture "$D/gemini.txt" gemini "$G" contract-holds "beta reasoning"
    mk_capture "$D/kimi.txt" kimi "$G" contract-holds "gamma reasoning"
    mk_capture "$D/internal.md" internal "$G" contract-holds "delta adversarial"
    {
        echo "CHOSEN: contract-holds"
        echo "BASIS: majority"
        echo "RATIONALE: The emitted type has no field, variant or accessor able to hold a credential value, and the multi-emitter probe covering serde, Debug, Display and the error path found no canary in any rendering."
        echo ""
        echo "## DISSENT"
        echo "None — all four verdicts agree, unanimous."
    } >"$D/DECISION.md"
}

self_test() {
    OPTS="contract-holds contract-cosmetic contract-leaks"
    TMP=$(mktemp -d)
    FAILS=0
    expect() { # $1=label $2=dir $3=expected rc (0 accept / 1 reject)
        check_panel "$2" $OPTS >/dev/null 2>&1
        GOT=$?
        [ "$GOT" -ne 0 ] && GOT=1
        if [ "$GOT" -ne "$3" ]; then
            echo "SELF-TEST FAIL: '$1' expected rc=$3 got rc=$GOT" >&2
            FAILS=$((FAILS + 1))
        else
            echo "  ok: $1"
        fi
    }

    # 1. ACCEPTS a well-formed directory (mandatory positive control).
    build_good "$TMP/good"
    expect "accepts a well-formed panel" "$TMP/good" 0

    # 2. missing directory
    expect "rejects a missing directory" "$TMP/does-not-exist" 1

    # 3. missing capture
    build_good "$TMP/nocap" && rm "$TMP/nocap/kimi.txt"
    expect "rejects a missing capture" "$TMP/nocap" 1

    # 4. capture with no verdict line
    build_good "$TMP/noverdict" &&
        /usr/bin/grep -v 'PANEL-VERDICT' "$TMP/noverdict/gemini.txt" >"$TMP/t" &&
        mv "$TMP/t" "$TMP/noverdict/gemini.txt"
    expect "rejects a capture with no verdict" "$TMP/noverdict" 1

    # 5. verdict outside the option set
    build_good "$TMP/badopt" &&
        /usr/bin/sed 's/PANEL-VERDICT: contract-holds/PANEL-VERDICT: contract-invented/' \
            "$TMP/badopt/codex.txt" >"$TMP/t" && mv "$TMP/t" "$TMP/badopt/codex.txt"
    expect "rejects a verdict outside the option set" "$TMP/badopt" 1

    # 6. two byte-identical external captures
    build_good "$TMP/dup" && cp "$TMP/dup/codex.txt" "$TMP/t" &&
        /usr/bin/sed 's/PANEL-MEMBER: codex/PANEL-MEMBER: gemini/' "$TMP/t" >"$TMP/dup/gemini.txt"
    # make the BODIES identical (header differs, body does not)
    /usr/bin/tail -n +4 "$TMP/dup/codex.txt" >"$TMP/body"
    {
        /usr/bin/head -3 "$TMP/dup/gemini.txt"
        cat "$TMP/body"
    } >"$TMP/dup/gemini.txt"
    expect "rejects two identical external captures" "$TMP/dup" 1

    # 7. mismatched question digest
    build_good "$TMP/baddigest" &&
        /usr/bin/sed 's/PANEL-QUESTION-SHA256: .*/PANEL-QUESTION-SHA256: deadbeef/' \
            "$TMP/baddigest/kimi.txt" >"$TMP/t" && mv "$TMP/t" "$TMP/baddigest/kimi.txt"
    expect "rejects a mismatched question digest" "$TMP/baddigest" 1

    # 8. CHOSEN outside the option set
    build_good "$TMP/badchosen" &&
        /usr/bin/sed 's/^CHOSEN: .*/CHOSEN: contract-nonsense/' \
            "$TMP/badchosen/DECISION.md" >"$TMP/t" && mv "$TMP/t" "$TMP/badchosen/DECISION.md"
    expect "rejects a CHOSEN outside the option set" "$TMP/badchosen" 1

    # 9. minority choice whose EVIDENCE names nothing that exists
    build_good "$TMP/badev" &&
        /usr/bin/sed -e 's/^CHOSEN: .*/CHOSEN: contract-cosmetic/' \
            -e 's/^BASIS: .*/BASIS: minority-with-evidence\nEVIDENCE: no-such-file.log/' \
            "$TMP/badev/DECISION.md" >"$TMP/t" && mv "$TMP/t" "$TMP/badev/DECISION.md"
    expect "rejects an unevidenced minority choice" "$TMP/badev" 1

    # 10. a dissenting captured verdict omitted from the DISSENT section
    build_good "$TMP/nodissent" &&
        /usr/bin/sed 's/PANEL-VERDICT: contract-holds/PANEL-VERDICT: contract-cosmetic/' \
            "$TMP/nodissent/kimi.txt" >"$TMP/t" && mv "$TMP/t" "$TMP/nodissent/kimi.txt"
    expect "rejects an omitted dissent" "$TMP/nodissent" 1

    rm -rf "$TMP"
    if [ "$FAILS" -ne 0 ]; then
        echo "SELF-TEST FAILED ($FAILS expectation(s) violated)" >&2
        return 1
    fi
    echo "SELF-TEST PASSED: rejects each malformed shape AND accepts a well-formed one"
    return 0
}

# --- entry ---------------------------------------------------------------

if [ "${1:-}" = "--self-test" ]; then
    self_test
    exit $?
fi

if [ "$#" -lt 2 ]; then
    echo "usage: $0 <panel-dir> <option-id>...   |   $0 --self-test" >&2
    exit 2
fi

DIR="$1"
shift
echo "checking panel: $DIR (options: $*)"
if check_panel "$DIR" "$@"; then
    echo "PANEL RECORD OK"
    exit 0
fi
echo "PANEL RECORD REJECTED" >&2
exit 1
