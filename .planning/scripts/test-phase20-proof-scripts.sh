#!/usr/bin/env bash
set -euo pipefail

scripts_dir=$(cd "$(dirname "$0")" && pwd -P)
source_repo=$(git rev-parse --show-toplevel)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/phase20-proof-scripts.XXXXXX")
cleanup_paths=("$tmp")
cleanup() {
  rm -rf -- "${cleanup_paths[@]}"
}
trap cleanup EXIT

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

source_tree=$(git -C "$tmp" rev-parse "${source_sha}^{tree}")
printf '%s\n' \
  '{' \
  '  "schema": "wayland-core.phase20-independent-review.v1",' \
  "  \"source_sha\": \"$source_sha\"," \
  "  \"source_tree\": \"$source_tree\"," \
  '  "source_executor_id": "gsd-agent:builder-test",' \
  '  "reviewer_id": "gsd-agent:reviewer-test",' \
  '  "checks": {"all_severity":"PASS","public_lifecycle":"PASS","retained_authority":"PASS"},' \
  '  "deferred": [],' \
  '  "findings": {"blocker":0,"critical":0,"high":0,"medium":0,"low":0},' \
  '  "evidence": [{"command":"fixture-proof","exit_code":0,"result":"PASS"}],' \
  '  "disposition": "PASS"' \
  '}' > "$tmp/20-06-INTERFACE-REVIEWS.md"
git -C "$tmp" add 20-06-INTERFACE-REVIEWS.md
git -C "$tmp" commit -qm 'review'
review_sha=$(git -C "$tmp" rev-parse HEAD)

if (
  cd "$tmp"
  bash "$scripts_dir/verify-review-pair.sh" \
    "${source_sha:0:12}" "$review_base_sha" "$review_sha" \
    20-06-SUMMARY.md 20-06-INTERFACE-REVIEWS.md source.rs
) >/dev/null 2>&1; then
  echo 'review verifier accepted an abbreviated object ID' >&2
  exit 1
fi

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
(
  cd "$tmp"
  node "$scripts_dir/verify-review-result.mjs" \
    "$review_sha" 20-06-INTERFACE-REVIEWS.md \
    "$source_sha" "$source_tree" f20-15
)

if (
  cd "$tmp"
  node "$scripts_dir/verify-review-result.mjs" \
    "$review_sha" 20-06-INTERFACE-REVIEWS.md \
    "$source_sha" "$source_tree" f20-16
) >/dev/null 2>&1; then
  echo 'review-result verifier accepted the wrong qualification profile' >&2
  exit 1
fi

assert_review_result_rejected() {
  local label=$1 mutation=$2 candidate
  git -C "$tmp" reset -q --hard "$review_base_sha"
  git -C "$tmp" show "$review_sha:20-06-INTERFACE-REVIEWS.md" \
    | sed "$mutation" > "$tmp/20-06-INTERFACE-REVIEWS.md"
  git -C "$tmp" add 20-06-INTERFACE-REVIEWS.md
  git -C "$tmp" commit -qm "invalid review: $label"
  candidate=$(git -C "$tmp" rev-parse HEAD)
  if (
    cd "$tmp"
    node "$scripts_dir/verify-review-result.mjs" \
      "$candidate" 20-06-INTERFACE-REVIEWS.md \
      "$source_sha" "$source_tree" f20-15
  ) >/dev/null 2>&1; then
    echo "review-result verifier accepted invalid result: $label" >&2
    exit 1
  fi
}

assert_review_result_rejected nonzero-finding 's/"low":0/"low":1/'
assert_review_result_rejected same-reviewer \
  's/"reviewer_id": "gsd-agent:reviewer-test"/"reviewer_id": "gsd-agent:builder-test"/'
