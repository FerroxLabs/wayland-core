#!/usr/bin/env bash
# Lane pre-flight: everything CI's ci-linux job checks on the HOST, before it
# builds the container image, plus the Desktop contract corpus gate (see its
# own block below for why that one is here and not in GATES).
#
# WHY THIS EXISTS. Lanes gate on hetzner because CI's Linux leg takes ~67
# minutes to report what hetzner reports in 2-3. But "gate on hetzner" was
# incomplete: hetzner ran the TESTS and none of the repo gates, so a lane could
# be fully green locally and still red CI. Measured 2026-08-29 — the #365 c2
# lane passed nextest 150/150 on hetzner and failed CI at "No unserialized test
# writes to shared process globals", because that check is a repo gate and not a
# test. Running these here makes the hetzner gate actually complete.
#
# This is NOT a substitute for CI's macOS and Windows legs. Nothing here
# compiles for another platform. A criterion needing cross-platform evidence
# still needs a real run on that platform.
set -uo pipefail

# `--self-test` proves this script's own status rendering in both directions
# against synthetic gates. It touches no gate, no ci.yml and no network, so it
# runs before every guard below and never depends on the state of the tree.
if [ "${1:-}" != "--self-test" ]; then
  cd "$(git rev-parse --show-toplevel)" || exit 2
fi

# ── GATE STATUS PROTOCOL (wayland#1254) ────────────────────────────────────
# Until 2026-09-04 this script rendered TWO values, `ok` and `FAIL`, from one
# bit (did the gate exit 0), and printed the gate's own output only on the
# FAIL branch. Both halves of that were wrong in the same direction.
#
# `check-criteria-ledger.py` deliberately DOWNGRADES itself in two situations
# and says so out loud, because "a check that quietly stops running is
# indistinguishable from one that ran and passed". It exits 0 while saying it.
# This script then discarded the sentence and printed `ok`. Measured 2026-08-30
# on 388de5d70: a shallow clone of a tree whose full-clone ledger check is
# EXIT=1 produced `PRE-FLIGHT PASSED` here while CI, which checks out at
# `fetch-depth: 0`, was red on the same commit. The script that insists on
# saying it and the script that decides what the operator sees disagreed, and
# this one won — converting "I did not check" into "I checked and it was fine".
#
# The fix is a THREE-valued status, and the third value is never inferred from
# the gate's prose. Grepping captured stdout for `NOTE:` or `THIS IS NOT A
# PASS` would reproduce the defect one layer up: "does this free-form English
# disclose a downgrade?" is undecidable over an open alphabet of future
# wordings, and the next gate to phrase a downgrade differently is silently
# `ok` again. So the status is decided out of band, two ways, both total:
#
#   1. EXIT CODE. 0 = ran fully armed. $DEGRADED_RC = ran, but disarmed part of
#      itself. Any other non-zero = failed. This is the general mechanism: any
#      gate that adopts it is rendered DEGRADED forever after, with no change
#      here, and a gate that does not signal degraded is not degraded.
#
#   2. THE GATE'S OWN CLI. One entry below is invoked by THIS script in a mode
#      the gate itself defines as switching off part of its checking:
#      `check-criteria-ledger.py --offline` means, in the gate's own words,
#      "tracker coverage and ledger/GitHub divergence were NOT checked". That
#      flag is a machine-readable marker of the gate's declared interface, and
#      it is read from the invocation THIS FILE wrote — not from the gate's
#      output. The declaration is verified against the invocation on every run
#      (see `run_gate`), so the table cannot rot into a lie: drop the flag and
#      the entry goes FAIL, not quietly `ok`.
#
# DEGRADED is rendered distinctly from `ok`, is never collapsed into it, and
# carries the gate's OWN disclosure verbatim into this script's stdout, which
# is the whole point: the operator reading a green pre-flight now sees the
# sentence the gate wrote.
#
# RESIDUAL, stated rather than hidden. `check-criteria-ledger.py` disarms for a
# second reason its CLI cannot express — a shallow clone — and does not (yet)
# use the reserved exit code, so neither mechanism above can see it. Preflight
# does not guess at it from prose. It refuses to run at all where its own clone
# is shallower than the checkout ci.yml gives that gate (CLONE-DEPTH GUARD
# below, derived from ci.yml, not hard-coded here). The durable fix is for that
# script to return $DEGRADED_RC when it skips sha resolution; that is a change
# to the gate, owned by the gate.
DEGRADED_RC=3

