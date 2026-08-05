#!/usr/bin/env bash
# F-27C3-04 — secret sweep. LANE-BRIEF §0: the live FluxRouter key reached
# hetzner on stdin; every changed file and every capture must be swept against
# the live value, with the hit count reported.
#
# WHY EVERY SECTION CARRIES A CONTROL: "the secret does not appear" is an
# ABSENCE claim, and §3b-i measured an unpiped grep being rewritten to report
# 9 matches for a true answer of zero. A typo'd path, an unquoted glob, a
# mangled pattern and a dead tool ALL produce a comforting zero. So each
# section greps for a string it is certain IS present, in the SAME invocation
# shape, and the section is VOID unless that control fires.
#
# The pattern reaches grep through a process substitution and is never written
# to disk, never in argv, never echoed. Run from the Mac:
#   bash secret-sweep.sh < ~/.wayland-secrets/flux.env

set -u +x
LANE=/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-f27-image-default
EVID="$LANE/.planning/phases/27-multimodal-browser-generation-voice/evidence/f27-image-default"
GREP=/usr/bin/grep
GIT=/usr/bin/git
BASE=eaff921d710876e87372f01dcce3b185004426bc

RAW="$(cat)"
KEY="$(printf '%s' "$RAW" | sed -n 's/^[[:space:]]*\(export[[:space:]]\{1,\}\)\{0,1\}FLUX_API_KEY[[:space:]]*=[[:space:]]*//p' | tr -d '"'"'"'\r' | head -1)"
[ -z "$KEY" ] && KEY="$(printf '%s' "$RAW" | tr -d '\r' | head -1)"
unset RAW
# Same post-parse reject as the probe: a mangled pattern silently produces the
# zero this sweep wants to see, which is the worst possible failure here.
case "$KEY" in *=*|*" "*) echo "SWEEP=ABORT reason=key_parse len=${#KEY}"; exit 2 ;; esac
[ ${#KEY} -lt 20 ] && { echo "SWEEP=ABORT reason=key_implausibly_short len=${#KEY}"; exit 2; }
echo "sweep key_len=${#KEY}   # length only"

FAILS=0
report() { # label hits control_hits
  echo "  $1: hits=$2  control=$3"
  if [ "$3" -lt 1 ]; then echo "    VOID — control did not fire, this zero proves nothing"; FAILS=$((FAILS+1));
  elif [ "$2" -ne 0 ]; then echo "    SECRET FOUND"; FAILS=$((FAILS+1)); fi
}

# 0. Known-positive on the instrument itself: the key file MUST match.
KF_HITS=$($GREP -c -F -f <(printf '%s\n' "$KEY") ~/.wayland-secrets/flux.env || true)
echo "  instrument control (the keyfile itself): hits=$KF_HITS   # must be >=1"
[ "$KF_HITS" -lt 1 ] && { echo "SWEEP=ABORT reason=pattern_does_not_match_its_own_source_file"; FAILS=$((FAILS+1)); }

# 1. Every file this lane changed under crates/.
CHANGED=$($GIT -C "$LANE" diff --name-only "$BASE" HEAD -- crates/)
H=0; C=0
for f in $CHANGED; do
  H=$((H + $($GREP -c -F -f <(printf '%s\n' "$KEY") "$LANE/$f" || true)))
  C=$((C + $($GREP -c -F 'image_model' "$LANE/$f" || true)))
done
report "changed crates/ files ($(printf '%s' "$CHANGED" | wc -w | tr -d ' ') files)" "$H" "$C"

# 2. The evidence directory (captures written by the probes).
H=$($GREP -rc -F -f <(printf '%s\n' "$KEY") "$EVID" 2>/dev/null | awk -F: '{s+=$2} END{print s+0}')
C=$($GREP -rc -F 'flux-image' "$EVID" 2>/dev/null | awk -F: '{s+=$2} END{print s+0}')
report "evidence dir" "$H" "$C"

# 3. The commit contents themselves (a value scrubbed from the worktree can
#    still be in history). Piped, so the control covers the pipe shape too.
H=$($GIT -C "$LANE" log -p "$BASE"..HEAD | $GREP -c -F -f <(printf '%s\n' "$KEY") || true)
C=$($GIT -C "$LANE" log -p "$BASE"..HEAD | $GREP -c -F 'image_model' || true)
report "git log -p BASE..HEAD" "$H" "$C"

# 4. Remote captures + the isolated home on hetzner. §4 of the 27-c3-media
#    summary: a remote sweep that never RAN is indistinguishable from one that
#    found zero — a heredoc ate stdin and the section printed PASS having
#    produced no output. So the pattern goes over stdin and the remote must
#    emit BOTH numbers or the section is void.
REMOTE=$(printf '%s\n' "$KEY" | ssh -o BatchMode=yes hetzner-dsm \
  'P=$(cat); H=$(grep -rc -F "$P" /root/wayland-f27imgdef-live /root/wayland-f27imgdef-revert /root/wayland-f27imgdef-mutbak 2>/dev/null | awk -F: "{s+=\$2} END{print s+0}"); C=$(grep -rc -F "fluxrouter" /root/wayland-f27imgdef-live /root/wayland-f27imgdef-revert 2>/dev/null | awk -F: "{s+=\$2} END{print s+0}"); echo "$H $C"')
if [ -z "$REMOTE" ]; then
  echo "  remote captures: VOID — the remote section produced NO OUTPUT"
  FAILS=$((FAILS+1))
else
  report "remote captures (hetzner)" "$(echo "$REMOTE" | awk '{print $1}')" "$(echo "$REMOTE" | awk '{print $2}')"
fi

unset KEY
echo
[ "$FAILS" -eq 0 ] && echo "SECRET_SWEEP=PASS" || echo "SECRET_SWEEP=FAIL failures=$FAILS"
