#!/usr/bin/env bash
# F27-C3 — sweep every artifact this lane produced for the live burn key.
#
# WHY THIS SCRIPT HAS A LIVENESS CONTROL
#
# LANE-BRIEF §3b-i: "A known-negative assertion is SELF-PASSING on a dead
# instrument." A sweep that reports zero hits is confirmed for free by a typo'd
# path, an unquoted glob, a rewritten grep, an empty pattern, or searching the
# wrong tree. **A zero is the success value here, which makes it the single
# easiest result to fake.**
#
# So every sweep below is paired with a KNOWN-POSITIVE run of the SAME
# instrument with the SAME pattern against a target the value is certainly in.
# If the control does not report a match, the sweep result is void — and this
# script says so rather than printing a comforting zero.
#
# THE SECRET IS NEVER WRITTEN TO DISK. The pattern reaches grep through a
# process substitution (`/dev/fd/N`), never a temp file, and is never echoed.
#
# Usage:
#   bash secret-sweep.sh              # sweeps the Mac worktree
#   SWEEP_REMOTE=1 bash secret-sweep.sh   # also sweeps the hetzner captures

set -u

KEYFILE="${KEYFILE:-$HOME/.wayland-secrets/flux.env}"
WT="${WT:-/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-27-c3-media}"
GREP=/usr/bin/grep

if [ ! -f "$KEYFILE" ]; then
  echo "SWEEP=VOID reason=keyfile_missing path=$KEYFILE"
  exit 2
fi

# Extract the bare value. `export FLUX_API_KEY=...` and `FLUX_API_KEY=...` both
# handled — the same parser bug that mangled the live probe would silently make
# this whole sweep search for the wrong string.
extract_key() {
  sed -n 's/^[[:space:]]*\(export[[:space:]]\{1,\}\)\{0,1\}FLUX_API_KEY[[:space:]]*=[[:space:]]*//p' "$KEYFILE" \
    | tr -d "\"'\r" | head -1
}

KEYLEN=$(extract_key | wc -c | tr -d ' ')
echo "SWEEP pattern_len=$((KEYLEN - 1))   # length only, never the value"
if [ "$KEYLEN" -lt 20 ]; then
  echo "SWEEP=VOID reason=extracted_pattern_too_short_to_be_the_key"
  exit 2
fi

VOID=0

# sweep <label> <expect: HIT|MISS> <target...>
sweep() {
  local label="$1" expect="$2"; shift 2
  local n
  n=$("$GREP" -r -a -l -F -f <(extract_key) "$@" 2>/dev/null | wc -l | tr -d ' ')
  echo "  $label: files_containing_key=$n (expected: $expect)"
  if [ "$expect" = "HIT" ] && [ "$n" -eq 0 ]; then
    echo "  !! CONTROL FAILED — the instrument cannot find the key even where it IS."
    echo "  !! Every MISS result in this run is therefore VOID."
    VOID=1
  fi
  if [ "$expect" = "MISS" ] && [ "$n" -ne 0 ]; then
    echo "  !! LEAK — the key appears in an artifact."
    "$GREP" -r -a -l -F -f <(extract_key) "$@" 2>/dev/null | sed 's/^/     /'
    VOID=2
  fi
}

echo
echo "=== 1. LIVENESS CONTROL (known-positive, same instrument, same pattern)"
sweep "control/keyfile-itself" HIT "$KEYFILE"

echo
echo "=== 2. SWEEP — every file this lane added or changed, plus all evidence"
sweep "worktree/crates" MISS "$WT/crates"
sweep "worktree/planning-27" MISS "$WT/.planning/phases/27-multimodal-browser-generation-voice"
sweep "worktree/evidence-27c3" MISS \
  "$WT/.planning/phases/27-multimodal-browser-generation-voice/evidence/27-c3-media"

