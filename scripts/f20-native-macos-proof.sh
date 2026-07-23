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

# WAYLAND_SANDBOX_LIVE_MACOS is the live-macOS opt-in consumed by the two native
# sandbox acceptance tests (retained-directory + process-tree). It mirrors the
# WAYLAND_SANDBOX_LIVE_WINDOWS gate the Windows live-acceptance tests use: those
# tests self-qualify (rather than skip) only when the harness has set this.
export WAYLAND_SANDBOX_LIVE_MACOS=1
export WAYLAND_SANDBOX_LIVE_DOCKER=1

emit_target_marker() {
    # $1 = target id
    echo "F20_NATIVE_TARGET=PASS platform=macos target=$1 commit=$EXPECTED_COMMIT tree=$EXPECTED_TREE nonce=$NONCE"
}

# ---- Wrong-OS anti-drift guard (REQ-native-r9 / mirrors REQ-native-r8) -------
#
# The macOS counterpart of Assert-TargetOsGate in f20-native-windows-proof.ps1,
# reusing the same shared canonical expectation as MACOS_TARGET_SOURCES in
# scripts/f20-native-uat-proof.mjs. The systemic defect it closes is the exact
# 07-22 macOS failure: a macOS proof target silently mapped to a wrong-OS test
# (`macos-retained-directory` pointed at the Windows-only retained-handle test,
# which is `#![cfg(windows)]` -> 0 tests, so `--no-tests=fail` fired). Before a target
# runs, an OS-specific ('macos') target's selected test MUST resolve to a source
# file affirmatively cfg-gated for macOS and NOT gated for a foreign OS; a
# cross-platform ('any') target is exempt.
#
# Source resolution is filename-independent: it locates the file that DEFINES the
# selected test — either the `--test <binary>` file, or (when a target selects by
# name only) the single tests/*.rs that declares the `test(<fn>)` function. This
# is stronger than trusting a `--test` binary name and is why the retained-
# directory target can select its macOS test by its own function name.
assert_target_os_gate() {
    local id="$1"
    local os="$2"
    shift 2
    [ "$os" = "any" ] && return 0
    if [ "$os" != "macos" ]; then
        echo "anti-drift: target $id declares unknown os=$os" >&2
        exit 1
    fi

    local crate="" test_bin="" filter=""
    while [ "$#" -gt 0 ]; do
        case "$1" in
            -p) crate="${2:-}"; shift 2 ;;
            --test) test_bin="${2:-}"; shift 2 ;;
            -E) filter="${2:-}"; shift 2 ;;
            *) shift ;;
        esac
    done
    if [ -z "$crate" ]; then
        echo "anti-drift: macos target $id has no -p crate to resolve a test source" >&2
        exit 1
    fi

    local tests_dir="$repo_root/crates/$crate/tests"
    local source=""
    if [ -n "$test_bin" ]; then
        source="$tests_dir/$test_bin.rs"
        if [ ! -f "$source" ]; then
            echo "anti-drift: macos target $id selects a missing test source: $source" >&2
            exit 1
        fi
    else
        # Derive the selected test function from a `test(<fn>)` filter and locate
        # the single source file that defines it.
        local fn
        fn="$(printf '%s' "$filter" | sed -n 's/^test(\([A-Za-z0-9_]*\))$/\1/p')"
        if [ -z "$fn" ]; then
            echo "anti-drift: macos target $id has neither --test nor a test(<fn>) filter to resolve a source" >&2
            exit 1
        fi
        source="$(grep -rlE "fn[[:space:]]+${fn}[[:space:]]*\(" "$tests_dir" 2>/dev/null || true)"
        if [ -z "$source" ]; then
            echo "anti-drift: macos target $id selects a missing test function $fn under $tests_dir" >&2
            exit 1
        fi
        if [ "$(printf '%s\n' "$source" | grep -c .)" != "1" ]; then
            echo "anti-drift: macos target $id function $fn is defined in more than one source" >&2
            exit 1
        fi
    fi

    # Gate on cfg ATTRIBUTES (`#[cfg...]` / `#![cfg...]`) only, never on prose in
    # doc comments — the macOS test files legitimately mention `#![cfg(windows)]`
    # when documenting the cross-platform mirror, and a comment is not a
    # compilation gate.
    local cfg_attrs
    cfg_attrs="$(grep -E '^[[:space:]]*#!?\[' "$source" | grep -F 'cfg' || true)"

    # Positive gate (load-bearing): the source must be cfg-gated for macOS. A
    # wrong-OS or ungated test cannot prove macOS containment.
    if ! printf '%s\n' "$cfg_attrs" | grep -Eq 'target_os[[:space:]]*=[[:space:]]*"macos"'; then
        echo "anti-drift: macos target $id source is not cfg-gated for macos: $source" >&2
        exit 1
    fi
    # Negative gate: a source affirmatively cfg-gated to a DIFFERENT OS must never
    # back a macOS target. `cfg(unix)` is NOT foreign here — macOS is a unix.
    local other
    for other in linux windows android ios freebsd netbsd openbsd dragonfly; do
        if printf '%s\n' "$cfg_attrs" | grep -Eq "target_os[[:space:]]*=[[:space:]]*\"${other}\""; then
            echo "anti-drift: macos target $id source is cfg-gated for foreign os ${other}: $source" >&2
            exit 1
        fi
    done
    if printf '%s\n' "$cfg_attrs" | grep -Eq 'cfg\([[:space:]]*windows[[:space:]]*\)'; then
        echo "anti-drift: macos target $id source is windows-gated: $source" >&2
        exit 1
    fi
}

