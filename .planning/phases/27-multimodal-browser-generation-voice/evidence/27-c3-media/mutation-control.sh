#!/usr/bin/env bash
# F27-C3 — prove the accounting gates can FAIL.
#
# LANE-BRIEF §3.2: "Gates must be able to fail. Before you trust any gate you
# write or run, ask whether it could fail." A green accounting suite proves
# nothing on its own — the failure this programme keeps finding is an
# observable that reports the same value regardless of what happened, and a
# test asserting that value would stay green forever.
#
# So each mutation below breaks ONE property the record claims, and the named
# test MUST turn red. A mutation that leaves the suite green is a test that was
# not measuring anything.
#
# Usage (on hetzner, inside the lane worktree):
#   bash mutation-control.sh
#
# Exit status: 0 only if EVERY mutation produced the expected red.

set -u

ROOT="${ROOT:-/root/wayland-27c3}"
CARGO="${CARGO:-/root/.cargo/bin/cargo}"
cd "$ROOT" || { echo "FATAL: $ROOT missing"; exit 2; }

MC="crates/wcore-tools/src/media_cost.rs"
IG="crates/wcore-agent/src/tool_backends/image_gen.rs"
BK="/tmp/f27c3-mutation-backup"
rm -rf "$BK"; mkdir -p "$BK/mc" "$BK/ig"
cp "$MC" "$BK/mc/"; cp "$IG" "$BK/ig/"

restore() { cp "$BK/mc/media_cost.rs" "$MC"; cp "$BK/ig/image_gen.rs" "$IG"; }
trap restore EXIT

FAILURES=0
RESULTS=""

# Run one cargo test filter and print the executed count. Never trusts exit
# status alone: a suite can exit 0 having run ZERO tests (LANE-BRIEF §3.2), so
# the executed count is read back and an unexpected zero is itself a failure.
run_case() {
  local pkg="$1" target="$2" filter="$3" want_n="$4" label="$5"
  local out rc line passed failed
  out="$($CARGO test -p "$pkg" $target "$filter" -- --exact 2>&1)"
  rc=$?
  line="$(printf '%s\n' "$out" | grep -E '^test result:' | tail -1)"
  passed="$(printf '%s\n' "$line" | sed -n 's/.*result: [a-zA-Z]*\. \([0-9]*\) passed.*/\1/p')"
  failed="$(printf '%s\n' "$line" | sed -n 's/.* \([0-9]*\) failed.*/\1/p')"
  passed="${passed:-0}"; failed="${failed:-0}"
  local n=$(( passed + failed ))
  if [ "$n" -ne "$want_n" ]; then
    echo "VACUOUS: $label ran $n tests, expected $want_n  [$line]"
    return 3
  fi
  echo "  $label: rc=$rc  $line"
  return "$rc"
}

check() {
  local label="$1" expect="$2" pkg="$3" target="$4" filter="$5" want_n="$6"
  echo "--- $label (expect: $expect)"
  run_case "$pkg" "$target" "$filter" "$want_n" "$label"
  local rc=$?
  if [ "$rc" -eq 3 ]; then
    RESULTS="${RESULTS}\nVACUOUS  $label"
    FAILURES=$((FAILURES+1)); return
  fi
  if [ "$expect" = "RED" ]; then
    if [ "$rc" -ne 0 ]; then
      RESULTS="${RESULTS}\nOK       $label -> RED as required"
    else
      RESULTS="${RESULTS}\nSELFPASS $label -> stayed GREEN under mutation"
      FAILURES=$((FAILURES+1))
    fi
  else
    if [ "$rc" -eq 0 ]; then
      RESULTS="${RESULTS}\nOK       $label -> GREEN as required"
    else
      RESULTS="${RESULTS}\nBROKEN   $label -> RED when it should be GREEN"
      FAILURES=$((FAILURES+1))
    fi
  fi
}

echo "=== BASELINE (unmutated) — every gate must be GREEN before any mutation"
echo "    A mutation control is meaningless if the gate was already red."
check "baseline/unit-variance" GREEN wcore-tools --lib \
  media_cost::tests::record_varies_with_the_work_done 1
check "baseline/e2e-variance" GREEN wcore-agent "--test f27_media_generation" \
  builtin_shape_record_varies_with_the_requested_work 1
check "baseline/e2e-header-cost" GREEN wcore-agent "--test f27_media_generation" \
  builtin_shape_reads_a_provider_reported_cost_from_the_response_header 1
check "baseline/e2e-unpriced" GREEN wcore-agent "--test f27_media_generation" \
  builtin_shape_records_units_and_reports_unpriced_when_provider_is_silent 1
check "baseline/e2e-failure-not-zero" GREEN wcore-agent "--test f27_media_generation" \
  builtin_shape_refusal_is_accounted_as_billing_unknown_not_zero 1

# --------------------------------------------------------------------------
# M1 — the record becomes INVARIANT: units are pinned regardless of the work.
#      This is precisely the defect the brief warns about ("reports the same
#      value regardless of what actually happened"). Both the unit-level and
#      the through-HTTP variance gates must catch it.
# --------------------------------------------------------------------------
restore
python3 - <<'PY'
p = "crates/wcore-tools/src/media_cost.rs"
s = open(p).read()
a = """    pub fn one_image(width: u32, height: u32) -> Self {
        Self {
            images: 1,
            width: Some(width),
            height: Some(height),
            billed_seconds: None,
        }
    }"""
