#!/usr/bin/env bash
set -euo pipefail

scripts_dir=$(cd "$(dirname "$0")" && pwd -P)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/phase20-proof-scripts.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

git -C "$tmp" init -q
git -C "$tmp" config user.name 'Phase 20 proof self-test'
git -C "$tmp" config user.email 'phase20-proof@example.invalid'

printf 'source\n' > "$tmp/source.rs"
git -C "$tmp" add source.rs
git -C "$tmp" commit -qm 'source'
source_sha=$(git -C "$tmp" rev-parse HEAD)

printf 'summary\n' > "$tmp/20-06-SUMMARY.md"
mkdir -p "$tmp/.planning"
printf 'requirements\n' > "$tmp/.planning/REQUIREMENTS.md"
git -C "$tmp" add 20-06-SUMMARY.md .planning/REQUIREMENTS.md
git -C "$tmp" commit -qm 'summary'
printf 'state\n' > "$tmp/.planning/STATE.md"
printf 'roadmap\n' > "$tmp/.planning/ROADMAP.md"
git -C "$tmp" add .planning/STATE.md .planning/ROADMAP.md
git -C "$tmp" commit -qm 'stock gsd tracking'
review_base_sha=$(git -C "$tmp" rev-parse HEAD)

printf 'review\n' > "$tmp/20-06-INTERFACE-REVIEWS.md"
git -C "$tmp" add 20-06-INTERFACE-REVIEWS.md
git -C "$tmp" commit -qm 'review'
review_sha=$(git -C "$tmp" rev-parse HEAD)

git -C "$tmp" --work-tree="$tmp" \
  -c core.safecrlf=false \
  -c core.autocrlf=false \
  diff --check
(
  cd "$tmp"
  bash "$scripts_dir/verify-review-pair.sh" \
    "$source_sha" "$review_base_sha" "$review_sha" \
    20-06-SUMMARY.md 20-06-INTERFACE-REVIEWS.md source.rs
)

printf 'tampered\n' >> "$tmp/source.rs"
git -C "$tmp" add source.rs
git -C "$tmp" commit -qm 'tamper reviewed source'
tampered_review_sha=$(git -C "$tmp" rev-parse HEAD)
if (
  cd "$tmp"
  bash "$scripts_dir/verify-review-pair.sh" \
    "$source_sha" "$review_base_sha" "$tampered_review_sha" \
    20-06-SUMMARY.md 20-06-INTERFACE-REVIEWS.md source.rs
) >/dev/null 2>&1; then
  echo 'review verifier accepted a non-sole-parent or source-tampered successor' >&2
  exit 1
fi

git -C "$tmp" reset -q --hard "$source_sha"
git -C "$tmp" clean -qfd
mkdir -p "$tmp/.planning"
printf 'summary\n' > "$tmp/20-06-SUMMARY.md"
printf 'unapproved metadata\n' > "$tmp/other.md"
git -C "$tmp" add 20-06-SUMMARY.md other.md
git -C "$tmp" commit -qm 'bad summary metadata'
bad_review_base_sha=$(git -C "$tmp" rev-parse HEAD)
printf 'review\n' > "$tmp/20-06-INTERFACE-REVIEWS.md"
git -C "$tmp" add 20-06-INTERFACE-REVIEWS.md
git -C "$tmp" commit -qm 'review bad summary'
bad_review_sha=$(git -C "$tmp" rev-parse HEAD)
if (
  cd "$tmp"
  bash "$scripts_dir/verify-review-pair.sh" \
    "$source_sha" "$bad_review_base_sha" "$bad_review_sha" \
    20-06-SUMMARY.md 20-06-INTERFACE-REVIEWS.md source.rs
) >/dev/null 2>&1; then
  echo 'review verifier accepted non-stock summary metadata' >&2
  exit 1
fi

