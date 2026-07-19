#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 <source-sha> <review-base-sha> <review-sha> <summary-file> <review-file> <source-path>..." >&2
  exit 64
}

[[ $# -ge 6 ]] || usage
source_sha=$1
review_base_sha=$2
review_sha=$3
summary_file=$4
review_file=$5
shift 5
source_paths=("$@")

git cat-file -e "${source_sha}^{commit}"
git cat-file -e "${review_base_sha}^{commit}"
git cat-file -e "${review_sha}^{commit}"

metadata_history=$(git rev-list --reverse --ancestry-path "${source_sha}..${review_base_sha}")
[[ -n "$metadata_history" ]] || {
  echo "review base does not descend from source through a metadata chain" >&2
  exit 1
}

expected_parent=$source_sha
summary_commit_count=0
while IFS= read -r metadata_commit; do
  [[ -n "$metadata_commit" ]] || continue
  parent_line=$(git rev-list --parents -n 1 "$metadata_commit")
  read -r -a parent_fields <<< "$parent_line"
  [[ ${#parent_fields[@]} -eq 2 && "${parent_fields[1]}" == "$expected_parent" ]] || {
    echo "source-to-review-base metadata history is not linear and merge-free" >&2
    exit 1
  }

  commit_changed=()
  while IFS= read -r -d '' status; do
    IFS= read -r -d '' first || exit 1
    commit_changed+=("$first")
    case "$status" in
      R*|C*)
        IFS= read -r -d '' second || exit 1
        commit_changed+=("$second")
        ;;
    esac
  done < <(git diff --name-status -z --find-renames --find-copies-harder "$expected_parent".."$metadata_commit")

  for path in "${commit_changed[@]}"; do
    case "$path" in
      "$summary_file") summary_commit_count=$((summary_commit_count + 1)) ;;
      .planning/STATE.md|.planning/ROADMAP.md|.planning/REQUIREMENTS.md) ;;
      *)
        echo "source-to-review-base changed non-stock metadata path: $path" >&2
        exit 1
        ;;
    esac
  done
  expected_parent=$metadata_commit
done <<< "$metadata_history"
[[ "$expected_parent" == "$review_base_sha" && "$summary_commit_count" == 1 ]] || {
  echo "metadata chain must reach review base and change the summary exactly once" >&2
  exit 1
}

parent_line=$(git rev-list --parents -n 1 "$review_sha")
read -r -a parent_fields <<< "$parent_line"
[[ ${#parent_fields[@]} -eq 2 && "${parent_fields[1]}" == "$review_base_sha" ]] || {
  echo "review commit is not the sole-parent child of review base" >&2
  exit 1
}

review_changed=()
while IFS= read -r -d '' status; do
  IFS= read -r -d '' first || exit 1
  review_changed+=("$first")
  case "$status" in
    R*|C*)
      IFS= read -r -d '' second || exit 1
      review_changed+=("$second")
      ;;
  esac
done < <(git diff --name-status -z --find-renames --find-copies-harder "$review_base_sha".."$review_sha")

[[ ${#review_changed[@]} -eq 1 && "${review_changed[0]}" == "$review_file" ]] || {
  printf 'review commit changed paths other than %s:' "$review_file" >&2
  printf ' %q' "${review_changed[@]:-}" >&2
  printf '\n' >&2
  exit 1
}

combined_changed=()
while IFS= read -r -d '' status; do
  IFS= read -r -d '' first || exit 1
  combined_changed+=("$first")
  case "$status" in
    R*|C*)
      IFS= read -r -d '' second || exit 1
      combined_changed+=("$second")
      ;;
  esac
done < <(git diff --name-status -z --find-renames --find-copies-harder "$source_sha".."$review_sha")

contains_summary=false
contains_review=false
for path in "${combined_changed[@]}"; do
  if [[ "$path" == "$summary_file" ]]; then
    contains_summary=true
  fi
  if [[ "$path" == "$review_file" ]]; then
    contains_review=true
  fi
done

for path in "${combined_changed[@]}"; do
  case "$path" in
    "$summary_file"|"$review_file"|.planning/STATE.md|.planning/ROADMAP.md|.planning/REQUIREMENTS.md) ;;
    *)
      echo "source-to-review changed non-stock metadata path: $path" >&2
      exit 1
      ;;
  esac
done
[[ "$contains_summary" == true && "$contains_review" == true ]] || {
  printf 'source-to-review delta is missing %s or %s:' "$summary_file" "$review_file" >&2
  printf ' %q' "${combined_changed[@]:-}" >&2
  printf '\n' >&2
  exit 1
}

for path in "${source_paths[@]}"; do
  source_blob=$(git rev-parse "${source_sha}:${path}")
  base_blob=$(git rev-parse "${review_base_sha}:${path}")
  review_blob=$(git rev-parse "${review_sha}:${path}")
  [[ "$source_blob" == "$base_blob" && "$source_blob" == "$review_blob" ]] || {
    echo "source blob changed in review commit: $path" >&2
    exit 1
  }
done

printf 'review-pair-ok source=%s review_base=%s review=%s\n' "$source_sha" "$review_base_sha" "$review_sha"