# Each entry is "<mode>|<the exact invocation ci.yml uses>", in ci.yml's order.
#   armed              — this script expects the gate to run fully armed.
#   disarmed:<flag>    — this script knowingly invokes the gate in a mode the
#                        gate's own interface defines as disarming; <flag> must
#                        appear in the invocation or the entry FAILS as stale.
GATES=(
  "armed|python3 scripts/check-no-vacuous-cargo-test.py --self-test"
  "armed|python3 scripts/check-no-vacuous-cargo-test.py"
  "armed|python3 scripts/check-model-limits-freshness.py --self-test"
  "armed|python3 scripts/check-no-personal-identifiers.py --self-test"
  "armed|python3 scripts/check-no-personal-identifiers.py"
  "armed|python3 scripts/check-test-env-globals.py --self-test"
  "armed|python3 scripts/check-test-env-globals.py"
  "armed|python3 scripts/check-message-whitespace.py --self-test"
  "armed|python3 scripts/check-message-whitespace.py crates"
  # Added 2026-09-04 with the wayland#1254 fix: ci.yml has run this gate
  # since 0.13.12 and this list did not, so the DRIFT GUARD below was
  # already refusing to run on origin/main. That refusal is the guard
  # working; the remedy it prints is this line.
  "armed|python3 scripts/check-ci-step-suppression.py --self-test"
  "armed|python3 scripts/check-ci-step-suppression.py"
  "armed|python3 scripts/check-criteria-ledger.py --self-test"
  "disarmed:--offline|python3 scripts/check-criteria-ledger.py --offline"
  "armed|python3 scripts/check-release-readiness.py --self-test"
  "armed|python3 scripts/check-windows-attribution.py --self-test"
  "armed|python3 scripts/check-windows-attribution.py"
  "armed|python3 scripts/flake-ledger.py --self-test"
  "armed|python3 .planning/evidence/ci-macos-budget/gate.py --self-test .github/workflows/ci.yml"
  "armed|python3 .planning/evidence/ci-macos-budget/gate.py .github/workflows/ci.yml"
)

nfail=0
ndeg=0
gate_status=""
gate_out=""
gate_why=""

# run_gate <mode> <invocation> -> sets gate_status (ok|degraded|fail), gate_out,
# gate_why. The ONLY inputs to gate_status are the exit code and the declared
# mode. The gate's stdout is captured for display and is never parsed.
run_gate() {
  local mode="$1" cmd="$2" rc flag
  gate_why=""
  gate_out="$($cmd 2>&1)"
  rc=$?
  if [ "$rc" -eq 0 ]; then
    gate_status=ok
  elif [ "$rc" -eq "$DEGRADED_RC" ]; then
    gate_status=degraded
    gate_why="the gate returned the reserved degraded exit code ${DEGRADED_RC}: it ran, and disarmed part of itself. Its own words:"
  else
    gate_status=fail
  fi
  if [ "$mode" != "armed" ]; then
    flag="${mode#disarmed:}"
    case " $cmd " in
      *" $flag "*)
        if [ "$gate_status" = "ok" ]; then
          gate_status=degraded
          gate_why="invoked with ${flag}, which this gate's own interface defines as switching off part of its checking. Its own words:"
        fi
        ;;
      *)
        gate_status=fail
        gate_why="STALE DECLARATION: this entry is declared disarmed via ${flag}, but the invocation no longer carries that flag. Fix the declaration or the invocation; a disarm nobody declares is the defect this protocol exists to prevent."
        ;;
    esac
  fi
}

