#!/usr/bin/env bash
# wayland-e2e-smoke.sh
#
# End-to-end smoke test for the DigitalOcean ephemeral-runner pipeline.
# Validates: DO API token, SSH key registration, cloud-init payload,
# deploy key clone path from inside a fresh droplet, and clean teardown.
#
# Run: scripts/wayland-e2e-smoke.sh
# Cost: ~$0.0001 per run (90-second droplet life).
# Safe: traps EXIT to always destroy the droplet, even on partial failure.

set -euo pipefail

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  cat <<'__HELP_END__'
wayland-e2e-smoke.sh — DO ephemeral-runner control-plane smoke test (~90s, ~$0.0001)

USAGE
  scripts/wayland-e2e-smoke.sh

VALIDATES
  · DO API token + droplet create/destroy
  · SSH key registered + private key on disk
  · cloud-init writes the deploy key correctly
  · git ls-remote via the deploy key from inside the droplet
  · trap-based cleanup always destroys the droplet

REQUIRES
  Env file at $WAYLAND_E2E_ENV (default ~/.config/wayland-e2e/do.env)
  See scripts/do.env.example for the template and docs/e2e.md for setup.

EXIT
  0   success (smoke passed)
  ≥1  failure (cleanup still ran — droplet always destroyed)
__HELP_END__
  exit 0
fi

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------
ENV_FILE="${WAYLAND_E2E_ENV:-$HOME/.config/wayland-e2e/do.env}"
DROPLET_NAME_PREFIX="wayland-e2e-smoke"
DROPLET_SIZE="s-1vcpu-1gb"
DROPLET_IMAGE="ubuntu-24-04-x64"
POLL_INTERVAL_SEC=5
POLL_MAX_ATTEMPTS=30
SSH_RETRY_COUNT=6
SSH_RETRY_WAIT=8

# ---------------------------------------------------------------------------
# Colors (only if stdout is a TTY)
# ---------------------------------------------------------------------------
if [ -t 1 ]; then
  RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; BLUE=$'\033[34m'; BOLD=$'\033[1m'; RESET=$'\033[0m'
else
  RED=""; GREEN=""; YELLOW=""; BLUE=""; BOLD=""; RESET=""
fi

ok()   { echo "${GREEN}✓${RESET} $*"; }
fail() { echo "${RED}✗${RESET} $*" >&2; }
note() { echo "${BLUE}·${RESET} $*"; }
phase(){ echo; echo "${BOLD}${YELLOW}═══ $* ═══${RESET}"; }

# ---------------------------------------------------------------------------
# Cleanup trap — always destroy droplet on exit
# ---------------------------------------------------------------------------
DROPLET_ID=""
TMPDIR_RUN=$(mktemp -d -t wayland-e2e-smoke-XXXXXX)

cleanup() {
  local rc=$?
  if [ -n "$DROPLET_ID" ]; then
    phase "CLEANUP: destroy droplet $DROPLET_ID"
    local http
    http=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE \
      -H "Authorization: Bearer $DIGITALOCEAN_API_TOKEN" \
      "https://api.digitalocean.com/v2/droplets/$DROPLET_ID")
    if [ "$http" = "204" ]; then
      ok "destroy returned HTTP 204"
    else
      fail "destroy returned HTTP $http (check DO dashboard manually)"
    fi
  fi
  rm -rf "$TMPDIR_RUN"
  if [ $rc -eq 0 ]; then
    echo; echo "${BOLD}${GREEN}SMOKE TEST: PASS${RESET}"
  else
    echo; echo "${BOLD}${RED}SMOKE TEST: FAIL (rc=$rc)${RESET}"
  fi
  exit $rc
}
trap cleanup EXIT INT TERM

# ---------------------------------------------------------------------------
# Phase 0: preflight
# ---------------------------------------------------------------------------
phase "PHASE 0 — preflight"

if [ ! -r "$ENV_FILE" ]; then
  fail "env file not found or unreadable: $ENV_FILE"
  echo "  set WAYLAND_E2E_ENV to override the path"
  exit 1
fi
note "env file: $ENV_FILE"

