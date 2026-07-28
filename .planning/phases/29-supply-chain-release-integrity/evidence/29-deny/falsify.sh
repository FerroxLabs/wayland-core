#!/usr/bin/env bash
set -u
export PATH=/root/.cargo/bin:$PATH
cd /root/wayland-29-deny || exit 99

BK=/root/deny-falsify-backup
rm -rf "$BK"; mkdir -p "$BK"
cp deny.toml "$BK/deny.toml"
cp crates/wcore-fixture-harness/Cargo.toml "$BK/fixture-Cargo.toml"

restore() {
  cp "$BK/deny.toml" deny.toml
  cp "$BK/fixture-Cargo.toml" crates/wcore-fixture-harness/Cargo.toml
}

run() {
  local tag="$1"
  cargo deny --manifest-path Cargo.toml check > "/root/falsify-$tag.txt" 2>&1
  local rc=$?
  local last errs
  last=$(tail -1 "/root/falsify-$tag.txt")
  errs=$(grep -c '^error\[' "/root/falsify-$tag.txt")
  echo "FALSIFY tag=$tag rc=$rc errors=$errs verdict=[$last]"
}

echo "===== F0 control: unmutated tree must be GREEN ====="
run F0-control

echo "===== F1: revert the license one-liner -> licenses MUST fail ====="
restore
grep -v '^license.workspace = true$' "$BK/fixture-Cargo.toml" > crates/wcore-fixture-harness/Cargo.toml
run F1-license-reverted

echo "===== F2: drop RUSTSEC-2025-0141 from ignore -> advisories MUST fail ====="
restore
grep -v 'id = "RUSTSEC-2025-0141"' "$BK/deny.toml" > deny.toml
run F2-bincode-unignored

echo "===== F3: drop RUSTSEC-2026-0192 from ignore -> advisories MUST fail ====="
restore
grep -v 'id = "RUSTSEC-2026-0192"' "$BK/deny.toml" > deny.toml
run F3-ttfparser-unignored

echo "===== F4: ban a crate that IS in the tree -> bans MUST fail ====="
restore
sed 's/^deny = \[\]$/deny = [{ name = "serde" }]/' "$BK/deny.toml" > deny.toml
run F4-bans-serde

echo "===== F5: empty allow-registry -> sources MUST fail ====="
restore
sed 's|^allow-registry = .*$|allow-registry = []|' "$BK/deny.toml" > deny.toml
run F5-sources-empty

echo "===== F6: remove MIT from allowlist -> licenses MUST fail ====="
restore
sed 's|^    "MIT",$||' "$BK/deny.toml" > deny.toml
run F6-license-no-mit

echo "===== restore + final control: must be GREEN again ====="
restore
run F7-restored-control
git status --short
