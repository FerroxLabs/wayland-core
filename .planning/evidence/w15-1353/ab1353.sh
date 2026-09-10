#!/usr/bin/env bash
# wayland#1353 c4: interleaved A/B of the #1301 isolated step on hetzner.
# Usage: ab1353.sh DIR N   (DIR holds A.bin and B.bin, release workflow_limits_test)
set -uo pipefail
dir=$1; n=$2
test_name=fix1_dispatch_budget_aborts_with_partial_result
out="$dir/samples.csv"
meta="$dir/meta.txt"
{
  echo "host=$(hostname) kernel=$(uname -r) nproc=$(nproc) started=$(date -u +%FT%TZ)"
  sha256sum "$dir/A.bin" "$dir/B.bin"
} >"$meta"
echo "round,arm,seconds,exit,load1,load5,load15,result" >"$out"
run_arm() {
  local round=$1 arm=$2
  local home="$dir/home-$arm"
  rm -rf "$home"; mkdir -p "$home"
  read -r l1 l5 l15 _ </proc/loadavg
  local start end rc result
  start=$(date +%s.%N)
  (cd "$dir" && WAYLAND_HOME="$home" "$dir/$arm.bin" --exact "$test_name" --test-threads=1 \
      >"$dir/last-$arm.log" 2>&1)
  rc=$?
  end=$(date +%s.%N)
  result=$(grep -m1 '^test result:' "$dir/last-$arm.log" | tr ',' ';')
  echo "$round,$arm,$(echo "$end - $start" | bc),$rc,$l1,$l5,$l15,$result" >>"$out"
}
for round in $(seq 1 "$n"); do
  if [ $((round % 2)) -eq 1 ]; then run_arm "$round" A; run_arm "$round" B
  else run_arm "$round" B; run_arm "$round" A; fi
done
rm -rf "$dir/home-A" "$dir/home-B"
echo "finished=$(date -u +%FT%TZ)" >>"$meta"
echo DONE >>"$meta"
