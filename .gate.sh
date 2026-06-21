#!/usr/bin/env bash
# Hermetic Hetzner gate for the token-spend-governance branch.
# Syncs the committed branch state to hetzner-dsm via git bundle, then runs
# clippy (-D warnings, --all-targets) + nextest for the given crates.
# Usage: .gate.sh -p wcore-agent -p wcore-providers
# No args => full workspace gate (slow; use before final).
set -euo pipefail
WT="$(cd "$(dirname "$0")" && pwd)"
BRANCH=feat/token-spend-governance
cd "$WT"

# Bundle only this branch's commits (origin/main is the prerequisite, present on Hetzner).
git bundle create /tmp/tb.bundle "$BRANCH" ^origin/main >/dev/null
scp -q /tmp/tb.bundle hetzner-dsm:/tmp/tb.bundle
ssh hetzner-dsm 'cd /root/wayland && git fetch -f /tmp/tb.bundle '"$BRANCH"':refs/heads/tb-work >/dev/null 2>&1 && git checkout -qf tb-work && echo "SYNCED: $(git log --oneline -1)"'

SCOPE="$*"
[ -z "$SCOPE" ] && SCOPE="--workspace"

echo "=== CLIPPY ($SCOPE) ==="
ssh hetzner-dsm "bash -lc 'cd /root/wayland && cargo clippy $SCOPE --all-targets -- -D warnings 2>&1 | tail -30'"
echo "=== NEXTEST ($SCOPE) ==="
# mold link-arg + RUSTC_WRAPPER poison nextest's metadata probe (see release-process notes) -> clear them.
ssh hetzner-dsm "bash -lc 'cd /root/wayland && RUSTC_WRAPPER= CARGO_BUILD_RUSTFLAGS= cargo nextest run $SCOPE 2>&1 | tail -30'"
echo "=== GATE DONE ==="
