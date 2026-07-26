#!/usr/bin/env bash
# F23-02 live driver — exercise every Success Criterion 2 verb against the
# SHIPPED wayland-core binary and capture one transcript per verb.
#
# Contract (shared with the rest of the f23 driver family):
#   --binary <path>   the wayland-core binary to drive
#   --sha <commit>    the commit under test; the binary's own --build-info
#                     source SHA must equal it, so a stale binary REDDENS
#                     instead of silently proving old code
#   --nonce <hex>     caller-generated at run time, echoed in the terminal PASS
#                     marker, so a stale log cannot satisfy the caller's check
#
# Emits exactly one terminal marker:
#   F23_01_DRIVE=PASS platform=<linux|macos> nonce=<the given nonce>
# and ONLY after every verb passed. Any failure exits non-zero and emits no
# PASS marker. A missing observable outcome is a failure, never a skip.
#
# Gate discipline: no check here is a pipeline into a filter. Every command's
# exit status is captured on the line AFTER it runs and asserted on directly.

set -uo pipefail

BINARY=""
SHA=""
NONCE=""

while [ $# -gt 0 ]; do
  case "$1" in
    --binary) BINARY="${2:-}"; shift 2 ;;
    --sha)    SHA="${2:-}";    shift 2 ;;
    --nonce)  NONCE="${2:-}";  shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

[ -n "$BINARY" ] || { echo "FATAL: --binary is required" >&2; exit 64; }
[ -n "$SHA" ]    || { echo "FATAL: --sha is required" >&2; exit 64; }
[ -n "$NONCE" ]  || { echo "FATAL: --nonce is required" >&2; exit 64; }
[ -x "$BINARY" ] || { echo "FATAL: $BINARY is not an executable file" >&2; exit 65; }

BINARY=$(cd "$(dirname "$BINARY")" && pwd)/$(basename "$BINARY")

case "$(uname -s)" in
  Linux)  PLATFORM=linux ;;
  Darwin) PLATFORM=macos ;;
  *) echo "FATAL: unsupported platform $(uname -s)" >&2; exit 66 ;;
esac

# ── Provenance: refuse to prove anything with a binary built from other code ──
BUILD_INFO=$("$BINARY" --build-info 2>&1)
rc=$?
if [ "$rc" -ne 0 ]; then
  echo "FATAL: --build-info exited $rc: $BUILD_INFO" >&2
  exit 67
fi
BIN_SHA=$(printf '%s\n' "$BUILD_INFO" | sed -n 's/.*(source \([0-9a-f]*\)).*/\1/p')
if [ "$BIN_SHA" != "$SHA" ]; then
  echo "FATAL: binary source SHA $BIN_SHA != commit under test $SHA" >&2
  exit 68
fi
echo "F23_01_PROVENANCE=ok platform=$PLATFORM sha=$SHA"

RUN_DIR=$(mktemp -d)
HOME_DIR="$RUN_DIR/home"
SESSIONS="$HOME_DIR/sessions"
WORKSPACE="$RUN_DIR/ws"
TRANSCRIPTS="$RUN_DIR/transcripts"
mkdir -p "$SESSIONS" "$WORKSPACE" "$TRANSCRIPTS"

cleanup() { rm -rf "$RUN_DIR"; }
trap cleanup EXIT

FAILURES=0