assert_review_result_rejected failed-disposition 's/"disposition": "PASS"/"disposition": "FAIL"/'
assert_review_result_rejected failed-evidence 's/"exit_code":0/"exit_code":1/'
assert_review_result_rejected missing-check 's/,"retained_authority":"PASS"//'
assert_review_result_rejected unknown-field 's/"schema":/"unexpected": true, "schema":/'
assert_review_result_rejected wrong-source \
  "s/\"source_sha\": \"$source_sha\"/\"source_sha\": \"$review_base_sha\"/"

git -C "$tmp" reset -q --hard "$review_sha"

# Regression: an inconsistent (source_sha, source_tree) pair is rejected even
# when the review JSON matches that wrong tree — the tree must be source_sha^{tree}.
git -C "$tmp" reset -q --hard "$review_base_sha"
bogus_tree=ffffffffffffffffffffffffffffffffffffffff
git -C "$tmp" show "$review_sha:20-06-INTERFACE-REVIEWS.md" \
  | sed "s/\"source_tree\": \"$source_tree\"/\"source_tree\": \"$bogus_tree\"/" \
  > "$tmp/20-06-INTERFACE-REVIEWS.md"
git -C "$tmp" add 20-06-INTERFACE-REVIEWS.md
git -C "$tmp" commit -qm 'review with matching-but-wrong source tree'
bogus_tree_review_sha=$(git -C "$tmp" rev-parse HEAD)
if (
  cd "$tmp"
  node "$scripts_dir/verify-review-result.mjs" \
    "$bogus_tree_review_sha" 20-06-INTERFACE-REVIEWS.md \
    "$source_sha" "$bogus_tree" f20-15
) >/dev/null 2>&1; then
  echo 'review-result verifier accepted a source tree that is not source_sha^{tree}' >&2
  exit 1
fi
git -C "$tmp" reset -q --hard "$review_sha"

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
scope_base_tree=$(git -C "$tmp" rev-parse HEAD^{tree})
git_dir=$(git -C "$tmp" rev-parse --absolute-git-dir)

# Non-task authority objects retain the exact two-line interface.
legacy_authority="$git_dir/review-authority-test"
node "$scripts_dir/task-base-authority.mjs" capture \
  "$legacy_authority" "$scope_base_sha" "$scope_base_tree"
[[ "$(node "$scripts_dir/task-base-authority.mjs" read "$legacy_authority")" == \
  "$scope_base_sha"$'\n'"$scope_base_tree" ]]
node "$scripts_dir/task-base-authority.mjs" capture \
  "$legacy_authority" "$scope_base_sha" "$scope_base_tree"
if node "$scripts_dir/task-base-authority.mjs" capture \
  "$legacy_authority" "$review_sha" "$(git -C "$tmp" rev-parse "${review_sha}^{tree}")" \
  >/dev/null 2>&1; then
  echo 'authority helper replaced an existing non-task authority tuple' >&2
  exit 1
fi

# A crash before publication may leave private directories, never a partial authority object.
interrupted_base="$git_dir/gsd-task-base-20-90"
if WAYLAND_TEST_TASK_AUTHORITY_FAIL_BEFORE_PUBLISH=1 \
  node "$scripts_dir/task-base-authority.mjs" task-begin \
    "$interrupted_base" 20-90 "$scope_base_sha" "$scope_base_tree" \
    >/dev/null 2>&1; then
  echo 'task authority interruption injection unexpectedly succeeded' >&2
  exit 1
fi
[[ ! -e "$interrupted_base/state.json" ]]
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$interrupted_base" 20-90 "$scope_base_sha" "$scope_base_tree" >/dev/null
[[ "$(node "$scripts_dir/task-base-authority.mjs" read "$interrupted_base" | sed -n '1p')" == \
  "$scope_base_sha" ]]
interrupted_generation=$(node "$scripts_dir/task-base-authority.mjs" \
  task-current "$interrupted_base" 20-90 | sed -n '3p')