b = """    pub fn one_image(_width: u32, _height: u32) -> Self {
        // MUTATION M1: pin the units so the record cannot vary.
        Self {
            images: 1,
            width: Some(1536),
            height: Some(1024),
            billed_seconds: None,
        }
    }"""
assert s.count(a) == 1, "M1 anchor not found"
open(p, "w").write(s.replace(a, b, 1))
PY
check "M1/unit-variance" RED wcore-tools --lib \
  media_cost::tests::record_varies_with_the_work_done 1
check "M1/e2e-variance" RED wcore-agent "--test f27_media_generation" \
  builtin_shape_record_varies_with_the_requested_work 1

# --------------------------------------------------------------------------
# M2 — the provider-reported cost channel goes dead. The unpriced tests would
#      STILL pass (nothing reports a price), which is the whole point: only the
#      known-positive control catches this.
# --------------------------------------------------------------------------
restore
python3 - <<'PY'
p = "crates/wcore-agent/src/tool_backends/image_gen.rs"
s = open(p).read()
a = "fn cost_from_headers(headers: &reqwest::header::HeaderMap) -> Option<ReportedCost> {"
b = ("fn cost_from_headers(headers: &reqwest::header::HeaderMap) -> Option<ReportedCost> {\n"
     "    // MUTATION M2: never read a provider-reported price.\n"
     "    if true {\n        let _ = headers;\n        return None;\n    }")
assert s.count(a) == 1, "M2 anchor not found"
open(p, "w").write(s.replace(a, b, 1))
PY
check "M2/e2e-header-cost" RED wcore-agent "--test f27_media_generation" \
  builtin_shape_reads_a_provider_reported_cost_from_the_response_header 1
# ...and the demonstration that the unpriced gate alone is NOT sufficient:
# under M2 it is still green, so it cannot be the only evidence.
check "M2/e2e-unpriced-still-green" GREEN wcore-agent "--test f27_media_generation" \
  builtin_shape_records_units_and_reports_unpriced_when_provider_is_silent 1

# --------------------------------------------------------------------------
# M3 — a failed call is silently recorded as free. This is the specific
#      dishonesty the record was written to prevent.
# --------------------------------------------------------------------------
restore
python3 - <<'PY'
p = "crates/wcore-tools/src/media_cost.rs"
s = open(p).read()
a = """            outcome: MediaOutcome::Failed {
                category: category.into(),
            },
            cost_usd: None,
            price_source: PriceSource::Unpriced {
                reason: UnpricedReason::CallFailedBillingUnknown,
            },"""
b = """            outcome: MediaOutcome::Failed {
                category: category.into(),
            },
            // MUTATION M3: pretend a failed call cost nothing.
            cost_usd: Some(0.0),
            price_source: PriceSource::Unpriced {
                reason: UnpricedReason::ProviderReportsNoCost,
            },"""
assert s.count(a) == 1, "M3 anchor not found"
open(p, "w").write(s.replace(a, b, 1))
PY
check "M3/unit-failure-not-zero" RED wcore-tools --lib \
  media_cost::tests::failure_is_unpriced_with_billing_unknown_not_zero 1
check "M3/e2e-failure-not-zero" RED wcore-agent "--test f27_media_generation" \
  builtin_shape_refusal_is_accounted_as_billing_unknown_not_zero 1

# --------------------------------------------------------------------------
# M4 — an operator's local estimate is mislabelled as the provider's own
#      figure. A host would render an invented number as authoritative.
# --------------------------------------------------------------------------
restore
python3 - <<'PY'
p = "crates/wcore-tools/src/media_cost.rs"
s = open(p).read()
a = """                Some((entry, usd_per_image)) => (
                    Some(usd_per_image * f64::from(units.images)),
                    PriceSource::LocalRateCard {
                        entry: entry.to_string(),
                    },
                ),"""
b = """                Some((entry, usd_per_image)) => (
                    Some(usd_per_image * f64::from(units.images)),
                    // MUTATION M4: launder a local estimate as provider truth.
                    PriceSource::ProviderHeader {
                        entry_unused_header: entry.to_string(),
                    },
                ),"""
assert s.count(a) == 1, "M4 anchor not found"
s = s.replace(a, b, 1).replace("entry_unused_header:", "header:")
open(p, "w").write(s)
PY
check "M4/unit-rate-card-label" RED wcore-tools --lib \
  media_cost::tests::rate_card_prices_the_same_call_that_was_otherwise_unpriced 1
check "M4/e2e-rate-card-label" RED wcore-agent "--test f27_media_generation" \
  builtin_shape_rate_card_prices_an_otherwise_unpriced_call 1

restore
echo
echo "=== RESULTS"
printf '%b\n' "$RESULTS"
echo
if [ "$FAILURES" -eq 0 ]; then
  echo "MUTATION_CONTROL=PASS  (every gate proved able to fail)"
  exit 0
fi
echo "MUTATION_CONTROL=FAIL  ($FAILURES unexpected outcome(s))"
exit 1
