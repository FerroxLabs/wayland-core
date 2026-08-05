#!/bin/sh
# F26-05 / Task 4 - RE-EXECUTE every claim the certification marks CLOSED.
#
# Usage: portability-evidence-replay.sh <certification.md> <out-dir>
#
# WHY THIS EXISTS
# ---------------
# A certification is a document its author wrote. Reading it back proves the
# artifact exists; it does not prove the claim still holds. This phase has
# already been corrected once for precisely that distinction - an earlier
# revision invented a fourth termination state so that F26-01 could close on
# Linux evidence alone while its mandatory macOS leg went unrun. A certification
# that reads well is the artifact that failure produced. A replay is the thing
# it could not have survived.
#
# So for every key the certification marks CLOSED this RE-EXECUTES that key's
# NAMED evidence at the certified SHA and prints its own verdict:
#
#   REPLAY: <key> evidence=<path> host=<host> result=reproduced|failed|not-replayable reason=<text>
#
# HOW EACH KIND OF EVIDENCE IS REPLAYED
#   *.sh   re-run on hetzner-dsm in this plan's own worktree /root/wayland-f26-04
#   *.ps1  re-run on SeanD@seandesktop in C:\ferrox-win
#   *.rs   re-run through a nextest filter; a run that selected ZERO tests is
#          FAILED, not reproduced - an empty filter is the oldest green here
#   *.md   re-validated against its own fixed grammar, and where the artifact is
#          26-01's macOS marker its provenance is re-derived from GitHub rather
#          than transcribed
#   *.txt  a captured report pair has its COMPARISON redone rather than believed
#
# `not-replayable` is honest for evidence that is inherently one-shot. It is not
# a synonym for inconvenient: it requires a reason naming WHY, and the plan's
# binding gate refuses to let either accept option rest on a key whose replay
# was not `reproduced`.
#
# SELF-RED: handed a certification path that does not exist this exits non-zero.

set -u

CERT="${1:-}"
OUT="${2:-}"

if [ -z "$CERT" ] || [ -z "$OUT" ]; then
    echo "usage: $0 <certification.md> <out-dir>" >&2
    exit 2
fi
if [ ! -s "$CERT" ]; then
    echo "REPLAY-FAIL: certification '$CERT' is missing or empty. There is nothing to replay." >&2
    exit 3
fi

mkdir -p "$OUT" || exit 3
VERDICTS="$OUT/replay-verdicts.txt"
: > "$VERDICTS"

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO=$(dirname "$HERE")
LINUX_HOST=hetzner-dsm
LINUX_WT=/root/wayland-f26-04
WIN_HOST=SeanD@seandesktop
WIN_WT='C:\ferrox-win'

CERT_SHA=$(/usr/bin/grep -oE '^F26-CERT-SHA: [0-9a-f]{40}$' "$CERT" | /usr/bin/sed 's/^F26-CERT-SHA: //' | /usr/bin/head -1)
if [ -z "$CERT_SHA" ]; then
    echo "REPLAY-FAIL: the certification names no 'F26-CERT-SHA: <40-hex>' tree, so there is no tree to re-run its evidence against." >&2
    exit 4
fi
echo "REPLAY-CERT-SHA: $CERT_SHA" | tee -a "$VERDICTS"

emit() {
    printf 'REPLAY: %s evidence=%s host=%s result=%s reason=%s\n' "$1" "$2" "$3" "$4" "$5" |
        tee -a "$VERDICTS"
}

# --- replay drivers ---------------------------------------------------------

replay_linux_script() {
    KEY=$1; EV=$2; ARGS=$3
    LOG="$OUT/replay-$KEY.log"
    ssh -n -o BatchMode=yes "$LINUX_HOST" \
        "set -e; export PATH=/root/.cargo/bin:\$PATH; cd $LINUX_WT; \
         test \"\$(git rev-parse HEAD)\" = $CERT_SHA; \
         cargo build --locked --release -p wcore-cli --bin wayland-core >/dev/null 2>&1; \
         sh $EV $ARGS; \
         test \"\$(git rev-parse HEAD)\" = $CERT_SHA" > "$LOG" 2>&1
    RC=$?
    if [ $RC -eq 0 ]; then
        emit "$KEY" "$EV" "$LINUX_HOST" reproduced "re-ran-at-certified-sha-exit-0"
    else
        emit "$KEY" "$EV" "$LINUX_HOST" failed "re-run-exited-$RC-see-$(basename "$LOG")"
        FAILED=$((FAILED + 1))
    fi
}

