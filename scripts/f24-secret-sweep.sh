#!/bin/sh
# Sweep a set of paths for a live secret VALUE, with a proof that the sweep
# instrument is alive.
#
# WHY THIS EXISTS. "The secret does not appear in my artefacts" is a KNOWN-NEGATIVE
# assertion, and LANE-BRIEF §3b-i is explicit that a known-negative is the single
# easiest claim to pass without doing any work: a typo'd path, a wrong flag, a glob
# the shell ate, or a tool that errors all return zero, and every one of those
# CONFIRMS the claim.
#
# This is not hypothetical. Lane 24-media-live hit it live. The sweep was written as:
#
#     PATHS="a b c"
#     printf '%s\n' "$KEY" | grep -rIl -F -f - $PATHS | wc -l     # -> 0
#
# Under zsh an unquoted parameter is NOT word-split, so `$PATHS` arrived as ONE
# path named "a b c", grep printed "No such file or directory", and the sweep
# reported **0 hits — clean**. The known-positive control reported 0 in the same
# breath, which is the only reason it was caught.
#
# So this script does two things the ad-hoc invocation did not:
#   1. takes paths as real positional arguments (no word-splitting to get wrong);
#   2. REFUSES TO REPORT A CLEAN SWEEP unless a known-positive control found
#      something first. A dead instrument cannot return "clean" from here.
#
# The needle is read from STDIN and never appears in argv, in a file, or in output.
#
# usage:
#   printf '%s\n' "$SECRET" | scripts/f24-secret-sweep.sh <path> [path...]
#   scripts/f24-secret-sweep.sh --selftest

set -u

GREP=/usr/bin/grep
[ -x "$GREP" ] || GREP=grep

# ── self-test: three assertions, per LANE-BRIEF §6b-ii ────────────────────
# (a) known-positive found, (b) known-negative not found, and (c) THE BROKEN
# INVOCATION WOULD HAVE MISSED IT. Only (c) proves the repair does anything.
if [ "${1:-}" = "--selftest" ]; then
  TMP=$(mktemp -d)
  trap 'rm -rf "$TMP"' EXIT
  mkdir -p "$TMP/dir one" "$TMP/dir two"
  printf 'harmless\nSENTINEL-VALUE-9f3a1c02\n' > "$TMP/dir one/hit.txt"
  printf 'nothing to see\n' > "$TMP/dir two/miss.txt"

  fails=0
  ok() { printf 'PASS  %s\n' "$1"; }
  bad() { printf 'FAIL  %s\n' "$1"; fails=$((fails + 1)); }

  # (a) known-positive
  n=$(printf 'SENTINEL-VALUE-9f3a1c02\n' \
      | $GREP -rIl -F -f - "$TMP/dir one" "$TMP/dir two" 2>/dev/null | wc -l | tr -d ' ')
  [ "$n" = "1" ] && ok "known-positive: the planted needle is found (n=$n)" \
                 || bad "known-positive: expected 1, got $n"

  # (b) known-negative
  n=$(printf 'ZZZ-NEVER-APPEARS-ZZZ\n' \
      | $GREP -rIl -F -f - "$TMP/dir one" "$TMP/dir two" 2>/dev/null | wc -l | tr -d ' ')
  [ "$n" = "0" ] && ok "known-negative: an absent needle is not found (n=$n)" \
                 || bad "known-negative: expected 0, got $n"

  # (c) THE THIRD ASSERTION. Reproduce the exact defect measured in-lane: the
  # paths collapsed into ONE argument. The broken form must report 0 — i.e. it
  # would have declared the planted secret absent.
  BROKEN_ARG="$TMP/dir one $TMP/dir two"
  n=$(printf 'SENTINEL-VALUE-9f3a1c02\n' \
      | $GREP -rIl -F -f - "$BROKEN_ARG" 2>/dev/null | wc -l | tr -d ' ')
  [ "$n" = "0" ] && ok "THIRD: the collapsed-path invocation MISSES a planted secret (n=$n) — the defect is real" \
                 || bad "THIRD: expected the broken form to miss (0), got $n"

  # (d) FOURTH. The aliveness guard must REFUSE on paths it cannot read. The
  # first version of this script reported CLEAN for two nonexistent paths,
  # because its control counted a file it had planted itself.
  printf 'SENTINEL-VALUE-9f3a1c02\n' | sh "$0" /nonexistent/aaa /nonexistent/bbb >/dev/null 2>&1
  rc=$?
  [ "$rc" = "4" ] && ok "FOURTH: nonexistent paths are REFUSED (rc=$rc), not reported clean" \
                  || bad "FOURTH: expected rc=4 on unreadable paths, got rc=$rc"

  # (e) FIFTH. End-to-end: a planted secret must actually be caught (rc=1).
  printf 'leaked SENTINEL-VALUE-9f3a1c02 here\n' > "$TMP/dir two/leak.txt"
  printf 'SENTINEL-VALUE-9f3a1c02\n' | sh "$0" "$TMP/dir two" >/dev/null 2>&1
  rc=$?
  [ "$rc" = "1" ] && ok "FIFTH: a planted secret is caught (rc=$rc)" \
                  || bad "FIFTH: expected rc=1 on a real leak, got rc=$rc"

  printf '\nselftest all_pass=%s\n' "$([ "$fails" -eq 0 ] && echo true || echo false)"
  [ "$fails" -eq 0 ] || exit 1
  exit 0