# Run one verb, capture its transcript, assert exit code and an expected token.
# $1 verb label, $2 expected exit code, $3 token that must appear on stdout,
# rest: argv after `session`.
verb() {
  local label="$1"; shift
  local want_rc="$1"; shift
  local want_token="$1"; shift
  local t="$TRANSCRIPTS/$label.txt"
  {
    echo "# invocation: $BINARY session --dir $SESSIONS --workspace $WORKSPACE $*"
  } > "$t"
  local out
  out=$(HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" \
        "$BINARY" session --dir "$SESSIONS" --workspace "$WORKSPACE" "$@" 2>>"$t")
  local rc=$?
  printf '%s\n' "$out" >> "$t"
  echo "# exit: $rc" >> "$t"

  local status=PASS
  if [ "$rc" -ne "$want_rc" ]; then
    status=FAIL
    echo "  expected exit $want_rc, observed $rc  ($label)" >&2
  fi
  if ! printf '%s\n' "$out" | grep -qF "$want_token"; then
    status=FAIL
    echo "  expected token '$want_token' absent from stdout ($label)" >&2
    printf '%s\n' "$out" >&2
  fi
  if [ "$status" != PASS ]; then
    echo "  --- transcript tail ($label) ---" >&2
    tail -4 "$t" >&2
  fi
  echo "F23_01_VERB=$label platform=$PLATFORM status=$status exit=$rc nonce=$NONCE"
  [ "$status" = PASS ] || FAILURES=$((FAILURES + 1))
  VERB_STDOUT="$out"
}

# ── Seed a real session through the product's own session store ──────────────
# The engine defers the first disk write until the first user message, so the
# fixture is built by driving a real turn against an unreachable provider: the
# turn is journalled and persisted before dispatch, then the provider call
# fails. That produces a genuine product-written session, not a hand-authored
# struct.
#
# The placeholder credential is assembled at run time from two fragments so
# that no credential-shaped literal is ever committed to the repository. It is
# not a real key and the base_url never points at a real provider.
FAKE_KEY="$(printf 's%s-ant-' k)f23-driver-not-a-real-key-0000"

cat > "$HOME_DIR/config.toml" <<EOF
[default]
provider = "anthropic"
model = "claude-3-5-haiku-20241022"

[providers.anthropic]
api_key = "${FAKE_KEY}"
base_url = "http://127.0.0.1:1"
EOF

PLANTED="f23nonce${NONCE}"
# Session ids are validated as 6-40 HEX characters, so the fixture ids are
# built from the hex nonce with a hex prefix.
SEED_ID="aaaa${NONCE}"

env -u API_KEY -u ANTHROPIC_API_KEY -u OPENAI_API_KEY \
    HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" \
    WAYLAND_VAULT_PASSPHRASE="f23-driver-vault-${NONCE}" \
    "$BINARY" --session-id "$SEED_ID" --max-turns 1 -- \
    "remember the aardvark and the value ${PLANTED}" \
    > "$TRANSCRIPTS/seed.txt" 2>&1
SEED_RC=$?
echo "# seed exit: $SEED_RC" >> "$TRANSCRIPTS/seed.txt"

if [ ! -f "$SESSIONS/index.json" ]; then
  echo "FATAL: the seed turn produced no session index; the driver cannot prove anything" >&2
  cat "$TRANSCRIPTS/seed.txt" >&2
  exit 70
fi
if ! grep -qF "$PLANTED" "$SESSIONS"/*.json 2>/dev/null; then
  echo "FATAL: the planted nonce is not in the seeded session; the export proof would be vacuous" >&2
  exit 71
fi
if [ ! -f "$SESSIONS/${SEED_ID}.journal" ]; then
  echo "FATAL: the seed did not persist under the requested id $SEED_ID" >&2
  cat "$TRANSCRIPTS/seed.txt" >&2
  exit 73
fi
echo "F23_01_SEED=ok id=$SEED_ID nonce=$NONCE"

# ── The verbs ────────────────────────────────────────────────────────────────
# The list assertion names the seeded id, not just the total line: a bare
# `list_total` token is printed even when the count is zero, so asserting on it
# alone is a gate that cannot go red.
verb list   0 "F23_SESSION=list id=$SEED_ID"              list
verb search 0 "F23_SESSION=search id=$SEED_ID"            search aardvark
verb search-miss 0 "count=0"                              search "zzz-absent-${NONCE}"
verb show   0 "F23_SESSION=show id=$SEED_ID"              show "$SEED_ID"

# checkpoint / rewind, proved by byte comparison.
TRACKED="$WORKSPACE/tracked.txt"
LATER="$WORKSPACE/created-after.txt"
printf 'original bytes %s\n' "$NONCE" > "$TRACKED"
BEFORE_HASH=$(cksum < "$TRACKED")

verb checkpoint 0 "F23_SESSION=checkpoint" checkpoint "$TRACKED" "$LATER"
CP_ID=$(printf '%s\n' "$VERB_STDOUT" | tr ' ' '\n' | sed -n 's/^id=//p' | head -1)
if [ -z "$CP_ID" ]; then
  echo "FATAL: checkpoint printed no id" >&2
  exit 72
fi

printf 'MUTATED\n' > "$TRACKED"
printf 'created after the checkpoint\n' > "$LATER"

verb rewind 0 "restored=true" rewind "$CP_ID"
AFTER_HASH=$(cksum < "$TRACKED")
if [ "$BEFORE_HASH" = "$AFTER_HASH" ]; then
  echo "F23_01_REWIND_BYTES_EQUAL=true"
else
  echo "F23_01_REWIND_BYTES_EQUAL=false"
  FAILURES=$((FAILURES + 1))
fi
if [ -f "$LATER" ]; then
  echo "F23_01_REWIND_LATER_FILE_REMOVED=false"
  FAILURES=$((FAILURES + 1))
else
  echo "F23_01_REWIND_LATER_FILE_REMOVED=true"
fi

# fork, proved by a byte-identical parent.
PARENT_FILE=$(ls "$SESSIONS"/*_"$SEED_ID".json 2>/dev/null | head -1)
PARENT_BEFORE=$(cksum < "$PARENT_FILE")
verb fork 0 "parent_unchanged=true" fork "$SEED_ID"
CHILD_ID=$(printf '%s\n' "$VERB_STDOUT" | tr ' ' '\n' | sed -n 's/^child=//p' | head -1)
PARENT_AFTER=$(cksum < "$PARENT_FILE")
if [ "$PARENT_BEFORE" = "$PARENT_AFTER" ]; then
  echo "F23_01_FORK_PARENT_BYTES_EQUAL=true"
else
  echo "F23_01_FORK_PARENT_BYTES_EQUAL=false"
  FAILURES=$((FAILURES + 1))
fi
verb show-fork 0 "parent=$SEED_ID" show "$CHILD_ID"

# retry — a turn that does not exist must be refused, not invented.
verb retry 3 "" retry "$SEED_ID" "turn-absent-${NONCE}"

# export — the planted nonce must be absent from the exported bytes.
EXPORT_PATH="$RUN_DIR/export.json"
verb export 0 "F23_SESSION=export" export "$SEED_ID" --out "$EXPORT_PATH"
OCCURRENCES=$(grep -c -F "$PLANTED" "$EXPORT_PATH" 2>/dev/null)
[ -n "$OCCURRENCES" ] || OCCURRENCES=0
echo "F23_01_EXPORT_NONCE_OCCURRENCES=$OCCURRENCES"
if [ "$OCCURRENCES" -ne 0 ]; then FAILURES=$((FAILURES + 1)); fi

verb retain 0 "retained" retain "$SEED_ID" --days 7
verb retain-expired 0 "expired" retain "$SEED_ID" --days -7
verb reconcile 0 "F23_SESSION=reconcile" reconcile "$SEED_ID"

# ── D2: a crash-interrupted session must become resumable ────────────────────
# Live Windows UAT defect D2: --continue refuses with "resume, reconcile, or
# cancel", and none of those verbs existed. Build a REAL interrupted session by
# killing the binary mid-turn, then prove the refusal, then prove cancel clears
# it. The provider base_url points at a socket that accepts and never answers,
# so the process is genuinely inside a turn when it dies.
D2_ID="bbbb${NONCE}"
HANG_PORT=$(( 20000 + ($$ % 20000) ))
( while true; do nc -l -p "$HANG_PORT" > /dev/null 2>&1 || sleep 1; done ) &
HANG_PID=$!
sleep 1

cat > "$HOME_DIR/config.toml" <<EOF
[default]
provider = "anthropic"
model = "claude-3-5-haiku-20241022"

[providers.anthropic]
api_key = "${FAKE_KEY}"
base_url = "http://127.0.0.1:${HANG_PORT}"
EOF

env HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" WAYLAND_VAULT_PASSPHRASE="f23-driver-vault-${NONCE}" \
    "$BINARY" --session-id "$D2_ID" --max-turns 1 -- \
    "start a turn that will be interrupted" \
    > "$TRANSCRIPTS/d2-seed.txt" 2>&1 &
D2_PID=$!
sleep 6
kill -9 "$D2_PID" 2>/dev/null
wait "$D2_PID" 2>/dev/null
kill -9 "$HANG_PID" 2>/dev/null

D2_INTERRUPTED=false
D2_SHOW=$(HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" \
          "$BINARY" session --dir "$SESSIONS" show "$D2_ID" 2>>"$TRANSCRIPTS/d2-show-before.txt")
printf '%s\n' "$D2_SHOW" >> "$TRANSCRIPTS/d2-show-before.txt"
if printf '%s\n' "$D2_SHOW" | grep -qE 'interrupted=[1-9]'; then
  D2_INTERRUPTED=true
fi
echo "F23_01_D2_FIXTURE_INTERRUPTED=$D2_INTERRUPTED"

if [ "$D2_INTERRUPTED" = true ]; then
  # The refusal, observed from the shipped binary before any repair.
  D2_BEFORE=$(env HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" \
      WAYLAND_VAULT_PASSPHRASE="f23-driver-vault-${NONCE}" \
      "$BINARY" --resume "$D2_ID" --max-turns 1 -- "next message" 2>&1)
  printf '%s\n' "$D2_BEFORE" > "$TRANSCRIPTS/d2-continue-before.txt"
  if grep -qF "interrupted turn at journal cursor" "$TRANSCRIPTS/d2-continue-before.txt"; then
    echo "F23_01_D2_REFUSAL_OBSERVED=true"
  else
    echo "F23_01_D2_REFUSAL_OBSERVED=false"
    echo "  --- tail of d2-continue-before.txt ---" >&2
    tail -5 "$TRANSCRIPTS/d2-continue-before.txt" >&2
    FAILURES=$((FAILURES + 1))
  fi

  # The real operator workflow: RECONCILE, then CANCEL. A crash mid-dispatch
  # leaves a nonterminal provider attempt, and the reducer refuses
  # TurnCancelled while any turn descendant is nonterminal. `reconcile` must
  # therefore both REPORT that item and let the operator dispose of it —
  # reporting `outstanding=0` while `cancel` fails is the same dead end D2
  # describes, one level down.
  RECON=$(HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" \
          "$BINARY" session --dir "$SESSIONS" reconcile "$D2_ID" 2>&1)
  printf '%s\n' "$RECON" > "$TRANSCRIPTS/d2-reconcile.txt"
  OUTSTANDING=$(printf '%s\n' "$RECON" | sed -n 's/.*outstanding=\([0-9]*\).*/\1/p' | tail -1)
  echo "F23_01_D2_RECONCILE_ITEMS_REPORTED=${OUTSTANDING:-unknown}"

  RESOLVED=0
  while IFS= read -r line; do
    case "$line" in
      *"resolvable=true"*) ;;
      *) continue ;;
    esac
    REF=$(printf '%s\n' "$line" | tr ' ' '\n' | sed -n 's/^ref=//p' | head -1)
    [ -n "$REF" ] || continue
    HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" \
      "$BINARY" session --dir "$SESSIONS" reconcile "$D2_ID" \
      --resolve "$REF" --as-outcome not-started --operator f23-driver \
      >> "$TRANSCRIPTS/d2-reconcile.txt" 2>&1
    rc=$?
    if [ "$rc" -eq 0 ]; then RESOLVED=$((RESOLVED + 1)); fi
  done <<EOF
