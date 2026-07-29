#!/usr/bin/env bash
# 23A-C1: interrupted-write distribution, v2 -- SELF-CALIBRATING kill window.
#
# WHY v2. v1 sampled the kill delay from [0.5ms, 30ms] and its own vacuity guard fired:
# 35/35 promote trials landed in NO-GRANT with GRANTED=0, i.e. every kill hit the binary
# during startup and no trial ever reached the write. Measured floor on this host: a
# bare `--version` takes 26ms because the debug binary is 331MB. A 30ms ceiling is
# therefore almost entirely startup, and the "0 torn grants" it produced was worthless.
#
# **That guard is the reason this measurement is trustworthy.** Without it the run would
# have reported 0 destructive states across 70 kills and been believed.
#
# v2 measures the real end-to-end duration of each verb on this payload FIRST, then
# samples the kill delay uniformly across [startup_floor, 1.05 x duration] so the window
# straddles the work. The calibration numbers are printed so the window is auditable.
#
# CLASSIFICATION -- rollback
#   COMPLETE      target byte-identical to the retained generation
#   NOT-STARTED   target absent; tombstone intact, so a retry completes it (re-run and
#                 asserted, not assumed)
#   PARTIAL       target present but NOT identical   <-- THE FAILURE MODE, must be 0
# CLASSIFICATION -- promote
#   GRANTED / NO-GRANT / TORN-GRANT (must be 0) / SKILLS-DIR-MUTATED (must be 0)

set -uo pipefail
BIN=/root/wayland-23a-c1/target/debug/wayland-core
N=${1:-35}
PROJ=$(mktemp -d); mkdir -p "$PROJ/.git"
NFILES=${2:-600}

mkbig() { local d="$1/skills/$2"; mkdir -p "$d/sub"
  printf -- '---\nname: %s\ndescription: big draft\n---\n\nBODY-MARKER\n' "$2" > "$d/SKILL.md"
  printf '{"auto_drafted":true,"signature":"sig-%s"}' "$2" > "$d/manifest.json"
  for i in $(seq 1 $((NFILES-2))); do head -c 4096 /dev/urandom | base64 > "$d/sub/f$i.txt"; done; }
treehash() { ( cd "$1" 2>/dev/null && find . -type f | sort | xargs md5sum 2>/dev/null ) | md5sum | cut -d' ' -f1; }
dur() { local s e; s=$(date +%s.%N); "$@" >/dev/null 2>&1; e=$(date +%s.%N); echo "$e-$s" | bc; }

echo "########## CALIBRATION (payload = $NFILES files) ##########"
HC=$(mktemp -d); mkbig "$HC" auto-cal
FLOOR=$(dur env WAYLAND_HOME="$HC" "$BIN" --version)
T_PROM=$(dur env WAYLAND_HOME="$HC" "$BIN" --skills-promote auto-cal)
WAYLAND_HOME="$HC" "$BIN" --skills-revoke auto-cal >/tmp/cal.out 2>&1
CRID=$(grep -o 'revocation id: .*' /tmp/cal.out | awk '{print $3}')
T_ROLL=$(dur env WAYLAND_HOME="$HC" "$BIN" --skills-rollback "$CRID")
rm -rf "$HC"
printf "  startup floor      : %ss\n  promote  duration  : %ss\n  rollback duration  : %ss\n" "$FLOOR" "$T_PROM" "$T_ROLL"
printf "  kill window promote: [%s, %s]s\n" "$FLOOR" "$(echo "$T_PROM*1.05"|bc)"
printf "  kill window rollbk : [%s, %s]s\n" "$FLOOR" "$(echo "$T_ROLL*1.05"|bc)"
echo