for authority_directory in \
  "$interrupted_base" \
  "$interrupted_base/generations" \
  "$interrupted_base/generations/$interrupted_generation"; do
  chmod 777 "$authority_directory"
  if node "$scripts_dir/task-base-authority.mjs" task-current \
    "$interrupted_base" 20-90 >/dev/null 2>&1; then
    echo 'task authority accepted permission drift in a private directory' >&2
    exit 1
  fi
  chmod 700 "$authority_directory"
done

# A generation base may exist before root-state publication; exact retry completes it.
state_gap_base="$git_dir/gsd-task-base-20-85"
if WAYLAND_TEST_TASK_AUTHORITY_TARGET_SUFFIX=/state.json \
  WAYLAND_TEST_TASK_AUTHORITY_FAIL_BEFORE_PUBLISH=1 \
  node "$scripts_dir/task-base-authority.mjs" task-begin \
    "$state_gap_base" 20-85 "$scope_base_sha" "$scope_base_tree" \
    >/dev/null 2>&1; then
  echo 'state-publication interruption unexpectedly succeeded' >&2
  exit 1
fi
[[ ! -e "$state_gap_base/state.json" ]]
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$state_gap_base" 20-85 "$scope_base_sha" "$scope_base_tree" >/dev/null

# A durable state link survives loss of acknowledgement and exact retry converges.
state_ack_base="$git_dir/gsd-task-base-20-84"
if WAYLAND_TEST_TASK_AUTHORITY_TARGET_SUFFIX=/state.json \
  WAYLAND_TEST_TASK_AUTHORITY_FAIL_AFTER_PUBLISH=1 \
  node "$scripts_dir/task-base-authority.mjs" task-begin \
    "$state_ack_base" 20-84 "$scope_base_sha" "$scope_base_tree" \
    >/dev/null 2>&1; then
  echo 'post-state-publication interruption unexpectedly succeeded' >&2
  exit 1
fi
[[ -f "$state_ack_base/state.json" ]]
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$state_ack_base" 20-84 "$scope_base_sha" "$scope_base_tree" >/dev/null

# A hard kill before state publication leaves only an ignorable unique temp object.
kill_gap_base="$git_dir/gsd-task-base-20-83"
if WAYLAND_TEST_TASK_AUTHORITY_TARGET_SUFFIX=/state.json \
  WAYLAND_TEST_TASK_AUTHORITY_KILL_BEFORE_PUBLISH=1 \
  node "$scripts_dir/task-base-authority.mjs" task-begin \
    "$kill_gap_base" 20-83 "$scope_base_sha" "$scope_base_tree" \
    >/dev/null 2>&1; then
  echo 'hard-kill interruption unexpectedly succeeded' >&2
  exit 1
fi
[[ ! -e "$kill_gap_base/state.json" ]]
compgen -G "$kill_gap_base/.state.json.tmp-*" >/dev/null
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$kill_gap_base" 20-83 "$scope_base_sha" "$scope_base_tree" >/dev/null

# Successor creation before abandonment and disposition acknowledgement are retry-safe.
fresh_gap_base="$git_dir/gsd-task-base-20-82"
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$fresh_gap_base" 20-82 "$scope_base_sha" "$scope_base_tree" >/dev/null
fresh_gap_generation=$(node "$scripts_dir/task-base-authority.mjs" \
  task-current "$fresh_gap_base" 20-82 | sed -n '3p')
if WAYLAND_TEST_TASK_AUTHORITY_TARGET_SUFFIX=/disposition.json \
  WAYLAND_TEST_TASK_AUTHORITY_FAIL_BEFORE_PUBLISH=1 \
  node "$scripts_dir/task-base-authority.mjs" task-start-fresh \
    "$fresh_gap_base" 20-82 "$fresh_gap_generation" \
    "$review_sha" "$(git -C "$tmp" rev-parse "${review_sha}^{tree}")" \
    >/dev/null 2>&1; then
  echo 'pre-disposition interruption unexpectedly succeeded' >&2
  exit 1
fi
node "$scripts_dir/task-base-authority.mjs" task-start-fresh \
  "$fresh_gap_base" 20-82 "$fresh_gap_generation" \
  "$review_sha" "$(git -C "$tmp" rev-parse "${review_sha}^{tree}")" >/dev/null

