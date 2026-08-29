#!/bin/bash -l
cd /root/w-f13/n-win-escape || exit 1
L=/root/w-f13/n-win-escape/GATE.log
: > $L
echo "== fmt ==" >> $L
cargo fmt --all --check >> $L 2>&1; echo "fmt EXIT=$?" >> $L
echo "== check ==" >> $L
cargo check --workspace --all-targets --all-features --locked >> $L 2>&1; echo "check EXIT=$?" >> $L
echo "== clippy ==" >> $L
cargo clippy --workspace --all-targets --all-features -- -D warnings >> $L 2>&1; echo "clippy EXIT=$?" >> $L
echo "== clippy windows-gnu ==" >> $L
cargo clippy -p wcore-cli --all-targets --target x86_64-pc-windows-gnu -- -D warnings >> $L 2>&1; echo "clippy-win EXIT=$?" >> $L
echo "== nextest wcore-cli ==" >> $L
cargo nextest run -p wcore-cli --profile ci >> $L 2>&1; echo "nextest EXIT=$?" >> $L
echo "GATE DONE" >> $L
