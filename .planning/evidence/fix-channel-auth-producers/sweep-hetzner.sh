#!/usr/bin/env bash
# Credential sweep on hetzner. Secrets on stdin; the sweeper is proved alive on
# a known-positive in the same run (LANE-BRIEF §3b-i: a broken grep returns a
# zero for free, and zero is exactly what a clean sweep looks like).
set -uo pipefail
VALS=()
while IFS= read -r line || [ -n "$line" ]; do
  val="${line#*=}"; val="${val%\"}"; val="${val#\"}"; val="${val%\'}"; val="${val#\'}"
  [ ${#val} -ge 12 ] && VALS+=("$val")
done

TARGETS="/tmp/lane-authprod-*.log /root/authprod-harness.sh /root/premise-check.sh /root/discord-base-sampler.sh"

echo "=== KNOWN-POSITIVE: sweeper alive? ==="
echo -n "grep for 'gateway' across targets -> "
grep -l 'gateway' $TARGETS 2>/dev/null | wc -l
echo "  (must be NON-zero, or every result below is meaningless)"

echo "=== gateway logs + harness scripts ==="
total=0
for v in "${VALS[@]}"; do
  n=$(grep -r -F -- "$v" $TARGETS 2>/dev/null | wc -l)
  echo "secret_len=${#v}  hits=$n"
  total=$((total+n))
done
echo "TOTAL_HITS=$total  (must be 0)"

echo "=== per-arm WAYLAND_HOME credential stores (expected: NON-zero; this is"
echo "    the product's own store, and it is what proves the arm was real) ==="
for v in "${VALS[@]}"; do
  n=$(grep -rl -F -- "$v" /root/wl-authprod-* 2>/dev/null | wc -l)
  echo "secret_len=${#v}  homes_containing=$n"
done