fresh_ack_base="$git_dir/gsd-task-base-20-81"
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$fresh_ack_base" 20-81 "$scope_base_sha" "$scope_base_tree" >/dev/null
fresh_ack_generation=$(node "$scripts_dir/task-base-authority.mjs" \
  task-current "$fresh_ack_base" 20-81 | sed -n '3p')
if WAYLAND_TEST_TASK_AUTHORITY_TARGET_SUFFIX=/disposition.json \
  WAYLAND_TEST_TASK_AUTHORITY_FAIL_AFTER_PUBLISH=1 \
  node "$scripts_dir/task-base-authority.mjs" task-start-fresh \
    "$fresh_ack_base" 20-81 "$fresh_ack_generation" \
    "$review_sha" "$(git -C "$tmp" rev-parse "${review_sha}^{tree}")" \
    >/dev/null 2>&1; then
  echo 'post-disposition interruption unexpectedly succeeded' >&2
  exit 1
fi
node "$scripts_dir/task-base-authority.mjs" task-start-fresh \
  "$fresh_ack_base" 20-81 "$fresh_ack_generation" \
  "$review_sha" "$(git -C "$tmp" rev-parse "${review_sha}^{tree}")" >/dev/null

# Nested mutable-path substitutions fail closed, not only top-level TASK_BASE paths.
nested_state_base="$git_dir/gsd-task-base-20-80"
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$nested_state_base" 20-80 "$scope_base_sha" "$scope_base_tree" >/dev/null
rm "$nested_state_base/state.json"
ln -s /dev/null "$nested_state_base/state.json"
if node "$scripts_dir/task-base-authority.mjs" task-current \
  "$nested_state_base" 20-80 >/dev/null 2>&1; then
  echo 'task authority accepted a symlink state object' >&2
  exit 1
fi

nested_base="$git_dir/gsd-task-base-20-79"
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$nested_base" 20-79 "$scope_base_sha" "$scope_base_tree" >/dev/null
nested_generation=$(node "$scripts_dir/task-base-authority.mjs" \
  task-current "$nested_base" 20-79 | sed -n '3p')
rm "$nested_base/generations/$nested_generation/base"
mkfifo "$nested_base/generations/$nested_generation/base"
if node "$scripts_dir/task-base-authority.mjs" task-current \
  "$nested_base" 20-79 >/dev/null 2>&1; then
  echo 'task authority accepted a FIFO generation base' >&2
  exit 1
fi

nested_disposition_base="$git_dir/gsd-task-base-20-78"
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$nested_disposition_base" 20-78 "$scope_base_sha" "$scope_base_tree" >/dev/null
nested_disposition_generation=$(node "$scripts_dir/task-base-authority.mjs" \
  task-current "$nested_disposition_base" 20-78 | sed -n '3p')
ln -s /dev/null \
  "$nested_disposition_base/generations/$nested_disposition_generation/disposition.json"
if node "$scripts_dir/task-base-authority.mjs" task-start-fresh \
  "$nested_disposition_base" 20-78 "$nested_disposition_generation" \
  "$review_sha" "$(git -C "$tmp" rev-parse "${review_sha}^{tree}")" \
  >/dev/null 2>&1; then
  echo 'task authority accepted a symlink disposition target' >&2
  exit 1
fi

# Canonical-looking generation IDs must still bind plan, parent, commit, and tree.
forged_root="$git_dir/gsd-task-base-20-86"
forged_generation="g-ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
mkdir -m 700 "$forged_root" "$forged_root/generations" \
  "$forged_root/generations/$forged_generation"
printf '%s\n%s\n' "$scope_base_sha" "$scope_base_tree" > \
  "$forged_root/generations/$forged_generation/base"