echo
echo "=== 3. SWEEP — git history of this lane branch (patch text, all commits)"
BASE=$(cd "$WT" && /usr/bin/git merge-base HEAD plan/f20-unified-audit-repair)
PATCH_HITS=$(cd "$WT" && /usr/bin/git log -p "$BASE..HEAD" 2>/dev/null \
  | "$GREP" -a -c -F -f <(extract_key) || true)
echo "  git-log-patch: lines_containing_key=$PATCH_HITS (expected: 0)   base=$BASE"
[ "${PATCH_HITS:-0}" -ne 0 ] && VOID=2
# Control for THAT grep, which is a different invocation shape (-c over a pipe):
CTRL_HITS=$("$GREP" -a -c -F -f <(extract_key) "$KEYFILE" || true)
echo "  git-log-patch CONTROL: lines_containing_key_in_keyfile=$CTRL_HITS (expected: >=1)"
if [ "${CTRL_HITS:-0}" -lt 1 ]; then
  echo "  !! CONTROL FAILED for the piped grep — its zero above is VOID."
  VOID=1
fi

if [ "${SWEEP_REMOTE:-0}" = "1" ]; then
  echo
  echo "=== 4. SWEEP — hetzner capture directory (remote, pattern over stdin)"
  # NOTE — the first version of this section produced NO OUTPUT AT ALL and the
  # script still printed SECRET_SWEEP=PASS. It piped the pattern into
  # `ssh ... 'bash -s' <<'REMOTE'`: the heredoc takes stdin, so `bash -s` read
  # the SCRIPT from stdin and the `cat` inside it got nothing. A remote sweep
  # that never ran is indistinguishable from a remote sweep that found zero —
  # which is the precise defect class this file exists to defend against.
  # Two channels now: the script (secret-free) goes in argv, the pattern goes
  # on stdin, and the section asserts it actually produced results.
  REMOTE_SCRIPT='
set -u
K="$(cat)"
if [ ${#K} -lt 20 ]; then echo "  remote/VOID: pattern did not arrive on stdin"; exit 9; fi
D=/root/wayland-27c3-live
CTRL=$(grep -r -a -l -F "fluxrouter" "$D" 2>/dev/null | wc -l | tr -d " ")
HITS=$(grep -r -a -l -F "$K" "$D" 2>/dev/null | wc -l | tr -d " ")
CONF=$(grep -r -a -l -F "$K" "$D/home" 2>/dev/null | wc -l | tr -d " ")
unset K
echo "  remote/control-known-positive(fluxrouter): $CTRL (expected: >=1)"
echo "  remote/captures: files_containing_key=$HITS (expected: 0)"
echo "  remote/isolated-home: files_containing_key=$CONF (expected: 0)"
[ "$CTRL" -ge 1 ] || { echo "  !! remote control did not fire"; exit 8; }
[ "$HITS" -eq 0 ] && [ "$CONF" -eq 0 ] || exit 7
exit 0
'
  REMOTE_OUT="$(extract_key | ssh -o BatchMode=yes hetzner-dsm "bash -c '$REMOTE_SCRIPT'" 2>&1)"
  REMOTE_RC=$?
  printf '%s\n' "$REMOTE_OUT"
  if [ -z "$REMOTE_OUT" ]; then
    echo "  !! REMOTE SECTION PRODUCED NO OUTPUT — it did not run. Result VOID."
    VOID=1
  elif [ "$REMOTE_RC" -eq 7 ]; then
    echo "  !! LEAK on the remote host."
    VOID=2
  elif [ "$REMOTE_RC" -ne 0 ]; then
    echo "  !! remote sweep exited $REMOTE_RC — result VOID."
    VOID=1
  fi
fi

echo
case "$VOID" in
  0) echo "SECRET_SWEEP=PASS  (0 hits in artifacts; liveness controls all fired)"; exit 0 ;;
  1) echo "SECRET_SWEEP=VOID  (a liveness control failed — the zeros prove nothing)"; exit 3 ;;
  *) echo "SECRET_SWEEP=LEAK  (the key appears in an artifact)"; exit 4 ;;
esac