fi

if [ "$#" -lt 1 ]; then
  echo "f24-secret-sweep: at least one path is required" >&2
  exit 2
fi

NEEDLE_FILE=$(mktemp)
trap 'rm -f "$NEEDLE_FILE"' EXIT
chmod 600 "$NEEDLE_FILE"
cat > "$NEEDLE_FILE"

# A sweep on an empty needle matches nothing and self-passes. Refuse.
if [ ! -s "$NEEDLE_FILE" ] || [ "$(tr -d '\n' < "$NEEDLE_FILE" | wc -c | tr -d ' ')" -lt 8 ]; then
  echo "f24-secret-sweep: needle missing or implausibly short — refusing to report a clean sweep" >&2
  exit 3
fi
printf 'needle length = %s (length only; the value is never printed)\n' \
  "$(tr -d '\n' < "$NEEDLE_FILE" | wc -c | tr -d ' ')"

# ── aliveness: the control must speak to THE PATHS BEING SWEPT ─────────────
# An earlier version of this script planted a control token in its OWN temp dir
# and swept `"$CONTROL_TMP" "$@"`. That control returned >=1 unconditionally —
# it was counting its own plant — so the script cheerfully reported CLEAN for
# two NONEXISTENT paths. The instrument written to catch self-passing gates was
# itself self-passing. Measured in lane 24-media-live; this is the repair.
#
# Aliveness is now asserted over the caller's actual paths, in two steps.

# (1) every supplied path must exist and be readable.
for p in "$@"; do
  if [ ! -r "$p" ]; then
    echo "f24-secret-sweep: path is missing or unreadable: $p" >&2
    echo "f24-secret-sweep: refusing to report a clean sweep over a path I cannot read" >&2
    exit 4
  fi
done

# (2) grep must actually traverse those paths and SEE text. An empty pattern is
# a fixed-string match against every line, so this returns every text file the
# same invocation can read. Zero means the sweep would have been vacuous —
# wrong tree, unreadable files, or all-binary — and a clean result meaningless.
CONTROL=$(printf '\n' | $GREP -rIl -F -f - "$@" 2>/dev/null | wc -l | tr -d ' ')
printf 'aliveness: readable text files under the swept paths = %s\n' "$CONTROL"
if [ "$CONTROL" -lt 1 ]; then
  echo "f24-secret-sweep: the swept paths yielded NO readable text — instrument dead, a clean sweep here would be meaningless" >&2
  exit 4
fi

HITS=$($GREP -rIl -F -f "$NEEDLE_FILE" "$@" 2>/dev/null | wc -l | tr -d ' ')
printf 'SWEEP secret-value hits = %s\n' "$HITS"
if [ "$HITS" -gt 0 ]; then
  echo "LEAK: the following files contain the secret value:" >&2
  $GREP -rIl -F -f "$NEEDLE_FILE" "$@" 2>/dev/null >&2
  exit 1
fi
echo "CLEAN (control alive, 0 value hits)"
exit 0
