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

# Round-trip meter. `grep -c` exits 1 on zero matches, so a naive
# `grep -c ... || echo 0` emits BOTH grep's `0` and the fallback `0` and the
# caller then evaluates `$(( "0\n0" - x ))`. That is exactly what the first run
# of this harness did: the arithmetic aborted the launch loop after L1 and the
# run looked like a product failure rather than a harness one. Count with `wc`
# instead — it always exits 0 and always emits one number.
billed_count() { /usr/bin/grep '^[0-9.]* BILLED ' "$MOCK_LOG" 2>/dev/null | /usr/bin/wc -l | tr -d ' '; }

# Self-test the meter before trusting it. Three assertions, because two would
# also pass on the broken version:
#   1. known-positive  — a log with BILLED lines counts them;
#   2. known-negative  — a log with none returns exactly `0`;
#   3. regression      — the returned value is usable in arithmetic, which is
#      the property the original defect destroyed (it returned "0\n0").
selftest_meter() {
  local keep="$MOCK_LOG" tmp; tmp=$(mktemp -d)
  MOCK_LOG="$tmp/empty.log"; : > "$MOCK_LOG"
  local zero; zero=$(billed_count)
  MOCK_LOG="$tmp/three.log"
  printf '1.0 listening on 1\n1.1 BILLED a\n1.2 BILLED b\n1.3 BILLED c\n' > "$MOCK_LOG"
  local three; three=$(billed_count)
  MOCK_LOG="$keep"
  local arith; arith=$(( three - zero )) || { echo "METER SELFTEST FAIL: not arithmetic-usable"; exit 1; }
  [ "$zero" = "0" ]  || { echo "METER SELFTEST FAIL: empty log returned '$zero'"; exit 1; }
  [ "$three" = "3" ] || { echo "METER SELFTEST FAIL: 3-line log returned '$three'"; exit 1; }
  [ "$arith" = "3" ] || { echo "METER SELFTEST FAIL: arithmetic gave '$arith'"; exit 1; }
  echo "METER SELFTEST OK: empty=$zero three=$three arith=$arith"
}
selftest_meter

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
#
# max_cost_usd is deliberately ABSENT. Setting it at all — even absurdly high —
# makes the product refuse every call outright on an unpriced model:
#   "pricing is unavailable for openai/mock-model, so the explicit or managed
#    USD cap cannot be enforced ... remove the explicit max_cost_usd to use
#    token-only governance."
# Measured on the first run of this harness (on-L1.stderr, 0 round-trips).
[budget]
max_tokens_in = ${CAP_TOKENS_IN}
max_tokens_out = 100000000
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
    # Both directions on the arm control: the degrade notice MUST appear in the
    # `off` arm and MUST NOT appear in the `on` arm. Without this the two arms
    # could silently be the same arm.
    local degraded=no
    /usr/bin/grep -q "durable session persistence is OFF" "$OUT/$arm-L$n.stderr" && degraded=yes
    local capped=no
    /usr/bin/grep -qi "budget cap\|budget-exceeded\|budget exceeded\|BudgetExceeded" \
      "$OUT/$arm-L$n.stderr" && capped=yes
    echo "ARM=$arm LAUNCH=$n rc=$rc round_trips=$((after - before)) cumulative_round_trips=$after degraded_notice=$degraded budget_refusal=$capped"
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
