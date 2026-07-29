#!/usr/bin/env bash
# 27-C2 A/B on ONE binary. Arm N = the box's natural dead state (no env
# manipulation of probe inputs). Arm R = a resolvable non-browser + a nominated
# but nonexistent display. One variable class differs.
BIN=/root/wayland-27bv/target/release/wayland-core
OUT=/root/wayland-27bv/evidence
mkdir -p "$OUT"

run_arm() {
  arm="$1"
  tmp=$(mktemp -d)
  (
    cd "$tmp" || exit 1
    unset BROWSERBASE_API_KEY BROWSERBASE_PROJECT_ID
    export HOME="$tmp" WAYLAND_HOME="$tmp"
    if [ "$arm" = "R" ]; then
      export WAYLAND_CAMOUFOX_BIN=/bin/true
      export DISPLAY=:99
    else
      unset WAYLAND_CAMOUFOX_BIN DISPLAY WAYLAND_DISPLAY
    fi
    printf '{"type":"stop"}\n' | timeout 60 "$BIN" --json-stream \
        --provider anthropic --api-key test-key-unused \
        > "$OUT/arm-$arm.jsonl" 2> "$OUT/arm-$arm.stderr"
    echo "arm $arm rc=$?"
  )
  rm -rf "$tmp"
}

run_arm N
run_arm R

for a in N R; do
  echo "=== ARM $a: bytes=$(wc -c < "$OUT/arm-$a.jsonl") lines=$(wc -l < "$OUT/arm-$a.jsonl") ==="
done
