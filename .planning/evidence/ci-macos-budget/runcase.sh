#!/bin/bash
export GITHUB_OUTPUT=$(mktemp) GITHUB_STEP_SUMMARY=$(mktemp)
export EVENT_NAME="$1" REF_NAME="$2" HEAD_MSG="$3" PUSH_MSGS="$4"
bash budget_script.sh > /tmp/wlci/case.log 2>&1
rc=$?
if [ $rc -ne 0 ]; then echo "SCRIPT_FAILED rc=$rc"; cat /tmp/wlci/case.log; exit $rc; fi
d=$(grep '^darwin=' "$GITHUB_OUTPUT" | cut -d= -f2)
ci=$(grep '^ci_os=' "$GITHUB_OUTPUT" | cut -d= -f2-)
bm=$(grep '^build_matrix=' "$GITHUB_OUTPUT" | cut -d= -f2-)
nmac_ci=$(echo "$ci" | jq '[.[]|select(.=="macos-latest")]|length')
nmac_b=$(echo "$bm" | jq '[.include[]|select(.os=="macos-latest")]|length')
ntot_b=$(echo "$bm" | jq '.include|length')
echo "darwin=$d macos_ci_cells=$nmac_ci macos_build_cells=$nmac_b build_total=$ntot_b"
