#!/usr/bin/env bash
# 23A-C1 live proof, v2. Drives the REAL wayland-core binary on hetzner-dsm.
#
# ---------------------------------------------------------------------------
# INSTRUMENT REPAIR (v1 -> v2), recorded rather than silently fixed
# ---------------------------------------------------------------------------
# v1 measured catalog membership by grepping `--skills-audit` output for the skill
# NAME. `--skills-audit` does not print names -- it prints `Total skills: N` and a
# findings list. So:
#
#   * v1's KNOWN-POSITIVE ("victim IS in the catalog before revocation") FAILED, which
#     is the control doing its job: it declared the negative assertion beneath it
#     meaningless rather than letting it pass.
#   * v1's `REVOKED SKILL IS ABSENT` PASSED -- for the wrong reason. A grep for a name
#     that is never printed is absent unconditionally. That is a manufactured green.
#   * v1's LEG 3 `grep auto-gen` PASSED anyway, a genuine FALSE PASS: findings-dependent
#     output plus a prefix that also matches `auto-gen2`.
#
# v2 uses `Total skills: N`, which is a deterministic count taken from the real
# `loader::load_catalog` call. A count alone is not enough either -- 2->1 and 2->0 both
# satisfy "the victim went away" -- so every count assertion is paired with a control
# skill whose survival is asserted in the same measurement.
#
# catalog_count() carries the three-assertion self-test §6b-ii requires, including the
# assertion that the OLD broken matcher would have missed the case. Without that third
# assertion the self-test passes on the broken instrument too.
# ---------------------------------------------------------------------------
#
# Isolation: WAYLAND_HOME is a fresh mktemp -d per leg -- what both
# paths::wayland_home_skills_dirs and govern::governance_root resolve against. Leg 0b
# asserts the real user directory was never touched.

set -uo pipefail
BIN=/root/wayland-23a-c1/target/debug/wayland-core
PROJ=$(mktemp -d); mkdir -p "$PROJ/.git"

pass=0; fail=0
ck() { if [ "$3" = "$2" ]; then echo "  OK   $1"; pass=$((pass+1));
       else echo "  FAIL $1  (expected $2, got $3)"; fail=$((fail+1)); fi; }
yn() { [ "$1" -eq 0 ] && echo PASS || echo FAIL; }

mkdraft() { local d="$1/skills/$2"; mkdir -p "$d"
  printf -- '---\nname: %s\ndescription: draft %s\n---\n\n%s\n' "$2" "$2" "$3" > "$d/SKILL.md"
  printf '{"auto_drafted":true,"signature":"sig-%s"}' "$2" > "$d/manifest.json"; }

# THE REPAIRED INSTRUMENT: how many skills the real catalog load returned.
catalog_count() { # catalog_count <wayland_home>
  ( cd "$PROJ" && WAYLAND_HOME="$1" "$BIN" --skills-audit 2>&1 ) \
    | awk '/^Total skills:/ { print $3; found=1; exit } END { if (!found) print "NOMATCH" }'
}
# v1's broken matcher, kept so the repair can be shown to differ from it.
old_matcher() { ( cd "$PROJ" && WAYLAND_HOME="$1" "$BIN" --skills-audit 2>&1 ) | grep -q "$2"; }

statusof() { echo "$1" | awk -v s="$2" '$1==s { for(i=1;i<=NF;i++) if ($i ~ /^status=/) { sub(/^status=/,"",$i); print $i; exit } }'; }

echo "=================================================================="
echo "LEG -1 -- SELF-TEST of the repaired catalog instrument"
echo "=================================================================="
HS=$(mktemp -d)
c0=$(catalog_count "$HS")                       # baseline: bundled skills only
mkdraft "$HS" auto-selftest "body"
c1=$(catalog_count "$HS")
# 1. KNOWN-POSITIVE: adding a skill raises the count.
ck "self-test 1/3 KNOWN-POSITIVE: count rises when a skill is added ($c0 -> $c1)" PASS \
   "$([ "$c1" -gt "$c0" ] 2>/dev/null && echo PASS || echo FAIL)"
# 2. KNOWN-NEGATIVE: removing it lowers the count again.
rm -rf "$HS/skills/auto-selftest"
c2=$(catalog_count "$HS")
ck "self-test 2/3 KNOWN-NEGATIVE: count falls when it is removed ($c1 -> $c2)" PASS \
   "$([ "$c2" -lt "$c1" ] 2>/dev/null && echo PASS || echo FAIL)"
# 3. THE ASSERTION THAT PROVES THE REPAIR DOES ANYTHING: the old name-grep matcher
#    reports ABSENT for a skill that is genuinely PRESENT. Without this, 1 and 2 pass
#    on the broken instrument too and the repair is unproven.
mkdraft "$HS" auto-selftest "body"
old_matcher "$HS" auto-selftest && oldres=PRESENT || oldres=ABSENT
ck "self-test 3/3 the OLD matcher wrongly reports ABSENT for a present skill" PASS \
   "$([ "$oldres" = ABSENT ] && echo PASS || echo FAIL)"
