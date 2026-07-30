#!/usr/bin/env bash
# effect-accounting / claim A, second half.
#
# run-budget.sh measured the CONTINUED-session path and found the `off` arm
# refuses outright ("Session 'x' not found", rc=1) rather than re-arming a
# ceiling. This script measures the other path — the one a crash-looping
# daemon and a restarted channel conversation actually take: a FRESH session
# per process.
#
# Three arms, all with the same cap and the same meter as run-budget.sh:
#
#   on-fresh    durable sessions ON,  no --resume: 5 fresh sessions.
#   off-fresh   durable sessions OFF, no --resume: 5 fresh sessions.
#   poison      does a degraded run leave state that breaks the NEXT run that
#               does have a vault? run-budget.sh left an orphan
#               `<id>.journal` + `<id>.journal.writer.lock` in the `off` arm's
#               session directory with no session index entry. This arm reuses
#               ONE home: launch 1 degraded, launch 2 with the vault unlocked,
#               same --session-id. If launch 2 fails, the degrade poisons the
#               profile.

set -u

BIN=${BIN:?set BIN to the wayland-core binary}
OUT=${OUT:-/root/effacc-fresh}
PORT=${PORT:-8473}
LAUNCHES=${LAUNCHES:-5}
CAP_TOKENS_IN=${CAP_TOKENS_IN:-25000}
MOCK_TOKENS_IN=${MOCK_TOKENS_IN:-20000}
MOCK_TOKENS_OUT=${MOCK_TOKENS_OUT:-100}
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

billed_count() { /usr/bin/grep '^[0-9.]* BILLED ' "$MOCK_LOG" 2>/dev/null | /usr/bin/wc -l | tr -d ' '; }

selftest_meter() {
  local keep="$MOCK_LOG" tmp; tmp=$(mktemp -d)
  MOCK_LOG="$tmp/empty.log"; : > "$MOCK_LOG"; local zero; zero=$(billed_count)
  MOCK_LOG="$tmp/three.log"
  printf '1.0 listening on 1\n1.1 BILLED a\n1.2 BILLED b\n1.3 BILLED c\n' > "$MOCK_LOG"
  local three; three=$(billed_count)
  MOCK_LOG="$keep"
  local arith; arith=$(( three - zero ))
  [ "$zero" = "0" ] && [ "$three" = "3" ] && [ "$arith" = "3" ] \
    || { echo "METER SELFTEST FAIL zero=$zero three=$three arith=$arith"; exit 1; }
  echo "METER SELFTEST OK: empty=$zero three=$three arith=$arith"
}
selftest_meter

write_config() {
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

[budget]
max_tokens_in = ${CAP_TOKENS_IN}
max_tokens_out = 100000000
max_wall_time_secs = 3600
EOF
}

# $1 arm, $2 home, $3 label, $4.. extra env / args after `--`
launch() {                # launch <arm> <home> <n> <extra-args...> -- <env...>
  local arm="$1" home="$2" n="$3"; shift 3
  local args=(); while [ "$1" != "--" ]; do args+=("$1"); shift; done; shift
  local before; before=$(billed_count)
  env -u DBUS_SESSION_BUS_ADDRESS -u XDG_RUNTIME_DIR -u DISPLAY \
      -u WAYLAND_VAULT_PASSPHRASE -u WAYLAND_VAULT_PASSPHRASE_FD \
      -u ANTHROPIC_API_KEY -u OPENAI_API_KEY \
      HOME="$home" WAYLAND_HOME="$home" "$@" \
      timeout 120 "$BIN" --no-tui "${args[@]}" "$PROMPT" \
      >"$OUT/$arm-L$n.stdout" 2>"$OUT/$arm-L$n.stderr"
  local rc=$?
  echo "$rc" > "$OUT/$arm-L$n.rc"
  local after; after=$(billed_count)
  local degraded=no
  /usr/bin/grep -q "durable session persistence is OFF" "$OUT/$arm-L$n.stderr" && degraded=yes
  local capped=no
  /usr/bin/grep -qi "budget cap" "$OUT/$arm-L$n.stderr" && capped=yes
  echo "ARM=$arm LAUNCH=$n rc=$rc round_trips=$((after - before)) cumulative=$after degraded_notice=$degraded budget_refusal=$capped"
}

run_fresh_arm() {         # $1 arm, $2.. env
  local arm="$1"; shift
  local home="$OUT/home-$arm"; write_config "$home"
  # billed_count is CUMULATIVE over the whole script, because one mock serves
  # every arm. The first revision multiplied the cumulative count by the
  # per-round-trip token figure and reported arm two's spend as 200000 when it
  # was 100000. Take the arm's own delta.
  local arm_start; arm_start=$(billed_count)
  local n
  for n in $(seq 1 "$LAUNCHES"); do launch "$arm" "$home" "$n" -- "$@"; done
  local arm_end; arm_end=$(billed_count)
  ls -1 "$home/sessions" 2>/dev/null > "$OUT/$arm-sessions.txt" || true
  ls -1 "$home/cache-ledger" 2>/dev/null > "$OUT/$arm-ledger.txt" || true
  echo "ARM=$arm session_dir_entries=$(/usr/bin/wc -l < "$OUT/$arm-sessions.txt" | tr -d ' ') \
ledger_files=$(/usr/bin/wc -l < "$OUT/$arm-ledger.txt" | tr -d ' ') \
arm_round_trips=$(( arm_end - arm_start )) \
arm_billed_tokens_in=$(( (arm_end - arm_start) * MOCK_TOKENS_IN )) \
cap_tokens_in=$CAP_TOKENS_IN"
}

echo "### cap max_tokens_in=$CAP_TOKENS_IN; each round-trip bills ${MOCK_TOKENS_IN} input tokens"

echo "### ARM on-fresh — durable ON, a NEW session per process"
run_fresh_arm on-fresh WAYLAND_VAULT_PASSPHRASE=effacc-throwaway-not-a-secret

echo "### ARM off-fresh — degraded OFF, a NEW session per process"
run_fresh_arm off-fresh

echo "### ARM poison — degraded run first, then a vault-unlocked run in the SAME home and session id"
POISON_HOME="$OUT/home-poison"; write_config "$POISON_HOME"
launch poison "$POISON_HOME" 1 --session-id bbbbbb-000002 --
launch poison "$POISON_HOME" 2 --session-id bbbbbb-000002 -- WAYLAND_VAULT_PASSPHRASE=effacc-throwaway-not-a-secret
launch poison "$POISON_HOME" 3 --resume     bbbbbb-000002 -- WAYLAND_VAULT_PASSPHRASE=effacc-throwaway-not-a-secret
ls -1 "$POISON_HOME/sessions" 2>/dev/null > "$OUT/poison-sessions.txt" || true
echo "ARM=poison session_dir_entries=$(/usr/bin/wc -l < "$OUT/poison-sessions.txt" | tr -d ' ')"

echo "WLDONE"