# render_gate <mode> <invocation> — three values, three renderings. DEGRADED is
# never printed as ok, and unlike ok it carries the gate's own output through.
render_gate() {
  run_gate "$1" "$2"
  case "$gate_status" in
    ok)
      printf '  ok        %s\n' "$2"
      ;;
    degraded)
      printf '  DEGRADED  %s\n' "$2"
      printf '        %s\n' "$gate_why"
      printf '%s\n' "$gate_out" | sed 's/^/        | /'
      ndeg=$((ndeg + 1))
      ;;
    *)
      printf '  FAIL      %s\n' "$2"
      if [ -n "$gate_why" ]; then printf '        %s\n' "$gate_why"; fi
      printf '%s\n' "$gate_out" | tail -25 | sed 's/^/        /'
      nfail=$((nfail + 1))
      ;;
  esac
}

# ── SELF-TEST ──────────────────────────────────────────────────────────────
# Both directions, for the reason this repo keeps re-learning: a change that
# renders EVERYTHING degraded, or that reds on any NOTE at all, satisfies the
# positive half and destroys the gate. So the armed arms and the degraded arms
# are asserted together, through the SAME run_gate/render_gate this script uses
# for real gates — not a reimplementation of the rule.
_st_pass()           { echo "checked everything"; return 0; }
_st_prose()          { echo "NOTE: THIS IS NOT A PASS for coverage"; return 0; }
_st_degraded()       { echo "I skipped half of myself"; return "$DEGRADED_RC"; }
_st_degraded_prose() { echo "OFFLINE: THIS IS NOT A PASS for coverage"; return "$DEGRADED_RC"; }
_st_fail()           { echo "a real problem"; return 1; }

self_test() {
  local ok=1 label mode cmd want good rendered_ok rendered_deg
  arm() {
    label="$1"; mode="$2"; cmd="$3"; want="$4"
    run_gate "$mode" "$cmd"
    if [ "$gate_status" = "$want" ]; then good=ok; else good="SELF-TEST FAILED"; ok=0; fi
    printf '  %-56s want %-8s got %-8s  %s\n' "${label:0:56}" "$want" "$gate_status" "$good"
  }

  # Positive direction: a fully-armed gate that passes is still ok.
  arm "armed gate, exit 0 -> ok" "armed" "_st_pass" "ok"
  # Negative direction: a gate that signals its own downgrade is NOT ok.
  arm "armed gate, reserved exit ${DEGRADED_RC} -> degraded" "armed" "_st_degraded" "degraded"
  # A real failure is still a failure and is not softened into degraded.
  arm "armed gate, exit 1 -> fail" "armed" "_st_fail" "fail"
  # THE CONTROL for the shape criterion: a gate whose stdout contains the exact
  # prose a substring search would trip on, exiting 0 while fully armed, must
  # stay ok. If this arm ever goes degraded, someone has reintroduced the
  # undecidable "does this English disclose a downgrade?" test.
  arm "armed gate printing THIS IS NOT A PASS, exit 0 -> ok" "armed" "_st_prose" "ok"
  # A mode this script itself declares disarming, per the gate's own CLI.
  arm "declared disarmed invocation, exit 0 -> degraded" "disarmed:--offline" "_st_pass --offline" "degraded"
  # ...and the declaration cannot rot: drop the flag and the entry fails loudly
  # rather than silently becoming ok.
  arm "disarmed declaration without its flag -> fail" "disarmed:--offline" "_st_pass" "fail"

  # Rendering, not just status: ok and degraded must be distinguishable on the
  # operator's screen, and the degraded rendering must carry the gate's OWN
  # disclosure into this script's stdout (wayland#1254 c2).
  #
  # The three arms below DO look at text. That is a self-test asserting what
  # this script PRINTS; it is not, and must never become, how a gate's status
  # is decided. Status comes from run_gate, above, which reads only the exit
  # code and the declared mode.
  rendered_ok="$(render_gate armed _st_pass)"
  rendered_deg="$(render_gate armed _st_degraded_prose)"

  local got
  if [ "$rendered_ok" != "$rendered_deg" ]; then got=differ; else got=same; fi
  if [ "$got" = "differ" ]; then good=ok; else good="SELF-TEST FAILED"; ok=0; fi
  printf '  %-56s want %-8s got %-8s  %s\n' \
    "ok and degraded render differently" "differ" "$got" "$good"

  case "$rendered_deg" in
    *"THIS IS NOT A PASS"*) got=quoted ;;
    *)                      got=dropped ;;
  esac
  if [ "$got" = "quoted" ]; then good=ok; else good="SELF-TEST FAILED"; ok=0; fi
  printf '  %-56s want %-8s got %-8s  %s\n' \
    "degraded rendering carries the gate's own words" "quoted" "$got" "$good"

  case "$rendered_ok" in
    *DEGRADED*) got=labelled ;;
    *)          got=clean ;;
  esac
  if [ "$got" = "clean" ]; then good=ok; else good="SELF-TEST FAILED"; ok=0; fi
  printf '  %-56s want %-8s got %-8s  %s\n' \
    "an ok gate is never labelled DEGRADED" "clean" "$got" "$good"

  printf 'self-test: %s\n' \
    "$([ "$ok" -eq 1 ] && echo "both directions proven" || echo "BROKEN -- the pre-flight cannot be trusted")"
  [ "$ok" -eq 1 ]
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit $?
fi

