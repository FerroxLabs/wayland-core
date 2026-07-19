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
scripts_dir=$(cd "$(dirname "$0")" && pwd -P)
accepted_plan=$(node "$scripts_dir/task-base-authority.mjs" read "$state_file" 600)
F20_03_ACCEPTED_PLAN_COMMIT=$(printf '%s\n' "$accepted_plan" | sed -n '1p')
F20_03_ACCEPTED_PLAN_TREE=$(printf '%s\n' "$accepted_plan" | sed -n '2p')
[[ "$F20_03_ACCEPTED_PLAN_COMMIT" =~ ^[0-9a-f]{40}([0-9a-f]{24})?$ ]] || exit 1
[[ "$F20_03_ACCEPTED_PLAN_TREE" =~ ^[0-9a-f]{40}([0-9a-f]{24})?$ ]] || exit 1
[[ "$(git rev-parse "${F20_03_ACCEPTED_PLAN_COMMIT}^{commit}")" == "$F20_03_ACCEPTED_PLAN_COMMIT" ]] || exit 1

head_sha=$(git rev-parse HEAD)
head_tree=$(git rev-parse HEAD^{tree})
accepted_tree=$(git rev-parse "${F20_03_ACCEPTED_PLAN_COMMIT}^{tree}")
[[ "$accepted_tree" == "$F20_03_ACCEPTED_PLAN_TREE" ]] || exit 1

source_sha=94f014d039b8babf3f5926385a3bbc5cb5cf3c41
source_base=$(git merge-base "$source_sha" HEAD)
[[ "$source_base" == "$source_sha" ]] || exit 1

status=$(git status --porcelain)

if [[ "$mode" == preflight ]]; then
  [[ -z "$status" ]] || exit 1
  [[ "$head_sha" == "$F20_03_ACCEPTED_PLAN_COMMIT" ]] || exit 1
  [[ "$head_tree" == "$F20_03_ACCEPTED_PLAN_TREE" ]] || exit 1
  printf 'f20-03-scope-ok mode=preflight base=%s tree=%s\n' "$head_sha" "$head_tree"
  exit 0
fi

F20_03_EXECUTION_BASE=$F20_03_ACCEPTED_PLAN_COMMIT
F20_03_EXECUTION_BASE_TREE=$F20_03_ACCEPTED_PLAN_TREE
execution_tree=$(git rev-parse "${F20_03_EXECUTION_BASE}^{tree}")
[[ "$execution_tree" == "$F20_03_EXECUTION_BASE_TREE" ]] || exit 1
execution_base=$(git merge-base "$F20_03_EXECUTION_BASE" HEAD)
[[ "$execution_base" == "$F20_03_EXECUTION_BASE" ]] || exit 1

tmp=$(mktemp -d "${TMPDIR:-/tmp}/f20-03-scope.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

plan_path=.planning/phases/20-transactional-delegated-mutation/20-03-PLAN.md
git show "${F20_03_ACCEPTED_PLAN_COMMIT}:${plan_path}" > "$tmp/accepted-plan.md"
awk '
  /^files_modified:$/ { in_files = 1; next }
  in_files && /^  - / { sub(/^  - /, ""); print; next }
  in_files { exit }
' "$tmp/accepted-plan.md" > "$tmp/canonical.unsorted"
LC_ALL=C sort -u "$tmp/canonical.unsorted" > "$tmp/canonical"
canonical_count=$(wc -l < "$tmp/canonical" | tr -d ' ')
[[ "$canonical_count" == 41 ]] || exit 1

if [[ "$mode" == task ]]; then
  git diff --name-only > "$tmp/unstaged"
  [[ ! -s "$tmp/unstaged" ]] || exit 1
  git ls-files --others --exclude-standard > "$tmp/untracked"
  [[ ! -s "$tmp/untracked" ]] || exit 1
  git diff --cached --name-only "$F20_03_EXECUTION_BASE" > "$tmp/actual.unsorted"
  git diff --cached --name-status "$F20_03_EXECUTION_BASE" > "$tmp/status"
else
  [[ -z "$status" ]] || exit 1
  git diff --name-only "$F20_03_EXECUTION_BASE..HEAD" > "$tmp/actual.unsorted"
  git diff --name-status "$F20_03_EXECUTION_BASE..HEAD" > "$tmp/status"
fi

LC_ALL=C sort -u "$tmp/actual.unsorted" > "$tmp/actual"
comm -13 "$tmp/canonical" "$tmp/actual" > "$tmp/outside"
[[ ! -s "$tmp/outside" ]] || exit 1

awk '$1 ~ /^[DR]/ { print }' "$tmp/status" > "$tmp/destructive"
[[ ! -s "$tmp/destructive" ]] || exit 1

if [[ "$mode" == final ]]; then
  cmp -s "$tmp/canonical" "$tmp/actual" || exit 1
fi

actual_count=$(wc -l < "$tmp/actual" | tr -d ' ')
printf 'f20-03-scope-ok mode=%s base=%s head=%s paths=%s\n' \
  "$mode" "$F20_03_EXECUTION_BASE" "$head_sha" "$actual_count"