replay_rust_tests() {
    KEY=$1; EV=$2; FILTER=$3
    LOG="$OUT/replay-$KEY.log"
    ssh -n -o BatchMode=yes "$LINUX_HOST" \
        "set -e; export PATH=/root/.cargo/bin:\$PATH; cd $LINUX_WT; \
         test \"\$(git rev-parse HEAD)\" = $CERT_SHA; \
         cargo nextest run --locked -p wcore-cli --no-fail-fast -E '$FILTER'; \
         test \"\$(git rev-parse HEAD)\" = $CERT_SHA" > "$LOG" 2>&1
    RC=$?
    # A run that selected ZERO tests is FAILED. An empty filter is the oldest
    # green in this repository and it must never read as reproduction.
    N=$(/usr/bin/grep -oE '[0-9]+ tests? run' "$LOG" | /usr/bin/tail -1 | /usr/bin/sed -E 's/ tests? run//')
    [ -n "$N" ] || N=0
    if [ "$N" -eq 0 ]; then
        emit "$KEY" "$EV" "$LINUX_HOST" failed "filter-selected-zero-tests"
        FAILED=$((FAILED + 1))
    elif [ $RC -eq 0 ]; then
        emit "$KEY" "$EV" "$LINUX_HOST" reproduced "re-ran-$N-tests-at-certified-sha-all-passed"
    else
        emit "$KEY" "$EV" "$LINUX_HOST" failed "re-run-exited-$RC-with-$N-tests"
        FAILED=$((FAILED + 1))
    fi
}

# A Windows claim must be re-executed ON WINDOWS. Replaying a mirrored Linux
# script and calling that a Windows proof is the same substitution this phase
# rejects everywhere else: a corroborating run presented as the evidence.
# Every step checks its OWN status, because a chain of `cmd /c` calls read
# through one trailing $LASTEXITCODE reports only the last.
replay_windows_script() {
    KEY=$1; EV=$2; ARGS=$3
    LOG="$OUT/replay-$KEY.log"
    WINEV=$(printf '%s\n' "$EV" | /usr/bin/sed 's|/|\\|g')
    # The SHA is proven from an ISOLATED capture, before and after.
    ssh -n -o BatchMode=yes "$WIN_HOST" \
        "cmd /c \"cd /d $WIN_WT && git rev-parse HEAD > $WIN_WT\\replay-head.txt\"" > "$LOG" 2>&1
    scp -o BatchMode=yes "$WIN_HOST:C:/ferrox-win/replay-head.txt" "$OUT/$KEY-winhead.txt" >/dev/null 2>&1 || {
        emit "$KEY" "$EV" "$WIN_HOST" failed "could-not-fetch-the-isolated-rev-parse-capture"
        FAILED=$((FAILED + 1)); return; }
    WH=$(tr -d ' \r\n' < "$OUT/$KEY-winhead.txt")
    echo "windows_head=[$WH] certified=[$CERT_SHA]" >> "$LOG"
    if [ "$WH" != "$CERT_SHA" ]; then
        emit "$KEY" "$EV" "$WIN_HOST" failed "windows-checkout-is-at-$WH-not-the-certified-sha"
        FAILED=$((FAILED + 1)); return
    fi
    # `powershell -NoProfile -File <missing.ps1>; exit $LASTEXITCODE` exits ZERO.
    # A replay written without this guard reports `reproduced` for a script that
    # is not there — the highest-leverage self-passing shape in this program, and
    # F26-03-C is the measurement that it is live on this very box. So the script
    # is PROVEN present before it is run, and its absence is a hard failure.
    ssh -n -o BatchMode=yes "$WIN_HOST" \
        "powershell -NoProfile -Command \"if (Test-Path -LiteralPath '$WIN_WT\\$WINEV' -PathType Leaf) { exit 0 } else { exit 66 }\"" >> "$LOG" 2>&1
    if [ $? -ne 0 ]; then
        emit "$KEY" "$EV" "$WIN_HOST" failed "named-evidence-script-is-absent-on-the-windows-checkout"
        FAILED=$((FAILED + 1)); return
    fi
    ssh -n -o BatchMode=yes -o ServerAliveInterval=30 "$WIN_HOST" \
        "powershell -NoProfile -File $WIN_WT\\$WINEV $ARGS; exit \$LASTEXITCODE" >> "$LOG" 2>&1
    RC=$?
    if [ $RC -eq 0 ]; then
        emit "$KEY" "$EV" "$WIN_HOST" reproduced "re-ran-on-real-windows-at-certified-sha-exit-0"
    else
        emit "$KEY" "$EV" "$WIN_HOST" failed "re-run-on-windows-exited-$RC"
        FAILED=$((FAILED + 1))
    fi
}

