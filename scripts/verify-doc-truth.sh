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

# ------------------------------------------------------------------ claim (7)
# Memory is ON by default. `docs/memory.md` said "off by default" four times and
# cited the regression test that asserts the opposite. Inverted privacy defaults
# are the worst class of doc error: the reader concludes nothing is recorded.
if grep -A14 'impl Default for MemoryConfig' \
  "$ROOT/crates/wcore-config/src/config.rs" | grep -qE '^[[:space:]]+enabled: true,'; then
  pass "C7-code: MemoryConfig::default has enabled: true (config.rs, F-091)"
else
  fail "C7-code: MemoryConfig::default no longer defaults enabled to true; docs/memory.md must be revisited"
fi

# A present `[memory]` table that omits `enabled` must also stay ON, or the doc's
# footgun note is wrong in the other direction.
if grep -B2 'pub enabled: bool,' "$ROOT/crates/wcore-config/src/config.rs" |
  grep -q 'serde(default = "default_true")'; then
  pass "C7-code: the serde default for memory.enabled is default_true"
else
  fail "C7-code: memory.enabled's serde default is no longer default_true"
fi

if grep -qiE 'Memory is (off|disabled) by default|Default impl is[[:space:]]*`?enabled: false' \
  "$ROOT/docs/memory.md"; then
  fail "C7-doc: docs/memory.md still says memory is off by default"
else
  pass "C7-doc: docs/memory.md does not claim memory is off by default"
fi

if grep -q 'Memory is \*\*on by default\*\*' "$ROOT/docs/memory.md"; then
  pass "C7-doc: docs/memory.md states that memory is on by default"
else
  fail "C7-doc: docs/memory.md does not state that memory is on by default"
fi

# ------------------------------------------------------------------ claim (8)
# MCP `deferred` defaults to TRUE. docs/mcp.md published `false (default)`.
if grep -rq 'deferred.unwrap_or(true)' "$ROOT/crates/wcore-cli/src/"; then
  pass "C8-code: MCP deferred resolves as unwrap_or(true) in wcore-cli"
else
  fail "C8-code: MCP deferred no longer resolves to true when omitted"
fi

if grep -q '`false` (default for config servers)' "$ROOT/docs/mcp.md"; then
  fail "C8-doc: docs/mcp.md still publishes deferred=false as the default"
else
  pass "C8-doc: docs/mcp.md no longer publishes deferred=false as the default"
fi

if grep -q '`true` (\*\*the default\*\* when the key is omitted)' "$ROOT/docs/mcp.md"; then
  pass "C8-doc: docs/mcp.md names deferred=true as the default"
else
  fail "C8-doc: docs/mcp.md does not name deferred=true as the default"
fi

# ------------------------------------------------------------------ claim (9)
# ToolSearch takes ONE parameter and caps nothing. The doc described a `select:`
# prefix and a 5-result default, neither of which exists in the tool.
if grep -q 'name_l.contains(&query_lower) || desc_l.contains(&query_lower)' \
  "$ROOT/crates/wcore-tools/src/tool_search.rs" &&
  ! grep -qE '"select:|starts_with\("select' "$ROOT/crates/wcore-tools/src/tool_search.rs"; then
  pass "C9-code: ToolSearch is a plain substring match with no select: prefix parsing"
else
  fail "C9-code: ToolSearch's matching changed; docs/tools.md must be revisited"
fi

if grep -qE 'max_results|\.take\(' "$ROOT/crates/wcore-tools/src/tool_search.rs"; then
  fail "C9-code: ToolSearch now has a result cap; docs/tools.md says it has none"
else
  pass "C9-code: ToolSearch has no result cap and no max_results parameter"
fi

if grep -q 'select:Read,Edit,Grep' "$ROOT/docs/tools.md" ||
  grep -q 'Returns up to 5 results by default' "$ROOT/docs/tools.md"; then
  fail "C9-doc: docs/tools.md still documents ToolSearch's nonexistent select: syntax or 5-result cap"
else
  pass "C9-doc: docs/tools.md documents neither the select: syntax nor a result cap"
fi