chmod 400 "$forged_root/generations/$forged_generation/base"
printf '%s\n' \
  "{\"schema\":\"wayland-core.task-base.v1\",\"plan\":\"20-86\",\"root_generation\":\"$forged_generation\"}" > \
  "$forged_root/state.json"
chmod 400 "$forged_root/state.json"
if node "$scripts_dir/task-base-authority.mjs" task-current \
  "$forged_root" 20-86 >/dev/null 2>&1; then
  echo 'task authority accepted a forged root generation ID' >&2
  exit 1
fi

# Concurrent identical publication converges; conflicting publication cannot both win.
concurrent_base="$git_dir/gsd-task-base-20-91"
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$concurrent_base" 20-91 "$scope_base_sha" "$scope_base_tree" >"$tmp/concurrent-one" &
concurrent_one=$!
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$concurrent_base" 20-91 "$scope_base_sha" "$scope_base_tree" >"$tmp/concurrent-two" &
concurrent_two=$!
wait "$concurrent_one"
wait "$concurrent_two"
cmp "$tmp/concurrent-one" "$tmp/concurrent-two"
rm "$tmp/concurrent-one" "$tmp/concurrent-two"

conflict_base="$git_dir/gsd-task-base-20-92"
set +e
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$conflict_base" 20-92 "$scope_base_sha" "$scope_base_tree" >/dev/null 2>&1 &
conflict_one=$!
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$conflict_base" 20-92 "$review_sha" "$(git -C "$tmp" rev-parse "${review_sha}^{tree}")" \
  >/dev/null 2>&1 &
conflict_two=$!
wait "$conflict_one"; conflict_one_status=$?
wait "$conflict_two"; conflict_two_status=$?
set -e
[[ $(( (conflict_one_status == 0) + (conflict_two_status == 0) )) -eq 1 ]]

fresh_race_base="$git_dir/gsd-task-base-20-99"
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$fresh_race_base" 20-99 "$scope_base_sha" "$scope_base_tree" >/dev/null
fresh_race_generation=$(node "$scripts_dir/task-base-authority.mjs" \
  task-current "$fresh_race_base" 20-99 | sed -n '3p')
node "$scripts_dir/task-base-authority.mjs" task-start-fresh \
  "$fresh_race_base" 20-99 "$fresh_race_generation" \
  "$review_sha" "$(git -C "$tmp" rev-parse "${review_sha}^{tree}")" >"$tmp/fresh-one" &
fresh_one=$!
node "$scripts_dir/task-base-authority.mjs" task-start-fresh \
  "$fresh_race_base" 20-99 "$fresh_race_generation" \
  "$review_sha" "$(git -C "$tmp" rev-parse "${review_sha}^{tree}")" >"$tmp/fresh-two" &
fresh_two=$!
wait "$fresh_one"
wait "$fresh_two"
cmp "$tmp/fresh-one" "$tmp/fresh-two"
rm "$tmp/fresh-one" "$tmp/fresh-two"

# Initial capture refuses a dirty tree.
dirty_base="$git_dir/gsd-task-base-20-93"
printf 'dirty\n' > "$tmp/dirty-before-capture.txt"
if (
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" --capture "$dirty_base"
) >/dev/null 2>&1; then
  echo 'scope verifier captured an initial base from a dirty checkout' >&2
  exit 1
fi
rm "$tmp/dirty-before-capture.txt"

# A failing Git status is not an empty clean status.
corrupt_capture_repo=$(mktemp -d "${TMPDIR:-/tmp}/phase20-corrupt-capture.XXXXXX")
cleanup_paths+=("$corrupt_capture_repo")
git clone -q --no-hardlinks "$source_repo" "$corrupt_capture_repo"
printf 'x' > "$corrupt_capture_repo/.git/index"
corrupt_capture_base="$corrupt_capture_repo/.git/gsd-task-base-20-89"
if (
  cd "$corrupt_capture_repo"
  bash "$scripts_dir/verify-task-scope.sh" --capture "$corrupt_capture_base"
) >/dev/null 2>&1; then
  echo 'scope verifier accepted a checkout whose Git status failed' >&2
  exit 1
