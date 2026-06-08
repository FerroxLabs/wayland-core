#!/usr/bin/env bash
# smoke.sh — wayland-core P0 SMOKE GATE (pre-release).
#
# Runs the live P0 smoke suite from
# .planning/audit/UX-AUDIT-AND-TEST-PLAN.md §4 — the ordered checks a new user
# hits in the first ten minutes. A release is BLOCKED if any HARD-GATE check
# fails. The original SMOKE §4 rule named 6, 7, 8, 9, 11, 14, 15, 17, 18, 19 as
# the aspirational gate; only the checks that EXIST as tests are enforced here.
# Implemented + enforced today: 6, 10, 17, 24. Interactive-pending (reported,
# not enforced): 15, 22, 23. Uncovered TODO (NOT claimed as covered, pending
# their wave): 7, 8, 9, 11, 14, 18, 19. See the table in smoke_p0.rs's module
# doc for the per-check status.
#
# The checks live as Rust integration tests in
#   crates/wcore-cli/tests/smoke_p0.rs
# split into two classes:
#   1. engine-behavior checks (default lane, run hermetically against the mock)
#   2. GAP checks + interactive-pending checks (#[ignore]'d; run with
#      --run-ignored so they are REPORTED, never silently skipped)
#
# The seven currently-RED GAP checks (D002, D009, D010, D011, D012, D013, D015)
# are EXPECTED to fail today — they prove the coverage gaps exist. This runner
# reports them as RED/uncovered and does NOT count their failure toward the hard
# gate yet (they are gated in once their remediation wave lands). The hard gate
# is the set of engine-behavior + interactive checks that must be GREEN to ship.
#
# Usage:
#   scripts/smoke.sh            # run the gate (hermetic, mock provider)
#   scripts/smoke.sh --help
#   SMOKE_LIVE=1 ANTHROPIC_API_KEY=sk-... scripts/smoke.sh   # + 1 real turn
#
# Exit: 0 if every HARD-GATE check passed; non-zero otherwise.

set -euo pipefail

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  cat <<'__HELP_END__'
smoke.sh — wayland-core P0 smoke gate

USAGE
  scripts/smoke.sh

WHAT IT DOES
  Runs crates/wcore-cli/tests/smoke_p0.rs:
    · engine-behavior checks #6/#10/#17/#24 (hermetic, mock provider) — MUST be
      green; these are the only implemented hard-gate checks
    · GAP checks D002/D009/D010/D011/D012/D013/D015 — currently RED (reported,
      not gated yet; they prove the 7 P0 coverage gaps exist)
    · interactive-pending checks (#15 AskUser arrows, #22 ?-help, #23 @-Tab) —
      reported as interactive-pending, not silently skipped
    · checks #7/#8/#9/#11/#14/#18/#19 — uncovered TODO, no test yet (NOT run,
      NOT claimed as covered; land with their remediation waves)

ENV
  SMOKE_LIVE=1 + ANTHROPIC_API_KEY=sk-...   also run the single real-key turn
  WCORE_TEST_RUNNER=cargo|nextest           override the test runner

EXIT
  0   every HARD-GATE check passed
  >0  a HARD-GATE check failed (release blocked)
__HELP_END__
  exit 0
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Route through vx so the pinned Rust + cargo are used (matches CI / justfile).
CARGO="vx cargo"
if ! command -v vx >/dev/null 2>&1; then
  echo "note: vx not found — falling back to bare cargo (versions may drift)" >&2
  CARGO="cargo"
fi

# HARD-GATE check fns in smoke_p0.rs that MUST be green to ship — ONLY the
# checks that exist as real tests (#6, #10, #17, #24). The GAP checks and the
# interactive-pending checks (#15/#22/#23) are reported separately and not
# gated; checks #7/#8/#9/#11/#14/#18/#19 have no test yet (uncovered TODO) and
# are deliberately absent so the gate never implies coverage it lacks. Filter
# names map to #[test] fn names.
HARD_GATE_TESTS=(
  "smoke_06_first_prompt_uses_configured_provider_and_key"
  "smoke_10_model_override_reaches_outgoing_request"
  "smoke_17_force_posture_auto_approves_mutating_tool_in_engine"
  "smoke_24_quit_exits_cleanly"
)

echo "=== wayland-core P0 SMOKE GATE ==="
echo "repo: $REPO_ROOT"
echo

# ---------------------------------------------------------------------------
# 1. HARD GATE — these must pass. Run the named engine-behavior/interactive
#    checks. A failure here blocks the release.
# ---------------------------------------------------------------------------
echo "--- HARD GATE (must be green) ---"
GATE_RC=0
for t in "${HARD_GATE_TESTS[@]}"; do
  echo "running hard-gate check: $t"
  if ! $CARGO test --package wcore-cli --test smoke_p0 -- --exact "$t"; then
    echo "HARD-GATE FAILURE: $t" >&2
    GATE_RC=1
  fi
done

# ---------------------------------------------------------------------------
# 2. GAP / interactive-pending REPORT — run the #[ignore]'d checks so they are
#    surfaced (NOT silently skipped). These are EXPECTED to fail today; their
#    failure is reported but does not (yet) block the gate.
# ---------------------------------------------------------------------------
echo
echo "--- GAP + interactive-pending checks (reported; currently RED) ---"
echo "These 7 P0 gap checks prove the coverage gaps and are expected to FAIL"
echo "until their remediation wave lands: D002 D009 D010 D011 D012 D013 D015."
# `|| true`: their red status is informational at this stage.
$CARGO test --package wcore-cli --test smoke_p0 -- --ignored || true

# ---------------------------------------------------------------------------
# 3. Optional real-key happy path.
# ---------------------------------------------------------------------------
if [[ "${SMOKE_LIVE:-}" == "1" ]]; then
  echo
  echo "--- SMOKE_LIVE=1: real-key happy path ---"
  if ! $CARGO test --package wcore-cli --test smoke_p0 -- --ignored --exact \
       live_real_key_first_prompt_round_trip; then
    echo "LIVE happy-path FAILURE" >&2
    GATE_RC=1
  fi
fi

echo
if [[ "$GATE_RC" -eq 0 ]]; then
  echo "SMOKE GATE: PASS (all hard-gate checks green)"
else
  echo "SMOKE GATE: FAIL (a hard-gate check failed — release blocked)" >&2
fi
exit "$GATE_RC"
