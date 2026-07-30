#!/usr/bin/env bash
# effect-accounting / claim A — what happens to budget spend across a restart
# when durable sessions are OFF?
#
# Two arms, same binary, same config, same prompt, same number of launches:
#
#   on   WAYLAND_VAULT_PASSPHRASE supplied -> durable sessions stay ON.
#        This is the CONTROL. If the ceiling is not enforced here either, the
#        experiment is measuring a broken cap rather than a lost journal, and
#        the `off` arm proves nothing.
#   off  no keyring, no vault passphrase -> Config::resolve degrades durable
#        sessions OFF (commit d51287b1). This is the condition under test.
#
# The meter is the LOOPBACK PROVIDER'S OWN LOG, not wayland-core's stdout: one
# `BILLED` line per provider round-trip, each carrying the token usage the mock
# will report. Spend is therefore counted by the thing being billed, not by the
# thing under test.
#
# Exit status is never taken from a pipeline; each launch writes its own rc
# file immediately, and the script writes WLDONE last.

set -u

BIN=${BIN:?set BIN to the wayland-core binary}
OUT=${OUT:-/root/effacc-out}
PORT=${PORT:-8471}
LAUNCHES=${LAUNCHES:-5}
CAP_TOKENS_IN=${CAP_TOKENS_IN:-25000}
MOCK_TOKENS_IN=${MOCK_TOKENS_IN:-20000}
MOCK_TOKENS_OUT=${MOCK_TOKENS_OUT:-100}
SID=${SID:-aaaaaa-000001}
PROMPT='Reply with exactly: EFFECT_TURN_OK'

rm -rf "$OUT"; mkdir -p "$OUT"

export MOCK_LOG="$OUT/mock.log"
export MOCK_TOKENS_IN MOCK_TOKENS_OUT
export MOCK_REPLY=EFFECT_TURN_OK
python3 "$(dirname "$0")/mock_provider.py" "$PORT" >"$OUT/mock.stdout" 2>&1 &
MOCK_PID=$!
trap 'kill $MOCK_PID 2>/dev/null' EXIT
for i in $(seq 1 20); do
  if /usr/bin/grep -q "listening on" "$MOCK_LOG" 2>/dev/null; then echo "mock up (${i})"; break; fi
  sleep 0.3
done

billed_count() { /usr/bin/grep -c '^[0-9.]* BILLED ' "$MOCK_LOG" 2>/dev/null || echo 0; }

write_config() {   # $1 = home
  mkdir -p "$1"
  cat > "$1/config.toml" <<EOF
[default]
provider = "mock"

[providers.mock]
provider = "openai"
model = "mock-model"
api_key = "effect-accounting-lane-not-a-secret"
base_url = "http://127.0.0.1:${PORT}"

[providers.mock.compat]
include_usage_in_stream = false

# One axis under test, every other axis deliberately slack so a trip can only
# be the token-in ceiling.
[budget]
max_tokens_in = ${CAP_TOKENS_IN}
max_tokens_out = 100000000
max_cost_usd = 100000.0
max_wall_time_secs = 3600
EOF
}

# $1 = arm id, $2.. = extra env assignments for the launches
run_arm() {
  local arm="$1"; shift
  local home="$OUT/home-$arm"
  write_config "$home"

  local n
  for n in $(seq 1 "$LAUNCHES"); do
    local before; before=$(billed_count)
    local sess_args
    if [ "$n" -eq 1 ]; then sess_args="--session-id $SID"; else sess_args="--resume $SID"; fi

    env -u DBUS_SESSION_BUS_ADDRESS -u XDG_RUNTIME_DIR -u DISPLAY \
        -u WAYLAND_VAULT_PASSPHRASE -u WAYLAND_VAULT_PASSPHRASE_FD \
        -u ANTHROPIC_API_KEY -u OPENAI_API_KEY \
        HOME="$home" WAYLAND_HOME="$home" "$@" \
        timeout 120 "$BIN" --no-tui $sess_args "$PROMPT" \
        >"$OUT/$arm-L$n.stdout" 2>"$OUT/$arm-L$n.stderr"
    local rc=$?
    echo "$rc" > "$OUT/$arm-L$n.rc"

    local after; after=$(billed_count)
    echo "ARM=$arm LAUNCH=$n rc=$rc round_trips=$((after - before)) cumulative_round_trips=$after"
  done

  # What the host has to show for it afterwards.
  ls -1 "$home/sessions" 2>/dev/null > "$OUT/$arm-sessions.txt" || true
  ls -1 "$home/cache-ledger" 2>/dev/null > "$OUT/$arm-ledger.txt" || true
  find "$home" -maxdepth 2 -type d 2>/dev/null | sort > "$OUT/$arm-home-tree.txt" || true
  echo "ARM=$arm sessions_on_disk=$(wc -l < "$OUT/$arm-sessions.txt" 2>/dev/null || echo 0) \
ledger_files=$(wc -l < "$OUT/$arm-ledger.txt" 2>/dev/null || echo 0)"
}

echo "### config: cap max_tokens_in=$CAP_TOKENS_IN, mock reports ${MOCK_TOKENS_IN} in / ${MOCK_TOKENS_OUT} out per round-trip"
echo "### expected ceiling trip after $(( (CAP_TOKENS_IN / MOCK_TOKENS_IN) + 1 )) round-trips in the same session"

echo "### ARM on — durable sessions ON (vault passphrase supplied). CONTROL."
run_arm on WAYLAND_VAULT_PASSPHRASE=effacc-throwaway-not-a-secret

echo "### ARM off — headless degrade: no keyring, no vault passphrase."
run_arm off

echo "WLDONE"