ck "  ...while the repaired instrument counts it" PASS \
   "$([ "$(catalog_count "$HS")" -gt "$c2" ] 2>/dev/null && echo PASS || echo FAIL)"

echo
echo "=================================================================="
echo "LEG 0 -- governance surface alive on the real binary"
echo "=================================================================="
REAL_SKILLS="${HOME:-/root}/.config/wayland-core/skills"
before_real=$(ls -laR "$REAL_SKILLS" 2>/dev/null | md5sum | cut -d' ' -f1)
H0=$(mktemp -d); mkdraft "$H0" auto-control "control body"
out=$(WAYLAND_HOME="$H0" "$BIN" --skills-govern 2>&1); rc=$?
ck "binary runs --skills-govern (rc=0)" PASS "$(yn $rc)"
ck "KNOWN-POSITIVE: control skill is listed" PASS \
   "$([ -n "$(statusof "$out" auto-control)" ] && echo PASS || echo FAIL)"
ck "KNOWN-NEGATIVE: a never-installed name is NOT listed" PASS \
   "$([ -z "$(statusof "$out" auto-never-existed)" ] && echo PASS || echo FAIL)"

echo
echo "=================================================================="
echo "LEG 1 -- PROMOTE"
echo "=================================================================="
H1=$(mktemp -d); mkdraft "$H1" auto-subject "subject body"; mkdraft "$H1" auto-sibling "sibling body"
pre=$(WAYLAND_HOME="$H1" "$BIN" --skills-govern 2>&1)
ck "PRE: subject is quarantined ('installed'), not promoted" PASS \
   "$([ "$(statusof "$pre" auto-subject)" = installed ] && echo PASS || echo FAIL)"
WAYLAND_HOME="$H1" "$BIN" --skills-promote auto-subject > /tmp/p.out 2>&1; prc=$?
post=$(WAYLAND_HOME="$H1" "$BIN" --skills-govern 2>&1)
ck "promote exits 0" PASS "$(yn $prc)"
ck "ON DISK: exactly one grant file" PASS \
   "$([ "$(ls -1 "$H1/skills-governance/promotions" 2>/dev/null | wc -l)" -eq 1 ] && echo PASS || echo FAIL)"
ck "grant records digest + authority + target" PASS \
   "$(grep -q 'sha256:' "$H1"/skills-governance/promotions/*.json && grep -q 'authority' "$H1"/skills-governance/promotions/*.json && grep -q 'target_dir' "$H1"/skills-governance/promotions/*.json && echo PASS || echo FAIL)"
ck "ON BEHAVIOUR: subject lists as promoted" PASS \
   "$([ "$(statusof "$post" auto-subject)" = promoted ] && echo PASS || echo FAIL)"
ck "NEGATIVE CONTROL: untouched sibling is STILL 'installed'" PASS \
   "$([ "$(statusof "$post" auto-sibling)" = installed ] && echo PASS || echo FAIL)"
printf -- '---\nname: auto-subject\n---\n\nTAMPERED\n' > "$H1/skills/auto-subject/SKILL.md"
ck "REDDENING CONTROL: editing the bytes breaks the grant" PASS \
   "$([ "$(statusof "$(WAYLAND_HOME=$H1 $BIN --skills-govern 2>&1)" auto-subject)" = quarantined-digest-mismatch ] && echo PASS || echo FAIL)"
ck "PROVENANCE IS CHECKABLE: journal records the promotion" PASS \
   "$(WAYLAND_HOME="$H1" "$BIN" --skills-govern 2>&1 | grep -q 'PROMOTED' && echo PASS || echo FAIL)"

echo
echo "=================================================================="
echo "LEG 2 -- REVOKE: a revoked skill does not LOAD, so it cannot execute"
echo "=================================================================="
H2=$(mktemp -d); mkdraft "$H2" auto-victim "victim body"; mkdraft "$H2" auto-bystander "bystander body"
c_before=$(catalog_count "$H2")
rm -rf "$H2/skills/auto-victim"; c_novictim=$(catalog_count "$H2"); mkdraft "$H2" auto-victim "victim body"
ck "KNOWN-POSITIVE: the victim contributes to the real catalog ($c_novictim -> $c_before)" PASS \
   "$([ "$c_before" -eq $((c_novictim + 1)) ] 2>/dev/null && echo PASS || echo FAIL)"
WAYLAND_HOME="$H2" "$BIN" --skills-revoke auto-victim > /tmp/r.out 2>&1; rrc=$?
REVID=$(grep -o 'revocation id: .*' /tmp/r.out | awk '{print $3}')
ck "revoke exits 0" PASS "$(yn $rrc)"
ck "the skill directory is gone from disk" PASS \
   "$([ ! -d "$H2/skills/auto-victim" ] && echo PASS || echo FAIL)"
ck "retained bytes exist (rollback has a target)" PASS \
   "$([ -d "$H2/skills-governance/generations/$REVID/payload" ] && echo PASS || echo FAIL)"
c_after=$(catalog_count "$H2")
ck "REVOKED SKILL LEFT THE REAL CATALOG ($c_before -> $c_after)" PASS \
   "$([ "$c_after" -eq "$c_novictim" ] 2>/dev/null && echo PASS || echo FAIL)"