# Each exact target runs separately. `--run-ignored all` runs the native
# #[ignore]d acceptance tests; `--no-tests=fail` fails closed if a selector
# matches nothing. The second positional arg is the target's expected OS,
# consumed by the wrong-OS anti-drift guard before cargo runs. Ordered
# identically to MACOS_TARGETS in scripts/f20-native-uat-proof.mjs.
run_target() {
    local id="$1"
    local os="$2"
    shift 2
    assert_target_os_gate "$id" "$os" "$@"
    if ! cargo nextest run --run-ignored all --no-tests=fail "$@" --nocapture; then
        echo "native macOS target $id failed" >&2
        exit 1
    fi
    emit_target_marker "$id"
}

# Feature flags per target:
#   * wcore-sandbox targets (1,2,3,4,6) all pass the SAME `--features live-docker`
#     set. The Docker targets (3,4,6) require it because `tests/docker_smoke.rs`
#     is `#![cfg(feature = "live-docker")]` and compiles to zero tests otherwise;
#     targets 1,2 carry the identical flag so every `-p wcore-sandbox` invocation
#     shares one compiled artifact (no per-target recompile).
#   * wcore-swarm targets (5,7) run delegated Swarm dispatch. On macOS the primary
#     sandbox-exec backend is NOT a hard-containment backend (owns_descendants_hard
#     is false), so `select_delegated_backend` (crates/wcore-swarm/src/dispatch.rs)
#     rejects it and falls back to `DockerBackend::connect()`. That returns
#     `Err(DockerDisabled)` unless wcore-sandbox's `live-docker` feature is on, so
#     these targets MUST enable it via the dependency-qualified
#     `--features wcore-sandbox/live-docker`. The bare `--features live-docker`
#     name is what errors on `-p wcore-swarm` (swarm defines no own feature by
#     that name); the `dep/feature` form is valid and enables it on the dependency.
#   * The `-p wcore-agent` target (8) needs no Docker feature.
# macos-retained-directory selects its macOS test by function name (not by the
# `--test <binary>` file) so the harness never hardcodes the Windows-mirrored
# binary filename; the anti-drift guard resolves + macOS-gate-checks the file
# that defines the function.
run_target "macos-retained-directory"            macos -p wcore-sandbox --features live-docker -E 'test(required_live_macos_retained_directory_confines_writes)'
run_target "macos-process-tree"                  macos -p wcore-sandbox --features live-docker --test hard_process_containment_macos -E 'test(required_live_macos_process_tree_contains_descendants)'
run_target "macos-docker-reject-path-replacement" any -p wcore-sandbox --features live-docker --test docker_smoke -E 'test(docker_rejects_allow_hosts_policy)'
run_target "macos-docker-roundtrip-delete"       any -p wcore-sandbox --features live-docker --test docker_smoke -E 'test(docker_runs_hello_world)'
# macos-public-dispatch asserts the SECURITY-CRITICAL macOS admission decision:
# the sandbox-exec primary (no hard descendant containment) is REFUSED for public
# Swarm dispatch and the caller is told to select Docker — i.e. macOS never runs an
# unconfined delegated worker. The full container-execution path (Bash worker inside
# the Docker fallback) additionally needs the ghcr.io/tradecanyon/wcore-sandbox:base
# image, which is not published in this repo/registry (no Dockerfile here — stripped
# at public release). FOLLOW-UP: publish wcore-sandbox:base, then restore the full
# `dispatch_smoke::required_live_macos_public_dispatch_bash_confines_parent_and_descendants`
# execution assertion. Until then this proves the refusal, not the container run.
run_target "macos-public-dispatch"               any -p wcore-swarm --features wcore-sandbox/live-docker --lib -E 'test(sandbox_exec_is_refused_before_descendant_escape_can_spawn)'
run_target "macos-docker-cancellation"           any -p wcore-sandbox --features live-docker --test docker_smoke -E 'test(docker_returns_enforced_resource_limits)'
run_target "macos-docker-budget"                 any -p wcore-swarm --features wcore-sandbox/live-docker --test workspace_authority -E 'test(required_live_macos_docker_rejects_over_budget_result)'
run_target "macos-f20-lifecycle"                 any -p wcore-agent --test transactional_delegated_mutation_test

# Exactly one final platform acceptance marker, only after all eight targets.
echo "F20_NATIVE_MACOS_ACCEPTANCE=PASS commit=$EXPECTED_COMMIT tree=$EXPECTED_TREE nonce=$NONCE"