fi
[[ ! -e "$corrupt_capture_base/state.json" ]]

# Legacy task-base tuples fail closed; only non-task two-line authorities remain compatible.
legacy_task_base="$git_dir/gsd-task-base-20-87"
printf '%s\n%s\n' "$scope_base_sha" "$scope_base_tree" > "$legacy_task_base"
chmod 400 "$legacy_task_base"
if (
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" --capture "$legacy_task_base"
) >/dev/null 2>&1; then
  echo 'scope verifier silently migrated a legacy task-base tuple' >&2
  exit 1
fi

base_file="$git_dir/gsd-task-base-20-03"
(
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" --capture "$base_file"
)
[[ "$(node "$scripts_dir/task-base-authority.mjs" read "$base_file" | sed -n '1p')" == \
  "$scope_base_sha" ]]
scope_generation=$(node "$scripts_dir/task-base-authority.mjs" \
  task-current "$base_file" 20-03 | sed -n '3p')

if ! (
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" --capture "$base_file"
) >/dev/null 2>&1; then
  echo 'scope verifier could not safely reuse an existing TASK_BASE' >&2
  exit 1
fi
[[ "$(node "$scripts_dir/task-base-authority.mjs" task-current \
  "$base_file" 20-03 | sed -n '3p')" == "$scope_generation" ]]

# Scope-stream producer failures cannot hide untracked or out-of-scope paths.
corrupt_scope_repo=$(mktemp -d "${TMPDIR:-/tmp}/phase20-corrupt-scope.XXXXXX")
cleanup_paths+=("$corrupt_scope_repo")
git clone -q --no-hardlinks "$source_repo" "$corrupt_scope_repo"
corrupt_scope_base="$corrupt_scope_repo/.git/gsd-task-base-20-88"
(
  cd "$corrupt_scope_repo"
  bash "$scripts_dir/verify-task-scope.sh" --capture "$corrupt_scope_base"
)
printf 'forbidden\n' > "$corrupt_scope_repo/forbidden.txt"
printf 'x' > "$corrupt_scope_repo/.git/index"
if (
  cd "$corrupt_scope_repo"
  bash "$scripts_dir/verify-task-scope.sh" "$corrupt_scope_base" source.rs
) >/dev/null 2>&1; then
  echo 'scope verifier accepted failed Git scope producers' >&2
  exit 1
fi

real_base_file=$base_file
symlink_base_file="$git_dir/gsd-task-base-20-94"
ln -s "$real_base_file" "$symlink_base_file"
if (
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" "$symlink_base_file" source.rs
) >/dev/null 2>&1; then
  echo 'scope verifier accepted a symlink TASK_BASE' >&2
  exit 1
fi

malformed_base_file="$git_dir/gsd-task-base-20-95"
mkdir -m 700 "$malformed_base_file"
printf '{not-json}\n' > "$malformed_base_file/state.json"
chmod 400 "$malformed_base_file/state.json"
if (
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" "$malformed_base_file" source.rs
) >/dev/null 2>&1; then
  echo 'scope verifier accepted a multiline TASK_BASE' >&2
  exit 1
fi

wrong_tree_base_file="$git_dir/gsd-task-base-20-96"
node "$scripts_dir/task-base-authority.mjs" task-begin \
  "$wrong_tree_base_file" 20-96 "$scope_base_sha" "$source_sha" >/dev/null
if (
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" "$wrong_tree_base_file" source.rs
) >/dev/null 2>&1; then
  echo 'scope verifier accepted a mismatched TASK_BASE tree' >&2
  exit 1
fi

fifo_base_file="$git_dir/gsd-task-base-20-97"
mkfifo "$fifo_base_file"
if (
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" "$fifo_base_file" source.rs
) >/dev/null 2>&1; then
  echo 'scope verifier accepted a FIFO TASK_BASE' >&2
  exit 1
fi

