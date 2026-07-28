#!/usr/bin/env bash
# Falsification battery for the cargo-deny gate (lane 29-deny).
# Every mutation must FLIP the verdict green -> red. A mutation that leaves the
# verdict green proves the corresponding section cannot fail.
# Run AFTER `[graph] all-features = true` and the 5-entry ignore list landed.
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

echo "===== F2..F6: drop ONE ignore id at a time -> advisories MUST fail each time ====="
echo "      (proves no id is a passenger riding on another's suppression)"
for id in RUSTSEC-2025-0141 RUSTSEC-2026-0192 RUSTSEC-2024-0436 RUSTSEC-2025-0119 RUSTSEC-2025-0134; do
  restore
  grep -v "id = \"$id\"" "$BK/deny.toml" > deny.toml
  run "F2-unignored-$id"
done

echo "===== F7: ban a crate that IS in the tree -> bans MUST fail ====="
restore
sed 's/^deny = \[\]$/deny = [{ name = "serde" }]/' "$BK/deny.toml" > deny.toml
run F7-bans-serde

echo "===== F8: empty allow-registry -> sources MUST fail ====="
restore
sed 's|^allow-registry = .*$|allow-registry = []|' "$BK/deny.toml" > deny.toml
run F8-sources-empty

echo "===== F9: remove MIT from allowlist -> licenses MUST fail ====="
restore
sed 's|^    "MIT",$||' "$BK/deny.toml" > deny.toml
run F9-license-no-mit

echo "===== F10: revert all-features true -> false -> the 3 optional-feature ====="
echo "      advisories become UNSEEN, so the gate goes GREEN with 3 fewer checks."
echo "      This one is EXPECTED to stay green; it is the control that proves the"
echo "      widened graph is what admits them, and it is reported as such."
restore
sed 's|^all-features = true$|all-features = false|' "$BK/deny.toml" > deny.toml
run F10-narrow-graph-control

echo "===== restore + final control: must be GREEN again ====="
restore
run F11-restored-control
git status --short
