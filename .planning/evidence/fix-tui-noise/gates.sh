#!/usr/bin/env bash
# gates.sh — hetzner gate runner.
#
# LANE-BRIEF §3.2: exit status is never trusted. Every gate writes
# `WL<NAME>=<code>` to a status file the caller reads back separately, and every
# test gate ALSO records the executed counts (`N passed`, `M ignored`,
# `K filtered out`) because a suite that runs zero tests exits 0 printing
# `test result: ok`. §3b: `rtk` strips `0 ignored` / `0 filtered out` from cargo
# output, so cargo is invoked by ABSOLUTE PATH and the counts are parsed from a
# file on this host, not from anything that crosses the ssh boundary rendered.
set -u
WT="${1:?worktree}"; OUT="${2:?outdir}"
CARGO=/root/.cargo/bin/cargo
mkdir -p "$OUT"; S="$OUT/gates.status"; : > "$S"
cd "$WT" || { echo "WLSETUP=94" >> "$S"; echo "WLDONE" >> "$S"; exit 94; }
export PATH=/root/.cargo/bin:$PATH

echo "WLHEAD=$(/usr/bin/git rev-parse HEAD)" >> "$S"

gate() { # gate <NAME> <logfile> <cmd...>
  local name="$1" log="$2"; shift 2
  "$@" > "$OUT/$log" 2>&1
  echo "WL${name}=$?" >> "$S"
}

gate FMT     fmt.log     "$CARGO" fmt --all -- --check
gate META    meta.log    "$CARGO" metadata --locked --format-version 1
gate CHECK   check.log   "$CARGO" check --workspace --all-targets
gate CLIPPYC clippy-cli.log   "$CARGO" clippy -p wcore-cli   --all-targets -- -D warnings
gate CLIPPYA clippy-agent.log "$CARGO" clippy -p wcore-agent --all-targets -- -D warnings

gate THELP   test-help.log  "$CARGO" test -p wcore-cli --test help_no_internal_ids
gate TAGENT  test-agent.log "$CARGO" test -p wcore-agent --lib output::
gate TCLI    test-cli.log   "$CARGO" test -p wcore-cli --lib

# ── executed-count readback (§3.2): exit 0 with zero tests run is the trap ────
#
# Harness defect #4, found on the first gate run and repaired here (§6b-ii).
# The naive `grep '^test result:'` produced a FALSE RED: `wcore-cli`'s
# `plugin::scaffold::tests::plugin_test_propagates_a_failing_suite` scaffolds a
# throwaway plugin crate containing `fn always_fails() { panic!("deliberate") }`
# and runs a NESTED `cargo test` on it to prove a failing suite propagates. That
# nested run's `test result: FAILED. 0 passed; 1 failed` lands in the same log,
# so the outer suite (1917 passed, 0 failed, rc=0) read as broken.
#
# The repair keys each result line to the `Running <target>` line above it and
# reports only targets built into THIS crate's deps directory, which is what
# separates an outer target from a nested fixture's. The exit code is recorded
# alongside, so the two can be cross-checked rather than either being trusted
# alone.
for pair in "THELP:test-help.log" "TAGENT:test-agent.log" "TCLI:test-cli.log"; do
  n="${pair%%:*}"; f="${pair##*:}"
  awk '
    /^ +Running / || /^ +Doc-tests / { own = ($0 ~ /target\/debug\/deps\//) ? 1 : 0; next }
    /^test result:/ { if (own) print }
  ' "$OUT/$f" > "$OUT/$f.own-results"
  echo "${n}_RESULT_LINES=$(wc -l < "$OUT/$f.own-results" | tr -d ' ')" >> "$S"
  echo "${n}_COUNTS=$(tr '\n' '|' < "$OUT/$f.own-results")" >> "$S"
  echo "${n}_ALL_RESULT_LINES_INCL_NESTED=$(/usr/bin/grep -c '^test result:' "$OUT/$f" || true)" >> "$S"
done

echo "WLDONE" >> "$S"
exit 0