printf 'tracked\n' > "$tmp/tracked.txt"
git -C "$tmp" add tracked.txt
git -C "$tmp" commit -qm 'tracked scope'
tracked_scope_sha=$(git -C "$tmp" rev-parse HEAD)

if (
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" --start-fresh \
    "$base_file" g-ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff \
    "$tracked_scope_sha"
) >/dev/null 2>&1; then
  echo 'Start Fresh accepted the wrong old generation' >&2
  exit 1
fi
if (
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" --start-fresh \
    "$base_file" "$scope_generation" "$scope_base_sha"
) >/dev/null 2>&1; then
  echo 'Start Fresh accepted a reconciled HEAD other than current HEAD' >&2
  exit 1
fi
(
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" --start-fresh \
    "$base_file" "$scope_generation" "$tracked_scope_sha"
)
fresh_generation=$(node "$scripts_dir/task-base-authority.mjs" \
  task-current "$base_file" 20-03 | sed -n '3p')
[[ "$fresh_generation" != "$scope_generation" ]]
[[ "$(node "$scripts_dir/task-base-authority.mjs" read "$base_file" | sed -n '1p')" == \
  "$tracked_scope_sha" ]]

# The abandoned disposition must bind the exact successor commit.
scope_disposition="$base_file/generations/$scope_generation/disposition.json"
chmod 600 "$scope_disposition"
sed "s/\"head\":\"$tracked_scope_sha\"/\"head\":\"$scope_base_sha\"/" \
  "$scope_disposition" > "$scope_disposition.tmp"
mv "$scope_disposition.tmp" "$scope_disposition"
chmod 400 "$scope_disposition"
if node "$scripts_dir/task-base-authority.mjs" task-current \
  "$base_file" 20-03 >/dev/null 2>&1; then
  echo 'task authority accepted a successor inconsistent with its disposition' >&2
  exit 1
fi
chmod 600 "$scope_disposition"
sed "s/\"head\":\"$scope_base_sha\"/\"head\":\"$tracked_scope_sha\"/" \
  "$scope_disposition" > "$scope_disposition.tmp"
mv "$scope_disposition.tmp" "$scope_disposition"
chmod 400 "$scope_disposition"

# Resume is plan-scoped: it survives a changed/missing agent identity and preserves the generation.
printf 'unrelated-agent-id\n' > "$tmp/.planning/current-agent-id.txt"
(
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" --capture "$base_file"
)
rm "$tmp/.planning/current-agent-id.txt"
[[ "$(node "$scripts_dir/task-base-authority.mjs" task-current \
  "$base_file" 20-03 | sed -n '3p')" == "$fresh_generation" ]]

printf 'dirty\n' > "$tmp/dirty-start-fresh.txt"
if (
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" --start-fresh \
    "$base_file" "$fresh_generation" "$tracked_scope_sha"
) >/dev/null 2>&1; then
  echo 'Start Fresh accepted a dirty checkout' >&2
  exit 1
fi
rm "$tmp/dirty-start-fresh.txt"

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
  bash "$scripts_dir/verify-task-scope.sh" --capture "$base_file"
)
[[ "$(node "$scripts_dir/task-base-authority.mjs" read "$base_file" | sed -n '1p')" == \
  "$tracked_scope_sha" ]]
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

# Completion is an explicit terminal disposition; completed generations cannot resume silently.
git -C "$tmp" reset -q --hard "$tracked_scope_sha"
git -C "$tmp" clean -qfd
complete_base="$git_dir/gsd-task-base-20-98"
(
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" --capture "$complete_base"
)
complete_generation=$(node "$scripts_dir/task-base-authority.mjs" \
  task-current "$complete_base" 20-98 | sed -n '3p')
(
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" --complete \
    "$complete_base" "$complete_generation" "$tracked_scope_sha"
)
# The exact wrapper retry converges after a crash between publication and acknowledgement.
(
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" --complete \
    "$complete_base" "$complete_generation" "$tracked_scope_sha"
)
if (
  cd "$tmp"
  bash "$scripts_dir/verify-task-scope.sh" --complete \
    "$complete_base" "$complete_generation" "$scope_base_sha"
) >/dev/null 2>&1; then
  echo 'completed task generation accepted a conflicting retry commit' >&2
  exit 1
