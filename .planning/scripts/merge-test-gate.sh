#!/usr/bin/env bash
# merge-test-gate.sh — the TEST half of the merge cadence.
#
# WHY THIS EXISTS
# ---------------
# The merge cadence gates on `cargo fmt --check` + `cargo metadata --locked` +
# `cargo check --workspace --all-targets`. **None of those runs a test.** So a
# merge that deliberately changes behaviour and thereby invalidates an existing
# test lands green. That is not hypothetical: `c73ac417` (the keyring fix) made a
# host with no secure store degrade and run the turn instead of erroring, and
# `crates/wcore-cli/tests/f14_sigkill_recovery.rs:1106`
# (`isolated_profile_without_secure_store_fails_before_turn_or_provider_intent`,
# written 2026-07-16) asserts the OPPOSITE. The merge never touched that file, so
# nothing flagged, and integration went red.
#
# WHY IT IS DIFFERENTIAL AND NOT "MUST BE ALL GREEN"
# --------------------------------------------------
# Integration is red on three tests RIGHT NOW (measured at `43c84493`, full
# workspace, `13562 tests run: 13559 passed, 3 failed, 72 skipped`). A gate that
# demands zero failures therefore has NO REACHABLE PASS STATE today, and a gate
# that cannot pass is worth as little as one that cannot fail
# (`LANE-BRIEF.md` §3b-iii). So the gate compares the failure SET against a
# committed baseline:
#
#   * a failure NOT in the baseline  -> RED. This is the keyring shape.
#   * a failure IN the baseline      -> tolerated, listed, and counted.
#   * a baseline entry that now PASSES -> RED, as a STALE-BASELINE error.
#
# That last rule is what stops the baseline rotting into a blanket suppression:
# the file can only ever shrink without a deliberate edit, so "add it to the
# baseline" is not a way to make a new failure go away quietly.
#
# COST, MEASURED — not estimated
# ------------------------------
# On `hetzner-dsm` (96 cores) at `43c84493`, `cargo nextest run --workspace
# --profile ci --no-fail-fast`:
#
#   * 216 s wall from a partially-warm tree = 137 s incremental compile + 77 s
#     test execution, for 13,562 tests across 592 binaries. Taken while the box
#     was LOADED (1-min load average 30.5, five lanes building) — this is the
#     pessimistic number, not the idle one.
#   * The cadence already runs `cargo check --workspace --all-targets`, which is
#     comparable compile work, so the marginal cost of the gate is dominated by
#     the ~77 s of execution.
#
# WHY NOT REVERSE-DEPENDENCY SELECTION
# ------------------------------------
# It was measured, and it works — but it buys almost nothing. The reverse-dep
# closure of the keyring diff's crates (`wcore-config`, `wcore-agent`) is
# **34 of 57 workspace crates (60%)**, and `wcore-cli` IS in it, so the rule would
# indeed have caught this incident. The problem is the shape of the saving: 60% of
# the workspace to save 40% of a 77-second run is ~30 seconds, in exchange for a
# selection rule that must be maintained and can be wrong. And the saving is
# INVERSELY correlated with risk — touching `wcore-types` or `wcore-config` (the
# changes most likely to break a distant test) selects nearly everything, while
# the changes it prunes hard are the isolated ones that were never going to break
# anything far away. Rejected on those grounds, with the numbers rather than a
# preference.
#
# A "fast smoke subset" was considered and discarded outright: to have caught this
# incident the subset would have had to contain a 2026-07-16 keyring-absence test
# in `wcore-cli`, chosen before anyone knew a keyring change was coming. You cannot
# pick that subset in advance. That is the honest answer, not a hedge.
#
# USAGE
#   merge-test-gate.sh                 run the gate in the current worktree
#   merge-test-gate.sh --self-test     prove the comparison logic both directions
#                                      (no cargo, runs in under a second)
#
# Exit status: 0 gate passed, 1 gate failed, 2 harness error.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BASELINE_DEFAULT="$REPO_ROOT/.planning/merge-test-baseline.txt"

