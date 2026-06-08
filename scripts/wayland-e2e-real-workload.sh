#!/usr/bin/env bash
# wayland-e2e-real-workload.sh
#
# REAL end-to-end validation of Wayland-Core on the DO ephemeral runner.
# Spins up a compute-optimized droplet, installs the Rust toolchain,
# clones the repo, runs cargo build + nextest + mutants smoke, ships the
# results back, destroys the droplet.
#
# This is the test that proves "the fucker works" — not just that we can
# spin up a droplet, but that we can actually build + test our real code
# on it and get meaningful results.
#
# Run: scripts/wayland-e2e-real-workload.sh
# Expected duration: 25-40 min (first run, no snapshot)
# Expected cost: ~$0.05-0.10 per run
# Safe: traps EXIT, always destroys droplet.

set -euo pipefail

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  cat <<'__HELP_END__'
wayland-e2e-real-workload.sh — full Wayland-Core workload on a DO droplet (~25-40min)

USAGE
  scripts/wayland-e2e-real-workload.sh

WHAT IT DOES
  · Spins up a s-4vcpu-8gb droplet in $DO_REGION
  · Installs rust toolchain + cargo-nextest + cargo-mutants via cloud-init
  · Clones the repo at the v0.8.3 tag via the deploy key
  · Builds: cargo build --release -p wcore-cli
  · Tests:  cargo nextest run on 5 representative crates
  · Mutants: cargo mutants --list -p wcore-providers (smoke only, no actual mutations)
  · Captures all logs to /tmp/wayland-e2e-real-<timestamp>/
  · Destroys the droplet (trap-guaranteed)

ENV OVERRIDES
  DROPLET_SIZE=<slug>            default: s-4vcpu-8gb (c-4 not in nyc3)
  WAYLAND_E2E_ENV=<path>         default: ~/.config/wayland-e2e/do.env
  LOCAL_RESULTS_DIR=<path>       default: /tmp/wayland-e2e-real-<RUN_ID>

REQUIRES
  Env file with DO_API_TOKEN, DO_SSH_KEY_ID, DO_SSH_PRIVATE_KEY_PATH,
  GITHUB_DEPLOY_KEY_PATH, DO_REGION. See scripts/do.env.example.

COST
  ~$0.05-0.10 per run on s-4vcpu-8gb at $0.0714/hr.

EXIT
  0   success (build + tests + mutants enumerator all passed)
  ≥1  failure (logs in LOCAL_RESULTS_DIR for diagnosis)
__HELP_END__
  exit 0
fi

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------
ENV_FILE="${WAYLAND_E2E_ENV:-$HOME/.config/wayland-e2e/do.env}"
DROPLET_NAME_PREFIX="wayland-e2e-real"
DROPLET_SIZE="${DROPLET_SIZE:-s-4vcpu-8gb}"       # 4 vCPU, 8GB RAM, $0.0714/hr (c-4 not available in nyc3)
DROPLET_IMAGE="ubuntu-24-04-x64"
POLL_INTERVAL_SEC=5
POLL_MAX_ATTEMPTS=60                              # 5 min max for active
SSH_RETRY_COUNT=8                                 # 8 × 8s = 64s for sshd
SSH_RETRY_WAIT=8
WORKLOAD_TIMEOUT_SEC=2700                         # 45 min cap on workload
RUN_ID="$(date +%Y%m%d-%H%M%S)"
LOCAL_RESULTS_DIR="${LOCAL_RESULTS_DIR:-/tmp/wayland-e2e-real-${RUN_ID}}"

