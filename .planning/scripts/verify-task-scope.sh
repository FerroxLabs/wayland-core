#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage:
  verify-task-scope.sh --capture <task-base-directory>
  verify-task-scope.sh --start-fresh <task-base-directory> <old-generation> <reconciled-head>
  verify-task-scope.sh --complete <task-base-directory> <generation> <reconciled-head>
  verify-task-scope.sh <task-base-directory> <required-path>...
EOF
  exit 64
}

scripts_dir=$(cd "$(dirname "$0")" && pwd -P)
authority_helper="$scripts_dir/task-base-authority.mjs"

derive_plan_id() {
  local name
  name=$(basename "$1")
  [[ "$name" =~ ^gsd-task-base-([0-9]+([.][0-9]+)?-[0-9]+)$ ]] || {
    echo "task authority path must end in gsd-task-base-<phase>-<plan>: $1" >&2
    return 1
  }
  plan_id=${BASH_REMATCH[1]}
}

require_clean_checkout() {
  local status
  status=$(git status --porcelain) || {
    echo "cannot verify checkout cleanliness" >&2
    return 1
  }
  [[ -z "$status" ]] || {
    echo "cannot change TASK_BASE generation from a dirty checkout" >&2
    return 1
  }
}

validate_base_file() {
  local file=$1 operation=${2:-task-current} content commit tree generation expected_tree
  derive_plan_id "$file"
  content=$(node "$authority_helper" "$operation" "$file" "$plan_id")
  commit=$(printf '%s\n' "$content" | sed -n '1p')
  tree=$(printf '%s\n' "$content" | sed -n '2p')
  generation=$(printf '%s\n' "$content" | sed -n '3p')
  git cat-file -e "${commit}^{commit}"
  expected_tree=$(git rev-parse "${commit}^{tree}")
  [[ "$tree" == "$expected_tree" ]] || {
    echo "TASK_BASE tree does not match its commit: $file" >&2
    return 1
  }
  git merge-base --is-ancestor "$commit" HEAD || {
    echo "TASK_BASE is not an ancestor of HEAD: $file" >&2
    return 1
  }
  validated_base=$commit
  validated_tree=$tree
  validated_generation=$generation
}

if [[ ${1:-} == --capture ]]; then
  [[ $# -eq 2 ]] || usage
  base_file=$2
  derive_plan_id "$base_file"
  if [[ -e "$base_file" || -L "$base_file" ]]; then
    if [[ -d "$base_file" && -f "$base_file/state.json" ]]; then
      validate_base_file "$base_file"
      printf 'scope-base-reused commit=%s tree=%s generation=%s\n' \
        "$validated_base" "$validated_tree" "$validated_generation"
      exit 0
    fi
    [[ -d "$base_file" && ! -L "$base_file" ]] || {
      echo "existing TASK_BASE is not a task authority directory: $base_file" >&2
      exit 1
    }
  fi
  require_clean_checkout
  mkdir -p "$(dirname "$base_file")"
  node "$authority_helper" task-begin \
    "$base_file" "$plan_id" "$(git rev-parse HEAD)" "$(git rev-parse HEAD^{tree})" >/dev/null
  validate_base_file "$base_file"
  printf 'scope-base-captured commit=%s tree=%s generation=%s\n' \
    "$validated_base" "$validated_tree" "$validated_generation"
  exit 0
fi

if [[ ${1:-} == --start-fresh ]]; then
  [[ $# -eq 4 ]] || usage
  base_file=$2
  old_generation=$3
  reconciled_head=$4
  derive_plan_id "$base_file"
  require_clean_checkout
  [[ "$(git rev-parse HEAD)" == "$reconciled_head" ]] || {
    echo "Start Fresh reconciled HEAD does not match current HEAD" >&2
    exit 1
  }
  node "$authority_helper" task-start-fresh \
    "$base_file" "$plan_id" "$old_generation" "$reconciled_head" \
    "$(git rev-parse "${reconciled_head}^{tree}")" >/dev/null
  validate_base_file "$base_file"
  [[ "$validated_base" == "$reconciled_head" ]] || {
    echo "Start Fresh did not publish the reconciled HEAD" >&2
    exit 1
  }
  printf 'scope-base-started-fresh commit=%s tree=%s generation=%s previous=%s\n' \
    "$validated_base" "$validated_tree" "$validated_generation" "$old_generation"
  exit 0
fi

if [[ ${1:-} == --complete ]]; then
  [[ $# -eq 4 ]] || usage
  base_file=$2
  generation=$3
  reconciled_head=$4
  derive_plan_id "$base_file"
  require_clean_checkout
  [[ "$(git rev-parse HEAD)" == "$reconciled_head" ]] || {
    echo "completion HEAD does not match current HEAD" >&2
    exit 1
  }
  validate_base_file "$base_file" task-tip
  [[ "$validated_generation" == "$generation" ]] || {
    echo "named completion generation is not active" >&2
    exit 1
  }
  node "$authority_helper" task-complete \
    "$base_file" "$plan_id" "$generation" "$reconciled_head" >/dev/null
  printf 'scope-base-completed commit=%s generation=%s\n' "$reconciled_head" "$generation"
  exit 0
fi

[[ $# -ge 2 ]] || usage
base_file=$1
shift
required=("$@")

validate_base_file "$base_file"
base=$validated_base

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

scope_stream_dir=$(mktemp -d "${TMPDIR:-/tmp}/wayland-task-scope.XXXXXXXX")
chmod 700 "$scope_stream_dir"
trap 'rm -rf -- "$scope_stream_dir"' EXIT

capture_name_status() {
  local name=$1
  shift
  if ! git "$@" >"$scope_stream_dir/$name"; then
    echo "cannot collect TASK_BASE scope stream: $name" >&2
    return 1
  fi
  collect_name_status <"$scope_stream_dir/$name"
}

capture_name_status committed diff --name-status -z --find-renames --find-copies-harder "$base"..HEAD
capture_name_status cached diff --cached --name-status -z --find-renames --find-copies-harder
capture_name_status unstaged diff --name-status -z --find-renames --find-copies-harder

if ! git ls-files --others --exclude-standard -z >"$scope_stream_dir/untracked"; then
  echo "cannot collect TASK_BASE untracked-file stream" >&2
  exit 1
fi
while IFS= read -r -d '' path; do
  add_path "$path"
done <"$scope_stream_dir/untracked"

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

printf 'scope-ok base=%s generation=%s paths=%s\n' \
  "$base" "$validated_generation" "${#observed[@]}"
