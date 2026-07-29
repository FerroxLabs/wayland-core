#!/usr/bin/env bash
# Drive the real binary over --json-stream and capture what a HOST would see.
#
# Design notes (each one is a trap this program has already paid for):
#  * stdout and stderr are captured to SEPARATE files. The defect under test is
#    precisely that the reason goes to stderr, so a 2>&1 merge would destroy it.
#  * exit status is written to a file immediately after the call, never read
#    through a pipeline (`${PIPESTATUS[0]}` after a pipe returns empty here).
#  * every case is byte-counted; emptiness is never inferred visually.
#  * P_OK is a POSITIVE CONTROL in the same run. Without it a refusal is
#    indistinguishable from a broken invocation -- the failure mode that
#    produced a false HIGH on this exact area previously.
#  * env -i so the ambient shell cannot leak a keyring/session var in and
#    change the condition under test.

set -u
BIN=${BIN:?set BIN to the wayland-core binary}
OUT=${OUT:-/tmp/jsr-out}
rm -rf "$OUT"; mkdir -p "$OUT"

# A dummy, syntactically-valid-looking key so Config::resolve SUCCEEDS and the
# run reaches the session/init stage. This is not a credential; it authenticates
# nothing. No real secret is read, written, or printed by this harness.
DUMMY_KEY="sk-ant-not-a-real-key-000000000000000000"

run_case() {
  local name=$1 session_enabled=$2 backend=$3
  local dir="$OUT/$name"
  mkdir -p "$dir/home" "$dir/fakehome" "$dir/proj"
  cat > "$dir/home/config.toml" <<EOF
[default]
provider = "anthropic"
model = "claude-sonnet-4-20250514"

[storage.credentials]
backend = "$backend"

[session]
enabled = $session_enabled
EOF
  env -i \
      PATH=/usr/bin:/bin \
      HOME="$dir/fakehome" \
      WAYLAND_HOME="$dir/home" \
      ANTHROPIC_API_KEY="$DUMMY_KEY" \
      TERM=dumb \
      "$BIN" --json-stream --project-dir "$dir/proj" \
      < /dev/null > "$dir/stdout.txt" 2> "$dir/stderr.txt"
  echo "$?" > "$dir/rc.txt"

  echo "=== CASE $name (session.enabled=$session_enabled backend=$backend)"
  echo "rc=$(cat "$dir/rc.txt")"
  echo "stdout_bytes=$(wc -c < "$dir/stdout.txt" | tr -d ' ')"
  echo "stderr_bytes=$(wc -c < "$dir/stderr.txt" | tr -d ' ')"
  python3 "$(dirname "$0")/framecount.py" tally "$dir/stdout.txt"
  echo "--- stderr (first 6 lines):"
  head -6 "$dir/stderr.txt"
  echo
}

echo "### instrument self-test (must pass before any case is believed)"
python3 "$(dirname "$0")/framecount.py" selftest || { echo "ABORT: instrument self-test failed"; exit 1; }
echo

# POSITIVE CONTROL: identical config except durable sessions are OFF, so
# init_session's confidential-storage check is never reached. MUST emit ready.
run_case P_OK false plaintext

# THE CASE UNDER TEST: single variable changed -- durable sessions ON.
run_case N_REFUSE true plaintext

echo "### harness gate (mirrors the R0 gate pattern):"
echo "### if P_OK shows no ready frame, the harness is broken and N_REFUSE is void."
