#!/usr/bin/env bash
# W10A corpus authoring script. Generates 30 good + 30 bad reference cases
# (corpus YAMLs + skill bodies + trace fixtures) under crates/wcore-eval/data/.
#
# This script is idempotent — re-running it regenerates every file exactly.
# Designed for the LOCKED scorer constants (w_outcome=0.7, w_cost=0.2,
# w_size=0.1, cost_saturate_usd=0.05, tokens_saturate=2000,
# size_saturate_bytes=2048, acceptance_cutoff=0.65).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EVAL="$ROOT/crates/wcore-eval"
CORPUS="$EVAL/data/corpus"
SKILLS="$EVAL/data/skills"
TRACES="$EVAL/data/traces"

mkdir -p "$CORPUS" "$SKILLS" "$TRACES"

# Clean any prior generation so the directory contents match this script exactly.
rm -f "$CORPUS"/*.yaml "$SKILLS"/*.md "$TRACES"/*.json
# Keep .gitkeep style sentinels if you add them later; for now glob is empty.

#-------------------------------------------------------------------------------
# Helpers
#-------------------------------------------------------------------------------

# Standard healthy skill body (sonnet-pinned). Used as the source-of-truth
# baseline; variants below differ only in frontmatter or surface phrasing.
HEALTHY_BODY='Hello! I am a friendly greeting skill that welcomes the user warmly. $ARGUMENTS'

# A baseline cheap trace fixture used by all good trace-paired cases.
write_cheap_trace () {
  local out="$1"
  cat > "$out" <<'JSON'
{
  "turn": 1,
  "model": "claude-sonnet-4-7",
  "provider": "anthropic",
  "input_tokens": 80,
  "output_tokens": 40,
  "cache_read": 0,
  "cache_write": 0,
  "cache_hit_rate": 0.0,
  "cost_usd": 0.0012,
  "tool_calls": [],
  "hook_actions": [],
  "source_product": "wayland-core"
}
JSON
}

# A saturated trace fixture (cost + tokens both at/above saturation point).
# Used to push bad cases that otherwise have only 2-3 failing structural
# checks below the acceptance cutoff via the cost penalty.
write_expensive_trace () {
  local out="$1"
  cat > "$out" <<'JSON'
{
  "turn": 1,
  "model": "claude-opus-4-7",
  "provider": "anthropic",
  "input_tokens": 12000,
  "output_tokens": 8000,
  "cache_read": 0,
  "cache_write": 0,
  "cache_hit_rate": 0.0,
  "cost_usd": 0.95,
  "tool_calls": [],
  "hook_actions": [],
  "source_product": "wayland-core"
}
JSON
}

write_skill () {
  # write_skill <name> <yaml-frontmatter-body> <body>
  local name="$1"
  local frontmatter="$2"
  local body="$3"
  {
    echo '---'
    printf '%s\n' "$frontmatter"
    echo '---'
    printf '%s\n' "$body"
  } > "$SKILLS/$name.md"
}

write_case () {
  # write_case <case-id> <category> <skill_body_name> <expected> <rationale> [trace_fixture]
  local id="$1"
  local cat="$2"
  local body="$3"
  local exp="$4"
  local rat="$5"
  local trace="${6:-}"
  local f="$CORPUS/$id.yaml"
  {
    echo "id: $id"
    echo "category: $cat"
    echo "skill_body: $body"
    if [[ -n "$trace" ]]; then
      echo "trace_fixture: $trace"
    fi
    echo "expected_outcome: $exp"
    printf 'rationale: |\n  %s\n' "$rat"
  } > "$f"
}

#-------------------------------------------------------------------------------
# Trace fixtures
#-------------------------------------------------------------------------------

write_cheap_trace "$TRACES/cheap-greet-1.json"
write_cheap_trace "$TRACES/cheap-greet-2.json"
write_cheap_trace "$TRACES/cheap-greet-3.json"
write_cheap_trace "$TRACES/cheap-greet-4.json"
write_cheap_trace "$TRACES/cheap-greet-5.json"
write_expensive_trace "$TRACES/expensive-bad-1.json"
write_expensive_trace "$TRACES/expensive-bad-2.json"
write_expensive_trace "$TRACES/expensive-bad-3.json"
write_expensive_trace "$TRACES/expensive-bad-4.json"
write_expensive_trace "$TRACES/expensive-bad-5.json"

#-------------------------------------------------------------------------------
# GOOD CASES (30)
# Every good case: passes all 9 structural checks; small body; optional cheap
# trace. Expected combined score >= 0.9 with default scorer.
#
# All names match their skill_body filename. All descriptions share a token
# (e.g. "greet", "hello", "welcome", "friendly") with the body.
#-------------------------------------------------------------------------------

# 1x exact baseline
write_skill "hello-baseline" \
'name: hello-baseline
description: A friendly greeting skill that welcomes the user.
when_to_use: At the start of every session to greet the user warmly.
allowed_tools: []
model: claude-sonnet-4-7' \
"$HEALTHY_BODY"
write_case "good-baseline-hello" "healthy" "hello-baseline" "good" \
"Unmodified bundled hello skill. Must pass all 9 structural checks."

# 4x alternate-wording variants
for i in 1 2 3 4; do
  body_name="hello-wording-$i"
  body="Hi there! I am the welcoming greeting skill (variant $i). \$ARGUMENTS"
  write_skill "$body_name" \
"name: $body_name
description: An alternate-wording friendly greeting skill (variant $i).
when_to_use: At the start of every session to greet the user.
allowed_tools: []
model: claude-sonnet-4-7" \
"$body"
  write_case "good-wording-$i" "healthy" "$body_name" "good" \
"Alternate-wording healthy variant $i of hello."
done

# 5x alternate-when_to_use variants
when_to_use_variants=(
  "Use when the user opens a new session and needs a warm greeting."
  "Invoke at conversation start, before any tool use, to welcome the user."
  "Run only when the user has not yet been greeted in this session."
  "Activate when a fresh chat is initiated by the user."
  "Trigger this on session bootstrap to introduce the assistant."
)
for i in 1 2 3 4 5; do
  body_name="hello-when-$i"
  write_skill "$body_name" \
"name: $body_name
description: A friendly greeting skill with an alternate when_to_use phrasing.
when_to_use: ${when_to_use_variants[$((i-1))]}
allowed_tools: []
model: claude-sonnet-4-7" \
"$HEALTHY_BODY"
  write_case "good-when-$i" "healthy" "$body_name" "good" \
"Healthy when_to_use paraphrase $i."
done

# 5x alternate-allowed_tools variants
tools_variants=(
  "[]"
  "[Read]"
  "[Read, Grep]"
  "[Read, Glob]"
  "[Read, Grep, Glob]"
)
for i in 1 2 3 4 5; do
  body_name="hello-tools-$i"
  # Use only tools that are in the allowlist for that case to avoid
  # tripping the disallowed-tool mention check (no tool names mentioned).
  write_skill "$body_name" \
"name: $body_name
description: A friendly greeting skill with alternate allowed_tools (variant $i).
when_to_use: At the start of every session to greet the user warmly.
allowed_tools: ${tools_variants[$((i-1))]}
model: claude-sonnet-4-7" \
"$HEALTHY_BODY"
  write_case "good-tools-$i" "healthy" "$body_name" "good" \
"Healthy allowed_tools variant $i — body still healthy, no tool-name leakage."
done

# 5x alternate-description variants (each shares >=1 token with body)
descs=(
  "Greets the user warmly at session start."
  "A welcoming hello skill for friendly session openings."
  "Sends a warm greeting to the user when a session begins."
  "Outputs a friendly hello to welcome the user."
  "Provides a session-opening greeting that welcomes the user."
)
for i in 1 2 3 4 5; do
  body_name="hello-desc-$i"
  write_skill "$body_name" \
"name: $body_name
description: ${descs[$((i-1))]}
when_to_use: At the start of every session to greet the user warmly.
allowed_tools: []
model: claude-sonnet-4-7" \
"$HEALTHY_BODY"
  write_case "good-desc-$i" "healthy" "$body_name" "good" \
"Healthy description variant $i — shares the 'greet'/'hello'/'welcome' token with the body."
done

# 5x alternate-model variants (all in allowlist)
models=(
  "claude-sonnet-4-7"
  "claude-opus-4-7"
  "claude-haiku-4-5"
  "claude-sonnet-4-7"
  "claude-opus-4-7"
)
for i in 1 2 3 4 5; do
  body_name="hello-model-$i"
  write_skill "$body_name" \
"name: $body_name
description: A friendly greeting skill pinned to an allowlisted model.
when_to_use: At the start of every session to greet the user warmly.
allowed_tools: []
model: ${models[$((i-1))]}" \
"$HEALTHY_BODY"
  write_case "good-model-$i" "healthy" "$body_name" "good" \
"Healthy model-pin variant $i — model is in the W10A allowlist."
done

# 5x trace-paired healthy variants (cheap traces)
for i in 1 2 3 4 5; do
  body_name="hello-trace-$i"
  write_skill "$body_name" \
"name: $body_name
description: A friendly greeting skill with a recorded cheap trace.
when_to_use: At the start of every session to greet the user warmly.
allowed_tools: []
model: claude-sonnet-4-7" \
"$HEALTHY_BODY"
  write_case "good-trace-$i" "healthy" "$body_name" "good" \
"Healthy variant $i paired with a cheap trace (within saturation thresholds)." \
"cheap-greet-$i.json"
done

#-------------------------------------------------------------------------------
# BAD CASES (30)
# Each bad case is engineered so the LOCKED scorer drops its combined score
# below the 0.65 cutoff. Strategy per case is documented inline.
#-------------------------------------------------------------------------------

# Helper: a deliberately oversize body (~3KB, > size_saturate_bytes=2048).
OVERSIZE_BODY=""
for i in $(seq 1 200); do
  OVERSIZE_BODY+="lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor "
done

# Family 1: truncated body (3 cases) — kills $ARGUMENTS + content_length nearly 0.
# At 90% truncation: body is empty or near-empty -> trips $ARGUMENTS, body-non-empty,
# description-distinct (with empty desc), description-shares-token (empty body
# -> returns true to avoid double-counting -> NOT a fail). Stack with empty
# when_to_use, name mismatch, off-topic description, disallowed-tool to push
# below.

# bad-truncated-1: 90% truncation — body is just "Hello!" no $ARGUMENTS.
write_skill "hello-truncated-1" \
'name: wrong-name-1
description: Translates English text to French phrases.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello!'
write_case "bad-truncated-1" "truncated-body" "hello-truncated-1" "bad" \
"90% truncation: no \$ARGUMENTS, empty when_to_use, name mismatch, off-topic description (translator), stale model pin. >=5 checks fail." \
"expensive-bad-1.json"

# bad-truncated-2: 50% truncation — body half-cut, no $ARGUMENTS.
write_skill "hello-truncated-2" \
'name: wrong-name-2
description: Debugs failing unit tests in TypeScript repositories.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I am a friendly greeting skill that welcomes'
write_case "bad-truncated-2" "truncated-body" "hello-truncated-2" "bad" \
"50% truncation: no \$ARGUMENTS, empty when_to_use, name mismatch, off-topic description (debugger), stale model pin." \
"expensive-bad-2.json"

# bad-truncated-3: 25% truncation — body partial, no $ARGUMENTS.
write_skill "hello-truncated-3" \
'name: wrong-name-3
description: Computes arithmetic expressions and returns numeric results.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I am a friendly greeting skill that welcomes the user warmly. The'
write_case "bad-truncated-3" "truncated-body" "hello-truncated-3" "bad" \
"25% truncation: no \$ARGUMENTS, empty when_to_use, name mismatch, off-topic description (calculator), stale model pin."

# Family 2: empty when_to_use (3 cases). The plan maps this to one check.
# Compound with multiple other defects to clear cutoff.
write_skill "hello-empty-when-1" \
'name: wrong-empty-when-1
description: Calculates compound interest on loan amortization schedules.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I am a friendly greeting skill that welcomes the user warmly. $ARGUMENTS'
write_case "bad-empty-when-1" "empty-when-to-use" "hello-empty-when-1" "bad" \
"Empty when_to_use, name mismatch, off-topic description (financial calculator), stale model pin. 4 fails + expensive trace." \
"expensive-bad-3.json"

write_skill "hello-empty-when-2" \
'name: wrong-empty-when-2
description: Lints Python code for PEP8 style violations.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I am a friendly greeting skill that welcomes the user warmly. $ARGUMENTS'
write_case "bad-empty-when-2" "empty-when-to-use" "hello-empty-when-2" "bad" \
"Empty when_to_use, name mismatch, off-topic description (linter), stale model pin. 4 fails + expensive trace." \
"expensive-bad-4.json"

write_skill "hello-empty-when-3" \
'name: wrong-empty-when-3
description: Encrypts secrets using AES-256 with random initialization vectors.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I am a friendly greeting skill that welcomes the user warmly. $ARGUMENTS'
write_case "bad-empty-when-3" "empty-when-to-use" "hello-empty-when-3" "bad" \
"Empty when_to_use, name mismatch, off-topic description (cryptography), stale model pin. 4 fails + expensive trace." \
"expensive-bad-5.json"

# Family 3: name != filename (3 cases). Trip name_matches_filename check.
# Compound: also stale model, off-topic, empty when_to_use, disallowed tool.
write_skill "hello-namemismatch-1" \
'name: calculator
description: Computes arithmetic expressions and returns numeric results.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I am a friendly greeting skill that welcomes the user warmly. $ARGUMENTS'
write_case "bad-namemismatch-1" "name-vs-filename" "hello-namemismatch-1" "bad" \
"Name claims 'calculator' but file is 'hello-namemismatch-1', off-topic description (calculator matches name not body), empty when_to_use, stale model pin. 4+ fails + expensive trace." \
"expensive-bad-1.json"

write_skill "hello-namemismatch-2" \
'name: translator
description: Translates English text into Spanish phrases.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I am a friendly greeting skill that welcomes the user warmly. $ARGUMENTS'
write_case "bad-namemismatch-2" "name-vs-filename" "hello-namemismatch-2" "bad" \
"Name says 'translator', file is hello-namemismatch-2, off-topic description, empty when_to_use, stale model. >=4 fails + expensive trace." \
"expensive-bad-2.json"

write_skill "hello-namemismatch-3" \
'name: file-mover
description: Moves files between directories and preserves permissions.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I am a friendly greeting skill that welcomes the user warmly. $ARGUMENTS'
write_case "bad-namemismatch-3" "name-vs-filename" "hello-namemismatch-3" "bad" \
"Name says 'file-mover', file is hello-namemismatch-3, off-topic description, empty when_to_use, stale model. >=4 fails + expensive trace." \
"expensive-bad-3.json"

# Family 4: off-topic description (3 cases). Trips description_shares_token.
# Compound: also stale model, empty when_to_use, name mismatch.
write_skill "hello-offtopic-1" \
'name: hello-offtopic-x
description: Compiles Rust crates into WebAssembly modules.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I am a friendly greeting skill that welcomes the user warmly. $ARGUMENTS'
write_case "bad-offtopic-1" "off-topic-description" "hello-offtopic-1" "bad" \
"Description about Rust/WASM compilation has zero token overlap with greeter body; name mismatch; empty when_to_use; stale model. 4+ fails + expensive trace." \
"expensive-bad-4.json"

write_skill "hello-offtopic-2" \
'name: hello-offtopic-y
description: Renders interactive 3D scenes using WebGL shaders.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I am a friendly greeting skill that welcomes the user warmly. $ARGUMENTS'
write_case "bad-offtopic-2" "off-topic-description" "hello-offtopic-2" "bad" \
"Description about 3D rendering shares no token with greeter; name mismatch; empty when_to_use; stale model. 4+ fails + expensive trace." \
"expensive-bad-5.json"

write_skill "hello-offtopic-3" \
'name: hello-offtopic-z
description: Provisions Kubernetes pods from declarative manifests.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I am a friendly greeting skill that welcomes the user warmly. $ARGUMENTS'
write_case "bad-offtopic-3" "off-topic-description" "hello-offtopic-3" "bad" \
"Description about k8s shares no token with greeter; name mismatch; empty when_to_use; stale model. 4+ fails + expensive trace." \
"expensive-bad-1.json"

# Family 5: oversize body (3 cases at 2x/5x/20x). Triggers size_penalty
# saturation (1.0). Compound checks: missing $ARGUMENTS, name mismatch,
# stale model.

write_skill "hello-oversize-1" \
'name: wrong-oversize-1
description: A friendly greeting skill that welcomes the user.
when_to_use: At the start of every session.
allowed_tools: []
model: claude-haiku-3-20240306' \
"$OVERSIZE_BODY"
write_case "bad-oversize-1" "oversize-body" "hello-oversize-1" "bad" \
"Oversize body (~15KB) shares NO tokens with description, missing \$ARGUMENTS, name mismatch, stale model. Size penalty saturated."

# 5x baseline body (~400 chars) — moderate size + missing $ARGUMENTS + naming.
MEDIUM_BODY=""
for i in $(seq 1 8); do
  MEDIUM_BODY+="$HEALTHY_BODY "
done
# Remove $ARGUMENTS from MEDIUM_BODY:
MEDIUM_BODY_NO_ARGS="${MEDIUM_BODY//\$ARGUMENTS/placeholder-removed}"
write_skill "hello-oversize-2" \
'name: wrong-oversize-2
description: A friendly greeting skill that welcomes the user warmly.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
"$MEDIUM_BODY_NO_ARGS"
write_case "bad-oversize-2" "oversize-body" "hello-oversize-2" "bad" \
"5x baseline body, missing \$ARGUMENTS, empty when_to_use, name mismatch, stale model. 4+ fails + expensive trace." \
"expensive-bad-2.json"

# 20x baseline body — huge.
HUGE_BODY=""
for i in $(seq 1 400); do
  HUGE_BODY+="$HEALTHY_BODY "
done
HUGE_BODY_NO_ARGS="${HUGE_BODY//\$ARGUMENTS/placeholder-removed}"
write_skill "hello-oversize-3" \
'name: wrong-oversize-3
description: Computes payroll tax withholding for U.S. salaried employees.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
"$HUGE_BODY_NO_ARGS"
write_case "bad-oversize-3" "oversize-body" "hello-oversize-3" "bad" \
"20x baseline body (saturated size penalty), missing \$ARGUMENTS, off-topic description, name mismatch, stale model, empty when_to_use. >=5 fails."

# Family 6: missing $ARGUMENTS (3 cases). Trips $ARGUMENTS check.
# Compound: name mismatch, stale model, off-topic, empty when_to_use.
write_skill "hello-noargs-1" \
'name: wrong-noargs-1
description: Renders SVG icons from vector path definitions.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I am a friendly greeting skill that welcomes the user warmly.'
write_case "bad-noargs-1" "missing-arguments" "hello-noargs-1" "bad" \
"No \$ARGUMENTS, name mismatch, off-topic (SVG/vector), empty when_to_use, stale model. 5 fails."

write_skill "hello-noargs-2" \
'name: wrong-noargs-2
description: Generates passwords using cryptographically secure entropy.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! Welcome to the system.'
write_case "bad-noargs-2" "missing-arguments" "hello-noargs-2" "bad" \
"Short body, no \$ARGUMENTS, name mismatch, off-topic, empty when_to_use, stale model. 5 fails."

write_skill "hello-noargs-3" \
'name: wrong-noargs-3
description: Validates JSON schemas against draft-07 specification.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hi.'
write_case "bad-noargs-3" "missing-arguments" "hello-noargs-3" "bad" \
"Tiny body, no \$ARGUMENTS, name mismatch, off-topic, empty when_to_use, stale model. 5 fails."

# Family 7: description == body (3 cases). Trips description-distinct check.
# Each body is `$ARGUMENTS`-free since we want the description and body equal.
# Compound: name mismatch, stale model, empty when_to_use, missing $ARGUMENTS.
write_skill "hello-descbody-1" \
'name: wrong-descbody-1
description: greet user warmly hello
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'greet user warmly hello'
write_case "bad-descbody-1" "description-equals-body" "hello-descbody-1" "bad" \
"Description identical to body (after trim), no \$ARGUMENTS, name mismatch, empty when_to_use, stale model. >=4 fails."

write_skill "hello-descbody-2" \
'name: wrong-descbody-2
description: friendly hello greeting welcome
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'friendly hello greeting welcome'
write_case "bad-descbody-2" "description-equals-body" "hello-descbody-2" "bad" \
"Description == body, no \$ARGUMENTS, name mismatch, empty when_to_use, stale model. >=4 fails."

write_skill "hello-descbody-3" \
'name: wrong-descbody-3
description: hello hello hello
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'hello hello hello'
write_case "bad-descbody-3" "description-equals-body" "hello-descbody-3" "bad" \
"Description == body, no \$ARGUMENTS, name mismatch, empty when_to_use, stale model. >=4 fails."

# Family 8: disallowed-tool reference (3 cases). allowed_tools empty but body
# mentions Spawn/Bash/Write. Compound: stale model, name mismatch, off-topic
# description, empty when_to_use.
write_skill "hello-disallowed-1" \
'name: wrong-disallowed-1
description: Translates Mandarin phrases into colloquial English.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I should Spawn a subprocess to greet the user. $ARGUMENTS'
write_case "bad-disallowed-1" "disallowed-tool" "hello-disallowed-1" "bad" \
"Body mentions Spawn but allowed_tools=[]; name mismatch; off-topic description (translator); empty when_to_use; stale model. >=4 fails."

write_skill "hello-disallowed-2" \
'name: wrong-disallowed-2
description: Schedules cron jobs across distributed worker nodes.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! I will Bash an echo command to print a greeting. $ARGUMENTS'
write_case "bad-disallowed-2" "disallowed-tool" "hello-disallowed-2" "bad" \
"Body mentions Bash but allowed_tools=[]; name mismatch; off-topic description (cron); empty when_to_use; stale model. >=4 fails."

write_skill "hello-disallowed-3" \
'name: wrong-disallowed-3
description: Provisions AWS S3 buckets with KMS encryption keys.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! Time to Write a file containing the welcome banner. $ARGUMENTS'
write_case "bad-disallowed-3" "disallowed-tool" "hello-disallowed-3" "bad" \
"Body mentions Write but allowed_tools=[]; name mismatch; off-topic (AWS); empty when_to_use; stale model. >=4 fails."

# Family 9: stale model pin (3 cases). Trips model_in_allowlist.
# Compound: name mismatch, off-topic description, empty when_to_use, missing $ARGUMENTS.
write_skill "hello-stalemodel-1" \
'name: wrong-stalemodel-1
description: Predicts stock prices with linear regression models.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello!'
write_case "bad-stalemodel-1" "stale-model-pin" "hello-stalemodel-1" "bad" \
"Stale model (haiku-3-20240306), no \$ARGUMENTS, off-topic description, name mismatch, empty when_to_use. 5 fails."

write_skill "hello-stalemodel-2" \
'name: wrong-stalemodel-2
description: Routes network packets through SOCKS5 proxies.
when_to_use: ""
allowed_tools: []
model: gpt-4-turbo-2024-04-09' \
'Hi.'
write_case "bad-stalemodel-2" "stale-model-pin" "hello-stalemodel-2" "bad" \
"Stale/non-Claude model, no \$ARGUMENTS, off-topic, name mismatch, empty when_to_use. 5 fails."

write_skill "hello-stalemodel-3" \
'name: wrong-stalemodel-3
description: Optimizes SQL queries using cost-based execution plans.
when_to_use: ""
allowed_tools: []
model: claude-instant-1.2' \
'Hi.'
write_case "bad-stalemodel-3" "stale-model-pin" "hello-stalemodel-3" "bad" \
"Stale claude-instant model, no \$ARGUMENTS, off-topic, name mismatch, empty when_to_use. 5 fails."

# Family 10: UTF-8 replacement-char chunks in body (3 cases). The plan says
# these may be caught by the loader OR a structural check; we treat them as
# additional structural defects since the loader uses read_to_string which
# already replaces invalid UTF-8. So we rely on compound failures.
write_skill "hello-utf8-1" \
'name: wrong-utf8-1
description: Encodes DNA sequences as one-hot tensors for ML.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'Hello! \u{FFFD}\u{FFFD}\u{FFFD} replacement chars.'
write_case "bad-utf8-1" "invalid-utf8" "hello-utf8-1" "bad" \
"UTF-8 replacement chars in body, no \$ARGUMENTS, off-topic description, name mismatch, empty when_to_use, stale model. 5+ fails."

write_skill "hello-utf8-2" \
'name: wrong-utf8-2
description: Aggregates Twitter sentiment scores across product launches.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'??? broken body ???'
write_case "bad-utf8-2" "invalid-utf8" "hello-utf8-2" "bad" \
"Broken body without \$ARGUMENTS, off-topic, name mismatch, empty when_to_use, stale model. 5+ fails."

write_skill "hello-utf8-3" \
'name: wrong-utf8-3
description: Synchronizes calendar events between Google and Outlook.
when_to_use: ""
allowed_tools: []
model: claude-haiku-3-20240306' \
'??? mojibake'
write_case "bad-utf8-3" "invalid-utf8" "hello-utf8-3" "bad" \
"Mojibake fragment, no \$ARGUMENTS, off-topic, name mismatch, empty when_to_use, stale model. 5+ fails."

#-------------------------------------------------------------------------------
# Final tally check (advisory)
#-------------------------------------------------------------------------------

GOOD_COUNT=$(grep -l "^expected_outcome: good$" "$CORPUS"/*.yaml | wc -l | tr -d ' ')
BAD_COUNT=$(grep -l "^expected_outcome: bad$" "$CORPUS"/*.yaml | wc -l | tr -d ' ')
TOTAL=$(ls "$CORPUS"/*.yaml | wc -l | tr -d ' ')

echo "Generated $TOTAL cases ($GOOD_COUNT good / $BAD_COUNT bad) in $CORPUS"
echo "Skill bodies: $(ls "$SKILLS"/*.md | wc -l | tr -d ' ') in $SKILLS"
echo "Traces: $(ls "$TRACES"/*.json | wc -l | tr -d ' ') in $TRACES"