# ------------------------------------------------------------------ claim (10)
# The Swarm topology cap is MAX_DISPATCH_WORKERS (100), not 20.
if grep -q 'pub const MAX_DISPATCH_WORKERS: usize = 100;' \
  "$ROOT/crates/wcore-swarm/src/lib.rs" &&
  grep -A3 'Self::Swarm => TopologyConfig' "$ROOT/crates/wcore-swarm/src/topology.rs" |
  grep -q 'max_agents: crate::MAX_DISPATCH_WORKERS'; then
  pass "C10-code: Swarm's cap is MAX_DISPATCH_WORKERS == 100"
else
  fail "C10-code: Swarm's cap is no longer MAX_DISPATCH_WORKERS == 100; README must be revisited"
fi

if grep -q '\*\*Swarm\*\* (20)' "$ROOT/README.md"; then
  fail "C10-doc: README.md still publishes the Swarm cap as 20"
else
  pass "C10-doc: README.md does not publish the Swarm cap as 20"
fi

# ------------------------------------------------------------------ claim (11)
# The default auto-approve set is no longer one flat `vec![...]`. #946 A-10
# split it into ONE audit table with a per-row scope, `AUDITED_DEFAULT_GRANTS`
# in config.rs, because `default_allow_list()` feeds two consumers with
# different threat models and only one of them has an operator behind it:
#
#   * LOCAL  — every row — is the `#[serde(default)]` for `[tools] allow_list`.
#   * REMOTE — the `GrantScope::Remote` rows — is what survives
#     `Config::retain_default_tool_allow_list()`, i.e. what an ACP/A2A network
#     session and a remote chat sender keep with no TTY to answer a prompt.
#
# getting-started.md now publishes BOTH numbers, so both are bound here.
#
# The previous binding counted `"X".into(),` lines inside `fn default_allow_list`.
# Do not restore that shape. It was never safe: the sed range `/fn
# default_allow_list/,/^}/` also opens on TEST functions whose names merely
# START with `default_allow_list` (e.g.
# `default_allow_list_only_applies_when_the_key_is_absent`), so against the
# reworked tree it reported SIX entries — a number that exists nowhere in the
# product. A binding that goes red for the wrong reason teaches the next reader
# to ignore it. Everything below anchors on `^const AUDITED_DEFAULT_GRANTS` /
# `^];` and on the row syntax itself.
grants=$(sed -n '/^const AUDITED_DEFAULT_GRANTS/,/^];/p' \
  "$ROOT/crates/wcore-config/src/config.rs")

# Known-positive control for the extraction. Without it, a renamed const makes
# every count below 0 and each one reads as "the tool was removed" rather than
# "this check stopped working".
if [ -z "$grants" ]; then
  fail "C11-code: AUDITED_DEFAULT_GRANTS not found in config.rs; every C11-code check below is vacuous"
else
  pass "C11-code: AUDITED_DEFAULT_GRANTS table located in config.rs (extraction control)"
fi

grant_rows=$(printf '%s\n' "$grants" | grep -cE '^[[:space:]]+\("[A-Za-z_]+", GrantScope::[A-Za-z]+\),$')
remote_rows=$(printf '%s\n' "$grants" | grep -cE '^[[:space:]]+\("[A-Za-z_]+", GrantScope::Remote\),$')
local_only_rows=$(printf '%s\n' "$grants" | grep -cE '^[[:space:]]+\("[A-Za-z_]+", GrantScope::LocalOnly\),$')

# Structural check FIRST: if a row carries a scope this script does not know
# about, the two scope counts stop summing and the doc numbers below would be
# graded against a set nobody classified.
if [ "$grant_rows" -eq $((remote_rows + local_only_rows)) ]; then
  pass "C11-code: every AUDITED_DEFAULT_GRANTS row is Remote or LocalOnly ($grant_rows = $remote_rows + $local_only_rows)"
else
  fail "C11-code: $grant_rows grant rows but only $remote_rows Remote + $local_only_rows LocalOnly; a row carries an unclassified GrantScope"
fi

if [ "$grant_rows" = "13" ]; then
  pass "C11-code: the LOCAL default allow list has 13 entries (AUDITED_DEFAULT_GRANTS)"
else
  fail "C11-code: the LOCAL default allow list has $grant_rows entries, not 13; docs/getting-started.md must be revisited"
fi

