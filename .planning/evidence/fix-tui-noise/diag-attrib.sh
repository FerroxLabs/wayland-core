#!/usr/bin/env bash
# diag-attrib.sh — attribute the boot walk to a subsystem WITHOUT a rebuild.
#
# Two candidate walkers exist in the boot path and they differ in exactly two
# observable ways, which makes them separable by construction:
#
#   wcore-repomap  scope_files()              standard_filters(TRUE)  , skips `.git` by name
#   wcore-tools    project_committed_secrets() standard_filters(FALSE), does NOT skip `.git`
#
# So: build a git repo whose `.gitignore` hides a large directory, run the
# product with that repo as cwd, and count how many times the product opens
# something inside the ignored directory.
#
#   many opens inside the ignored dir  => gitignore NOT respected => workspace_policy
#   ~zero opens inside the ignored dir => gitignore respected     => repomap
#
# This is a two-sided test: BOTH outcomes are reachable and each falsifies the
# other hypothesis, which is what LANE-BRIEF §3b-iii demands of a gate.
set -u
LABEL="${1:?}"; BIN="${2:?}"; OUT="${3:?}"; LANEHOME="${4:?}"; PROBE="${5:?}"; N="${6:-20000}"
mkdir -p "$OUT"; R="$OUT/$LABEL"; : > "$R.result"
say() { echo "$1" >> "$R.result"; }

KEY=$(/usr/bin/grep -m1 '^ANTHROPIC_API_KEY=' /root/.wayland/.env | cut -d= -f2-)
[ -z "$KEY" ] && { say "NO_KEY"; say "WLRC=97"; say "WLDONE"; exit 97; }
export ANTHROPIC_API_KEY="$KEY"; unset KEY
export HOME="$LANEHOME" PROVIDER=anthropic MODEL=claude-sonnet-4-5-20250929

# ── build the probe repo ─────────────────────────────────────────────────────
rm -rf "$PROBE"; mkdir -p "$PROBE/ignored_big" "$PROBE/src"
cd "$PROBE" || exit 94
/usr/bin/git init -q .
printf 'ignored_big/\n' > .gitignore
printf 'fn main() {}\n' > src/main.rs
seq 1 "$N" | xargs -P8 -n500 -I{} true            # warm xargs
seq 1 "$N" | awk '{print "ignored_big/f" $1 ".txt"}' | xargs -P8 -n2000 touch
say "PROBE=$PROBE"
say "IGNORED_FILE_COUNT=$(ls -U "$PROBE/ignored_big" | wc -l | tr -d ' ')"
say "GITIGNORE=$(cat "$PROBE/.gitignore")"
# Prove git itself honours the ignore file — if it does not, the whole test is void.
say "GIT_SEES_IGNORED=$(/usr/bin/git status --porcelain --untracked-files=all | /usr/bin/grep -c ignored_big || true)"

# ── run under a narrow strace (openat only: cheap enough not to distort) ─────
/usr/bin/strace -f -e trace=openat -o "$R.strace" \
  "$BIN" --no-tui 'What is 17 times 23? Reply with just the number.' \
  > "$R.stdout" 2> "$R.stderr"
say "PROC_RC=$?"
say "ANSWER_HAS_391=$(/usr/bin/grep -c 391 "$R.stdout" || true)"
say "STRACE_LINES=$(wc -l < "$R.strace" | tr -d ' ')"

# ── control pair (§3b-i), same capture ───────────────────────────────────────
# POSITIVE: the product MUST open the non-ignored source dir at least once if it
#           walked at all; if this is 0 no walk happened and the negative below
#           is meaningless.
# NEGATIVE: a path that does not exist must be 0.
say "CTRL_POS_probe_root=$(/usr/bin/grep -c "\"$PROBE" "$R.strace" || true)"
say "CTRL_NEG_bogus=$(/usr/bin/grep -c 'zzqq_never_9f3a' "$R.strace" || true)"

say "OPENS_IN_IGNORED_DIR=$(/usr/bin/grep -c "$PROBE/ignored_big/" "$R.strace" || true)"
say "OPENS_IN_SRC=$(/usr/bin/grep -c "$PROBE/src" "$R.strace" || true)"
say "OPENS_IN_DOTGIT=$(/usr/bin/grep -c "$PROBE/.git/" "$R.strace" || true)"

say "WLRC=0"
say "WLDONE"
exit 0
