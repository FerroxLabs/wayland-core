#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 --capture <task-base-file> | <task-base-file> <required-path>..." >&2
  exit 64
}

scripts_dir=$(cd "$(dirname "$0")" && pwd -P)
authority_helper="$scripts_dir/task-base-authority.mjs"

validate_base_file() {
  local file=$1 content commit tree expected_tree
  content=$(node "$authority_helper" read "$file")
  commit=$(printf '%s\n' "$content" | sed -n '1p')
  tree=$(printf '%s\n' "$content" | sed -n '2p')
  git cat-file -e "${commit}^{commit}"
  expected_tree=$(git rev-parse "${commit}^{tree}")
  [[ "$tree" == "$expected_tree" ]] || {
    echo "TASK_BASE tree does not match its commit: $file" >&2
    return 1
  }
  validated_base=$commit
  validated_tree=$tree
}

if [[ ${1:-} == --capture ]]; then
  [[ $# -eq 2 ]] || usage
  base_file=$2
  if [[ -e "$base_file" || -L "$base_file" ]]; then
    validate_base_file "$base_file"
    git merge-base --is-ancestor "$validated_base" HEAD
    printf 'scope-base-reused commit=%s tree=%s\n' \
      "$validated_base" "$validated_tree"
    exit 0
  fi
  [[ -z "$(git status --porcelain)" ]] || {
    echo "cannot capture TASK_BASE from a dirty checkout" >&2
    exit 1
  }
  mkdir -p "$(dirname "$base_file")"
  node "$authority_helper" capture "$base_file" "$(git rev-parse HEAD)" "$(git rev-parse HEAD^{tree})"
  validate_base_file "$base_file"
  printf 'scope-base-captured commit=%s tree=%s\n' \
    "$validated_base" "$validated_tree"
  exit 0
fi

[[ $# -ge 2 ]] || usage
base_file=$1
shift
required=("$@")

validate_base_file "$base_file"
base=$validated_base
git merge-base --is-ancestor "$base" HEAD

observed=()

contains() {
  local needle=$1 item
  shift
  for item in "$@"; do
    [[ "$item" == "$needle" ]] && return 0
  done
  return 1
}

add_path() {
  local path=$1
  contains "$path" "${observed[@]:-}" || observed+=("$path")
}

collect_name_status() {
  local status first second
  while IFS= read -r -d '' status; do
    IFS= read -r -d '' first || {
      echo "truncated git name-status stream after $status" >&2
      exit 1
    }
    add_path "$first"
    case "$status" in
      R*|C*)
        IFS= read -r -d '' second || {
          echo "truncated rename/copy record after $first" >&2
          exit 1
        }
        add_path "$second"
        ;;
    esac
  done
}

collect_name_status < <(git diff --name-status -z --find-renames --find-copies-harder "$base"..HEAD)
collect_name_status < <(git diff --cached --name-status -z --find-renames --find-copies-harder)
collect_name_status < <(git diff --name-status -z --find-renames --find-copies-harder)

while IFS= read -r -d '' path; do
  add_path "$path"
done < <(git ls-files --others --exclude-standard -z)

for path in "${observed[@]:-}"; do
  [[ -n "$path" ]] || continue
  contains "$path" "${required[@]}" || {
    echo "out-of-scope path: $path" >&2
    exit 1
  }
done

for path in "${required[@]}"; do
  contains "$path" "${observed[@]:-}" || {
    echo "required path absent from complete TASK_BASE scope union: $path" >&2
    exit 1
  }
done

printf 'scope-ok base=%s paths=%s\n' "$base" "${#observed[@]}"