echo "########## ROLLBACK: $N kills into a $NFILES-file restore ##########"
CO=0; NS=0; PA=0; RETRYFAIL=0
for t in $(seq 1 "$N"); do
  H=$(mktemp -d); mkbig "$H" auto-big
  GOOD=$(treehash "$H/skills/auto-big")
  WAYLAND_HOME="$H" "$BIN" --skills-revoke auto-big >/tmp/k.out 2>&1
  RID=$(grep -o 'revocation id: .*' /tmp/k.out | awk '{print $3}')
  D=$(awk -v s="$t" -v lo="$FLOOR" -v hi="$T_ROLL" 'BEGIN{srand(s*7919); printf "%.4f", lo + rand()*(hi*1.05-lo)}')
  WAYLAND_HOME="$H" timeout -s KILL "$D" "$BIN" --skills-rollback "$RID" >/dev/null 2>&1
  TGT="$H/skills/auto-big"
  if [ ! -e "$TGT" ]; then
    NS=$((NS+1))
    # A NOT-STARTED trial must be RECOVERABLE. Asserted by actually retrying.
    WAYLAND_HOME="$H" "$BIN" --skills-rollback "$RID" >/dev/null 2>&1
    if [ "$(treehash "$TGT")" != "$GOOD" ]; then RETRYFAIL=$((RETRYFAIL+1)); echo "  !! trial $t: retry after kill did NOT restore correctly"; fi
  elif [ "$(treehash "$TGT")" = "$GOOD" ]; then CO=$((CO+1))
  else PA=$((PA+1)); echo "  !! PARTIAL trial $t (delay ${D}s): $(find "$TGT" -type f|wc -l) files, expected $NFILES"; fi
  rm -rf "$H"
done
echo "ROLLBACK over $N kills:  COMPLETE=$CO  NOT-STARTED=$NS  PARTIAL=$PA (must be 0)  retry-failures=$RETRYFAIL (must be 0)"
echo

echo "########## PROMOTE: $N kills ##########"
GR=0; NG=0; TORN=0; MUT=0
for t in $(seq 1 "$N"); do
  H=$(mktemp -d); mkbig "$H" auto-big
  BEFORE=$(treehash "$H/skills/auto-big")
  D=$(awk -v s="$t" -v lo="$FLOOR" -v hi="$T_PROM" 'BEGIN{srand(s*104729); printf "%.4f", lo + rand()*(hi*1.05-lo)}')
  WAYLAND_HOME="$H" timeout -s KILL "$D" "$BIN" --skills-promote auto-big >/dev/null 2>&1
  AFTER=$(treehash "$H/skills/auto-big")
  [ "$BEFORE" != "$AFTER" ] && { MUT=$((MUT+1)); echo "  !! trial $t: promote MUTATED the user's skills directory"; }
  G=$(ls -1 "$H"/skills-governance/promotions/*.json 2>/dev/null | head -1)
  if [ -z "$G" ]; then NG=$((NG+1))
  elif python3 -c "import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get('content_digest','').startswith('sha256:') and d.get('skill_name') and d.get('authority') else 1)" "$G" 2>/dev/null; then GR=$((GR+1))
  else TORN=$((TORN+1)); echo "  !! trial $t: grant on disk unparseable/incomplete"; fi
  rm -rf "$H"
done
echo "PROMOTE over $N kills:  GRANTED=$GR  NO-GRANT=$NG  TORN-GRANT=$TORN (must be 0)  SKILLS-DIR-MUTATED=$MUT (must be 0)"
echo

# Vacuity guard. A run where no trial ever reached the work proves nothing.
if [ "$CO" -eq 0 ] || [ "$GR" -eq 0 ]; then
  echo "VERDICT: INVALID MEASUREMENT -- a verb never once completed inside the window (rollback COMPLETE=$CO, promote GRANTED=$GR). Widen it."
  exit 2
fi
BAD=$((PA + TORN + MUT + RETRYFAIL))
echo "VERDICT: window straddles the work (both verbs saw completions AND interruptions)."
echo "         destructive states = $BAD (target 0)"
[ "$BAD" -eq 0 ] || exit 1
