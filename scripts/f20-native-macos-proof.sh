#!/usr/bin/env bash
# f20-native-macos-proof.sh
#
# Final exact-candidate native macOS UAT helper for Phase 20. WRITTEN here in
# plan 20-08; it is NOT run on Sean's Mac and NOT run in this plan. It executes
# only on the externally provisioned ephemeral macOS runner during the
# Sean-authorized terminal UAT (plan 20-18), against the exact committed
# candidate.
#
# It fails closed unless: it is at the repository root, the checkout is clean,
# HEAD is the exact expected commit, the tree is the exact expected tree,
# WAYLAND_F20_NATIVE_ACCEPTANCE=1, the host is Darwin, and a live Docker Desktop
# daemon answers `docker info`. It then runs the eight exact native-acceptance
# selectors, emitting one candidate-bound target marker after each success and
# exactly one final platform acceptance marker at the end. The marker grammar is
# validated by scripts/f20-native-uat-proof.mjs.
#
# No Cargo runs on the local Mac (AGENTS.md): this script's cargo invocations
# only ever execute on the ephemeral macOS runner.

set -euo pipefail

EXPECTED_COMMIT=""
EXPECTED_TREE=""
NONCE=""

usage() {
    echo "usage: $0 --expected-commit <hex40|hex64> --expected-tree <hex40|hex64> --nonce <hex32-64>" >&2
    exit 2
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --expected-commit)
            EXPECTED_COMMIT="${2:-}"
            shift 2
            ;;
        --expected-tree)
            EXPECTED_TREE="${2:-}"
            shift 2
            ;;
        --nonce)
            NONCE="${2:-}"
            shift 2
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage
            ;;
    esac
done

# Strict lowercase 40- or 64-hex parsing; anything else fails closed.
hex_ok() {
    printf '%s' "$1" | grep -Eq '^[0-9a-f]{40}([0-9a-f]{24})?$'
}
nonce_ok() {
    printf '%s' "$1" | grep -Eq '^[0-9a-f]{32,64}$'
}

[ -n "$EXPECTED_COMMIT" ] || usage
[ -n "$EXPECTED_TREE" ] || usage
[ -n "$NONCE" ] || usage
hex_ok "$EXPECTED_COMMIT" || { echo "invalid --expected-commit (need lowercase 40/64 hex)" >&2; exit 1; }
hex_ok "$EXPECTED_TREE" || { echo "invalid --expected-tree (need lowercase 40/64 hex)" >&2; exit 1; }
nonce_ok "$NONCE" || { echo "invalid --nonce (need lowercase 32-64 hex)" >&2; exit 1; }

if [ "${WAYLAND_F20_NATIVE_ACCEPTANCE:-}" != "1" ]; then
    echo "native F20 acceptance requires WAYLAND_F20_NATIVE_ACCEPTANCE=1" >&2
    exit 1
fi

if [ "$(uname -s)" != "Darwin" ]; then
    echo "native macOS acceptance requires a Darwin host" >&2
    exit 1
fi

# Repository-root + exact-checkout gate.
repo_root="$(git rev-parse --show-toplevel)"
script_root="$(cd "$(dirname "$0")/.." && pwd -P)"
if [ "$(cd "$repo_root" && pwd -P)" != "$script_root" ]; then
    echo "wrong repository: expected $script_root, observed $repo_root" >&2
    exit 1
fi
if [ ! -f "$repo_root/crates/wcore-sandbox/Cargo.toml" ]; then
    echo "wrong repository: wcore-sandbox manifest is absent" >&2
    exit 1
fi

status="$(git status --porcelain=v1 --untracked-files=all)"
if [ -n "$status" ]; then
    echo "native F20 acceptance requires a clean checkout" >&2
    echo "$status" >&2
    exit 1
fi

actual_commit="$(git rev-parse HEAD)"
actual_tree="$(git rev-parse 'HEAD^{tree}')"
if [ "$actual_commit" != "$EXPECTED_COMMIT" ]; then
    echo "wrong commit: expected $EXPECTED_COMMIT, observed $actual_commit" >&2
    exit 1
fi
if [ "$actual_tree" != "$EXPECTED_TREE" ]; then
    echo "wrong tree: expected $EXPECTED_TREE, observed $actual_tree" >&2
    exit 1
fi

# Live Docker Desktop daemon required for the Docker transport/cancellation/budget targets.
if ! docker info >/dev/null 2>&1; then
    echo "native macOS acceptance requires a live Docker Desktop daemon (docker info)" >&2
    exit 1
fi

export WAYLAND_SANDBOX_LIVE_MACOS=1
export WAYLAND_SANDBOX_LIVE_DOCKER=1

emit_target_marker() {
    # $1 = target id
    echo "F20_NATIVE_TARGET=PASS platform=macos target=$1 commit=$EXPECTED_COMMIT tree=$EXPECTED_TREE nonce=$NONCE"
}

# Each exact target runs separately. `--run-ignored all` runs the native
# #[ignore]d acceptance tests; `--no-tests=fail` fails closed if a selector
# matches nothing. Ordered identically to MACOS_TARGETS in
# scripts/f20-native-uat-proof.mjs.
run_target() {
    local id="$1"
    shift
    if ! cargo nextest run --run-ignored all --no-tests=fail "$@" --nocapture; then
        echo "native macOS target $id failed" >&2
        exit 1
    fi
    emit_target_marker "$id"
}

run_target "macos-retained-directory"            -p wcore-sandbox --test live_integrity
run_target "macos-process-tree"                  -p wcore-sandbox --test hard_process_containment -E 'test(contained_detached_child_exit)'
run_target "macos-docker-reject-path-replacement" -p wcore-sandbox --test docker_smoke -E 'test(docker_rejects_allow_hosts_policy)'
run_target "macos-docker-roundtrip-delete"       -p wcore-sandbox --test docker_smoke -E 'test(docker_runs_hello_world)'
run_target "macos-public-dispatch"               -p wcore-swarm --test dispatch_smoke
run_target "macos-docker-cancellation"           -p wcore-sandbox --test docker_smoke -E 'test(docker_returns_enforced_resource_limits)'
run_target "macos-docker-budget"                 -p wcore-swarm --test worker_runtime_limits
run_target "macos-f20-lifecycle"                 -p wcore-agent --test transactional_delegated_mutation_test

# Exactly one final platform acceptance marker, only after all eight targets.
echo "F20_NATIVE_MACOS_ACCEPTANCE=PASS commit=$EXPECTED_COMMIT tree=$EXPECTED_TREE nonce=$NONCE"
