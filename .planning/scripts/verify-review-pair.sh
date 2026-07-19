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

parent_line=$(git rev-list --parents -n 1 "$review_base_sha")
read -r -a parent_fields <<< "$parent_line"
[[ ${#parent_fields[@]} -eq 2 && "${parent_fields[1]}" == "$source_sha" ]] || {
  echo "review base is not the sole-parent summary child of source commit" >&2
  exit 1
}

map_changed=()
while IFS= read -r -d '' status; do
  IFS= read -r -d '' first || exit 1
  map_changed+=("$first")
  case "$status" in
    R*|C*)
      IFS= read -r -d '' second || exit 1
      map_changed+=("$second")
      ;;
  esac
done < <(git diff --name-status -z --find-renames --find-copies-harder "$source_sha".."$review_base_sha")

[[ ${#map_changed[@]} -eq 1 && "${map_changed[0]}" == "$summary_file" ]] || {
  echo "source-to-review-base metadata delta is not exactly $summary_file" >&2
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

[[ ${#combined_changed[@]} -eq 2 && "$contains_summary" == true && "$contains_review" == true ]] || {
    printf 'source-to-review delta is not exactly %s plus %s:' "$summary_file" "$review_file" >&2
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
