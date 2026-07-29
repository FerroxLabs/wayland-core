#!/usr/bin/env bash
# 26-SC2-PEERS — live proof that grok and gemini-cli import END TO END.
#
# A unit test over a synthetic fixture does not establish that a real peer tree
# imports. This drives the SHIPPED wayland-core binary over homes staged from
# the REAL `~/.grok` and `~/.gemini` on Sean's Mac (see
# `scripts/f26-sc2-peers-stage.sh` for exactly what was copied verbatim and
# what was placeholdered), and reads the result back off the FILESYSTEM.
#
# Every absence claim is paired with a positive control taken in the SAME
# invocation, and the F26-SC2-M1 exec-bit mitigation is measured with `stat` on
# the imported copy rather than asserted from the code path.
#
# USAGE:  f26-sc2-peers-live-proof.sh <wayland-core binary> <staged-dir> [workdir]
set -uo pipefail

BIN="${1:?usage: f26-sc2-peers-live-proof.sh <binary> <staged-dir> [workdir]}"
STAGED="${2:?usage: f26-sc2-peers-live-proof.sh <binary> <staged-dir> [workdir]}"
WORK="${3:-$(mktemp -d)}"
rc=0
say()  { printf '%s\n' "$*"; }
fail() { say "FAIL: $*"; rc=1; }
ok()   { say "PASS: $*"; }
mode() { stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1"; }

[ -x "$BIN" ] || { say "FATAL: $BIN is not executable"; exit 2; }
say "=== 26-SC2-PEERS live proof ==="
say "binary : $BIN"
say "version: $("$BIN" --version 2>&1 | head -1)"
say "staged : $STAGED"
say "workdir: $WORK"

# ---------------------------------------------------------------------------
# P0 POSITIVE CONTROL — the instrument that decides every claim below is alive.
#
# Three things must be shown working BEFORE any of them is used to report an
# absence: `stat` can see an execute bit, `stat` can see its absence, and the
# filesystem under test actually honours the bit (a noexec/mode-flattening
# mount would make every X2 below pass for free).
# ---------------------------------------------------------------------------
say
say "--- P0: the exec-bit instrument, proven on a known-positive AND a known-negative ---"
mkdir -p "$WORK"
printf '#!/bin/sh\ntrue\n' >"$WORK/probe-exec"
printf '#!/bin/sh\ntrue\n' >"$WORK/probe-noexec"
chmod 0755 "$WORK/probe-exec"
chmod 0644 "$WORK/probe-noexec"
p_yes="$(mode "$WORK/probe-exec")"
p_no="$(mode "$WORK/probe-noexec")"
say "  chmod 0755 -> stat reports $p_yes ; chmod 0644 -> stat reports $p_no"
if [ "$p_yes" = "755" ] && [ "$p_no" = "644" ] && [ -x "$WORK/probe-exec" ] \
   && [ ! -x "$WORK/probe-noexec" ]; then
  ok "P0 the filesystem honours the execute bit and stat reports it BOTH ways"
else
  fail "P0 the exec-bit instrument is dead — every X2 below would pass for free"
fi

# ===========================================================================
# One peer, end to end.
#   $1 subcommand   $2 staged home   $3 a relative path under the home that is
#                                       a REAL exec-bit helper at the source
# ===========================================================================
run_peer() {
  local peer="$1" src="$2" helper_rel="$3"
  local home="$WORK/$peer-wayland-home"
  local out="$WORK/$peer-import.out"
  rm -rf "$home"; mkdir -p "$home"

  say
  say "############################################################"
  say "### PEER: $peer"
  say "############################################################"

  # --- what the SOURCE actually contains -----------------------------------
  local src_skills src_exec
  src_skills="$(find "$src/skills" -name SKILL.md -type f 2>/dev/null | wc -l | tr -d ' ')"
  src_exec="$(find "$src/skills" -type f -perm -u+x 2>/dev/null | wc -l | tr -d ' ')"
  say "source: $src"
  say "  SKILL.md in <home>/skills : $src_skills"
  say "  exec-bit files there      : $src_exec"
  if [ "$src_skills" -lt 1 ]; then
    fail "$peer the staged source has NO skills — the import below cannot mean anything"
    return
  fi
  if [ "$src_exec" -lt 1 ]; then
    fail "$peer the staged source has NO exec-bit file — the hostile case is absent"
    return
  fi

  local src_helper="$src/$helper_rel"
  if [ -x "$src_helper" ]; then
    ok "$peer H0-CONTROL the hostile helper IS executable at source ($(mode "$src_helper")): $helper_rel"
  else
    fail "$peer H0-CONTROL the source helper is not executable; the X2 below proves nothing"
    return
  fi
  local src_sum
  src_sum="$(sha256sum "$src_helper" | cut -c1-16)"

  # --- drive the real binary ------------------------------------------------
  say
  say "--- driving: migrate $peer --home <staged> --yes ---"
  env WAYLAND_HOME="$home" "$BIN" migrate "$peer" --home "$src" --yes >"$out" 2>&1
  local irc=$?
  say "import rc=$irc"
  sed 's/^/    /' "$out"
  [ "$irc" -eq 0 ] || { fail "$peer the import did not exit 0"; return; }

  # --- L1: something actually landed ---------------------------------------
  say
  local live_skills
  live_skills="$(find "$home/skills" -name SKILL.md -type f 2>/dev/null | wc -l | tr -d ' ')"
  say "L1 SKILL.md that LANDED under <wayland home>/skills: $live_skills (source had $src_skills)"
  if [ "$live_skills" -ge 1 ]; then
    ok "$peer L1 real peer skills imported"
  else
    fail "$peer L1 nothing landed — the import reported success over an empty result"
  fi

  # --- L2: the profile landed in config.toml -------------------------------
  if grep -q "^\[profiles\.\"$peer/root\"\]\|^\[profiles\.'$peer/root'\]\|$peer/root" \
       "$home/config.toml" 2>/dev/null; then
    ok "$peer L2 the root profile is in the written config.toml"
  else
    say "  (config.toml head)"; sed 's/^/    /' "$home/config.toml" 2>/dev/null | head -20
    fail "$peer L2 no root profile in config.toml"
  fi

  # --- X1/X2: the HOSTILE case, measured ON DISK ---------------------------
  say
  say "--- hostile case: a real skill carrying a real executable helper ---"
  local dst_helper
  dst_helper="$(find "$home/skills" -type f -name "$(basename "$helper_rel")" 2>/dev/null | head -1)"
  if [ -z "$dst_helper" ]; then
    fail "$peer X1 the helper did not import at all (a filter, not a migration)"
    return
  fi
  local dst_sum
  dst_sum="$(sha256sum "$dst_helper" | cut -c1-16)"
  say "  source : $(mode "$src_helper")  sha256[0:16]=$src_sum  $helper_rel"
  say "  landed : $(mode "$dst_helper")  sha256[0:16]=$dst_sum  ${dst_helper#"$home"/}"
  if [ "$src_sum" = "$dst_sum" ]; then
    ok "$peer X1 the helper's BYTES crossed unchanged (a migration, not a filter)"
  else
    fail "$peer X1 the helper's bytes were altered in transit"
  fi
  if [ -x "$dst_helper" ]; then
    fail "$peer X2 the imported helper is STILL EXECUTABLE ($(mode "$dst_helper"))"
  else
    ok "$peer X2 the imported helper carries no execute bit ($(mode "$dst_helper")) — measured with stat, on disk"
  fi

  # Not one file — EVERY file. A mitigation that holds for the one path the
  # test names and leaks on the other twelve is not a mitigation.
  local landed_exec
  landed_exec="$(find "$home/skills" -type f -perm -u+x 2>/dev/null | wc -l | tr -d ' ')"
  say "  exec-bit files under the LIVE skills root: $landed_exec (source had $src_exec)"
  if [ "$landed_exec" -eq 0 ]; then
    ok "$peer X4 ZERO of $src_exec source exec bits survived the import"
  else
    say "  survivors:"; find "$home/skills" -type f -perm -u+x | sed 's/^/    /'
    fail "$peer X4 $landed_exec execute bits survived"
  fi

  # --- F26-SC2-M1 re-measured on THIS peer ---------------------------------
  local contained directive_live
  contained="$(find "$home/migrate-quarantine" -type f 2>/dev/null | wc -l | tr -d ' ')"
  directive_live="$(find "$home/skills" -name SKILL.md -type f 2>/dev/null \
    | xargs -r grep -lF '```!' 2>/dev/null | wc -l | tr -d ' ')"
  say
  say "  F26-SC2-M1 on $peer: skills carrying Wayland's \`\`\`! directive = $directive_live"
  say "  F26-SC2-M1 on $peer: files in quarantine                     = $contained"
  if [ "$directive_live" -eq 0 ]; then
    ok "$peer M1 confirmed — no real $peer skill uses Wayland's directive, so all import live"
  else
    fail "$peer M1 a directive-carrying skill reached the live root"
  fi

  # --- provenance, read back FROM THE PRODUCT ------------------------------
  say
  say "--- provenance: ask the product where a landed artifact came from ---"
  local a_skill rel
  a_skill="$(find "$home/skills" -name SKILL.md -type f | head -1)"
  rel="${a_skill#"$home"/}"
  env WAYLAND_HOME="$home" "$BIN" migrate imported --path "$rel" >"$WORK/$peer-q1.out" 2>&1
  sed 's/^/    /' "$WORK/$peer-q1.out"
  if grep -qi "$peer" "$WORK/$peer-q1.out"; then
    ok "$peer Q1 known-positive — a landed artifact resolves to its $peer source"
  else
    fail "$peer Q1 the product could not attribute a landed artifact to $peer"
  fi

  # KNOWN-NEGATIVE. A skill THIS machine authored, in the same root, queried
  # through the identical command. If this returns an attribution, Q1 above was
  # matching the peer name for free.
  mkdir -p "$home/skills/locally-authored"
  printf -- '---\nname: locally-authored\n---\nmine\n' \
    >"$home/skills/locally-authored/SKILL.md"
  env WAYLAND_HOME="$home" "$BIN" migrate imported \
    --path skills/locally-authored/SKILL.md >"$WORK/$peer-q2.out" 2>&1
  sed 's/^/    /' "$WORK/$peer-q2.out"
  if grep -qi 'No import record' "$WORK/$peer-q2.out"; then
    ok "$peer Q2 known-negative — a locally authored skill is NOT attributed to $peer"
  else
    fail "$peer Q2 the product attributed a LOCAL file to $peer"
  fi

  # --- S1: the source tree is untouched ------------------------------------
  say
  local after_sum
  after_sum="$(sha256sum "$src_helper" | cut -c1-16)"
  if [ "$after_sum" = "$src_sum" ] && [ -x "$src_helper" ]; then
    ok "$peer S1 the SOURCE helper is byte- and mode-identical after the import"
  else
    fail "$peer S1 the import mutated its own source"
  fi
}

run_peer grok   "$STAGED/grok-home"   "skills/brand/scripts/extract-colors.cjs"
run_peer gemini "$STAGED/gemini-home" "skills/async-pr-review/scripts/async-review.sh"

say
if [ "$rc" -eq 0 ]; then
  say "26-SC2-PEERS LIVE PROOF: PASS"
else
  say "26-SC2-PEERS LIVE PROOF: FAIL"
fi
exit "$rc"