# THE RESURRECTION FENCE: put the bytes back. Count must NOT recover.
mkdraft "$H2" auto-victim "victim body"
c_resurrect=$(catalog_count "$H2")
ck "RESURRECTION FENCE: count does NOT recover after the files return ($c_resurrect)" PASS \
   "$([ "$c_resurrect" -eq "$c_novictim" ] 2>/dev/null && echo PASS || echo FAIL)"
# NEGATIVE CONTROL, same invocation: the bystander is still counted. This is what
# separates "the victim was dropped" from "the catalog collapsed".
rm -rf "$H2/skills/auto-bystander"; c_nobystander=$(catalog_count "$H2"); mkdraft "$H2" auto-bystander "bystander body"
ck "NEGATIVE CONTROL: the bystander is still loading ($c_nobystander -> $c_resurrect)" PASS \
   "$([ "$c_resurrect" -eq $((c_nobystander + 1)) ] 2>/dev/null && echo PASS || echo FAIL)"
prom=$(WAYLAND_HOME="$H2" "$BIN" --skills-promote auto-victim 2>&1); prc2=$?
ck "PROMOTION OF A REVOKED SKILL IS REFUSED" PASS "$([ $prc2 -ne 0 ] && echo PASS || echo FAIL)"
ck "  the refusal names the reason (revoked)" PASS \
   "$(echo "$prom" | grep -qi 'revoked' && echo PASS || echo FAIL)"
WAYLAND_HOME="$H2" "$BIN" --skills-promote auto-bystander >/dev/null 2>&1
ck "NEGATIVE CONTROL: promoting the bystander in the same state SUCCEEDS" PASS "$(yn $?)"
ck "the refusal is journalled (PROMOTE-REFUSED / REFUSED)" PASS \
   "$(WAYLAND_HOME="$H2" "$BIN" --skills-govern 2>&1 | grep -q 'REFUSED' && echo PASS || echo FAIL)"

echo
echo "=================================================================="
echo "LEG 3 -- ROLLBACK restores the PRIOR GENERATION'S behaviour"
echo "=================================================================="
H3=$(mktemp -d); mkdraft "$H3" auto-gen "GENERATION-ONE-MARKER"
gen1=$(md5sum "$H3/skills/auto-gen/SKILL.md" | cut -d' ' -f1)
c_g1=$(catalog_count "$H3")
WAYLAND_HOME="$H3" "$BIN" --skills-revoke auto-gen > /tmp/r3.out 2>&1
RID=$(grep -o 'revocation id: .*' /tmp/r3.out | awk '{print $3}')
c_gone=$(catalog_count "$H3")
ck "PRE: generation one is absent from disk and from the catalog ($c_g1 -> $c_gone)" PASS \
   "$([ ! -f "$H3/skills/auto-gen/SKILL.md" ] && [ "$c_gone" -lt "$c_g1" ] 2>/dev/null && echo PASS || echo FAIL)"
WAYLAND_HOME="$H3" "$BIN" --skills-rollback "$RID" > /tmp/rb.out 2>&1; rbrc=$?
ck "rollback exits 0" PASS "$(yn $rbrc)"
gen1b=$(md5sum "$H3/skills/auto-gen/SKILL.md" 2>/dev/null | cut -d' ' -f1)
ck "restored BYTE FOR BYTE (md5 identical to the pre-revocation generation)" PASS \
   "$([ "$gen1" = "$gen1b" ] && echo PASS || echo FAIL)"
ck "  restored content carries GENERATION-ONE-MARKER" PASS \
   "$(grep -q 'GENERATION-ONE-MARKER' "$H3/skills/auto-gen/SKILL.md" 2>/dev/null && echo PASS || echo FAIL)"
c_back=$(catalog_count "$H3")
ck "BEHAVIOUR OBSERVED: the restored generation LOADS again ($c_gone -> $c_back)" PASS \
   "$([ "$c_back" -eq "$c_g1" ] 2>/dev/null && echo PASS || echo FAIL)"
ck "suppression cleared (REVOKED (0))" PASS \
   "$(WAYLAND_HOME="$H3" "$BIN" --skills-govern 2>&1 | grep -q 'REVOKED (0)' && echo PASS || echo FAIL)"
WAYLAND_HOME="$H3" "$BIN" --skills-rollback 00000000-0000-0000-0000-000000000000 >/dev/null 2>&1
ck "REDDENING CONTROL: rollback of an unknown id exits nonzero" PASS \
   "$([ $? -ne 0 ] && echo PASS || echo FAIL)"

echo
echo "=================================================================="
echo "LEG 0b -- the real user skills directory was never touched"
echo "=================================================================="
after_real=$(ls -laR "$REAL_SKILLS" 2>/dev/null | md5sum | cut -d' ' -f1)
ck "real global skills dir unchanged across every leg" PASS \
   "$([ "$before_real" = "$after_real" ] && echo PASS || echo FAIL)"

echo
echo "SUMMARY: pass=$pass fail=$fail"
[ $fail -eq 0 ] || exit 1