git -C "$tmp" reset -q --hard "$source_sha"
git -C "$tmp" clean -qfd
mkdir -p "$tmp/.planning"
printf 'summary\n' > "$tmp/20-06-SUMMARY.md"
printf 'temporarily tampered\n' > "$tmp/source.rs"
git -C "$tmp" add 20-06-SUMMARY.md source.rs
git -C "$tmp" commit -qm 'tamper inside metadata chain'
printf 'source\n' > "$tmp/source.rs"
printf 'state\n' > "$tmp/.planning/STATE.md"
git -C "$tmp" add source.rs .planning/STATE.md
git -C "$tmp" commit -qm 'restore source before review'
restored_review_base_sha=$(git -C "$tmp" rev-parse HEAD)
printf 'review\n' > "$tmp/20-06-INTERFACE-REVIEWS.md"
git -C "$tmp" add 20-06-INTERFACE-REVIEWS.md
git -C "$tmp" commit -qm 'review restored source'
restored_review_sha=$(git -C "$tmp" rev-parse HEAD)
if (
  cd "$tmp"
  bash "$scripts_dir/verify-review-pair.sh" \
    "$source_sha" "$restored_review_base_sha" "$restored_review_sha" \
    20-06-SUMMARY.md 20-06-INTERFACE-REVIEWS.md source.rs
) >/dev/null 2>&1; then
  echo 'review verifier accepted source tampering hidden inside metadata history' >&2
  exit 1
fi

git -C "$tmp" reset -q --hard "$source_sha"
git -C "$tmp" clean -qfd
mkdir -p "$tmp/.planning"
printf 'summary one\n' > "$tmp/20-06-SUMMARY.md"
git -C "$tmp" add 20-06-SUMMARY.md
git -C "$tmp" commit -qm 'first summary mutation'
printf 'summary two\n' > "$tmp/20-06-SUMMARY.md"
git -C "$tmp" add 20-06-SUMMARY.md
git -C "$tmp" commit -qm 'second summary mutation'
duplicate_summary_base_sha=$(git -C "$tmp" rev-parse HEAD)
printf 'review\n' > "$tmp/20-06-INTERFACE-REVIEWS.md"
git -C "$tmp" add 20-06-INTERFACE-REVIEWS.md
git -C "$tmp" commit -qm 'review duplicate summary'
duplicate_summary_review_sha=$(git -C "$tmp" rev-parse HEAD)
if (
  cd "$tmp"
  bash "$scripts_dir/verify-review-pair.sh" \
    "$source_sha" "$duplicate_summary_base_sha" "$duplicate_summary_review_sha" \
    20-06-SUMMARY.md 20-06-INTERFACE-REVIEWS.md source.rs
) >/dev/null 2>&1; then
  echo 'review verifier accepted multiple summary mutations' >&2
  exit 1
fi

git -C "$tmp" reset -q --hard "$review_sha"
git -C "$tmp" clean -qfd
printf 'delete\n' > "$tmp/delete-me.txt"
printf 'rename\n' > "$tmp/rename-me.txt"
printf 'copy\n' > "$tmp/copy-me.txt"
git -C "$tmp" add delete-me.txt rename-me.txt copy-me.txt
git -C "$tmp" commit -qm 'scope fixtures'
scope_base_sha=$(git -C "$tmp" rev-parse HEAD)
base_file="$(git -C "$tmp" rev-parse --absolute-git-dir)/phase20-task-base"
printf '%s\n' "$scope_base_sha" > "$base_file"
printf 'tracked\n' > "$tmp/tracked.txt"
git -C "$tmp" add tracked.txt
git -C "$tmp" commit -qm 'tracked scope'
printf 'staged\n' > "$tmp/staged.txt"
git -C "$tmp" add staged.txt
printf 'unstaged\n' >> "$tmp/tracked.txt"
printf 'untracked\n' > "$tmp/untracked.txt"
rm "$tmp/delete-me.txt"
git -C "$tmp" mv rename-me.txt renamed.txt
cp "$tmp/copy-me.txt" "$tmp/copied.txt"
git -C "$tmp" add copied.txt
(
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" "$base_file" \
    tracked.txt staged.txt untracked.txt delete-me.txt \
    rename-me.txt renamed.txt copy-me.txt copied.txt
)

printf 'forbidden\n' > "$tmp/forbidden.txt"
if (
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" "$base_file" \
    tracked.txt staged.txt untracked.txt delete-me.txt \
    rename-me.txt renamed.txt copy-me.txt copied.txt
) >/dev/null 2>&1; then
  echo 'scope verifier accepted an untracked out-of-scope path' >&2
  exit 1
fi

printf 'phase20-proof-script-tests-ok\n'
