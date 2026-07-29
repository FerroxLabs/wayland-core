#!/bin/bash
# Gate over the budget decision script. Exits non-zero on any mismatch.
SCRIPT="${1:-budget_script.sh}"
fails=0
check(){ # name event ref headmsg pushmsgs expected_darwin
  local name="$1" ev="$2" ref="$3" hm="$4" pm="$5" exp="$6"
  export GITHUB_OUTPUT=$(mktemp) GITHUB_STEP_SUMMARY=$(mktemp)
  export EVENT_NAME="$ev" REF_NAME="$ref" HEAD_MSG="$hm" PUSH_MSGS="$pm"
  bash "$SCRIPT" >/dev/null 2>&1 || { echo "  FAIL $name: script exited non-zero"; fails=$((fails+1)); return; }
  local got=$(grep '^darwin=' "$GITHUB_OUTPUT" | cut -d= -f2)
  if [ "$got" != "$exp" ]; then echo "  FAIL $name: darwin=$got expected=$exp"; fails=$((fails+1));
  else echo "  ok   $name (darwin=$got)"; fi
}
check "lane/no-token->false"      push lane/x "docs"                '["docs"]'                false
check "lane/[ci-darwin]->true"    push lane/x "[ci-darwin] x"       '["[ci-darwin] x"]'       true
check "lane/[ci-macos]->true"     push lane/x "[ci-macos] x"        '["[ci-macos] x"]'        true
check "lane/token-non-tip->true"  push lane/x "fixup"               '["[ci-darwin] a","fixup"]' true
check "integration->true"         push plan/f20-unified-audit-repair "m" '["m"]'              true
check "main->true"                push main "r"                     '["r"]'                   true
check "pull_request->true"        pull_request main ""              'null'                    true
echo "GATE_FAILURES=$fails"
[ "$fails" -eq 0 ]