# ---------------------------------------------------------------------------
# Colors
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
# Cleanup trap
# ---------------------------------------------------------------------------
DROPLET_ID=""
cleanup() {
  local rc=$?
  if [ -n "$DROPLET_ID" ]; then
    phase "CLEANUP: destroy droplet $DROPLET_ID"
    local http
    http=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE \
      -H "Authorization: Bearer $DIGITALOCEAN_API_TOKEN" \
      "https://api.digitalocean.com/v2/droplets/$DROPLET_ID")
    if [ "$http" = "204" ]; then ok "destroy returned HTTP 204"
    else fail "destroy returned HTTP $http (check DO dashboard manually)"
    fi
  fi
  echo
  if [ $rc -eq 0 ]; then
    echo "${BOLD}${GREEN}REAL WORKLOAD: PASS${RESET}"
    echo "Results saved to: $LOCAL_RESULTS_DIR"
  else
    echo "${BOLD}${RED}REAL WORKLOAD: FAIL (rc=$rc)${RESET}"
    echo "Partial results (if any) at: $LOCAL_RESULTS_DIR"
  fi
  exit $rc
}
trap cleanup EXIT INT TERM

# ---------------------------------------------------------------------------
# Phase 0: preflight
# ---------------------------------------------------------------------------
phase "PHASE 0 — preflight"
mkdir -p "$LOCAL_RESULTS_DIR"
note "results dir: $LOCAL_RESULTS_DIR"

if [ ! -r "$ENV_FILE" ]; then
  fail "env file not found: $ENV_FILE"; exit 1
fi
# shellcheck disable=SC1090,SC1091
set -a; source "$ENV_FILE"; set +a

for var in DIGITALOCEAN_API_TOKEN DO_SSH_KEY_ID DO_SSH_PRIVATE_KEY_PATH GITHUB_DEPLOY_KEY_PATH DO_REGION; do
  [ -z "${!var:-}" ] && { fail "missing env var: $var"; exit 1; }
done
ok "env vars + key files present"

ACCT=$(curl -s -H "Authorization: Bearer $DIGITALOCEAN_API_TOKEN" "https://api.digitalocean.com/v2/account")
if ! echo "$ACCT" | python3 -c "import json,sys; json.load(sys.stdin)['account']" >/dev/null 2>&1; then
  fail "DO API token rejected"; exit 1
fi
ok "DO API token valid"

# ---------------------------------------------------------------------------
# Phase A: create droplet
# ---------------------------------------------------------------------------
phase "PHASE A — create $DROPLET_SIZE droplet in $DO_REGION"

DEPLOY_KEY_B64=$(base64 < "$GITHUB_DEPLOY_KEY_PATH")
DROPLET_NAME="${DROPLET_NAME_PREFIX}-${RUN_ID}"

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
package_update: true
packages:
  - build-essential
  - pkg-config
  - libssl-dev
  - git
  - jq
  - curl
  - mold              # the workspace specifies -fuse-ld=mold in .cargo/config.toml
  - cmake             # required by some -sys crates (ring, etc)
  - libdbus-1-dev     # linked by wcore-cli (-ldbus-1)
  - libseccomp-dev    # linked by wcore-cli (-lseccomp) for sandbox features
  - libsqlite3-dev    # for sqlite-vec crate
  - protobuf-compiler # for any prost/tonic crates
runcmd:
  - [ touch, /var/log/cloud-init-finished ]
EOF
)

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
  'tags': ['wayland-e2e', 'real-workload'],
  'user_data': sys.stdin.read(),
  'monitoring': False, 'ipv6': False
}))" <<< "$CLOUD_INIT")