# ---------------------------------------------------------------------------
# Failure-ID extraction from a nextest log. Separated out because it is a
# MATCHER, and matchers in this repo have a history of silently under-reporting.
# The first cut of this one did: it allowed no space inside the progress counter,
# so `FAIL [ 0.279s] ( 6501/13562) crate::suite test` came through with a
# leading `( 6501/13562)` still attached while the two unpadded siblings did not
# — one baseline entry that could never match, invisibly. Found by running it
# against a real 13,562-test log rather than against the sample in my head.
# Arms E1-E4 of the self-test pin it, and E2 is the exact line that broke it.
#
# nextest prints one line per attempt, so retries duplicate; `sort -u` collapses
# them. Reads a log on stdin, writes sorted unique "<binary> <test>" on stdout.
# ---------------------------------------------------------------------------
# `sed -E` (ERE), NOT BRE with `\?`. The BRE form is a GNU extension: it matched
# on hetzner and silently matched NOTHING under the Mac's BSD sed, so the gate
# would have reported "no failures" — a clean pass — on any host with BSD sed.
# A gate that self-passes on a whole platform is the failure mode this repo keeps
# hitting; ERE `?` is understood by both. Caught by running the self-test on the
# Mac after developing the matcher against a log on Linux.
extract_failures() {
  tr '\r' '\n' \
    | sed -nE 's/^ *(TRY [0-9]+ )?FAIL \[[^]]*\] *(\( *[0-9]+\/[0-9]+\) *)?//p' \
    | sed -E 's/[[:space:]]+/ /g; s/^ //; s/ $//' \
    | grep -v '^$' \
    | sort -u
}

