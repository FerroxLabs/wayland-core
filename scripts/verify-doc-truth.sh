#!/usr/bin/env bash
# Lane doc-truth: binds each customer-facing claim to the code that implements it.
#
# Every claim gets TWO checks: a CODE half (the fact, so the check cannot pass
# vacuously once the code moves) and a DOC half (the sentence). A doc check that
# is not paired with a code check is a check that silently stops meaning
# anything the day the product changes.
#
# Run from the repo root. Pass a tree prefix as $1 to check another checkout.
set -uo pipefail
ROOT="${1:-.}"
fails=0

pass() { printf 'PASS  %s\n' "$1"; }
fail() {
  printf 'FAIL  %s\n' "$1"
  fails=$((fails + 1))
}

# ------------------------------------------------------------------ claim (1)
# The `--i-accept-exfil-risk` interlock DOES NOT EXIST. The product answers
# `error: unexpected argument`. No user-facing doc may instruct anyone to pass
# it. The owner has decided not to add it, so this is permanent, not a TODO.
if grep -rqE '(long|arg|value_name)[[:space:]]*[=(].*i.accept.exfil' \
  "$ROOT/crates/wcore-cli/src/" 2>/dev/null; then
  fail "C1-code: a CLI arg definition for --i-accept-exfil-risk now EXISTS; revisit the doc fix"
else
  pass "C1-code: no CLI arg definition for --i-accept-exfil-risk anywhere in wcore-cli"
fi

if grep -q -- '--i-accept-exfil-risk' "$ROOT/README.md"; then
  fail "C1-doc: README.md still tells the operator to pass --i-accept-exfil-risk, which the CLI rejects"
else
  pass "C1-doc: README.md no longer advertises the nonexistent --i-accept-exfil-risk flag"
fi

# What ACTUALLY governs egress: the operator's trusted GLOBAL [security] layer.
# A project config that travels with a cloned repo gets no say at all.
if grep -q 'enabled: global\.security\.enabled,' "$ROOT/crates/wcore-config/src/config.rs"; then
  pass "C1-code: [security] enabled is read from the global layer alone (config.rs)"
else
  fail "C1-code: config.rs no longer reads security.enabled from global alone; README replacement text is stale"
fi

# Bind to the SPECIFIC replacement sentence, not to the mere presence of the
# words "global" and "[security]" — both already appear elsewhere in the file,
# so a loose match here is green against the unfixed tree and proves nothing.
if grep -q 'read from your \*\*global\*\* config alone' "$ROOT/README.md"; then
  pass "C1-doc: README.md names the global config as the sole source of [security] enabled"
else
  fail "C1-doc: README.md does not state that [security] enabled comes from the global config alone"
fi

# ------------------------------------------------------------------ claim (2)
# Slack and Discord declare at-most-once. The trait method is the
# build-enforced fact; the prose must not contradict it.
for adapter in slack discord; do
  decl=$(grep -A2 'fn supports_outbound_idempotency' \
    "$ROOT/crates/wcore-channel-$adapter/src/lib.rs" |
    grep -m1 -E '^[[:space:]]+(true|false)[[:space:]]*$' | tr -d '[:space:]')
  if [ "$decl" = "false" ]; then
    pass "C2-code: $adapter declares supports_outbound_idempotency() == false (at-most-once)"
  else
    fail "C2-code: $adapter declares '$decl', not false; docs/channels.md must be revisited"
  fi
done

# The claim wraps across source lines, so FLATTEN before matching. A
# line-oriented grep passes vacuously on the very sentence it must catch —
# `docs/channels.md` splits "Slack, Matrix and Discord already transmit one and
# are / exactly-once" across a newline.
for doc in README.md docs/channels.md; do
  flat=$(tr '\n' ' ' <"$ROOT/$doc" | tr -s ' ')
  if printf '%s' "$flat" | grep -qE '(Slack|Discord)[^.]*(are|is) exactly-once'; then
    fail "C2-doc: $doc still claims Slack and/or Discord are exactly-once"
  else
    pass "C2-doc: $doc does not claim Slack/Discord exactly-once"
  fi
done

# ------------------------------------------------------------------ claim (3)
# Matrix's exactly-once guarantee is CONDITIONAL on the message cap. Above it
# the body is chunked and sent unkeyed, which is at-least-once. A doc that
# states the guarantee without the precondition overstates the product.
cap=$(grep -A1 'fn max_message_len' "$ROOT/crates/wcore-channel-matrix/src/lib.rs" |
  grep -oE '[0-9_]{4,}' | head -1 | tr -d '_')
if [ "$cap" = "32768" ]; then
  pass "C3-code: Matrix max_message_len() == 32768 (the cap the guarantee is conditional on)"
else
  fail "C3-code: Matrix max_message_len() is '$cap', not 32768"
fi

# Any doc asserting Matrix exactly-once must state the cap in the same file.
for doc in README.md docs/channels.md; do
  if grep -q 'exactly-once' "$ROOT/$doc"; then
    if grep -q '32,768' "$ROOT/$doc"; then
      pass "C3-doc: $doc states the 32,768-char precondition alongside its exactly-once claim"
    else
      fail "C3-doc: $doc claims exactly-once without stating the 32,768-char precondition"
    fi
  else
    pass "C3-doc: $doc makes no exactly-once claim (precondition not required)"
  fi
