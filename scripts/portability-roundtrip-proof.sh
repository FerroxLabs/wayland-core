#!/bin/sh
# portability-roundtrip-proof.sh — F26-03 export/restore round-trip evidence.
#
# Usage:  sh scripts/portability-roundtrip-proof.sh <path-to-wayland-core> [outdir]
#
# Runs the REAL binary against real filesystem locations and DIFFS the result,
# rather than asserting equality from inside a test process.
#
# Two legs, because a backup has two honest modes and they round-trip
# differently:
#
#   LEG A — full-fidelity (`--include-secrets`). The archive carries the whole
#           home, so `diff -r` between source and restored tree must be EMPTY.
#           This is the lossless claim, and it is checked by diff, not by digest
#           alone, so a digest that agreed for the wrong reason cannot pass it.
#
#   LEG B — redacted (the DEFAULT). The archive deliberately omits the secret
#           entries `wcore_config::profile::is_secret_entry` classifies. Such an
#           archive CANNOT round-trip those secret values -- by construction, not
#           by defect. So the assertion is not "the trees match": it is that the
#           difference is EXACTLY the entries the manifest recorded as absent,
#           and nothing else. Claiming a redacted round trip is lossless would be
#           false, and this leg is written to make that falsity visible.
#
# Every credential value here is a canary string created by this script. No real
# home and no real secret is read, on any host.

set -u

FAIL() { echo "ROUNDTRIP-FAIL: $*"; exit 1; }

BIN="${1:-}"
OUTDIR="${2:-}"
[ -n "$BIN" ] || FAIL "usage: $0 <path-to-wayland-core> [outdir]"
[ -x "$BIN" ] || FAIL "binary does not exist or is not executable: $BIN"
"$BIN" backup --help >/dev/null 2>&1 || FAIL "binary does not support 'backup': $BIN"

WORK=$(mktemp -d) || FAIL "could not create a work directory"
trap 'rm -rf "$WORK"' EXIT

SRC="$WORK/profile-home"
mkdir -p "$SRC/skills/demo" "$SRC/memory" "$SRC/oauth" || FAIL "fixture"

# A realistic profile home: config, a skill, a memory note, and BOTH shapes of
# secret the profile module classifies -- a `credentials*` file and the `oauth/`
# directory. Canary values only.
cat > "$SRC/config.toml" <<'CFG'
[storage.credentials]
backend = "plaintext"

[model]
default = "claude-opus-5"
CFG
printf -- '---\nname: demo\n---\nA demo skill body.\n' > "$SRC/skills/demo/SKILL.md"
printf 'remembered: the user prefers terse output\n' > "$SRC/memory/notes.md"
printf 'api_key = "CANARY-SECRET-DO-NOT-SHIP-0001"\n' > "$SRC/credentials.toml"
printf '{"refresh_token":"CANARY-SECRET-DO-NOT-SHIP-0002"}\n' > "$SRC/oauth/google.json"

echo "=== SOURCE HOME ==="
( cd "$SRC" && find . -type f | LC_ALL=C sort )

# ---------------------------------------------------------------- LEG A ------
echo
echo "=== LEG A: full-fidelity round trip (--include-secrets) ==="
ARC_A="$WORK/full.tar.gz"
DST_A="$WORK/restored-full"
"$BIN" backup create --home "$SRC" --out "$ARC_A" --include-secrets > "$WORK/a-create.log" 2>&1 \
    || { cat "$WORK/a-create.log"; FAIL "leg A: create"; }
sed -n 's/^\(payloads\|secrets_carried\|tree_digest\): /  create \1: /p' "$WORK/a-create.log"

"$BIN" backup verify "$ARC_A" > "$WORK/a-verify.log" 2>&1 \
    || { cat "$WORK/a-verify.log"; FAIL "leg A: verify"; }
echo "  verify: OK"

"$BIN" backup restore "$ARC_A" --home "$DST_A" > "$WORK/a-restore.log" 2>&1 \
    || { cat "$WORK/a-restore.log"; FAIL "leg A: restore"; }

if diff -r "$SRC" "$DST_A" > "$WORK/a-diff.txt" 2>&1; then
    echo "  DIFF: empty — the full-fidelity round trip is LOSSLESS"
else
    echo "  DIFF (unexpected):"; cat "$WORK/a-diff.txt"
    FAIL "leg A: a full-fidelity round trip must reproduce the source exactly"