# ---------------------------------------------------------------------------
# The comparison. Pure text in, verdict out, so it is testable without cargo.
#   $1 baseline file, $2 file of observed failing test IDs,
#   $3 OPTIONAL file of every test ID that the suite will collect and run.
# Echoes a report and returns 0 (pass) / 1 (fail).
#
# WHY $3 EXISTS — a soundness hole found on this gate's FIRST real cadence run
# -----------------------------------------------------------------------------
# Without it, a baseline entry leaves the observed-failure set for two completely
# different reasons that this function could not tell apart:
#
#   1. the test ran and PASSED            -> genuinely fixed, baseline is stale
#   2. the test NO LONGER EXISTS          -> deleted, renamed, or no longer collected
#
# Both looked like case 1, so both printed "STALE BASELINE ... PASSED". That is
# not merely a wrong message: it means DELETING A FAILING TEST SATISFIES THE GATE.
# The escape hatch this file's header explicitly refuses to leave open ("add it to
# the baseline is not a way to make a new failure go away quietly") was reachable
# by deletion instead of by suppression.
#
# Found for real, not hypothetically: on 2026-07-31 the gate reported
# `wcore-cli::f14_sigkill_recovery isolated_profile_without_secure_store_fails_before_turn_or_provider_intent`
# as "now PASSES". It does not pass — commit `368d4f5f` SPLIT it into
# `without_secure_store_an_operator_who_requires_durability_gets_a_refusal` and
# `without_secure_store_the_default_runs_degraded_and_leaves_nothing_durable`.
# The old name does not exist at any commit on the branch. The gate could not see
# the difference, and the difference is the whole point of the baseline.
#
# WHAT $3 DOES AND DOES NOT CATCH — stated exactly, because a half-true guarantee
# is how the rest of this repo's gates went quiet:
#   * CATCHES deleted, renamed, and feature/platform-gated-out entries: they are
#     absent from `cargo nextest list` for the profile that ran.
#   * CATCHES a test newly excluded by a profile default-filter.
#   * DOES NOT catch a test that still exists but was newly `#[ignore]`d, because
#     `nextest list` prints ignored tests by default. Verified on hetzner-dsm
#     rather than assumed. That residual is recorded, not papered over.
# When $3 is absent the old two-way behaviour is kept, so the pure-text self-test
# arms that predate this change still mean what they meant.
# ---------------------------------------------------------------------------
compare_failures() {
  local baseline="$1" observed="$2" existing="${3:-}" rc=0
  local b_clean o_clean
  b_clean="$(mktemp)"; o_clean="$(mktemp)"
  # Strip comments and blanks, sort, dedupe.
  grep -v '^[[:space:]]*#' "$baseline" 2>/dev/null | sed 's/[[:space:]]*$//' \
    | grep -v '^[[:space:]]*$' | sort -u > "$b_clean"
  sed 's/[[:space:]]*$//' "$observed" | grep -v '^[[:space:]]*$' | sort -u > "$o_clean"

  local new fixed
  new="$(comm -13 "$b_clean" "$o_clean")"
  fixed="$(comm -23 "$b_clean" "$o_clean")"

  # Split "left the failure set" into "passed" and "no longer exists". Only
  # possible when the caller supplied the collected-test set.
  local vanished=""
  if [ -n "$existing" ] && [ -f "$existing" ]; then
    local e_clean f_tmp
    e_clean="$(mktemp)"; f_tmp="$(mktemp)"
    sed 's/[[:space:]]*$//' "$existing" | grep -v '^[[:space:]]*$' | sort -u > "$e_clean"
    printf '%s\n' "$fixed" | grep -v '^$' | sort -u > "$f_tmp"
    vanished="$(comm -23 "$f_tmp" "$e_clean")"
    fixed="$(comm -12 "$f_tmp" "$e_clean")"
    rm -f "$e_clean" "$f_tmp"
  fi

  echo "baseline entries : $(grep -c . "$b_clean" || true)"
  echo "observed failures: $(grep -c . "$o_clean" || true)"

  # Indent with sed, NOT `printf '  %s\n' $list`. A test ID is "<binary> <test>"
  # and therefore contains a space, so the unquoted-expansion form word-splits it
  # and prints one ID across two lines — which is how the first real run of this
  # gate reported the keyring failure. Cosmetic in a terminal, not cosmetic if
  # anyone ever pipes this output into a matcher.
  if [ -n "$new" ]; then
    echo ""
    echo "GATE FAILED — NEW test failures not present in the baseline:"
    printf '%s\n' "$new" | sed 's/^/  /'
    rc=1
  fi
  if [ -n "$fixed" ]; then
    echo ""
    echo "GATE FAILED — STALE BASELINE. These are listed as known-failing but PASSED:"
    printf '%s\n' "$fixed" | sed 's/^/  /'
    echo "Remove them from the baseline. A baseline that only ever grows is a suppression list."
    rc=1
  fi
  if [ -n "$vanished" ]; then
    echo ""
    echo "GATE FAILED — BASELINE ENTRY NO LONGER EXISTS. Listed as known-failing, but the"
    echo "suite never collected it — deleted, renamed, or gated out. It did NOT pass:"
    printf '%s\n' "$vanished" | sed 's/^/  /'
    echo "Deleting a failing test is not fixing it. Either restore the test, or replace the"
    echo "baseline line with the successor test name and say in the comment what replaced it."
    rc=1
  fi
  [ "$rc" -eq 0 ] && echo "" && echo "GATE PASSED — no new failures, baseline exactly matched."
  rm -f "$b_clean" "$o_clean"
  return "$rc"
}

