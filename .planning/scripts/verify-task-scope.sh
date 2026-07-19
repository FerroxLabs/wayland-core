#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 <task-base-file> <required-path>..." >&2
  exit 64
}

[[ $# -ge 2 ]] || usage
base_file=$1
shift
required=("$@")

[[ -f "$base_file" ]] || {
  echo "missing recorded TASK_BASE: $base_file" >&2
  exit 1
}

base=$(tr -d '\r\n' < "$base_file")
git cat-file -e "${base}^{commit}"
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