fi
# The secret values really did come back — otherwise 'lossless' is unearned.
grep -q 'CANARY-SECRET-DO-NOT-SHIP-0001' "$DST_A/credentials.toml" 2>/dev/null \
    || FAIL "leg A: the credentials value did not survive the round trip"
grep -q 'CANARY-SECRET-DO-NOT-SHIP-0002' "$DST_A/oauth/google.json" 2>/dev/null \
    || FAIL "leg A: the oauth value did not survive the round trip"
echo "  LEG-A: lossless=yes secrets_restored=2"

# ---------------------------------------------------------------- LEG B ------
echo
echo "=== LEG B: redacted round trip (default) ==="
ARC_B="$WORK/redacted.tar.gz"
DST_B="$WORK/restored-redacted"
"$BIN" backup create --home "$SRC" --out "$ARC_B" > "$WORK/b-create.log" 2>&1 \
    || { cat "$WORK/b-create.log"; FAIL "leg B: create"; }
sed -n 's/^\(payloads\|secrets_carried\|absent_secrets\): /  create \1: /p' "$WORK/b-create.log"

ABSENT=$(sed -n 's/^absent_secrets: //p' "$WORK/b-create.log")
[ -n "$ABSENT" ] || FAIL "leg B: a redacted archive must RECORD what it omitted"

# Default restore REFUSES, because the archive cannot carry the source's
# secrets and a config pointing at credentials that are not there is worse than
# a refusal.
if "$BIN" backup restore "$ARC_B" --home "$DST_B" > "$WORK/b-refuse.log" 2>&1; then
    FAIL "leg B: the restore did NOT refuse an archive with absent credential sources"
fi
echo "  refusal (verbatim):"
sed 's/^/    | /' "$WORK/b-refuse.log"
[ ! -d "$DST_B" ] || [ -z "$(ls -A "$DST_B" 2>/dev/null)" ] \
    || FAIL "leg B: a refusal wrote to the target anyway"
echo "  refusal left the target unwritten: yes"

# With the operator's explicit acknowledgement it proceeds.
"$BIN" backup restore "$ARC_B" --home "$DST_B" --accept-missing-secrets \
    > "$WORK/b-restore.log" 2>&1 || { cat "$WORK/b-restore.log"; FAIL "leg B: restore"; }

diff -r "$SRC" "$DST_B" > "$WORK/b-diff.txt" 2>&1
echo "  DIFF source vs restored:"
sed 's/^/    | /' "$WORK/b-diff.txt"

# The difference must be EXACTLY the recorded absent set. Anything else present
# in the diff is real data loss wearing redaction's clothes.
UNEXPECTED=$(grep -v -e 'credentials\.toml' -e 'oauth' "$WORK/b-diff.txt" | grep -c . )
[ "$UNEXPECTED" -eq 0 ] \
    || { echo "  unexpected diff lines: $UNEXPECTED"; FAIL "leg B: the redacted round trip lost something OTHER than the recorded secrets"; }

# And the secret VALUES are provably gone from the restored tree.
if grep -rq 'CANARY-SECRET-DO-NOT-SHIP' "$DST_B" 2>/dev/null; then
    FAIL "leg B: a canary secret value survived into a REDACTED restore"
fi
# Positive control: the same search DOES find them in the source, so the
# absence above measures redaction rather than a broken search.
grep -rq 'CANARY-SECRET-DO-NOT-SHIP' "$SRC" 2>/dev/null \
    || FAIL "leg B: the canary search found nothing even in the SOURCE, so it proves nothing"

# Everything non-secret did come back.
diff -r "$SRC/skills" "$DST_B/skills" >/dev/null 2>&1 || FAIL "leg B: skills did not round-trip"
diff -r "$SRC/memory" "$DST_B/memory" >/dev/null 2>&1 || FAIL "leg B: memory did not round-trip"
diff "$SRC/config.toml" "$DST_B/config.toml" >/dev/null 2>&1 || FAIL "leg B: config did not round-trip"

echo "  LEG-B: lossless=no absent_recorded=[$ABSENT] canaries_in_restore=0 non_secret_roundtrip=exact"

echo
echo "ROUNDTRIP-OK"
echo "WHAT A REDACTED EXPORT CANNOT ROUND-TRIP: $ABSENT"
exit 0
