#!/bin/sh
# Both-directions prover for the durability-policy gates.
# For each mutation: apply -> run the three new tests -> record result -> restore.
# Every stage writes a marker so a stage that never ran cannot look like a pass.
set -u
cd /root/wayland-durable-posture || exit 9
export PATH=/root/.cargo/bin:$PATH
OUT=/root/wayland-durable-posture/dp-bothdir.log
: > "$OUT"

TESTS="requiring_durability_refuses_exactly_where_accepting_it_would_degrade an_untrusted_project_config_cannot_clear_a_global_durability_requirement the_durability_refusal_names_its_cause_and_all_three_remedies"

run_suite() {
  label="$1"
  echo "===== BEGIN $label =====" >> "$OUT"
  for t in $TESTS; do
    echo "--- $label :: $t ---" >> "$OUT"
    /root/.cargo/bin/cargo test -p wcore-config --lib -- --exact "config::tests::$t" >> "$OUT" 2>&1
    echo "RC[$label::$t]=$?" >> "$OUT"
  done
  echo "===== END $label =====" >> "$OUT"
}

run_suite BASELINE_UNMUTATED

for m in M1 M2 M3; do
  python3 dp-mutate.py "$m" >> "$OUT" 2>&1
  if ! /usr/bin/grep -q "MUTATION_${m}_APPLIED" "$OUT"; then
    echo "FATAL: mutation $m did not apply" >> "$OUT"
    /usr/bin/git checkout -- crates/wcore-config/src/config.rs
    continue
  fi
  run_suite "MUTATED_$m"
  /usr/bin/git checkout -- crates/wcore-config/src/config.rs
  echo "RESTORED_$m porcelain=[$(/usr/bin/git status --porcelain crates/wcore-config/src/config.rs)]" >> "$OUT"
done

run_suite RESTORED_FINAL
echo "WLDONE" >> "$OUT"
