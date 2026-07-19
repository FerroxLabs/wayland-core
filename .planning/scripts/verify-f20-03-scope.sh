#!/usr/bin/env bash
set -euo pipefail

mode=${1:-}
case "$mode" in
  preflight|task|final) ;;
  *)
    echo "usage: $0 preflight|task|final" >&2
    exit 64
    ;;
esac

state_file=$(git rev-parse --git-path f20-03-accepted-plan)
[[ -f "$state_file" && ! -L "$state_file" ]]
state_lines=$(wc -l < "$state_file" | tr -d ' ')
[[ "$state_lines" == 2 ]]
F20_03_ACCEPTED_PLAN_COMMIT=$(sed -n '1p' "$state_file")
F20_03_ACCEPTED_PLAN_TREE=$(sed -n '2p' "$state_file")
[[ "$F20_03_ACCEPTED_PLAN_COMMIT" =~ ^[0-9a-f]{40,64}$ ]]
[[ "$F20_03_ACCEPTED_PLAN_TREE" =~ ^[0-9a-f]{40,64}$ ]]

head_sha=$(git rev-parse HEAD)
head_tree=$(git rev-parse HEAD^{tree})
accepted_tree=$(git rev-parse "${F20_03_ACCEPTED_PLAN_COMMIT}^{tree}")
[[ "$accepted_tree" == "$F20_03_ACCEPTED_PLAN_TREE" ]]

source_sha=94f014d039b8babf3f5926385a3bbc5cb5cf3c41
source_base=$(git merge-base "$source_sha" HEAD)
[[ "$source_base" == "$source_sha" ]]

status=$(git status --porcelain)

if [[ "$mode" == preflight ]]; then
  [[ -z "$status" ]]
  [[ "$head_sha" == "$F20_03_ACCEPTED_PLAN_COMMIT" ]]
  [[ "$head_tree" == "$F20_03_ACCEPTED_PLAN_TREE" ]]
  printf 'f20-03-scope-ok mode=preflight base=%s tree=%s\n' "$head_sha" "$head_tree"
  exit 0
fi

F20_03_EXECUTION_BASE=$F20_03_ACCEPTED_PLAN_COMMIT
F20_03_EXECUTION_BASE_TREE=$F20_03_ACCEPTED_PLAN_TREE
execution_tree=$(git rev-parse "${F20_03_EXECUTION_BASE}^{tree}")
[[ "$execution_tree" == "$F20_03_EXECUTION_BASE_TREE" ]]
execution_base=$(git merge-base "$F20_03_EXECUTION_BASE" HEAD)
[[ "$execution_base" == "$F20_03_EXECUTION_BASE" ]]

tmp=$(mktemp -d "${TMPDIR:-/tmp}/f20-03-scope.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

node /Users/seandonahoe/.codex/gsd-core/bin/gsd-tools.cjs phase-plan-index 20 --raw > "$tmp/index.json"
jq -r '.plans[] | select(.id == "20-03") | .files_modified[]' "$tmp/index.json" > "$tmp/canonical.unsorted"
LC_ALL=C sort -u "$tmp/canonical.unsorted" > "$tmp/canonical"
canonical_count=$(wc -l < "$tmp/canonical" | tr -d ' ')
[[ "$canonical_count" == 41 ]]

if [[ "$mode" == task ]]; then
  git diff --name-only > "$tmp/unstaged"
  [[ ! -s "$tmp/unstaged" ]]
  git ls-files --others --exclude-standard > "$tmp/untracked"
  [[ ! -s "$tmp/untracked" ]]
  git diff --cached --name-only "$F20_03_EXECUTION_BASE" > "$tmp/actual.unsorted"
  git diff --cached --name-status "$F20_03_EXECUTION_BASE" > "$tmp/status"
else
  [[ -z "$status" ]]
  git diff --name-only "$F20_03_EXECUTION_BASE..HEAD" > "$tmp/actual.unsorted"
  git diff --name-status "$F20_03_EXECUTION_BASE..HEAD" > "$tmp/status"
fi

LC_ALL=C sort -u "$tmp/actual.unsorted" > "$tmp/actual"
comm -13 "$tmp/canonical" "$tmp/actual" > "$tmp/outside"
[[ ! -s "$tmp/outside" ]]

awk '$1 ~ /^[DR]/ { print }' "$tmp/status" > "$tmp/destructive"
[[ ! -s "$tmp/destructive" ]]

if [[ "$mode" == final ]]; then
  cmp -s "$tmp/canonical" "$tmp/actual"
fi

actual_count=$(wc -l < "$tmp/actual" | tr -d ' ')
printf 'f20-03-scope-ok mode=%s base=%s head=%s paths=%s\n' \
  "$mode" "$F20_03_EXECUTION_BASE" "$head_sha" "$actual_count"
