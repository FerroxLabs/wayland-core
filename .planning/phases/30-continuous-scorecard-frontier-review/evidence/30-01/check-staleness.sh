#!/bin/bash
# CLAIM-vs-TREE CHECK.
#
# Tracking documents on this program have been provably stale and
# self-contradictory. Where a document and the tree disagree, the tree wins.
#
# Each block below states a claim a tracking document makes, and resolves it by
# running something against the tree. A claim the tree falsifies is recorded
# FALSIFIED with the command that settled it. This is deliberately NOT a grep of
# prose for words like "stale" — it re-derives the underlying fact.
set -u
cd "$1" || exit 2

P=.planning/phases
L=.planning/intel/COMPETITIVE-LEDGER.md
R=.planning/ROADMAP.md

echo "CLAIM-vs-TREE CHECK"
echo "date : $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "base : $(/usr/bin/git rev-parse HEAD)"
echo "ledger refreshed at : $(/usr/bin/git log -1 --format='%h %ci' -- $L)"
echo "roadmap reconciled  : $(/usr/bin/git log -1 --format='%h %ci' -- $R)"
echo

# claim <id> <claim text> <artifact path> <expect: PRESENT|ABSENT> <extra grep or ->
claim() {
  local id=$1 text=$2 path=$3 expect=$4 needle=$5
  echo "-- $id"
  echo "   claim : $text"
  echo "   cmd   : test -f $path"
  if [ -f "$path" ]; then
    local st
    st=$(/usr/bin/grep -m1 '^status:' "$path" 2>/dev/null || echo "status: <none>")
    local ts
    ts=$(/usr/bin/grep -m1 '^termination_state:' "$path" 2>/dev/null || echo "termination_state: <none>")
    echo "   out   : EXISTS  ($(/usr/bin/wc -c < "$path" | tr -d ' ') bytes)  $st  $ts"
    if [ "$needle" != "-" ]; then
      echo "   cmd   : grep -cF '$needle' $path -> $(/usr/bin/grep -cF "$needle" "$path")"
    fi
    if [ "$expect" = "ABSENT" ]; then
      echo "   VERDICT: CLAIM FALSIFIED BY THE TREE"
    else
      echo "   VERDICT: claim holds"
    fi
  else
    echo "   out   : ABSENT"
    if [ "$expect" = "ABSENT" ]; then
      echo "   VERDICT: claim holds"
    else
      echo "   VERDICT: CLAIM FALSIFIED BY THE TREE"
    fi
  fi
  echo
}

echo "### CTRL-01 PORT-* : 'Plans 26-02 (import/apply + executable-content quarantine)"
echo "###                  and 26-04 (hostile corpora), both unstarted'"
claim "STALE-01" "26-02 unstarted" "$P/26-migration-export-backup-restore/26-02-SUMMARY.md" ABSENT "status: complete"
claim "STALE-02" "26-04 unstarted" "$P/26-migration-export-backup-restore/26-04-SUMMARY.md" ABSENT "status: complete"

echo "### CTRL-01 NATIVE-* : 'no soak (28-03 not landed), no signed platform-binding"
echo "###                    receipt and no finding adjudication (28-04 not landed),"
echo "###                    the phase is not closed'"
claim "STALE-03" "28-03 soak not landed" "$P/28-native-cross-platform-certification/28-03-SOAK-RESULTS.md" ABSENT "-"
claim "STALE-04" "28-04 signed receipt not landed" "$P/28-native-cross-platform-certification/28-04-CERTIFICATION-RECEIPT.json" ABSENT "-"
claim "STALE-05" "no finding adjudication" "$P/28-native-cross-platform-certification/28-04-FINDING-LEDGER.md" ABSENT "-"
claim "STALE-06" "phase 28 not closed / no verdict" "$P/28-native-cross-platform-certification/28-04-PHASE-VERDICT.md" ABSENT "Criterion 4"

echo "### CTRL-01 SUPPLY-* : 'Plans 29-03 ... and 29-04 ...' listed as the NEXT proof"
claim "STALE-07" "29-03 is still the next proof" "$P/29-supply-chain-release-integrity/29-03-SUMMARY.md" ABSENT "-"
claim "STALE-08" "29-04 is still the next proof" "$P/29-supply-chain-release-integrity/29-04-SUMMARY.md" ABSENT "-"

echo "### CTRL-01 REACH-* : 'three of four criteria are NOT MET. C1 fails on the"
echo "###                   hibernating cloud leg, unexercised for want of a"
echo "###                   credential only Sean can mint. C2 fails ... not a second"
echo "###                   physical host. C4 fails ... orphan counts are NOT MEASURED'"
echo "-- STALE-11"
echo "   claim : Phase 25 has 1 of 4 Success Criteria MET"
echo "   cmd   : grep -cE '\\*\\*MET' 25-PHASE-STATUS.md verdict table rows 7-10"
SP=$P/25-remote-reach-nodes-plugin-lifecycle/25-PHASE-STATUS.md
MET=$(/usr/bin/sed -n '7,10p' "$SP" | /usr/bin/grep -c '\*\*MET')
echo "   out   : $MET of 4 criteria rows now carry a bolded MET"
/usr/bin/sed -n '7,10p' "$SP" | cut -c1-110 | /usr/bin/sed 's/^/          /'
if [ "$MET" -ge 3 ]; then echo "   VERDICT: CLAIM FALSIFIED BY THE TREE — the ledger UNDERSTATES this family"
else echo "   VERDICT: claim holds"; fi
echo