# ---------------------------------------------------------------------------
# Self-test. Six arms. Three of them are the ones that matter:
# known-positive passes, known-negative fails, AND a stale entry fails — that
# third is what proves the comparison is doing more than "is the list empty".
# ---------------------------------------------------------------------------
self_test() {
  # `total` is counted, not hardcoded. The literal "12/12" that used to be here
  # became a lie the moment arms were added, and a self-test that misreports its
  # own size is the wrong thing to be trusting.
  local tmp fails=0 total=0
  tmp="$(mktemp -d)"
  printf 'crate::suite a\ncrate::suite b\n' > "$tmp/base2"
  : > "$tmp/base0"

  arm() { # name expected_rc baseline observed [existing]
    local name="$1" want="$2"
    total=$((total + 1))
    shift 2
    local out got
    out="$(compare_failures "$1" "$2" "${3:-}" 2>&1)"; got=$?
    if [ "$got" -eq "$want" ]; then
      echo "  PASS  $name (rc=$got, expected $want)"
    else
      echo "  FAIL  $name (rc=$got, expected $want)"
      printf '%s\n' "$out" | sed 's/^/        /'
      fails=$((fails + 1))
    fi
  }

  echo "self-test:"
  # 1. Clean tree, empty baseline -> pass. The gate CAN pass.
  : > "$tmp/obs"; arm "empty baseline, no failures -> pass" 0 "$tmp/base0" "$tmp/obs"
  # 2. A brand-new failure against an empty baseline -> fail. The gate CAN fail.
  printf 'crate::suite z\n' > "$tmp/obs"; arm "new failure, empty baseline -> fail" 1 "$tmp/base0" "$tmp/obs"
  # 3. Exactly the baseline failures -> pass. Known reds are tolerated.
  printf 'crate::suite a\ncrate::suite b\n' > "$tmp/obs"; arm "observed == baseline -> pass" 0 "$tmp/base2" "$tmp/obs"
  # 4. Baseline PLUS one new -> fail. This is the keyring shape, and it is the arm
  #    a naive "any failure at all" check would get right for the wrong reason and
  #    a naive "count == baseline count" check would get WRONG.
  printf 'crate::suite a\ncrate::suite b\ncrate::suite z\n' > "$tmp/obs"; arm "baseline + 1 new -> fail" 1 "$tmp/base2" "$tmp/obs"
  # 5. A baseline entry now passing -> fail (stale baseline). Without this the
  #    baseline silently becomes a permanent suppression list.
  printf 'crate::suite a\n' > "$tmp/obs"; arm "baseline entry now passes -> fail (stale)" 1 "$tmp/base2" "$tmp/obs"
  # 6. Swap: same COUNT as the baseline but a different test. A count-based gate
  #    passes this; this one must not.
  printf 'crate::suite a\ncrate::suite z\n' > "$tmp/obs"; arm "same count, different test -> fail" 1 "$tmp/base2" "$tmp/obs"

  # --- vanished-entry arms. rc is 1 for BOTH "passed" and "no longer exists", so
  #     an rc-only arm cannot tell them apart and would pass on a gate that had
  #     merged the two cases back together. These assert the MESSAGE.
  marm() { # name want_substring reject_substring baseline observed existing
    local name="$1" want="$2" reject="$3" out
    total=$((total + 1))
    out="$(compare_failures "$4" "$5" "${6:-}" 2>&1)"
    if printf '%s' "$out" | grep -qF "$want" && ! printf '%s' "$out" | grep -qF "$reject"; then
      echo "  PASS  $name"
    else
      echo "  FAIL  $name"
      printf '%s\n' "$out" | sed 's/^/        /'
      fails=$((fails + 1))
    fi
  }
  # Collected-test set containing only 'a': so baseline entry 'b' does not exist.
  printf 'crate::suite a\n' > "$tmp/exists_a"
  printf 'crate::suite a\ncrate::suite b\n' > "$tmp/exists_ab"

  # 7. Baseline entry that the suite never collected -> the NEW error, and
  #    explicitly NOT the "PASSED" wording. This is the deletion escape hatch.
  printf 'crate::suite a\n' > "$tmp/obs"
  marm "baseline entry deleted -> 'NO LONGER EXISTS', not 'PASSED'" \
       "BASELINE ENTRY NO LONGER EXISTS" "but PASSED" \
       "$tmp/base2" "$tmp/obs" "$tmp/exists_a"
  # 8. Baseline entry that DOES exist and passed -> still the stale-baseline error,
  #    and NOT the vanished one. Proves the split discriminates rather than
  #    relabelling every case as "vanished".
  marm "baseline entry exists and passes -> 'PASSED', not 'NO LONGER EXISTS'" \
       "but PASSED" "BASELINE ENTRY NO LONGER EXISTS" \
       "$tmp/base2" "$tmp/obs" "$tmp/exists_ab"
  # 9. Back-compat: with NO collected-test set supplied, behaviour is the old
  #    two-way one, so every arm above that predates this change still means what
  #    it meant when it was written.
  marm "no existing-set supplied -> old two-way behaviour" \
       "but PASSED" "BASELINE ENTRY NO LONGER EXISTS" \
       "$tmp/base2" "$tmp/obs"
  # 10. A clean tree must still be able to PASS with the existing-set wired in.
  #     Without this the new rule could redden every run and nobody would notice
  #     until the gate was ignored wholesale.
  printf 'crate::suite a\ncrate::suite b\n' > "$tmp/obs"
  arm "observed == baseline, existing-set supplied -> pass" 0 "$tmp/base2" "$tmp/obs" "$tmp/exists_ab"

  # --- extraction arms. Verbatim lines from a real 13,562-test nextest log. ---
  earm() { # name expected_output_line  <<< input
    local name="$1" want="$2" input="$3" got
    total=$((total + 1))
    got="$(printf '%s\n' "$input" | extract_failures)"
    if [ "$got" = "$want" ]; then
      echo "  PASS  $name"
    else
      echo "  FAIL  $name"
      echo "        want: [$want]"
      echo "        got : [$got]"
      fails=$((fails + 1))
    fi
  }

  # E1 unpadded counter.
  earm "extract: unpadded counter" \
    "wcore-cli::proving_ground connect_all_env_keys_persists_across_relaunch" \
    "  TRY 3 FAIL [  10.080s] (13557/13562) wcore-cli::proving_ground connect_all_env_keys_persists_across_relaunch"
  # E2 SPACE-PADDED counter — the line the first version of the matcher mangled.
  #    Without this arm the repair is unverified and the self-test passes on the
  #    broken matcher too.
  earm "extract: space-padded counter (the line that broke v1)" \
    "wcore-cli::f14_sigkill_recovery isolated_profile_without_secure_store_fails_before_turn_or_provider_intent" \
    "  TRY 3 FAIL [   0.279s] ( 6501/13562) wcore-cli::f14_sigkill_recovery isolated_profile_without_secure_store_fails_before_turn_or_provider_intent"
  # E3 no TRY prefix and no counter at all.
  earm "extract: bare FAIL, no TRY, no counter" \
    "crate::suite some_test" \
    "        FAIL [   1.000s] crate::suite some_test"
  # E4 retries of ONE test must collapse to one entry, not three.
  earm "extract: three retries collapse to one" \
    "wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte" \
    "  TRY 1 FAIL [   0.1s] (   1/2) wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte
  TRY 2 FAIL [   0.1s] (   1/2) wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte
  TRY 3 FAIL [   0.143s] (10456/13562) wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte"
  # E5 KNOWN-NEGATIVE: a passing line must yield nothing. Without this the
  #     extractor could match everything and every arm above would still pass.
  earm "extract: a PASS line yields nothing" \
    "" \
    "        PASS [   0.010s] crate::suite some_test"

  # R1 REPORT FORMATTING. A test ID contains a space, and the first version of
  #    the reporter word-split it across two lines. Assert the ID survives on ONE
  #    line, because a report that mangles the identifier is a report nobody can
  #    grep or paste back into `nextest -E`.
  local rep spaced_id
  total=$((total + 1))
  spaced_id='wcore-cli::f14_sigkill_recovery isolated_profile_without_secure_store_fails_before_turn_or_provider_intent'
  printf '%s\n' "$spaced_id" > "$tmp/obs"
  rep="$(compare_failures "$tmp/base0" "$tmp/obs" 2>&1 | grep -c "^  $spaced_id\$")"
  if [ "$rep" -eq 1 ]; then
    echo "  PASS  report: a spaced test ID stays on one line"
  else
    echo "  FAIL  report: a spaced test ID stays on one line (matched $rep, expected 1)"
    fails=$((fails + 1))
  fi

  rm -rf "$tmp"
  if [ "$fails" -eq 0 ]; then
    echo "self-test: $total/$total arms correct"
    return 0
  fi
  echo "self-test: $fails of $total arm(s) WRONG"
  return 1
}