set -a
# shellcheck disable=SC1090,SC1091
source "$ENV_FILE"
set +a

for var in DIGITALOCEAN_API_TOKEN DO_SSH_KEY_ID DO_SSH_PRIVATE_KEY_PATH GITHUB_DEPLOY_KEY_PATH DO_REGION; do
  if [ -z "${!var:-}" ]; then
    fail "missing env var: $var (check $ENV_FILE)"
    exit 1
  fi
done
ok "all required env vars present"

if [ ! -r "$DO_SSH_PRIVATE_KEY_PATH" ]; then
  fail "DO SSH private key not readable: $DO_SSH_PRIVATE_KEY_PATH"
  exit 1
fi
ok "DO SSH private key readable"

if [ ! -r "$GITHUB_DEPLOY_KEY_PATH" ]; then
  fail "deploy key not readable: $GITHUB_DEPLOY_KEY_PATH"
  exit 1
fi
ok "deploy key readable"

# Verify token works
ACCT_JSON=$(curl -s -H "Authorization: Bearer $DIGITALOCEAN_API_TOKEN" \
  "https://api.digitalocean.com/v2/account")
if ! echo "$ACCT_JSON" | python3 -c "import json,sys; json.load(sys.stdin)['account']" >/dev/null 2>&1; then
  fail "DO API token rejected — got: $(echo "$ACCT_JSON" | head -c 200)"
  exit 1
fi
ACCT_EMAIL=$(echo "$ACCT_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['account']['email'])")
ok "DO API token valid (account: $ACCT_EMAIL)"

# ---------------------------------------------------------------------------
# Phase A: create droplet
# ---------------------------------------------------------------------------
phase "PHASE A — create droplet ($DROPLET_SIZE $DROPLET_IMAGE in $DO_REGION)"

DEPLOY_KEY_B64=$(base64 < "$GITHUB_DEPLOY_KEY_PATH")
DROPLET_NAME="${DROPLET_NAME_PREFIX}-$(date +%s)"

# Build cloud-init payload
CLOUD_INIT=$(cat <<EOF
#cloud-config
write_files:
  - path: /root/.ssh/deploy_wayland_core
    permissions: '0600'
    owner: root:root
    encoding: b64
    content: ${DEPLOY_KEY_B64}
  - path: /root/.ssh/config
    permissions: '0600'
    owner: root:root
    content: |
      Host github.com
        IdentityFile /root/.ssh/deploy_wayland_core
        IdentitiesOnly yes
        StrictHostKeyChecking accept-new
        UserKnownHostsFile /dev/null
runcmd:
  - [ touch, /var/log/cloud-init-finished ]
EOF
)

