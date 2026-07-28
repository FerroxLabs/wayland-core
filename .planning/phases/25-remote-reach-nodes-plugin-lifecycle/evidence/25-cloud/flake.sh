#!/bin/bash
# Is registry::tests::a_recorded_task_is_readable_by_another_caller_and_removable
# a regression caused by lane/25-cloud, or a pre-existing race?
#
# Measured by running the SAME suite N times with this lane's cloud.rs, and N
# times with the MERGE-BASE cloud.rs, in the same worktree at the same commit
# for everything else. If the base flakes too, the lane did not cause it.
export PATH=/root/.cargo/bin:$PATH
cd /root/wayland-25-cloud || exit 1
F=crates/wcore-exec-backend/src/backends/cloud.rs
BASE=c743f3984a8e8642b2ac8b399664fc811992600b
N=12

cp "$F" /tmp/cloud.mine.rs
git show "$BASE:$F" > /tmp/cloud.base.rs || exit 1

count_fail() {
  local label="$1" fails=0 i out
  for i in $(seq 1 $N); do
    out=$(cargo test -p wcore-exec-backend --lib 2>&1 | grep -E '^test result')
    # capture the status of the TEST, not of grep, by inspecting the text
    if printf '%s' "$out" | grep -q 'FAILED'; then
      fails=$((fails+1))
    fi
  done
  echo "$label: $fails/$N runs had a FAILED lib result"
}

echo "=== WITH THE MERGE-BASE cloud.rs (this lane's change reverted) ==="
cp /tmp/cloud.base.rs "$F"
cargo build -p wcore-exec-backend --tests 2>&1 | tail -1
count_fail "MERGE-BASE cloud.rs"

echo
echo "=== WITH THIS LANE'S cloud.rs ==="
cp /tmp/cloud.mine.rs "$F"
cargo build -p wcore-exec-backend --tests 2>&1 | tail -1
count_fail "LANE cloud.rs"

echo
echo "=== single-threaded, this lane's cloud.rs (races cannot occur) ==="
cargo test -p wcore-exec-backend --lib -- --test-threads=1 2>&1 | grep -E '^test result'

echo
echo "=== restoration check ==="
git diff --exit-code -- "$F" >/dev/null 2>&1 && echo "RESTORED CLEAN (matches committed HEAD)" || echo "FILE DIRTY"