if [ "$remote_rows" = "11" ]; then
  pass "C11-code: 11 entries are GrantScope::Remote and survive retain_default_tool_allow_list()"
else
  fail "C11-code: $remote_rows entries are GrantScope::Remote, not 11; docs/getting-started.md must be revisited"
fi

# Named rows, with their SCOPE, not merely their presence. `web`/`WebFetch`
# reach the network and `Skill` can run a skill's embedded `!` shell directives,
# so the doc warns about them by name; demoting any of them to LocalOnly, or
# dropping it, makes that warning wrong.
for needle in '("WebFetch", GrantScope::Remote),' '("Skill", GrantScope::Remote),' '("web", GrantScope::Remote),'; do
  if printf '%s\n' "$grants" | grep -qF "$needle"; then
    pass "C11-code: $needle is in the audited grant table"
  else
    fail "C11-code: $needle is gone from the audited grant table; the doc's warning about it is stale"
  fi
done

# The extractors are the point of #946 A-10 and are LOCAL ONLY. `doc_extract`
# writes $TMPDIR/wayland-doc-extract/<hash>.md, which the doc discloses; if it
# ever became Remote both that sentence and the remote count would be wrong.
for needle in '("pdf_extract", GrantScope::LocalOnly),' '("doc_extract", GrantScope::LocalOnly),'; do
  if printf '%s\n' "$grants" | grep -qF "$needle"; then
    pass "C11-code: $needle is local-only in the audited grant table"
  else
    fail "C11-code: $needle is not a LocalOnly row; docs/getting-started.md says both extractors are stripped from every remote surface"
  fi
done

# `doc_extract`'s temp-artifact path is quoted verbatim in the doc, so bind it.
if grep -q 'wayland-doc-extract' "$ROOT/crates/wcore-tools/src/doc_tool.rs"; then
  pass "C11-code: doc_extract still writes under wayland-doc-extract (doc_tool.rs)"
else
  fail "C11-code: doc_tool.rs no longer mentions wayland-doc-extract; the doc quotes that path"
fi

if grep -q 'Read-only tools (Read, Grep, Glob) are auto-approved by default' \
  "$ROOT/docs/getting-started.md"; then
  fail "C11-doc: docs/getting-started.md still describes the default auto-approve set as three read-only tools"
else
  pass "C11-doc: docs/getting-started.md does not describe the default auto-approve set as three tools"
fi

if grep -q '\*\*Thirteen\*\* tools are auto-approved for you, the local operator' \
  "$ROOT/docs/getting-started.md"; then
  pass "C11-doc: docs/getting-started.md states the real size of the LOCAL default auto-approve set"
else
  fail "C11-doc: docs/getting-started.md does not state the real size of the LOCAL default auto-approve set"
fi

if grep -q '\*\*Eleven\*\* of those thirteen survive for a REMOTE caller' \
  "$ROOT/docs/getting-started.md"; then
  pass "C11-doc: docs/getting-started.md states the real size of the REMOTE retained set"
else
  fail "C11-doc: docs/getting-started.md does not state the real size of the REMOTE retained set"
fi

# The honesty clause. The list is NOT "nothing that writes": `doc_extract`
# writes a temp artifact and `Skill` can write declared artifacts and run a
# shell. The doc must disclose both, or it is back to the sentence this claim
# exists to correct.
if grep -q 'wayland-doc-extract/<hash>.md' "$ROOT/docs/getting-started.md"; then
  pass "C11-doc: docs/getting-started.md discloses the file doc_extract writes"
else
  fail "C11-doc: docs/getting-started.md does not disclose that doc_extract writes \$TMPDIR/wayland-doc-extract/<hash>.md"
fi

if grep -q 'embedded `!` shell directives' "$ROOT/docs/getting-started.md"; then
  pass "C11-doc: docs/getting-started.md discloses that an auto-approved Skill can run a shell"
else
  fail "C11-doc: docs/getting-started.md no longer discloses that an auto-approved Skill can run a shell"
fi

printf '\n%s\n' "----------------------------------------"
if [ "$fails" -eq 0 ]; then
  printf 'GREEN — all doc-truth bindings hold\n'
  exit 0
fi
printf 'RED — %d doc-truth binding(s) broken\n' "$fails"
exit 1
