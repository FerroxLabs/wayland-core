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

# invoke <name> <with_key:yes|no> [extra args...] -- config.toml must already exist
invoke() {
  local name=$1 with_key=$2; shift 2
  local dir="$OUT/$name"
  if [ "$with_key" = yes ]; then
    env -i PATH=/usr/bin:/bin HOME="$dir/fakehome" WAYLAND_HOME="$dir/home" \
        ANTHROPIC_API_KEY="$DUMMY_KEY" TERM=dumb \
        "$BIN" --json-stream --project-dir "$dir/proj" "$@" \
        < /dev/null > "$dir/stdout.txt" 2> "$dir/stderr.txt"
  else
    env -i PATH=/usr/bin:/bin HOME="$dir/fakehome" WAYLAND_HOME="$dir/home" \
        TERM=dumb \
        "$BIN" --json-stream --project-dir "$dir/proj" "$@" \
        < /dev/null > "$dir/stdout.txt" 2> "$dir/stderr.txt"
  fi
  echo "$?" > "$dir/rc.txt"
}

report() {
  local name=$1 label=$2
  local dir="$OUT/$name"
  echo "=== CASE $name ($label)"
  echo "rc=$(cat "$dir/rc.txt")"
  echo "stdout_bytes=$(wc -c < "$dir/stdout.txt" | tr -d ' ')"
  echo "stderr_bytes=$(wc -c < "$dir/stderr.txt" | tr -d ' ')"
  python3 "$(dirname "$0")/framecount.py" tally "$dir/stdout.txt"
  echo "--- stderr LAST line (the reason the host never sees):"
  tail -1 "$dir/stderr.txt"
  echo
}

scaffold() {
  local name=$1
  mkdir -p "$OUT/$name/home" "$OUT/$name/fakehome" "$OUT/$name/proj"
}

run_case() {
  local name=$1 session_enabled=$2 backend=$3
  local dir="$OUT/$name"
  scaffold "$name"
  cat > "$dir/home/config.toml" <<EOF
[default]
provider = "anthropic"
model = "claude-sonnet-4-20250514"

[storage.credentials]
backend = "$backend"

[session]
enabled = $session_enabled
EOF
  invoke "$name" yes
  report "$name" "session.enabled=$session_enabled backend=$backend"
}

echo "### instrument self-test (must pass before any case is believed)"
python3 "$(dirname "$0")/framecount.py" selftest || { echo "ABORT: instrument self-test failed"; exit 1; }
echo

# POSITIVE CONTROL: identical config except durable sessions are OFF, so
# init_session's confidential-storage check is never reached. MUST emit ready.
run_case P_OK false plaintext

# THE CASE UNDER TEST: single variable changed -- durable sessions ON.
run_case N_REFUSE true plaintext

# --- Other startup refusal doors, to establish coverage across the whole path ---

# D_PARSE: corrupt config.toml. Hits main.rs:1729-1733, which returns Err
# ABOVE the #186 emit at 1789 -- i.e. a gap inside an already-"fixed" path.
scaffold D_PARSE
printf '[default\nthis is not valid toml = = =\n' > "$OUT/D_PARSE/home/config.toml"
invoke D_PARSE yes
report D_PARSE "corrupt config.toml (ConfigLoadError::ParseFailed)"

# D_NOKEY: no credential at all. Hits the #186 emit at main.rs:1789 and is the
# ONE refusal expected to already emit pre-fix. If it does not, the #186 fix is
# itself ineffective (e.g. the frame is lost because nothing flushes the pump).
scaffold D_NOKEY
cat > "$OUT/D_NOKEY/home/config.toml" <<EOF
[default]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
EOF
invoke D_NOKEY no
report D_NOKEY "no API key (MissingApiKey -> the #186 emit site)"

# D_PROFILE: --profile signalled with no isolated home. Hits the fail-closed
# bail at main.rs:1651-1657, well before any emit site.
scaffold D_PROFILE
cat > "$OUT/D_PROFILE/home/config.toml" <<EOF
[default]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
EOF
env -i PATH=/usr/bin:/bin HOME="$OUT/D_PROFILE/fakehome" \
    ANTHROPIC_API_KEY="$DUMMY_KEY" TERM=dumb \
    "$BIN" --json-stream --profile work --project-dir "$OUT/D_PROFILE/proj" \
    < /dev/null > "$OUT/D_PROFILE/stdout.txt" 2> "$OUT/D_PROFILE/stderr.txt"
echo "$?" > "$OUT/D_PROFILE/rc.txt"
report D_PROFILE "--profile without WAYLAND_HOME (json_stream_profile_guard bail)"

echo "### harness gate (mirrors the R0 gate pattern):"
echo "### if P_OK shows no ready frame, the harness is broken and N_REFUSE is void."