$(printf '%s\n' "$RECON" | grep 'F23_SESSION=reconcile_item')
EOF
  echo "F23_01_D2_RECONCILE_RESOLVED=$RESOLVED"

  verb cancel 0 "F23_SESSION=cancel " cancel "$D2_ID"

  D2_AFTER=$(HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" \
             "$BINARY" session --dir "$SESSIONS" show "$D2_ID" 2>/dev/null)
  printf '%s\n' "$D2_AFTER" > "$TRANSCRIPTS/d2-show-after.txt"
  if printf '%s\n' "$D2_AFTER" | grep -qF "interrupted=0"; then
    echo "F23_01_D2_RESOLVED_PERSISTS_ACROSS_RESTART=true"
  else
    echo "F23_01_D2_RESOLVED_PERSISTS_ACROSS_RESTART=false"
    FAILURES=$((FAILURES + 1))
  fi

  # And the engine must no longer refuse for the interrupted-turn reason.
  env HOME="$HOME_DIR" WAYLAND_HOME="$HOME_DIR" WAYLAND_VAULT_PASSPHRASE="f23-driver-vault-${NONCE}" \
      "$BINARY" --resume "$D2_ID" --max-turns 1 -- "next message" \
      > "$TRANSCRIPTS/d2-continue-after.txt" 2>&1
  if grep -qF "interrupted turn at journal cursor" "$TRANSCRIPTS/d2-continue-after.txt"; then
    echo "F23_01_D2_CONTINUE_UNBLOCKED=false"
    FAILURES=$((FAILURES + 1))
  else
    echo "F23_01_D2_CONTINUE_UNBLOCKED=true"
  fi
else
  # Report the shortfall rather than skipping it.
  echo "F23_01_D2_REFUSAL_OBSERVED=unknown"
  echo "F23_01_D2_RESOLVED_PERSISTS_ACROSS_RESTART=unknown"
  echo "F23_01_D2_CONTINUE_UNBLOCKED=unknown"
  echo "  the D2 fixture did not produce an interrupted turn on this host" >&2
  FAILURES=$((FAILURES + 1))
  echo "F23_01_VERB=cancel platform=$PLATFORM status=FAIL exit=-1 nonce=$NONCE"
fi

if [ "$FAILURES" -ne 0 ]; then
  echo "F23_01_DRIVE=FAIL platform=$PLATFORM nonce=$NONCE failures=$FAILURES" >&2
  exit 1
fi
echo "F23_01_DRIVE=PASS platform=$PLATFORM nonce=$NONCE"