# ---------------------------------------------------------------------------
main() {
  local baseline="${MERGE_TEST_BASELINE:-$BASELINE_DEFAULT}"
  if [ ! -f "$baseline" ]; then
    echo "harness error: baseline '$baseline' not found" >&2
    return 2
  fi

  local log obs
  log="$(mktemp)"; obs="$(mktemp)"
  echo "running: cargo nextest run --workspace --profile ci --no-fail-fast"
  cargo nextest run --workspace --profile ci --no-fail-fast > "$log" 2>&1
  local nextest_rc=$?
  echo "nextest exit: $nextest_rc"

  # Anti-vacuity: a suite that ran ZERO tests exits in ways that can look fine,
  # and an empty failure set would then read as a clean pass. Read the executed
  # count back and refuse to grade a run that executed nothing.
  local summary ran
  summary="$(tr '\r' '\n' < "$log" | grep -E '^ *Summary' | tail -1)"
  ran="$(printf '%s' "$summary" | sed -n 's/.*\] *\([0-9][0-9]*\) tests run.*/\1/p')"
  echo "summary: ${summary:-<none>}"
  if [ -z "$ran" ] || [ "$ran" -lt 1 ]; then
    echo "harness error: nextest reported no executed test count; refusing to grade a run that may have executed nothing" >&2
    tail -40 "$log" >&2
    rm -f "$log" "$obs"
    return 2
  fi
  echo "tests executed: $ran"

  extract_failures < "$log" > "$obs"

  # The collected-test set, so a baseline entry that VANISHED is distinguishable
  # from one that was FIXED. See the long comment on compare_failures.
  #
  # It has to come from `nextest list`, NOT from the run log: `[profile.ci]` sets
  # `status-level = "fail"`, so the log contains no PASS lines at all and an
  # "executed set" derived from it would be empty — every baseline entry would
  # then read as vanished. Checked in .config/nextest.toml before writing this
  # rather than after it misfired.
  #
  # `list` emits the same "<crate>::<binary> <test>" ID shape the failure matcher
  # produces, so the two sets are directly comparable with no normalisation.
  local existing
  existing="$(mktemp)"
  cargo nextest list --workspace --profile ci > "$existing" 2>/dev/null

  # Anti-vacuity, and the reason this is a guard and not an assumption: if `list`
  # fails or prints nothing, the set is empty, EVERY baseline entry looks vanished,
  # and the gate reddens for a reason that has nothing to do with the merge. Worse,
  # a future edit that inverted the comparison would make an empty set pass
  # everything. Refuse to grade on a set that cannot be right — and require it to
  # be at least as large as the number of tests we just watched execute.
  local listed
  listed="$(grep -c . "$existing" 2>/dev/null || echo 0)"
  if [ "${listed:-0}" -lt "$ran" ]; then
    echo "harness error: 'cargo nextest list' returned $listed test IDs but the run executed $ran;" >&2
    echo "the collected-test set is unusable, so vanished-vs-fixed cannot be graded." >&2
    rm -f "$log" "$obs" "$existing"
    return 2
  fi
  echo "tests collected: $listed"

  compare_failures "$baseline" "$obs" "$existing"
  local rc=$?

  # Keep the nextest log when the gate reddens. The first time this gate failed
  # for real, it deleted the only copy of the output that said WHY -- the report
  # named the failing test and nothing else, so diagnosing it needed a second
  # 125-second full run. A gate that discards the evidence for its own verdict
  # makes every failure cost twice what it should.
  if [ "$rc" -ne 0 ]; then
    local keep="${MERGE_TEST_LOG_DIR:-${TMPDIR:-/tmp}}/merge-test-gate-failure.log"
    if cp "$log" "$keep" 2>/dev/null; then
      echo ""
      echo "full nextest output kept at: $keep"
    fi
  fi

  rm -f "$log" "$obs" "$existing"
  return $rc
}

case "${1:-}" in
  --self-test) self_test; exit $? ;;
  "")          main; exit $? ;;
  *)           echo "usage: $0 [--self-test]" >&2; exit 2 ;;
esac