# 26-01's mandatory macOS real-state leg. Re-derived from GitHub, never
# transcribed: an EXPIRED artifact still appears in the listing, so the name
# alone proves nothing.
replay_macos_marker() {
    KEY=$1; EV=$2
    BASE="$REPO/$EV"
    LOG="$OUT/replay-$KEY.log"
    : > "$LOG"
    M=$(/usr/bin/grep -oE 'F26-SC1-MACOS: RAN — run=[0-9]+ sha=[0-9a-f]{40} binary=[^[:space:]]+ arch=arm64 hermes_profiles=12 openclaw_items=[1-9][0-9]* secret_hits=0' "$BASE" 2>/dev/null | /usr/bin/head -1)
    if [ -z "$M" ]; then
        emit "$KEY" "$EV" github failed "no-well-formed-F26-SC1-MACOS-RAN-marker"
        FAILED=$((FAILED + 1))
        return
    fi
    echo "$M" >> "$LOG"
    RUN=$(echo "$M" | /usr/bin/sed -E 's/.* run=([0-9]+) .*/\1/')
    SHA=$(echo "$M" | /usr/bin/sed -E 's/.* sha=([0-9a-f]{40}) .*/\1/')
    if ! command -v gh >/dev/null 2>&1; then
        emit "$KEY" "$EV" github not-replayable "gh-unavailable-so-provenance-cannot-be-re-derived"
        NOTREPRO=$((NOTREPRO + 1))
        return
    fi
    # A TRANSPORT failure is not a CLAIM failure, and conflating them makes this
    # script produce false reds — which is exactly as useless as a false green.
    # Measured on 2026-07-28: `gh` returned an empty body twice in one run with
    # `net/http: TLS handshake timeout`, while the same query answered correctly
    # seconds later. So every GitHub read is retried, and an EMPTY result after
    # the retries is reported as `not-replayable` naming the transport — never as
    # `failed`, which would assert the claim is broken, and never as
    # `reproduced`, which would assert it holds. `not-replayable` still blocks
    # acceptance, which is the correct treatment of a claim this run could not
    # check.
    gh_retry() {
        _out=""
        _n=0
        while [ $_n -lt 3 ]; do
            _out=$(eval "$1" 2>>"$LOG")
            if [ -n "$_out" ]; then
                printf '%s\n' "$_out"
                return 0
            fi
            _n=$((_n + 1))
            sleep 3
        done
        return 1
    }

    HS=$(gh_retry "gh run view $RUN -R FerroxLabs/wayland-core --json headSha --jq .headSha") || {
        emit "$KEY" "$EV" github not-replayable "github-unreachable-after-3-attempts-transport-not-claim"
        NOTREPRO=$((NOTREPRO + 1)); return; }
    echo "headSha=$HS expected=$SHA" >> "$LOG"
    if [ "$HS" != "$SHA" ]; then
        emit "$KEY" "$EV" github failed "run-$RUN-headSha-$HS-not-$SHA"
        FAILED=$((FAILED + 1))
        return
    fi
    JOB=$(gh_retry "gh run view $RUN -R FerroxLabs/wayland-core --json jobs --jq '.jobs[] | select(.name==\"Build (aarch64-apple-darwin)\") | .conclusion'") || {
        emit "$KEY" "$EV" github not-replayable "github-unreachable-reading-the-build-job"
        NOTREPRO=$((NOTREPRO + 1)); return; }
    printf '%s\n' "$JOB" > "$OUT/$KEY-job.txt"
    if ! /usr/bin/grep -qx 'success' "$OUT/$KEY-job.txt"; then
        emit "$KEY" "$EV" github failed "run-$RUN-has-no-successful-aarch64-apple-darwin-build"
        FAILED=$((FAILED + 1))
        return
    fi
    ART=$(gh_retry "gh api repos/FerroxLabs/wayland-core/actions/runs/$RUN/artifacts --jq '.artifacts[] | select(.expired==false and .size_in_bytes>0) | .name'") || {
        emit "$KEY" "$EV" github not-replayable "github-unreachable-reading-the-artifact-listing"
        NOTREPRO=$((NOTREPRO + 1)); return; }
    printf '%s\n' "$ART" > "$OUT/$KEY-art.txt"
    if ! /usr/bin/grep -qx 'wayland-core-aarch64-apple-darwin' "$OUT/$KEY-art.txt"; then
        emit "$KEY" "$EV" github failed "run-$RUN-publishes-no-live-non-empty-macos-arm64-artifact"
        FAILED=$((FAILED + 1))
        return
    fi
    emit "$KEY" "$EV" github reproduced "run-$RUN-headSha-and-build-job-and-live-artifact-all-re-derived"
}

