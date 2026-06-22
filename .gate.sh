#!/usr/bin/env bash
# Hermetic Hetzner gate for the launch-0.13.0 integration branch.
# Syncs committed branch state to hetzner-dsm via thin git bundle (^origin/main),
# DETACHED checkout of the exact tip with a HEAD-match assertion (a failed sync can
# never silently pass), then runs fmt --check + clippy (-D warnings, --all-targets) +
# nextest, subtracting known flakes.
# Usage: .gate.sh -p wcore-providers [-p wcore-agent ...]   (no args => --workspace)
set -uo pipefail
WT="$(cd "$(dirname "$0")" && pwd)"
BRANCH=feat/issue-158-plan-tier
cd "$WT"

KNOWN_FLAKES="new_via_slash_invokes_run_new_and_aborts_on_empty_stdin|http_fetch_honors_per_call_timeout|draft_writes_md_and_json"

LOCAL_HEAD="$(git rev-parse HEAD)"
# Make sure Hetzner has current origin/main objects so the thin bundle applies.
ssh hetzner-dsm 'cd /root/wayland && git fetch -q origin main 2>&1 | tail -1' >/dev/null 2>&1
git bundle create /tmp/lb.bundle "$BRANCH" ^origin/main >/dev/null 2>&1
scp -q /tmp/lb.bundle hetzner-dsm:/tmp/lb.bundle
SYNCED_HEAD="$(ssh hetzner-dsm 'cd /root/wayland && git fetch -f /tmp/lb.bundle '"$BRANCH"' >/dev/null 2>&1 && git checkout -qf --detach FETCH_HEAD >/dev/null 2>&1 && git rev-parse HEAD')"
echo "LOCAL_HEAD=$LOCAL_HEAD"
echo "SYNCED_HEAD=$SYNCED_HEAD"
if [ "$LOCAL_HEAD" != "$SYNCED_HEAD" ]; then
  echo "!!! SYNC MISMATCH — Hetzner HEAD != local HEAD. ABORTING GATE (results would be stale). !!!"
  exit 2
fi
echo "SYNCED OK: $(ssh hetzner-dsm 'cd /root/wayland && git log --oneline -1')"

SCOPE="$*"; [ -z "$SCOPE" ] && SCOPE="--workspace"

echo "=== FMT CHECK (always workspace) ==="
ssh hetzner-dsm "bash -lc 'cd /root/wayland && cargo fmt --all -- --check > /tmp/fmt.log 2>&1'; echo FMT_EXIT=\$?"
ssh hetzner-dsm 'head -20 /tmp/fmt.log'

echo "=== CLIPPY ($SCOPE) ==="
ssh hetzner-dsm "bash -lc 'cd /root/wayland && cargo clippy $SCOPE --all-targets -- -D warnings > /tmp/clippy.log 2>&1'; echo CLIPPY_EXIT=\$?"
ssh hetzner-dsm 'tail -8 /tmp/clippy.log'

echo "=== NEXTEST ($SCOPE) ==="
ssh hetzner-dsm "bash -lc 'cd /root/wayland && RUSTC_WRAPPER= CARGO_BUILD_RUSTFLAGS= cargo nextest run $SCOPE --no-fail-fast > /tmp/nextest.log 2>&1'; echo NEXTEST_EXIT=\$?"
echo "--- summary + any failures ---"
ssh hetzner-dsm 'grep -E "Summary|^ *FAIL|TRY [0-9]+ FAIL" /tmp/nextest.log | tail -40 || true'
echo "--- failing test names (excluding known flakes) ---"
ssh hetzner-dsm "grep -oE 'FAIL \[[^]]*\] \([0-9/]+\) [^ ]+ [^ ]+' /tmp/nextest.log | awk '{print \$NF}' | sort -u" 2>/dev/null \
  | grep -vE "$KNOWN_FLAKES" || echo "(none — only known flakes or clean)"
echo "=== GATE DONE — SYNCED==LOCAL; FMT/CLIPPY/NEXTEST exits must be 0; no non-flake failures ==="
