#!/usr/bin/env bash
# Headless-keyring lane: does the remedy named in the error text actually work?
#
# Condition under test: a headless host with NO OS keyring reachable. hetzner-dsm
# ships a running gnome-keyring-daemon + org.freedesktop.secrets on the session
# bus, so the condition is CREATED by stripping the session bus from the child's
# environment — which is exactly what a container / CI runner / minimal cloud VM
# looks like. Route R0 is the control that PROVES the condition is established:
# if R0 does not reproduce the keyring error, every other route is void.
#
# Exit status is never taken from a pipeline. Each run writes rc to its own file
# immediately after the command, and a DONE sentinel last.

set -u

BIN=${BIN:?set BIN to the wayland-core binary}
OUT=${OUT:-/root/hlkr-out}
PORT=${PORT:-8399}
PROMPT='Reply with exactly: HEADLESS_TURN_OK'

rm -rf "$OUT"; mkdir -p "$OUT"

# ---- loopback provider ------------------------------------------------------
export MOCK_LOG="$OUT/mock.log"
python3 "$(dirname "$0")/mock_provider.py" "$PORT" >"$OUT/mock.stdout" 2>&1 &
MOCK_PID=$!
trap 'kill $MOCK_PID 2>/dev/null' EXIT
for i in $(seq 1 20); do
  if grep -q "listening on" "$MOCK_LOG" 2>/dev/null; then echo "mock up (${i})"; break; fi
  sleep 0.3
done

# The config file name is `config.toml` inside WAYLAND_HOME, confirmed against the
# product itself: `wayland-core --config-path` prints `$WAYLAND_HOME/config.toml`.
# An earlier revision of this harness wrote `wcore.toml`; route R0 then failed with
# "No API key found" instead of the keyring error, i.e. the CONTROL DID NOT
# REPRODUCE, which is what caught the mistake. Keep R0 as the harness's own gate.
write_config() {           # $1 = home dir, $2 = extra toml
  mkdir -p "$1"
  cat > "$1/config.toml" <<EOF
[default]
provider = "mock"

[providers.mock]
provider = "openai"
model = "mock-model"
api_key = "headless-keyring-lane-not-a-secret"
base_url = "http://127.0.0.1:${PORT}"

[providers.mock.compat]
include_usage_in_stream = false
$2
EOF
}

# Run one route. $1 = id, $2 = extra toml, $3.. = extra env assignments.
run_route() {
  local id="$1"; shift
  local extra_toml="$1"; shift
  local home="$OUT/home-$id"
  write_config "$home" "$extra_toml"
  local mark; mark=$(wc -c < "$MOCK_LOG")

  # A genuinely keyring-free environment: no session bus, no runtime dir, no
  # display. `env -i` would also drop PATH/HOME, so strip explicitly instead.
  env -u DBUS_SESSION_BUS_ADDRESS -u XDG_RUNTIME_DIR -u DISPLAY \
      -u WAYLAND_VAULT_PASSPHRASE -u WAYLAND_VAULT_PASSPHRASE_FD \
      HOME="$home" WAYLAND_HOME="$home" "$@" \
      timeout 90 "$BIN" --no-tui "$PROMPT" \
      >"$OUT/$id.stdout" 2>"$OUT/$id.stderr"
  local rc=$?
  echo "$rc" > "$OUT/$id.rc"

  # Provider contact is measured from the mock's own log growth, NOT from the
  # product's stdout — the product is the thing under test.
  local now; now=$(wc -c < "$MOCK_LOG")
  if [ "$now" -gt "$mark" ]; then echo yes > "$OUT/$id.contact"; else echo no > "$OUT/$id.contact"; fi
  echo "ROUTE=$id rc=$rc contact=$(cat "$OUT/$id.contact") \
stdout_bytes=$(wc -c < "$OUT/$id.stdout") stderr_bytes=$(wc -c < "$OUT/$id.stderr")"
}

echo "### R0 control — no remedy applied at all"
run_route R0 ""

echo "### R1a literal reading of the error text: [credentials] backend, NO passphrase"
run_route R1a '
[credentials]
backend = "encrypted-file"'

echo "### R1b literal reading + passphrase via the env var (knowledge NOT in the text)"
run_route R1b '
[credentials]
backend = "encrypted-file"' WAYLAND_VAULT_PASSPHRASE=hlkr-throwaway-not-a-secret

echo "### R1c real config path [storage.credentials], NO passphrase"
run_route R1c '
[storage.credentials]
backend = "encrypted-file"'

echo "### R1d real config path + passphrase — the maximal-knowledge attempt"
run_route R1d '
[storage.credentials]
backend = "encrypted-file"' WAYLAND_VAULT_PASSPHRASE=hlkr-throwaway-not-a-secret

echo "### R2 the other advertised remedy: disable session persistence"
run_route R2 '
[session]
enabled = false'

echo "### R3 what does --help offer a headless operator?"
env -u DBUS_SESSION_BUS_ADDRESS -u XDG_RUNTIME_DIR -u DISPLAY \
  "$BIN" --help >"$OUT/help.txt" 2>&1
echo "help_rc=$? help_bytes=$(wc -c < "$OUT/help.txt")"

echo "WLDONE"
