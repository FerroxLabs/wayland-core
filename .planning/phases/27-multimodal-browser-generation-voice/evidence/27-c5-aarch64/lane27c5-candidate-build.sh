#!/bin/bash
# Candidate aarch64-unknown-linux-gnu release build, matched to release.yml:136-142.
export PATH=/root/.cargo/bin:$PATH
export CARGO_BUILD_JOBS=10
export CARGO_TARGET_DIR=/root/wayland-27c5/target-27c5
SHA="$1"
export WAYLAND_BUILD_SOURCE_SHA="$SHA"
export CROSS_CONTAINER_OPTS="-e WAYLAND_BUILD_SOURCE_SHA=$SHA"

S=/root/wayland-27c5/lane27c5-build4-status.txt
L=/root/wayland-27c5/lane27c5-build4.log
rm -f "$S" "$L"

{
  echo "START $(date -u +%FT%TZ) SHA=$SHA"
  cd /root/wayland-27c5 || exit 9
  cross build --release --target aarch64-unknown-linux-gnu -p wcore-cli 2>&1 | tail -40
  rc=${PIPESTATUS[0]}
  echo "BUILD_RC=$rc"
  ls -la /root/wayland-27c5/target-27c5/aarch64-unknown-linux-gnu/release/wayland-core 2>&1
  printf 'WLRC=%s\nWLDONE\n' "$rc" > "$S"
} > "$L" 2>&1
