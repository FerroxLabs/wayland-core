#!/usr/bin/env bash
# 22-remaining — live proof that the sub-agent learned-policy pre-filter is
# WIRED in the shipped binary, and that it narrows the CHILD and not the parent.
#
# One on-disk policy (`~/.wayland/permissions.toml`, DenyAlways on Read).
# One run. Two Read calls, differing in exactly one property: who is asking.
#
#   parent (CallActor::Root)      -> Read must NOT be denied by the policy
#   child  (CallActor::SubAgent)  -> Read MUST be denied by the policy
#
# Everything is read back out of the product's own JSON stream. Which provider
# actually served the turn is read back from the canned endpoint's own request
# log, because LANE-BRIEF section 3b-ii records that /root/.wayland/.env injects
# ANTHROPIC_API_KEY regardless of what the shell unsets.
set -u

BIN="${BIN:-/root/wayland-22-remaining/target/release/wayland-core}"
PORT="${PORT:-18734}"
ROOT="${ROOT:-/tmp/wl22p}"
OUT="${OUT:-$ROOT/out}"
# "1" = policy file present (positive), "0" = no policy file (control)
POLICY="${POLICY:-1}"
DELEGATE_ARM="${DELEGATE_ARM:-1}"

rm -rf "$ROOT"
mkdir -p "$ROOT/home/.config/wayland-core" "$ROOT/home/.wayland" "$OUT" "$ROOT/work"

# Real, readable files: a denial must be attributable to the policy, not to the
# file being missing. Both live under the session cwd so Read's path validation
# accepts them.
echo "parent probe content" > "$ROOT/work/parent-probe.txt"
echo "child probe content"  > "$ROOT/work/child-probe.txt"

cat > "$ROOT/home/.config/wayland-core/config.toml" <<'TOML'
[default]
provider = "canned"

[providers.canned]
provider = "openai"
model = "canned-model"
api_key = "sk-synthetic-not-a-secret-wl22p"
base_url = "http://127.0.0.1:18734"

[providers.canned.compat]
include_usage_in_stream = false
TOML

if [ "$POLICY" = "1" ]; then
  cat > "$ROOT/home/.wayland/permissions.toml" <<'TOML'
[[rules]]
tool = "Read"
arg_pattern = "*"
decision = "deny-always"
TOML
  echo "POLICY_FILE=present"
else
  echo "POLICY_FILE=absent"
fi

export CANNED_LOG="$OUT/canned-requests.log"
export CANNED_PORT="$PORT"
export CANNED_DELEGATE="$DELEGATE_ARM"
export PARENT_PATH="$ROOT/work/parent-probe.txt"
export CHILD_PATH="$ROOT/work/child-probe.txt"

python3 "$(dirname "$0")/canned_delegate.py" &
SRV=$!
trap 'kill $SRV 2>/dev/null' EXIT
sleep 1

POS=$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  "http://127.0.0.1:$PORT/v1/chat/completions" \
  -H 'Content-Type: application/json' -d '{"model":"probe","messages":[],"tools":[]}')
NEG=$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 -X POST \
  "http://127.0.0.1:$((PORT+1))/v1/chat/completions" \
  -H 'Content-Type: application/json' -d '{}' ; echo "rc=$?")
echo "PROBE_POSITIVE_HTTP=$POS"
echo "PROBE_NEGATIVE=$NEG"

cd "$ROOT/work" || exit 1
# Durable sessions stay ON: `Delegate` refuses without child session authority
# ("durable child session authority is not bound"), and the delegated child is
# the entire point of this proof. On a headless box with no OS keyring the
# product asks for an encrypted-vault passphrase; this is a SYNTHETIC literal
# for a throwaway vault under /tmp that protects nothing, in the same class as
# the synthetic `api_key` above. No real credential is involved.
HOME="$ROOT/home" XDG_CONFIG_HOME="$ROOT/home/.config" \
  WAYLAND_VAULT_PASSPHRASE="wl22p-synthetic-throwaway-not-a-secret" \
  timeout 240 "$BIN" --json-stream --auto-approve --no-tui \
  > "$OUT/stream.jsonl" 2> "$OUT/stream.err" <<'STDIN'
{"type":"message","msg_id":"wl22p-1","content":"Read the parent probe file, then delegate the child probe file to a sub-agent."}
STDIN
echo "PRODUCT_RC=$?"
echo "STREAM_BYTES=$(wc -c < "$OUT/stream.jsonl")"