done

# ------------------------------------------------------------------ claim (4)
# The sub-agent turn default. `docs/advanced.md` published 10 while the code has
# said 200 since before v0.12.25 — a standing documentation error, never a
# behaviour change. Bind the doc's table cell to the constant.
turns=$(grep -m1 'DEFAULT_SUB_AGENT_MAX_TURNS: usize =' \
  "$ROOT/crates/wcore-agent/src/spawn_tool.rs" | grep -oE '[0-9]+')
if [ "$turns" = "200" ]; then
  pass "C4-code: DEFAULT_SUB_AGENT_MAX_TURNS == 200 (spawn_tool.rs)"
else
  fail "C4-code: DEFAULT_SUB_AGENT_MAX_TURNS is '$turns', not 200; docs/advanced.md must be revisited"
fi

# Match the table ROW, not the bare number — "200" appears elsewhere in the file
# and a loose grep would be green against the unfixed tree.
if grep -qE '^\|[[:space:]]*Sub-agent max turns[[:space:]]*\|[[:space:]]*'"$turns"'[[:space:]]*\|' \
  "$ROOT/docs/advanced.md"; then
  pass "C4-doc: docs/advanced.md publishes the sub-agent turn default as $turns"
else
  fail "C4-doc: docs/advanced.md's 'Sub-agent max turns' row does not read $turns"
fi

# ------------------------------------------------------------------ claim (5)
# A Spawn child is READ-ONLY and its registry is INTERSECTED with the parent's.
# Two code facts hold this up: the read-only floor's membership, and the fact
# that the intersection has no skip arm.
floor=$(grep -m1 'pub const SHARED_READ_ONLY_CHILD_TOOLS' \
  "$ROOT/crates/wcore-types/src/spawner.rs")
if printf '%s' "$floor" | grep -q '"Read", "Grep", "Glob"' &&
  ! printf '%s' "$floor" | grep -qE '"(Bash|Write|Edit)"'; then
  pass "C5-code: the read-only child floor is exactly Read/Grep/Glob (spawner.rs)"
else
  fail "C5-code: SHARED_READ_ONLY_CHILD_TOOLS is no longer Read/Grep/Glob alone: $floor"
fi

if grep -q 'permitted && parent_tool_authority.contains' \
  "$ROOT/crates/wcore-agent/src/spawner.rs"; then
  pass "C5-code: build_tool_registry intersects with parent_tool_authority unconditionally"
else
  fail "C5-code: the parent-authority intersection is gone from build_tool_registry"
fi

# The doc must not promise the child the parent's tools. Flatten first: the
# claim wrapped across a line break in the original text.
flat=$(tr '\n' ' ' <"$ROOT/docs/advanced.md" | tr -s ' ')
if printf '%s' "$flat" | grep -qE 'sub-agent has its own conversation context and full tool set'; then
  fail "C5-doc: docs/advanced.md still tells the reader each sub-agent gets the full tool set"
else
  pass "C5-doc: docs/advanced.md does not claim sub-agents get the full tool set"
fi

if printf '%s' "$flat" | grep -q 'Spawn children are read-only'; then
  pass "C5-doc: docs/advanced.md states the read-only floor a Spawn child actually gets"
else
  fail "C5-doc: docs/advanced.md does not state that Spawn children are read-only"
fi

# ------------------------------------------------------------------ claim (6)
# A sub-agent INHERITS the parent's approval posture. The code fact is the
# ABSENCE of an auto_approve flip in child_config; bind it to the comment that
# names the invariant, so deleting the guarantee also reds this check.
if grep -q 'deliberately does NOT flip `auto_approve`' \
  "$ROOT/crates/wcore-agent/src/spawner.rs"; then
  pass "C6-code: child_config does not flip auto_approve (spawner.rs H-7 / M-9)"
else
  fail "C6-code: the 'does NOT flip auto_approve' invariant is gone from child_config"
fi

if grep -qE '^[[:space:]]*config\.tools\.auto_approve[[:space:]]*=[[:space:]]*true' \
  "$ROOT/crates/wcore-agent/src/spawner.rs"; then
  fail "C6-code: something in spawner.rs now forces a child's auto_approve = true"
else
  pass "C6-code: no path in spawner.rs forces a child's auto_approve = true"
fi

if grep -q 'Sub-agents auto-approve all tool calls' "$ROOT/docs/advanced.md"; then
  fail "C6-doc: docs/advanced.md still claims sub-agents auto-approve all tool calls"
else
  pass "C6-doc: docs/advanced.md no longer claims sub-agents auto-approve all tool calls"
fi

if grep -q "inherit the parent's approval posture" "$ROOT/docs/advanced.md"; then
  pass "C6-doc: docs/advanced.md states that a sub-agent inherits the parent's approval posture"
else
  fail "C6-doc: docs/advanced.md does not state the inherited approval posture"
fi

printf '\n%s\n' "----------------------------------------"
if [ "$fails" -eq 0 ]; then
  printf 'GREEN — all doc-truth bindings hold\n'
  exit 0
fi
printf 'RED — %d doc-truth binding(s) broken\n' "$fails"
exit 1