# ── DRIFT GUARD + CLONE-DEPTH GUARD ────────────────────────────────────────
# A hand-maintained copy of a CI step list goes stale silently, and a
# pre-flight that under-covers is worse than none: it converts "I did not
# check" into "I checked and it was fine". So derive ci.yml's real host-side
# gate set and refuse to run if this list no longer matches it.
#
# The CLONE-DEPTH half is the same argument about the ENVIRONMENT rather than
# the step list. ci.yml gives its ledger step a `fetch-depth: 0` checkout for a
# stated reason: at depth 1 the ledger gate cannot resolve any
# `last_verified_commit`, disarms that check, and reports green. If this
# worktree is shallower than the one ci.yml hands the gate, then this script is
# not predicting CI — it is running a different, weaker check and calling the
# answer CI's. Refuse, rather than certify from a position CI never occupies.
python3 - <<'PY' || exit 2
import re, subprocess, sys
lines = open(".github/workflows/ci.yml").read().splitlines()
start = stop = None
for i, l in enumerate(lines):
    if re.match(r"^  ci-linux:", l): start = i
    if start is not None and "- name: Build CI image" in l and i > start:
        stop = i; break
if start is None or stop is None:
    print("PREFLIGHT DRIFT GUARD: could not locate the ci-linux host-side region "
          "in ci.yml. The job or the image step was renamed; fix this script.")
    sys.exit(2)
region = "\n".join(lines[start:stop])
# Any .py gate, not only scripts/ -- the macOS admission gate lives under
# .planning/evidence/, and a guard that only looks in one directory is exactly
# how that gate stayed uncovered while it was red.
PAT = r"(?:scripts|\.planning/evidence)/[A-Za-z0-9_./-]+\.py"
in_ci = set(re.findall(PAT, region))
mine  = set(re.findall(PAT, open("scripts/preflight.sh").read()))
missing = in_ci - mine
if missing:
    print("PREFLIGHT DRIFT GUARD: ci.yml runs host-side gate(s) this pre-flight "
          "does NOT, so a green here would not mean a green there:")
    for m in sorted(missing): print("   ", m)
    print("Add them to GATES above, then re-run.")
    sys.exit(2)
print(f"drift guard: ok ({len(in_ci)} host-side gate script(s) in ci.yml, all covered)")

# Derived from ci.yml, not asserted here: if that job checks out full history,
# so must this worktree, or the gates below run disarmed against a tree CI
# checks armed.
ci_wants_full = re.search(r"^\s*fetch-depth:\s*0\s*$", region, re.M) is not None
try:
    shallow = subprocess.run(["git", "rev-parse", "--is-shallow-repository"],
                             capture_output=True, text=True).stdout.strip() == "true"
except OSError:
    shallow = False
