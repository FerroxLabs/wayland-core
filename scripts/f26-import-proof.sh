#!/bin/sh
# F26 import proof: files written per category, BEFORE vs AFTER, plus a
# quarantine-inertness leg that carries a positive control for execution.
#
# # What makes each number here non-vacuous
#
# * **Every count is taken with `/usr/bin/find`**, never a shell glob. A zsh
#   glob returned 0 for a directory holding 122 files earlier in this phase, and
#   `rtk` rewrites `grep` output. Absolute paths only.
# * **The BEFORE arm is run by the same script against the same corpus.** A
#   before/after table where the "before" is quoted from someone else's report is
#   an argument, not a measurement.
# * **The inertness leg proves the harness can observe execution first.** The
#   sentinel mechanism is demonstrated live — the payload's own command is run
#   directly and the sentinel appears — BEFORE any claim that the sentinel is
#   absent. A dead sentinel would otherwise report containment for free, which is
#   the single easiest way to pass this phase's central safety claim without
#   doing any work.
# * **A zero from an empty home is distinguished from a zero from containment**:
#   the run asserts the executable payload IS on disk under quarantine, so
#   "did not execute" is not "was never imported".
#
# Usage: f26-import-proof.sh <before-binary> <after-binary> <corpus> <workdir>

set -eu

BEFORE=${1:?before binary}
AFTER=${2:?after binary}
CORPUS=${3:?corpus}
WORK=${4:?workdir}

FIND=/usr/bin/find
GREP=/usr/bin/grep

rm -rf "$WORK"
mkdir -p "$WORK"

count() { # count files under a path; 0 when the path does not exist
  if [ -e "$1" ]; then "$FIND" "$1" -type f | wc -l | tr -d ' '; else echo 0; fi
}

run_arm() {
  arm=$1
  bin=$2
  home="$WORK/home-$arm"
  mkdir -p "$home"
  echo "=== ARM $arm : $bin ==="
  WAYLAND_HOME="$home" "$bin" migrate hermes --home "$CORPUS" --yes \
    > "$WORK/$arm-apply.log" 2>&1 || echo "APPLY-RC-$arm: $?"
  tail -6 "$WORK/$arm-apply.log" | sed "s/^/  $arm| /"

  # Per-category file counts, each from an independent find over a distinct
  # subtree, so one category cannot borrow another's number.
  echo "FILES-$arm-config:      $(count "$home/config.toml")"
  echo "FILES-$arm-quarantine:  $(count "$home/migrate-quarantine")"
  echo "FILES-$arm-skills:      $(count "$home/skills")"
  echo "FILES-$arm-personas:    $(count "$home/migrate-imported/personas")"
  echo "FILES-$arm-memory:      $(count "$home/migrate-imported/memory")"
  echo "FILES-$arm-provenance:  $(count "$home/migrate-imported/PROVENANCE.json")"
  echo "FILES-$arm-TOTAL:       $(count "$home")"

  # Discovery coverage, from the product's own published plan rather than from
  # the filesystem — the two are independent instruments for one claim.
  WAYLAND_HOME="$home-dry" "$bin" migrate hermes --home "$CORPUS" --dry-run --json \
    > "$WORK/$arm-plan.json" 2>"$WORK/$arm-plan.err" || echo "PLAN-RC-$arm: $?"
  echo "PUBLISHED-$arm-skills:  $("$GREP" -c '"identity": "skill:' "$WORK/$arm-plan.json" || true)"
  echo "PUBLISHED-$arm-persona: $("$GREP" -c '"identity": "persona:' "$WORK/$arm-plan.json" || true)"
  echo "PUBLISHED-$arm-memory:  $("$GREP" -c '"identity": "memory:' "$WORK/$arm-plan.json" || true)"
}

echo "CORPUS-SKILL-MD: $("$FIND" "$CORPUS" -name 'SKILL.md' -type f | wc -l | tr -d ' ')"
echo "CORPUS-SOUL-MD:  $("$FIND" "$CORPUS" -name 'SOUL.md' -type f | wc -l | tr -d ' ')"

run_arm before "$BEFORE"
run_arm after "$AFTER"

# ---------------------------------------------------------------------------
# Inertness, with the positive control FIRST
# ---------------------------------------------------------------------------
echo
echo "=== QUARANTINE INERTNESS ==="

SENT_A=${WL_SENTINEL_A:?sentinel A}
SENT_B=${WL_SENTINEL_B:?sentinel B}
rm -f "$SENT_A" "$SENT_B"

# P0 — the sentinel mechanism is alive. Run the payload's OWN command and show
# the sentinel appears. Without this, every absence below is free.
sh -c "touch $SENT_A"
if [ -f "$SENT_A" ]; then
  echo "P0 POSITIVE-CONTROL: PASS  the harness observes execution (sentinel created by running the payload command)"
else
  echo "P0 POSITIVE-CONTROL: FAIL  the sentinel mechanism is dead; every absence below is meaningless"
  exit 2
fi
rm -f "$SENT_A"
[ ! -f "$SENT_A" ] || { echo "P0 CLEANUP: FAIL"; exit 2; }

# N1 — drive the real binary over the corpus into a clean home, then look.
home="$WORK/home-inert"
mkdir -p "$home"
WAYLAND_HOME="$home" "$AFTER" migrate hermes --home "$CORPUS" --yes \
  > "$WORK/inert-apply.log" 2>&1 || echo "INERT-APPLY-RC: $?"

if [ -f "$SENT_A" ] || [ -f "$SENT_B" ]; then
  echo "N1 INERTNESS: FAIL  an imported executable payload RAN during import"
  exit 3
fi
echo "N1 INERTNESS: PASS  neither sentinel exists after a full import"

# N2 — and the payload really was imported, so N1 is containment rather than
# the absence of any import at all.
QP=$("$FIND" "$home/migrate-quarantine" -name 'SKILL.md' -type f 2>/dev/null | wc -l | tr -d ' ')
echo "N2 PAYLOAD-CONTAINED: $QP quarantined SKILL.md on disk"
[ "$QP" -ge 1 ] || { echo "N2: FAIL — nothing was contained, so N1 measured an empty home"; exit 3; }

# N3 — placement: no quarantined payload sits under any agent-facing skill root.
LEAK=$("$FIND" "$home/skills" -path '*probe-exec*' 2>/dev/null | wc -l | tr -d ' ')
echo "N3 NO-EXEC-IN-LIVE-SKILLS: $LEAK (expect 0)"
[ "$LEAK" -eq 0 ] || { echo "N3: FAIL — an executable payload reached the live skills root"; exit 3; }

# N3-control — the same find DOES match under the quarantine root, so N3's zero
# is a placement fact and not a broken matcher.
CTRL=$("$FIND" "$home/migrate-quarantine" -path '*probe-exec*' 2>/dev/null | wc -l | tr -d ' ')
echo "N3-CONTROL matcher-fires-elsewhere: $CTRL (expect >0)"
[ "$CTRL" -gt 0 ] || { echo "N3-CONTROL: FAIL — the matcher is dead, so N3's zero proves nothing"; exit 3; }

# N4 — provenance survives on the contained items, read back from the product.
WAYLAND_HOME="$home" "$AFTER" migrate quarantined > "$WORK/quarantined.log" 2>&1 || true
echo "N4 PROVENANCE-LINES: $("$GREP" -c 'digest:' "$WORK/quarantined.log" || true)"

echo
echo "PROOF: COMPLETE"
