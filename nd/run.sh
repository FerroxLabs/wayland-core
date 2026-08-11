#!/bin/bash
export PATH=/root/.cargo/bin:$PATH
cd /root/wt-libnondet
N=${1:-6}
EXTRA="$2"
for i in $(seq 1 $N); do
  cargo test -p wcore-agent --lib $EXTRA > /root/wt-libnondet/nd/run_$i.log 2>&1
  echo "run $i exit=$? $(grep -c "^test result" /root/wt-libnondet/nd/run_$i.log)"
done