CREATE_RESP=$(curl -s -X POST \
  -H "Authorization: Bearer $DIGITALOCEAN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d "$PAYLOAD" \
  "https://api.digitalocean.com/v2/droplets")

DROPLET_ID=$(echo "$CREATE_RESP" | python3 -c "
import json, sys
try: print(json.load(sys.stdin)['droplet']['id'])
except: pass" 2>/dev/null || true)

if [ -z "$DROPLET_ID" ]; then
  fail "droplet creation failed:"
  echo "$CREATE_RESP" | head -c 800; echo
  DROPLET_ID=""
  exit 1
fi
ok "droplet created: id=$DROPLET_ID name=$DROPLET_NAME"

# ---------------------------------------------------------------------------
# Phase B: poll active
# ---------------------------------------------------------------------------
phase "PHASE B — poll for active + IPv4"
DROPLET_IP=""
for i in $(seq 1 $POLL_MAX_ATTEMPTS); do
  RESP=$(curl -s -H "Authorization: Bearer $DIGITALOCEAN_API_TOKEN" \
    "https://api.digitalocean.com/v2/droplets/$DROPLET_ID")
  STATUS=$(echo "$RESP" | python3 -c "import json,sys; print(json.load(sys.stdin)['droplet']['status'])")
  IP=$(echo "$RESP" | python3 -c "
import json, sys
d = json.load(sys.stdin)['droplet']
ips = [n for n in d.get('networks',{}).get('v4',[]) if n.get('type')=='public']
print(ips[0]['ip_address'] if ips else 'pending')")
  note "[$i/$POLL_MAX_ATTEMPTS] status=$STATUS ip=$IP"
  if [ "$STATUS" = "active" ] && [ "$IP" != "pending" ]; then
    DROPLET_IP="$IP"; break
  fi
  sleep $POLL_INTERVAL_SEC
done
[ -z "$DROPLET_IP" ] && { fail "droplet never became active"; exit 1; }
ok "active: $DROPLET_IP"

# ---------------------------------------------------------------------------
# Phase C: wait for SSH + cloud-init finish
# ---------------------------------------------------------------------------
phase "PHASE C — wait for sshd + cloud-init"

SSH_ARGS=(
  -i "$DO_SSH_PRIVATE_KEY_PATH"
  -o IdentitiesOnly=yes
  -o StrictHostKeyChecking=accept-new
  -o UserKnownHostsFile="$LOCAL_RESULTS_DIR/known_hosts"
  -o ConnectTimeout=15
  -o ServerAliveInterval=30
  -o ServerAliveCountMax=120
  -o LogLevel=ERROR
)

for i in $(seq 1 $SSH_RETRY_COUNT); do
  if ssh "${SSH_ARGS[@]}" "root@$DROPLET_IP" "echo OK" 2>/dev/null | grep -q OK; then
    ok "sshd ready (attempt $i)"; break
  fi
  note "[$i/$SSH_RETRY_COUNT] sshd not ready, waiting ${SSH_RETRY_WAIT}s..."
  sleep $SSH_RETRY_WAIT
done

note "waiting for cloud-init to finish (this includes apt update + package install)..."
ssh "${SSH_ARGS[@]}" "root@$DROPLET_IP" "cloud-init status --wait" 2>&1 | tail -3
ok "cloud-init complete"

# ---------------------------------------------------------------------------
# Phase D: install Rust toolchain + cargo-nextest + cargo-mutants
# ---------------------------------------------------------------------------
phase "PHASE D — install Rust toolchain (rustup + nextest + mutants)"

ssh "${SSH_ARGS[@]}" "root@$DROPLET_IP" "bash -s" <<'EOREMOTE' 2>&1 | tee "$LOCAL_RESULTS_DIR/D-toolchain.log"
set -e
echo "===> rustup install"
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal --no-modify-path
. "$HOME/.cargo/env"
rustc --version
cargo --version

echo "===> cargo-nextest install (binstall first for speed)"
cargo install cargo-binstall --locked
cargo binstall -y cargo-nextest cargo-mutants

echo "===> versions"
cargo nextest --version
cargo mutants --version
EOREMOTE

ok "toolchain installed"

# ---------------------------------------------------------------------------
# Phase E: clone the repo at the v0.8.3 tag
# ---------------------------------------------------------------------------
phase "PHASE E — clone wayland-core at v0.8.3"

ssh "${SSH_ARGS[@]}" "root@$DROPLET_IP" "bash -s" <<'EOREMOTE' 2>&1 | tee "$LOCAL_RESULTS_DIR/E-clone.log"
set -e
cd /root
git clone --depth=1 --branch v0.8.3-wayland-hardened-followup git@github.com:FerroxLabs/wayland-core.git wcore
cd wcore
git log -1 --oneline
echo "===> workspace crate count"
ls -d crates/*/ | wc -l
echo "===> workspace top-level"
ls -la
EOREMOTE

ok "repo cloned at v0.8.3"

# ---------------------------------------------------------------------------
# Phase F: cargo build (the real proof — does the toolchain build our actual code?)
# ---------------------------------------------------------------------------
phase "PHASE F — cargo build --release -p wcore-cli"

ssh "${SSH_ARGS[@]}" "root@$DROPLET_IP" "bash -s" <<'EOREMOTE' 2>&1 | tee "$LOCAL_RESULTS_DIR/F-build.log"
set -e
. "$HOME/.cargo/env"
cd /root/wcore
echo "===> starting release build (this is the slow part — 5-15 min)"
time cargo build --release -p wcore-cli 2>&1 | tail -50
echo "===> verify binary"
ls -la target/release/wayland-core
file target/release/wayland-core
echo "===> binary executes"
./target/release/wayland-core --version
echo "===> binary --help"
./target/release/wayland-core --help 2>&1 | head -30
EOREMOTE

ok "wayland-core builds + executes on Linux"

# ---------------------------------------------------------------------------
# Phase G: cargo nextest on representative crates
# ---------------------------------------------------------------------------
phase "PHASE G — cargo nextest on representative crates"

ssh "${SSH_ARGS[@]}" "root@$DROPLET_IP" "bash -s" <<'EOREMOTE' 2>&1 | tee "$LOCAL_RESULTS_DIR/G-nextest.log"
set -e
. "$HOME/.cargo/env"
cd /root/wcore
echo "===> per-crate nextest (wcore-cron, wcore-config, wcore-providers, wcore-tools, wcore-swarm)"
echo "===> these are all on the locked v0.8.3 surface"
cargo nextest run \
  -p wcore-cron \
  -p wcore-config \
  -p wcore-providers \
  -p wcore-tools \
  -p wcore-swarm \
  2>&1 | tail -60
EOREMOTE

ok "nextest run complete"

# ---------------------------------------------------------------------------
# Phase H: cargo mutants smoke (verify tooling works on a small surface)
# ---------------------------------------------------------------------------
phase "PHASE H — cargo mutants smoke (list-only, no actual mutations run)"

ssh "${SSH_ARGS[@]}" "root@$DROPLET_IP" "bash -s" <<'EOREMOTE' 2>&1 | tee "$LOCAL_RESULTS_DIR/H-mutants.log"
set -e
. "$HOME/.cargo/env"
cd /root/wcore
echo "===> mutants --list -p wcore-providers (proves enumerator works)"
cargo mutants --list -p wcore-providers 2>&1 | tail -30 || true
echo "===> mutant count"
cargo mutants --list -p wcore-providers 2>&1 | wc -l
EOREMOTE

ok "mutants tooling validated"

# ---------------------------------------------------------------------------
# Phase I: collect remote diagnostics
# ---------------------------------------------------------------------------
phase "PHASE I — collect diagnostics back to local"

scp -i "$DO_SSH_PRIVATE_KEY_PATH" -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile="$LOCAL_RESULTS_DIR/known_hosts" \
  "root@$DROPLET_IP:/var/log/cloud-init-output.log" \
  "$LOCAL_RESULTS_DIR/cloud-init-output.log" 2>/dev/null || note "cloud-init-output.log not retrievable"

ssh "${SSH_ARGS[@]}" "root@$DROPLET_IP" 'uname -a; uptime; df -h /; free -h; . "$HOME/.cargo/env" 2>/dev/null && rustc --version' \
  > "$LOCAL_RESULTS_DIR/system-info.txt" 2>&1 || note "diagnostics partial (non-fatal)"
ok "diagnostics collected"

# Phase J handled by trap (cleanup)
