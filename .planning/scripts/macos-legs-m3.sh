#!/usr/bin/env bash
# 27-C2(c) — the three policy baselines, macOS leg.
#
# Run from the repo root. Writes one raw capture to
# .planning/evidence/macos-legs/raw-m3-baselines-macos.log
#
# Instrument liveness first (LANE-BRIEF §3b-i): `--list` on every binary BEFORE
# running it. A test file that is `#![cfg(target_os = "linux")]` compiles to an
# EMPTY harness on macOS and prints `0 passed; 0 failed` — which reads as a pass
# and is not one. The listed-count line below is what distinguishes the two.
set -u
cd "$(dirname "$0")/../.." || exit 2

B1=$(ls target/debug/deps/downloads_root_baseline_test-* 2>/dev/null | /usr/bin/grep -v '\.' | head -1)
B2=$(ls target/debug/deps/process_count_reaper_baseline_test-* 2>/dev/null | /usr/bin/grep -v '\.' | head -1)
B3=$(ls target/debug/deps/approval_gate_baseline_test-* 2>/dev/null | /usr/bin/grep -v '\.' | head -1)

echo "### 27-C2(c) three policy baselines — macOS leg"
echo "host:   $(hostname)"
echo "uname:  $(uname -a)"
echo "sw_vers: $(sw_vers -productVersion) build $(sw_vers -buildVersion)"
echo "HEAD:   $(/usr/bin/git rev-parse HEAD)"
echo "branch: $(/usr/bin/git rev-parse --abbrev-ref HEAD)"
echo "date:   $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "rustc:  $(rustc --version)"
echo

echo "=== INSTRUMENT LIVENESS — --list on each binary ==="
for b in "$B1" "$B2" "$B3"; do
  echo "--- $b"
  "$b" --list 2>&1
  n=$("$b" --list 2>/dev/null | /usr/bin/grep -c ': test$')
  echo "LISTED_TESTS=$n"
  echo
done

echo "=== BASELINE 1 — downloads-root confinement ==="
"$B1" --nocapture --test-threads=1 2>&1
echo "BASELINE1_RC=$?"
echo

echo "=== BASELINE 3 — browser process count + reaper ==="
"$B2" --nocapture --test-threads=1 2>&1
echo "BASELINE3_RC=$?"
echo

echo "=== BASELINE 2 — CUA approval gate (default features) ==="
"$B3" --nocapture --test-threads=1 2>&1
echo "BASELINE2_RC=$?"
echo
echo "DONE $(date -u +%Y-%m-%dT%H:%M:%SZ)"