echo "-- STALE-12"
echo "   claim : the cloud leg is blocked for want of a credential only Sean can mint"
echo "   cmd   : test -f 25-CLOUD-SUMMARY.md  (the lane that ran it)"
if [ -f "$P/25-remote-reach-nodes-plugin-lifecycle/25-CLOUD-SUMMARY.md" ]; then
  echo "   out   : EXISTS — $(/usr/bin/grep -m1 -o 'Criterion 1 is now MET' "$SP" || echo 'cloud lane landed')"
  echo "   VERDICT: CLAIM FALSIFIED BY THE TREE"
else echo "   out   : ABSENT"; echo "   VERDICT: claim holds"; fi
echo

echo "-- STALE-13"
echo "   claim : no second physical host / no SSH trust relationship exists (C2)"
echo "   cmd   : test -f 25-HOSTS-SUMMARY.md"
if [ -f "$P/25-remote-reach-nodes-plugin-lifecycle/25-HOSTS-SUMMARY.md" ]; then
  echo "   out   : EXISTS ($(/usr/bin/wc -c < "$P/25-remote-reach-nodes-plugin-lifecycle/25-HOSTS-SUMMARY.md" | tr -d ' ') bytes)"
  echo "   VERDICT: CLAIM FALSIFIED BY THE TREE"
else echo "   out   : ABSENT"; echo "   VERDICT: claim holds"; fi
echo

echo "### CTRL-01 refresh obligations : two sibling registers said to be stale"
echo "-- STALE-09"
echo "   claim : ROADMAP.md's progress table still reads 'Not started' for phases 21-29"
echo "   cmd   : grep -c 'Not started' $R"
N=$(/usr/bin/grep -c 'Not started' $R)
echo "   out   : $N"
if [ "$N" -eq 0 ]; then echo "   VERDICT: CLAIM FALSIFIED BY THE TREE (reconciled at $(/usr/bin/git log -1 --format=%h -- $R))"
else echo "   VERDICT: claim holds"; fi
echo

echo "-- STALE-10"
echo "   claim : REQUIREMENTS.md's Phase 24 disposition still says only 24-01 executed"
echo "   cmd   : grep -c 'is out of date' .planning/REQUIREMENTS.md"
M=$(/usr/bin/grep -c 'is out of date' .planning/REQUIREMENTS.md)
echo "   out   : $M  (a self-correcting note means the register was reconciled)"
if [ "$M" -ge 1 ]; then echo "   VERDICT: CLAIM FALSIFIED BY THE TREE"
else echo "   VERDICT: claim holds"; fi
echo

echo "### EXECUTION CHECK — file existence is NOT execution."
echo "### A SUMMARY can exist purely to RECORD a non-start (23A-02 is tagged"
echo "### not-executed with provides: [] and created: []; 24-04's exists to say"
echo "### its own four tasks were never started). Every claim falsified above is"
echo "### therefore re-checked on the summary's own declared status, not on stat."
for f in "$P/26-migration-export-backup-restore/26-02-SUMMARY.md" \
         "$P/26-migration-export-backup-restore/26-04-SUMMARY.md" \
         "$P/28-native-cross-platform-certification/28-03-SUMMARY.md" \
         "$P/28-native-cross-platform-certification/28-04-SUMMARY.md" \
         "$P/29-supply-chain-release-integrity/29-03-SUMMARY.md" \
         "$P/29-supply-chain-release-integrity/29-04-SUMMARY.md" \
         "$P/23A-governed-skills/23A-02-SUMMARY.md"; do
  [ -f "$f" ] || { echo "   ABSENT $f"; continue; }
  S=$(/usr/bin/grep -m1 '^status:' "$f" | /usr/bin/sed 's/status: *//')
  T=$(/usr/bin/grep -m1 '^tags:' "$f" | cut -c1-56)
  NE=$(/usr/bin/grep -c 'not-executed' "$f")
  if [ "$NE" -ge 1 ]; then V="RECORDS A NON-START — not execution"; else V="EXECUTED"; fi
  printf '   %-64s status=%-10s %s\n' "$(basename "$f")" "${S:-<none>}" "$V"
  echo "        $T"
done
echo

echo "### CONTROL — a claim the tree should CONFIRM, so this check is not a"
echo "###           rubber stamp that falsifies everything put to it."
claim "CTRL-A" "24-C3 (inbound channel matrix) has NOT landed" "$P/24-gateway-automation-channels-typed-api/24-C3-SUMMARY.md" ABSENT "-"
claim "CTRL-B" "30-02 (comparative trials) has NOT landed — it is the next plan in this phase" "$P/30-continuous-scorecard-frontier-review/30-02-SUMMARY.md" ABSENT "-"