# The cross-platform report pair: the COMPARISON is redone, not believed.
replay_report_pair() {
    KEY=$1; EV=$2
    LOG="$OUT/replay-$KEY.log"
    L=/tmp/rep-linux.txt
    W=/tmp/rep-windows.txt
    {
        echo "linux=$L windows=$W"
        ls -l "$L" "$W" 2>&1
    } > "$LOG"
    if [ ! -s "$L" ] || [ ! -s "$W" ]; then
        emit "$KEY" "$EV" both failed "one-or-both-normalised-reports-missing-or-empty"
        FAILED=$((FAILED + 1))
        return
    fi
    if /usr/bin/diff "$L" "$W" >> "$LOG" 2>&1; then
        emit "$KEY" "$EV" both reproduced "normalised-reports-re-compared-and-byte-identical"
    else
        emit "$KEY" "$EV" both failed "normalised-reports-differ-on-re-comparison"
        FAILED=$((FAILED + 1))
    fi
}

# --- dispatch ---------------------------------------------------------------

FAILED=0
NOTREPRO=0
CLOSED=0

/usr/bin/grep -E '^F26-(SC[1-4]|0[1-5]): CLOSED — ' "$CERT" > "$OUT/closed.txt"
while IFS= read -r LINE; do
    [ -n "$LINE" ] || continue
    KEY=$(printf '%s\n' "$LINE" | /usr/bin/sed -E 's/^([A-Z0-9-]+):.*/\1/')
    EV=$(printf '%s\n' "$LINE" | /usr/bin/grep -oE 'evidence=[^[:space:]]+' | /usr/bin/sed 's/^evidence=//')
    CLOSED=$((CLOSED + 1))
    if [ -z "$EV" ]; then
        emit "$KEY" NONE none failed "closed-without-named-evidence"
        FAILED=$((FAILED + 1))
        continue
    fi
    case "$EV" in
        *26-01-BASELINE.md)            replay_macos_marker "$KEY" "$EV" ;;
        *portability-native-matrix.ps1) replay_windows_script "$KEY" "$EV" '-Binary C:\ferrox-win\target\release\wayland-core.exe -Report C:\ferrox-win\replay-report-windows.txt' ;;
        *portability-native-matrix.sh) replay_linux_script "$KEY" "$EV" "./target/release/wayland-core /tmp/26-04-replay-$KEY.txt" ;;
        *portability-remap-capture.sh) replay_linux_script "$KEY" "$EV" "./target/release/wayland-core /tmp/26-04-replay-remap-$KEY" ;;
        *portability_hostile_corpus.rs) replay_rust_tests  "$KEY" "$EV" 'test(/hostile/)' ;;
        *migrate_quarantine.rs)        replay_rust_tests   "$KEY" "$EV" 'binary(migrate_quarantine)' ;;
        *rep-linux.txt|*report-pair*)  replay_report_pair  "$KEY" "$EV" ;;
        *.sh)                          replay_linux_script "$KEY" "$EV" "" ;;
        *)
            emit "$KEY" "$EV" none not-replayable "no-replay-driver-for-this-evidence-kind"
            NOTREPRO=$((NOTREPRO + 1))
            ;;
    esac
done < "$OUT/closed.txt"

echo "REPLAY-SUMMARY: closed_keys=$CLOSED failed=$FAILED not_replayable=$NOTREPRO" | tee -a "$VERDICTS"
[ "$CLOSED" -ge 1 ] || {
    echo "REPLAY-FAIL: the certification marks NOTHING closed, so this replayed nothing." >&2
    exit 5
}
[ "$FAILED" -eq 0 ] && [ "$NOTREPRO" -eq 0 ] || exit 1
exit 0
