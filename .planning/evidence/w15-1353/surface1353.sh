#!/usr/bin/env bash
# wayland#1353 c4 surface control: does the timed #1301 step actually publish
# effect checkpoints through store_effect_checkpoint? Counts hard-link syscalls
# whose target is inside a session `.effects` directory, per arm. Syscall counts
# only -- NOT a timing measurement (strace slows the run).
set -uo pipefail
dir=/root/w15-quota1353-c4
test_name=fix1_dispatch_budget_aborts_with_partial_result
for arm in A B; do
  trace="$dir/strace-$arm.txt"
  home="$dir/strace-home-$arm"
  rm -rf "$home"; mkdir -p "$home"
  (cd "$dir" && WAYLAND_HOME="$home" strace -f -qq -e trace=link,linkat -o "$trace" \
      "$dir/$arm.bin" --exact "$test_name" --test-threads=1 >"$dir/strace-$arm.log" 2>&1)
  rc=$?
  total=$(grep -cE 'link(at)?\(' "$trace")
  effects_ok=$(grep -E 'link(at)?\(' "$trace" | grep '\.effects/' | grep -cE '= 0$')
  effects_any=$(grep -E 'link(at)?\(' "$trace" | grep -c '\.effects/')
  result=$(grep -m1 '^test result:' "$dir/strace-$arm.log")
  echo "arm=$arm rc=$rc link_calls=$total effects_link_calls=$effects_any effects_link_ok=$effects_ok result=[$result]"
  grep -E 'link(at)?\(' "$trace" | grep '\.effects/' | head -1 | cut -c1-300
  rm -rf "$home" "$trace"
done
echo SURFACE DONE