# Build JSON payload via python for safe escaping
PAYLOAD=$(DROPLET_NAME="$DROPLET_NAME" DO_REGION="$DO_REGION" \
  DROPLET_SIZE="$DROPLET_SIZE" DROPLET_IMAGE="$DROPLET_IMAGE" \
  DO_SSH_KEY_ID="$DO_SSH_KEY_ID" \
  python3 -c "
import json, sys, os
print(json.dumps({
  'name': os.environ['DROPLET_NAME'],
  'region': os.environ['DO_REGION'],
  'size': os.environ['DROPLET_SIZE'],
  'image': os.environ['DROPLET_IMAGE'],
  'ssh_keys': [int(os.environ['DO_SSH_KEY_ID'])],
  'tags': ['wayland-e2e', 'smoke-test'],
  'user_data': sys.stdin.read(),
  'monitoring': False,
  'ipv6': False
}))" <<< "$CLOUD_INIT")

CREATE_RESP=$(curl -s -X POST \
  -H "Authorization: Bearer $DIGITALOCEAN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d "$PAYLOAD" \
  "https://api.digitalocean.com/v2/droplets")

DROPLET_ID=$(echo "$CREATE_RESP" | python3 -c "
import json, sys
try:
  d = json.load(sys.stdin)
  print(d['droplet']['id'])
except (KeyError, json.JSONDecodeError) as e:
  print('', file=sys.stderr)
  sys.exit(1)
" 2>/dev/null || true)

if [ -z "$DROPLET_ID" ]; then
  fail "droplet creation failed — response:"
  echo "$CREATE_RESP" | head -c 500
  echo
  DROPLET_ID=""  # don't try to destroy a non-existent droplet
  exit 1
fi
ok "droplet created: id=$DROPLET_ID name=$DROPLET_NAME"

# ---------------------------------------------------------------------------
# Phase B: poll until active + IPv4
# ---------------------------------------------------------------------------
phase "PHASE B — poll for active status + public IPv4"

DROPLET_IP=""
for i in $(seq 1 $POLL_MAX_ATTEMPTS); do
  RESP=$(curl -s -H "Authorization: Bearer $DIGITALOCEAN_API_TOKEN" \
    "https://api.digitalocean.com/v2/droplets/$DROPLET_ID")
  STATUS=$(echo "$RESP" | python3 -c "import json,sys; print(json.load(sys.stdin)['droplet']['status'])")
  IP=$(echo "$RESP" | python3 -c "
import json, sys
d = json.load(sys.stdin)['droplet']
ips = [n for n in d.get('networks',{}).get('v4',[]) if n.get('type')=='public']
print(ips[0]['ip_address'] if ips else 'pending')
")
  note "[$i/$POLL_MAX_ATTEMPTS] status=$STATUS ip=$IP"
  if [ "$STATUS" = "active" ] && [ "$IP" != "pending" ]; then
    DROPLET_IP="$IP"
    break
  fi
  sleep $POLL_INTERVAL_SEC
done

if [ -z "$DROPLET_IP" ]; then
  fail "droplet never became active+IP'd within $((POLL_MAX_ATTEMPTS * POLL_INTERVAL_SEC))s"
  exit 1
fi
ok "active: $DROPLET_IP"

# ---------------------------------------------------------------------------
# Phase C: SSH + run validations
# ---------------------------------------------------------------------------
phase "PHASE C — SSH in and run validations"

SSH_ARGS=(
  -i "$DO_SSH_PRIVATE_KEY_PATH"
  -o IdentitiesOnly=yes
  -o StrictHostKeyChecking=accept-new
  -o UserKnownHostsFile="$TMPDIR_RUN/known_hosts"
  -o ConnectTimeout=10
  -o LogLevel=ERROR
)

# Retry loop for sshd readiness
SSH_READY=0
for i in $(seq 1 $SSH_RETRY_COUNT); do
  if ssh "${SSH_ARGS[@]}" "root@$DROPLET_IP" "echo CONNECTED" 2>/dev/null | grep -q CONNECTED; then
    ok "SSH ready on attempt $i"
    SSH_READY=1
    break
  fi
  note "[$i/$SSH_RETRY_COUNT] sshd not ready, waiting ${SSH_RETRY_WAIT}s..."
  sleep $SSH_RETRY_WAIT
done

if [ $SSH_READY -ne 1 ]; then
  fail "SSH never became ready within $((SSH_RETRY_COUNT * SSH_RETRY_WAIT))s"
  exit 1
fi

# Run all validations in one SSH session
ssh "${SSH_ARGS[@]}" "root@$DROPLET_IP" '
set -e
echo "--- whoami ---"
whoami
echo "--- uname ---"
uname -a
echo "--- cloud-init status (waiting up to 90s) ---"
cloud-init status --wait 2>&1 | tail -3
echo "--- /root/.ssh listing ---"
ls -la /root/.ssh/
echo "--- deploy key fingerprint ---"
ssh-keygen -lf /root/.ssh/deploy_wayland_core.pub 2>/dev/null \
  || ssh-keygen -yf /root/.ssh/deploy_wayland_core | ssh-keygen -lf - -E sha256
echo "--- git ls-remote via deploy key (proves end-to-end clone path) ---"
git ls-remote git@github.com:FerroxLabs/wayland-core.git HEAD refs/heads/main 2>&1
echo "--- SUCCESS ---"
'

ok "all in-droplet validations passed"

# Phase D handled by trap (cleanup runs always)