fi
if node "$scripts_dir/task-base-authority.mjs" task-current \
  "$complete_base" 20-98 >/dev/null 2>&1; then
  echo 'completed task generation remained active' >&2
  exit 1
fi

self_auth_repo="$tmp/f20-self-authorizing-plan"
git clone -q --no-hardlinks "$source_repo" "$self_auth_repo"
cp "$scripts_dir/verify-f20-03-scope.sh" \
  "$self_auth_repo/.planning/scripts/verify-f20-03-scope.sh"
accepted_sha=$(git -C "$self_auth_repo" rev-parse HEAD)
accepted_tree=$(git -C "$self_auth_repo" rev-parse HEAD^{tree})
accepted_file="$(git -C "$self_auth_repo" rev-parse --absolute-git-dir)/f20-03-accepted-plan"
printf '%s\n%s\n' "$accepted_sha" "$accepted_tree" > "$accepted_file"
chmod 600 "$accepted_file"
plan_file="$self_auth_repo/.planning/phases/20-transactional-delegated-mutation/20-03-PLAN.md"
PLAN_FILE="$plan_file" node -e '
  const fs = require("node:fs");
  const file = process.env.PLAN_FILE;
  const original = fs.readFileSync(file, "utf8");
  const replacement = original.replace(
    "  - Cargo.lock\n",
    "  - .planning/phases/20-transactional-delegated-mutation/20-03-PLAN.md\n",
  );
  if (replacement === original) throw new Error("self-authorization fixture did not mutate plan");
  fs.writeFileSync(file, replacement);
'
git -C "$self_auth_repo" add .planning/phases/20-transactional-delegated-mutation/20-03-PLAN.md
if (
  cd "$self_auth_repo"
  bash .planning/scripts/verify-f20-03-scope.sh task
) >/dev/null 2>&1; then
  echo 'F20-03 scope verifier accepted a plan that authorized its own mutation' >&2
  exit 1
fi

# --- Regression: F20-03 scope gate rejects a source-file -> symlink type change ---
# A type change (regular file, mode 100644 -> symlink, mode 120000) at an in-scope
# path stays inside the 41-path canonical set and is neither delete nor rename, so
# the old `^[DR]` destructive guard let it land. It must now fail closed.
symlink_repo="$tmp/f20-03-symlink-swap"
git clone -q --no-hardlinks "$source_repo" "$symlink_repo"
cp "$scripts_dir/verify-f20-03-scope.sh" "$symlink_repo/.planning/scripts/verify-f20-03-scope.sh"
cp "$scripts_dir/task-base-authority.mjs" "$symlink_repo/.planning/scripts/task-base-authority.mjs"
symlink_head=$(git -C "$symlink_repo" rev-parse HEAD)
symlink_tree=$(git -C "$symlink_repo" rev-parse HEAD^{tree})
symlink_state="$(git -C "$symlink_repo" rev-parse --absolute-git-dir)/f20-03-accepted-plan"
printf '%s\n%s\n' "$symlink_head" "$symlink_tree" > "$symlink_state"
chmod 600 "$symlink_state"
in_scope_swap="crates/wcore-sandbox/src/backends/appcontainer.rs"
(
  cd "$symlink_repo"
  rm "$in_scope_swap"
  ln -s /tmp/attacker-shadow.rs "$in_scope_swap"
  git add "$in_scope_swap"
)
if (
  cd "$symlink_repo"
  bash .planning/scripts/verify-f20-03-scope.sh task
) >/dev/null 2>&1; then
  echo 'F20-03 scope verifier accepted a source-file-to-symlink type change' >&2
  exit 1
fi

printf 'phase20-proof-script-tests-ok\n'