if ci_wants_full and shallow:
    print("PREFLIGHT CLONE-DEPTH GUARD: this worktree is a SHALLOW clone, and "
          "ci.yml gives the same gates a `fetch-depth: 0` checkout.")
    print("At depth 1 only HEAD resolves, so check-criteria-ledger.py skips "
          "`last_verified_commit` resolution for every entry and can report "
          "green on a tree CI reds. A pre-flight run from here cannot predict "
          "CI, and a prediction that cannot be made must not be printed.")
    print("Run `git fetch --unshallow` (or clone at full depth) and re-run.")
    sys.exit(2)
print("clone-depth guard: ok (%s)"
      % ("full history, matching ci.yml" if ci_wants_full else
         "ci.yml sets no fetch-depth: 0 in this region"))
PY

# ── DESKTOP CONTRACT CORPUS ────────────────────────────────────────────────
# NOT in GATES above, because GATES is a mirror of ci.yml's HOST-side steps and
# this one runs inside ci-linux's container. It is here because of how a lane
# gets it wrong, measured on lane/f13-w2-mcp-transports, 2026-08-30:
#
#   The corpus hashes a fixed list of source files (`SOURCE_INPUTS`) BY PATH,
#   read from disk at test time. `crates/wcore-cli/src/main.rs` is one of them.
#   A lane added 773 lines to that file, ran `cargo nextest run -p wcore-mcp
#   -p wcore-cli`, ran this pre-flight, got 0 from both, and reported green.
#   `cargo nextest run -p wcore-protocol` was 100: two corpus tests red.
#
# The lane's mistake was choosing which crates to test, and the class of that
# mistake is not closable by telling lanes to also run -p wcore-protocol, or by
# diffing the changed-file list against SOURCE_INPUTS -- both of those are
# proxies that need a correct base, a matching path spelling, and someone to
# have remembered. The question asked here instead is the one that actually
# decides, and it is total: IS THE CHECKED-IN CORPUS CURRENT WITH THE TREE ON
# DISK, RIGHT NOW. No diff, no base, no path list, no crate selection. Any
# staleness reddens here whatever the lane edited and whatever it chose to test.
#
# It is the REAL generator, not a reimplementation of the digest -- a second
# implementation of a hash is a thing that drifts silently, and the binary that
# writes the corpus is the only honest oracle for whether the corpus is current.
#
# Remedy when it fails is printed by the binary: `wcore-contract diff` to see
# which manifest keys moved, then `wcore-contract generate` and commit
# crates/wcore-protocol/contracts/desktop/v1/. Only fixture_digest and
# source_inputs_digest may move for a source-hash rebase; schema_digest moving
# means a WIRE change and is not a re-pin.
CORPUS_GATE="cargo run -q -p wcore-protocol --bin wcore-contract -- check"
render_gate armed "$CORPUS_GATE"

for entry in "${GATES[@]}"; do
  render_gate "${entry%%|*}" "${entry#*|}"
done

echo
if [ "$nfail" -ne 0 ]; then
  echo "PRE-FLIGHT FAILED — CI would red on this tree. Fix before pushing."
  exit 1
fi
if [ "$ndeg" -ne 0 ]; then
  # Deliberately NOT the PASSED banner. Nothing went red, and something was
  # not checked; collapsing those two into one word here would undo, at the
  # summary line, exactly what the three-valued status above is for.
  echo "PRE-FLIGHT INCOMPLETE — nothing CI checks on the host went red, but"
  echo "$ndeg gate(s) ran DEGRADED and said so above in their own words. What"
  echo "they switched off was not checked by anything. This is not a pass for"
  echo "that part; re-run those gates armed before believing the tree is clean."
  echo "Still owed separately: cargo fmt --check, clippy -D warnings, nextest,"
  echo "and any macOS/Windows evidence your criteria name."
  exit 0
fi
echo "PRE-FLIGHT PASSED — every host-side gate ci.yml runs before the image build,"
echo "each one fully armed."
echo "Still owed separately: cargo fmt --check, clippy -D warnings, nextest,"
echo "and any macOS/Windows evidence your criteria name."
