---
phase: 23A-governed-skills
plan: "01"
type: execute
wave: 1
depends_on: []
files_modified:
  - .planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md
  - .planning/phases/23A-governed-skills/23A-01-LIVE-EVIDENCE.md
  - crates/wcore-skills/tests/generated_execution_boundary.rs
  - crates/wcore-skills/src/loader.rs
  - crates/wcore-skills/src/refs.rs
  - crates/wcore-agent/src/skill_tool.rs
  - crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs
  - scripts/f23a-boundary-drive.sh
  - scripts/f23a-boundary-drive.ps1
autonomous: true
requirements:
  - F23-01
domain: code
must_haves:
  truths:
    - "THE SECURITY CLAIM IS 'CANNOT EXECUTE', AND A GUARD FUNCTION RETURNING FALSE IS NOT THAT CLAIM. Success Criterion 1 says generated skills cannot execute before governed promotion. `crates/wcore-skills/src/loader.rs:443-449` sets `disable_model_invocation = true` for generated provenance and four downstream surfaces honour it — `refs.rs::resolve_for_model` (286-343), the system-prompt filter in `wcore-agent/src/context.rs:327`, the router candidate pool `catalog.visible()` in `bootstrap.rs:2026`, and the `/skill run` refusal in `slash/skill.rs:115-119`. Every one of those is a unit-testable predicate. NONE of them is evidence that the shipped `wayland-core` binary refuses. Phase 20A drove Windows and macOS acceptance to CI-green and nobody ever launched the binary; this plan exists so that does not happen to the phase's only security criterion."
    - "THE ENFORCEMENT BOUNDARY MUST BE ENUMERATED BEFORE IT IS TRUSTED, BECAUSE ONE UNGATED ROUTE REFUTES THE WHOLE CLAIM. Generated skill content reaches execution or the model through more than one route, and they do not share a chokepoint. Routes to enumerate and resolve: the `Skill` tool call (`wcore-agent/src/skill_tool.rs:187` via `resolve_for_model`); the `/skill run|show|list` slash surface (`wcore-agent/src/slash/skill.rs`, dispatched from `wcore-cli/src/main.rs:100` and `:4826`); the system-prompt skill listing (`wcore-agent/src/context.rs:327`); the per-turn `SkillRouter` hint (`engine.rs:5312`) and its seed pool (`bootstrap.rs:2026, 2052`, hydrated from the `auto_drafter` PromptStore rows the drafter writes at `auto_skill/drafter.rs:124-134`); the cron skill sink, which renders the post-substitution `!shell:` body through `wcore_skills::executor::render_shell_input` before dispatch (`wcore-agent/src/cron.rs:262-265` and `bootstrap.rs:3260-3263`); skill-declared hooks (`wcore_skills::hooks::parse_skill_hooks`, called from `skill_tool.rs:440`); skill-declared MCP servers (`wcore-skills/src/mcp.rs`); conditional path-glob activation (`wcore-skills/src/conditional.rs`); and artifact resolution (`wcore-skills/src/artifacts.rs`). The census resolves each to GATED-BY-<mechanism> or UNGATED with the evidence, and an UNGATED route at CRITICAL or HIGH is fixed inside this plan."
    - "THE QUARANTINE VERDICT IS COMPUTED FROM TWO MUTABLE FILES THAT LIVE INSIDE THE QUARANTINED DIRECTORY ITSELF, AND THAT IS A NAMED HYPOTHESIS THIS PLAN MUST RESOLVE BY MEASUREMENT, NOT ASSUME. `loader::is_generated_draft` (loader.rs:463-474) reads `<skill_dir>/manifest.json` and, when `auto_drafted` is absent or not true, falls back to `draft::is_released_generated_skill(name, content)`, which matches the exact released body shape that `auto_skill/drafter.rs::compose_body` emits. Both inputs are bytes inside `$WAYLAND_HOME/skills/<name>/`. On its face, rewriting `SKILL.md` out of that body shape while clearing or deleting `manifest.json` yields a skill no longer classified as generated — and the crate's own test `user_authored_auto_prefixed_skill_remains_visible` demonstrates that exact shape loading model-visible. Whether that is REACHABLE is a separate question: whether the agent's own Write and Edit tool policy actually permits writing under `$WAYLAND_HOME/skills/`. Reachable makes it a Tampering finding at HIGH; unreachable makes it a residual risk with a stated precondition. Neither is asserted without the measurement, and the two measurements are never collapsed into one conclusion."
    - "THE HONEST OUTCOME MAY BE THAT NO GAP EXISTS, AND THAT IS A COMPLETE RESULT. If the census resolves every enumerated route to GATED and the hostile corpus cannot get a planted nonce to execute or into an outbound provider body, the correct finding is REFUTED-NO-GAP: record it with its evidence, change no enforcement code, and still deliver the live refusal proof, because the live proof is what Success Criterion 1 actually owes. Manufacturing a change to justify the plan is the failure mode here. 20A-03 defined REFUTED-NO-DEFECT as a first-class termination state and closing in it was correct."
    - "THE ONLY HONEST PROOF THAT CONTENT DID NOT EXECUTE IS A RUN-TIME NONCE ABSENT FROM EVERY OBSERVABLE SINK. Assert absence from sinks, never presence of an internal flag. Plant a nonce generated at run time into the draft body, into a `!shell:` directive inside that body, and into the draft's frontmatter description. Then prove the nonce never appears in: the shell's observable effect (a file the directive would create), the outbound provider request body recorded by `crates/wcore-cli/tests/support/mock_llm.rs`'s `RecordedRequest` / `received_requests` surface, the `Skill` tool result the model receives, and the rendered system prompt. `crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs:920-926` already asserts the rejection does not disclose the draft body — extend that discipline rather than re-deriving it."
    - "EVERY GATE HERE MUST BE ABLE TO GO RED, AND TWO SHAPES THAT CANNOT ARE BANNED OUTRIGHT. A pipeline's exit status is the LAST command's, so `ssh host 'cmd' | grep -v CLIXML` reports success whenever any line survives the filter no matter what the remote did — the 20A-03 Hetzner gate `... 2>&1 | tail -30` has exactly this defect and it is not copied here. Reading an exit code out of a block that also emits output has the same effect: a `$exit = & { cargo ... | Tee-Object ...; $LASTEXITCODE }` block returns an ARRAY and every comparison against it is truthy, which is how a 12/12 and 6/6 all-PASS soak reported failure. Every remote gate here redirects to a file, captures the status on the NEXT line, filters only for READING, and exits with the captured status. Every driver script asserts its own checkout SHA before doing anything and carries a NEGATIVE control whose success is a failure."
    - "A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, add an ignore or allow attribute, raise a timeout, re-gate, or delete an inconvenient test to reach a gate. Findings at CRITICAL or HIGH must be fixed or disproved inside this plan; MEDIUM and below go to `.planning/BACKLOG.md` and DO NOT BLOCK. Never invent a stricter rule than that."
  artifacts:
    - path: .planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md
      provides: "The enumerated set of routes from generated on-disk skill content to byte execution or model-visible context, each resolved to GATED-BY-<mechanism> or UNGATED with file:line evidence, severity, and disposition"
    - path: crates/wcore-skills/tests/generated_execution_boundary.rs
      provides: "The hostile corpus: a run-time nonce planted in a generated draft's body, its shell directive and its frontmatter, driven at every enumerated route, asserting absence from the shell's observable effect, the recorded outbound provider body, the tool result and the system prompt, plus a user-authored negative control that must succeed"
    - path: crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs
      provides: "The refusal probes added to the existing packaged lifecycle matrix, which already builds the real wayland-core binary, drives /skill list, /skill show and a Skill tool call against a fixture provider, and asserts quarantine"
    - path: scripts/f23a-boundary-drive.sh
      provides: "The Linux live driver: SHA-asserting, negative-controlled, exits nonzero if an unpromoted generated skill ever executes through the shipped binary"
    - path: scripts/f23a-boundary-drive.ps1
      provides: "The Windows live driver with the same contract, invoked via powershell -NoProfile -File so its own exit status is the gate"
    - path: .planning/phases/23A-governed-skills/23A-01-LIVE-EVIDENCE.md
      provides: "The recorded live outcome per route per platform: the exact invocation, the observed refusal text, the exit code, and the negative control's result"
  key_links:
    - from: crates/wcore-skills/src/loader.rs
      to: crates/wcore-skills/src/refs.rs
      via: "is_generated_draft setting disable_model_invocation, which resolve_for_model then treats as absent — the single decision every model-facing surface inherits"
      pattern: "provenance-to-visibility"
    - from: .planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md
      to: crates/wcore-skills/tests/generated_execution_boundary.rs
      via: "each enumerated route becoming one hostile case, so the corpus is complete by construction rather than by intuition"
      pattern: "census-to-corpus"
    - from: scripts/f23a-boundary-drive.sh
      to: .planning/phases/23A-governed-skills/23A-01-LIVE-EVIDENCE.md
      via: "the captured per-route transcript promoted into the recorded live outcome"
      pattern: "live-evidence"
---

<objective>
Establish, prove and — only where a real gap is found — close the enforcement boundary that makes "a generated skill cannot execute before governed promotion" true, and prove it by driving the shipped `wayland-core` binary until it refuses, on Linux and on Windows.

Purpose: This is the security half of Phase 23A's only Success Criterion, and it is the half that must be true before promotion is worth building. Plan 23A-02 replaces the currently-suspended promotion path with a governed transaction; that transaction is only meaningful if the pre-promotion state is genuinely inert. The engine already carries a quarantine mechanism and four surfaces that honour it, but the mechanism has never been enumerated against the full set of routes by which skill content reaches execution, and it has never been proved at the product surface a user actually touches.
Output: One route census with a disposition per route; one hostile nonce corpus covering every enumerated route with a working negative control; any CRITICAL or HIGH gap fixed or explicitly disproved; and one recorded live refusal per platform from the real binary, produced by a driver that fails loudly when the refusal does not happen.
</objective>

<execution_context>
@$HOME/.codex/gsd-core/workflows/execute-plan.md
@$HOME/.codex/gsd-core/templates/summary.md
</execution_context>

<context>
@AGENTS.md
@.planning/HANDOFF-2026-07-26-phase20-20A-complete.md
@crates/wcore-skills/src/loader.rs
@crates/wcore-skills/src/refs.rs
@crates/wcore-skills/src/draft.rs
@crates/wcore-agent/src/auto_skill/drafter.rs
@crates/wcore-agent/src/slash/skill.rs
@crates/wcore-agent/src/skill_tool.rs
@crates/wcore-skills/tests/wayland_home_auto_skill_loop.rs
@crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs
@crates/wcore-cli/tests/support/mock_llm.rs
</context>

<execution_rules>

**THE TWO AMENDED PHASE RULES — verbatim, and they bound this plan.**
- Findings at CRITICAL or HIGH must be fixed or disproved. MEDIUM and below are logged to `.planning/BACKLOG.md` and DO NOT BLOCK execution.
- Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean.

**TERMINATION CRITERION (hard — this plan STOPS rather than spawning more work).** It ends in exactly one of three states and writes its SUMMARY in all three:
1. **GAP-CLOSED** — one or more enumerated routes were UNGATED at CRITICAL or HIGH, the fix is confined to this plan's declared files, it was implemented, and the live drivers prove refusal on both platforms.
2. **REFUTED-NO-GAP** — every enumerated route resolves to GATED and the hostile corpus cannot get the nonce out. Change no enforcement code. Still deliver the census, the corpus and the live refusal proof. This is a complete and successful outcome.
3. **ESCALATED** — a CRITICAL or HIGH gap exists whose fix reaches outside this plan's declared files, for example into the tool permission model or `wcore-config`. Record it with severity and blast radius, do NOT follow it, and stop.
Under no circumstances does this plan create additional plans, extend its own task list, or begin a second census cycle.

**SCOPE BOUNDARY (hard).** This plan proves the pre-promotion state is inert. It does NOT build promotion (23A-02), observe/revoke/rollback (23A-03), or the end-to-end journey driver (23A-04). It does not touch operator lifecycle, memory controls, the repository index, cache economics or the multi-day journey — those are Phase 23B and are already planned under `.planning/phases/23B-continuous-agency/`. If the census leads toward any of those surfaces, record the connection and stop.

**FOUR-PLAN CAP.** Phase 20 produced 74 plans; Phase 20A produced 4 and shipped. This phase has exactly 4. Do not propose a fifth.

**ENVIRONMENT.**
- Repository: `/Users/seandonahoe/dev/waylandcore-ferrox`, branch `plan/f20-unified-audit-repair`. NEVER touch `/Users/seandonahoe/dev/waylandcore` — a different, heavily-dirty checkout.
- NEVER run Cargo on this Mac. `cargo fmt --all -- --check` is the only cargo command used locally.
- Linux authority: `ssh -o BatchMode=yes hetzner-dsm`, `/root/wayland`. The full workspace aggregate runs there in roughly 194 seconds.
- Windows: `ssh -o BatchMode=yes SeanD@seandesktop`, checkout `C:\ferrox-win`, PowerShell is the default shell, cargo at `C:\Users\seand\.cargo\bin\cargo.exe`. `cargo fmt --all` FAILS there with os error 206 and `justfile:96-98` already skips it. Windows CI runs clippy `-D warnings` BEFORE tests, so a lint failure means tests never execute.
- Both hosts' fetch refspecs are pinned to an unrelated branch, so `git fetch --all` silently misses this branch. ALWAYS `git fetch origin plan/f20-unified-audit-repair`. On the Mac `origin` is a stale local worktree and the real remote is `gh`; on the remote hosts `origin` IS correct.
- Mac `grep` is rtk-proxied and SILENTLY DROPS LINES — measured 32 returned versus 674 for the same inverted match on one file. ALWAYS `/usr/bin/grep`, with `-F` for literals. Same for `ls`. Use `/usr/bin/git` on the Mac.
- In `cmd`, `set VAR=x && ...` appends a TRAILING SPACE and Rust silently ignores the value. Use `set "VAR=x"` or `$env:VAR='x'` and PROVE it took effect before trusting anything downstream of it.
- Push the work branch to `gh` so the hosts can fetch it. NO push to main, merge, PR, tag, release, deployment or issue closure — Sean-only.
- No git write commands in this repository beyond the executor's own commit discipline: no reset, checkout, stash or rebase here. `git checkout --detach <SHA>` on the disposable remote checkouts is permitted and is how they are pinned.

**THE SELF-PASSING GATE BAN (hard).**
- `ssh host 'cmd' | grep -v CLIXML` is FORBIDDEN as a gate; the pipeline's status is grep's. Filtering for READING is fine, filtering as the gate is not.
- Reading an exit code from a block that also emits output — for example around a `Tee-Object` pipeline — is FORBIDDEN. Read the status on the line AFTER the pipeline.
- For every command written here, ask what makes it go red. "Nothing", or "only if output is empty", means it is not a verification.
- Do NOT close any part of Success Criterion 1 by grepping an evidence file this plan's own tasks wrote. The proof is the remote command's exit status and the driver's negative control.

**AGENTS.md discipline.** Surgical diffs only; every changed line traces to a recorded CRITICAL or HIGH finding. No drive-by refactor of the loader, the catalog or the skill tool. Clippy-clean at `-D warnings`. `thiserror` for public error types, `anyhow` internally, no `unwrap()` in production code. Stage the exact paths in `files_modified`, never `-A` and never `.`. No `Co-Authored-By` trailers.
</execution_rules>

<tasks>

<task type="auto">
  <name>Task 1: Enumerate every route from generated skill content to execution or model-visible context, and resolve each with file:line evidence</name>
  <files>.planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md</files>
  <read_first>crates/wcore-skills/src/loader.rs (the is_generated_draft classifier at 463-474, its single call site at 443-449, and every public load entry point with its _with_bundled variant), crates/wcore-skills/src/refs.rs (resolve versus resolve_for_model at 286-343, and SkillCatalog::visible at 127-129), crates/wcore-skills/src/draft.rs (is_released_generated_skill and the exact released body shape it matches), crates/wcore-agent/src/auto_skill/drafter.rs (the bytes actually written under the resolved user skills dir — manifest.json first, then SKILL.md, plus the PromptStore row), crates/wcore-agent/src/skill_tool.rs (the resolution call at 187 and the hook parse at 440), crates/wcore-agent/src/slash/skill.rs (the list, show and run surface), crates/wcore-agent/src/context.rs (the system-prompt skill filter near 327), crates/wcore-agent/src/bootstrap.rs (the router candidate pool near 2026, the auto_drafter seed pass near 2052, and the cron skill sink near 3260), crates/wcore-agent/src/cron.rs (the skill sink near 262 and its pre-dispatch body scan), crates/wcore-skills/src/hooks.rs, crates/wcore-skills/src/mcp.rs, crates/wcore-skills/src/conditional.rs, crates/wcore-skills/src/artifacts.rs, crates/wcore-skills/src/executor.rs (render_shell_input and prepare_inline_content — the functions that compose the bytes a shell receives)</read_first>
  <behavior>
    - Every route by which bytes originating in a generated skill artifact can reach either a process the product spawns or a payload the product sends to a provider is enumerated by name, with the file and line where the route is taken.
    - Each route resolves to GATED-BY-&lt;mechanism&gt; with the file and line of the gate, or UNGATED with the file and line of the unguarded call.
    - The router seed path is resolved explicitly, because the drafter writes an auto_drafter PromptStore row for every draft and bootstrap hydrates it: state whether the seed pool is drawn from visible refs only, and cite the line.
    - The cron skill sink is resolved explicitly, because it composes the post-substitution shell body before dispatch: state whether the dispatch path it hands off to applies the model-facing resolver or the unrestricted one, and cite the line.
    - The forgery hypothesis is resolved as two separate measurements: whether rewriting the body out of the released shape and clearing or removing the manifest un-quarantines the content, AND whether the agent's Write and Edit tool policy permits writing under the resolved user skills directory in a default configuration.
    - Every UNGATED route and the forgery hypothesis carry a severity of critical, high, medium or low, with the impact and likelihood that produced it.
    - The census states the plan's termination state as GAP-CLOSED with the routes to fix, REFUTED-NO-GAP, or ESCALATED with the blast radius.
  </behavior>
  <action>Read the code before writing a line of the census. Every claim carries a path-and-line citation, and a claim without one is not admissible — a fabricated call site on an enforcement census is the most expensive error available in this plan, because 23A-02 and 23A-03 both build on its conclusions.

Enumerate the routes by starting from the artifact on disk — the manifest and body pair the drafter writes under the resolved user skills directory — and following every consumer forward. The routes known to exist at planning time are the Skill tool call, the slash surface's list, show and run actions, the system-prompt skill listing, the per-turn router hint and its seed pool, the cron skill sink, skill-declared hooks, skill-declared MCP servers, conditional path-glob activation, and artifact resolution. Treat that list as a floor and not a ceiling: if reading the code reveals another consumer of skill metadata, a skill ref, or a skill body, it goes in the census too.

For each route record what reaches it, which resolver or filter stands in front of it with its path and line, what that filter keys on, and whether generated-unpromoted content survives it. Where a route is gated, name the mechanism precisely — the loader setting the model-invocation flag, the model-facing resolver treating hidden entries as absent, the visible-refs iterator restricting a pool — because 23A-02 must preserve every one of them while adding promotion, and 23A-03 must be able to revoke back through them.

Resolve the forgery hypothesis by measurement, one variable at a time. First, construct in a tempdir a directory whose body does not match the released generated shape and whose manifest is absent or does not set the auto-drafted marker true, and observe what the loader classifies it as. Second, and separately, determine from the tool permission model whether the agent's own file-writing tools can write under the resolved user skills directory in a default configuration. Record both observations with their commands and outputs. A hypothesis where only the classifier half holds is a residual risk with a precondition; a hypothesis where both hold is a reachable Tampering defect. Do not collapse the two measurements into one conclusion, because the fix differs completely between them.

Assign a severity to every UNGATED route and to the forgery hypothesis, and state the impact and likelihood you used. Then state the termination state. If it is REFUTED-NO-GAP, say so plainly and do not manufacture a change — Tasks 2 and 3 still run, because the live refusal proof is what the Success Criterion owes regardless of whether a gap was found.

Records evidence for F23-01; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -f .planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cE 'GAP-CLOSED|REFUTED-NO-GAP|ESCALATED' .planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cE 'GATED-BY-|UNGATED' .planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md)" -ge 10</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cE 'crates/[a-z-]+/src/[a-z_/]+\.rs:[0-9]+' .planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md)" -ge 12</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git status --porcelain -- crates/</automated>
  </verify>
  <done>Every route is enumerated with a path-and-line citation and resolved to GATED-BY-&lt;mechanism&gt; or UNGATED. The router seed path and the cron skill sink are each resolved explicitly with citations. The forgery hypothesis is resolved as two separate measurements — classifier behaviour and tool-write reachability — with commands and outputs recorded, and it is not collapsed into a single conclusion. Every UNGATED route and the hypothesis carry a severity with the impact and likelihood behind it. The termination state is stated. No production file was modified in this task.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 2: Build the hostile nonce corpus at every enumerated route, and close any CRITICAL or HIGH gap it exposes</name>
  <files>crates/wcore-skills/tests/generated_execution_boundary.rs, crates/wcore-skills/src/loader.rs, crates/wcore-skills/src/refs.rs, crates/wcore-agent/src/skill_tool.rs</files>
  <read_first>.planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md (the enumerated routes and their dispositions — the corpus covers exactly this set), crates/wcore-skills/tests/wayland_home_auto_skill_loop.rs (the WAYLAND_HOME guard idiom, the serial-attribute discipline that keeps the process-global env var safe, and the existing quarantine cases so nothing is duplicated or displaced), crates/wcore-cli/tests/support/mock_llm.rs (the RecordedRequest and received_requests surface that reads the actual outbound POST body), crates/wcore-skills/src/executor.rs (render_shell_input — the exact function that composes the bytes a shell receives, so the corpus scans the executed bytes rather than a lookalike)</read_first>
  <behavior>
    - Test 1: a generated draft carrying a run-time nonce in its body is resolved through the model-facing path and the nonce is absent from the result the model receives.
    - Test 2: the same draft's shell directive does not run — proved by the absence of the file the directive would have created, not by an assertion about a guard's return value.
    - Test 3: the nonce is absent from the rendered system prompt built from a catalog containing the draft.
    - Test 4: the nonce is absent from the outbound provider request body recorded by the mock provider across a full turn.
    - Test 5: the draft's name is absent from the router's candidate pool and from the router hint path.
    - Test 6: the cron skill sink refuses the draft, and the refusal precedes any observable effect of shell composition.
    - Test 7 (hostile): a draft whose manifest is malformed, absent, or claims the auto-drafted marker false while retaining the released body shape stays quarantined. Three of these shapes already have crate tests; the corpus must not weaken, displace or duplicate them.
    - Test 8 (negative control): a genuinely user-authored skill carrying the same nonce DOES reach the model and DOES execute its directive, so the corpus proves discrimination rather than blanket denial.
    - Every CRITICAL or HIGH gap the census recorded is closed, and the closing change is the smallest one that closes it.
  </behavior>
  <action>Write the corpus first and watch it fail for the right reason before changing any enforcement code. A test that passes on its first run against unmodified code either found no gap — which is the REFUTED-NO-GAP outcome and is fine — or is asserting something weaker than it claims. Distinguish those two before moving on, and record which one it was.

Generate the nonce at run time, per test, from a random source. A hardcoded needle can be matched by a stale artifact from an earlier run, which turns the whole corpus into a tautology. Plant it in three places within one draft: the body prose, a shell directive inside that body whose observable effect is creating a file named after the nonce in the test's tempdir, and the frontmatter description. The description is what the system-prompt listing renders and the body is what execution consumes; they are different sinks and a fix for one does not imply a fix for the other.

Assert ABSENCE from observable sinks, never presence of an internal flag. The nonce-named file must not exist. The recorded outbound provider body must not contain the nonce. The tool result the model receives must not contain it and must not disclose the draft body — the packaged driver gate already holds that line at its hidden-skill probe, and this corpus holds the same one. The rendered system prompt must not contain it.

Pin the home directory environment variable to a tempdir and mark every test that does so with the same serial attribute used in the existing auto-skill loop suite. The variable is process-global; a corpus that races other suites on it produces a confidently wrong result in either direction.

Include the negative control and give it equal weight. A user-authored skill carrying the same nonce must reach the model and must execute its directive. Without it the corpus cannot distinguish "generated content is quarantined" from "skills are broken", and a later refactor that breaks every skill would leave this corpus green.

Then close gaps — only the ones the census recorded at CRITICAL or HIGH, and only inside the four declared source files. Make the smallest change that closes the route and justify it in-source: state which route it closes and what the pre-existing behaviour was. Do not widen a filter that already works, do not refactor the loader or the catalog while you are inside them, and do not modify, rename, re-gate or delete any existing test. If a gap's fix reaches outside those four files, that is termination state ESCALATED: record the blast radius and stop. If the census concluded REFUTED-NO-GAP, change nothing here and let the corpus stand as the proof.

Log every MEDIUM and below finding to `.planning/BACKLOG.md` and move on; they do not block.

Records evidence for F23-01; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -f crates/wcore-skills/tests/generated_execution_boundary.rs</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'wayland_home_env' crates/wcore-skills/tests/generated_execution_boundary.rs)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'is_generated_draft' crates/wcore-skills/src/loader.rs)" -ge 1 &amp;&amp; test "$(/usr/bin/grep -cF 'resolve_for_model' crates/wcore-skills/src/refs.rs)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git diff --stat -- crates/wcore-skills/src crates/wcore-agent/src</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; git rev-parse HEAD &amp;&amp; cargo nextest run --profile ci -p wcore-skills --test generated_execution_boundary --no-fail-fast" &gt; /tmp/f23a-01-corpus-linux.log 2&gt;&amp;1; rc=$?; tail -60 /tmp/f23a-01-corpus-linux.log; exit $rc</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git rev-parse HEAD &amp;&amp; cargo nextest run --profile ci -p wcore-skills -p wcore-agent --no-fail-fast" &gt; /tmp/f23a-01-regress-linux.log 2&gt;&amp;1; rc=$?; tail -40 /tmp/f23a-01-regress-linux.log; exit $rc</automated>
  </verify>
  <done>The corpus exists, covers every route the census enumerated, and each case asserts absence from an observable sink rather than the value of an internal flag. The nonce is generated at run time. Every home-directory-pinning test carries the serial attribute. The negative control passes, so the corpus proves discrimination and not blanket denial. Any CRITICAL or HIGH gap is closed by the smallest change inside the four declared files, with its route named in-source; MEDIUM and below are in BACKLOG. No existing test was modified, renamed, re-gated or deleted. The corpus and the two crate suites are green on Hetzner at the pinned SHA, with the gate's exit status taken from ssh and not from a pipeline.</done>
</task>

<task type="auto">
  <name>Task 3: Prove the refusal through the shipped binary on Linux and Windows with a negative-controlled, SHA-asserting driver</name>
  <files>scripts/f23a-boundary-drive.sh, scripts/f23a-boundary-drive.ps1, crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs, .planning/phases/23A-governed-skills/23A-01-LIVE-EVIDENCE.md</files>
  <read_first>crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs (the packaged lifecycle memory matrix near 781-926: how it builds the real binary, seeds a fixture provider, drives slash turns and a Skill tool call, and asserts the hidden-skill probe returns not-found without disclosing the body — this is the existing live mechanism and it is extended, not replaced), justfile (the f01-packaged-driver-gate recipes near 163-185: the clean-tree requirement, the externally pinned source SHA, the binary discovery variable, and the separate Windows recipe), crates/wcore-cli/tests/skills_lifecycle_cmd.rs (how an integration test invokes the real binary against a fixture project with an isolated memory root), crates/wcore-cli/src/main.rs (the skills flags near 447-473 and their dispatch near 1400-1422 — the exact flag spellings the driver must use)</read_first>
  <behavior>
    - A generated draft is produced by the product's own drafting path, not hand-written, so the live proof exercises the same artifact shape a real session produces.
    - The shipped binary is asked to run that draft and refuses, and the refusal is observable in captured output.
    - The binary is asked to list and show the draft, and the draft is reported as present but not model-invocable — so the operator can still see and inspect what is quarantined.
    - The `Skill` tool call for the draft returns not-found to the model and does not disclose the draft body.
    - The negative control runs in the same driver: a user-authored skill is invoked and DOES succeed. A run where everything is refused exits nonzero, because that outcome is indistinguishable from a broken build.
    - The driver asserts its own checkout SHA against an externally supplied expected SHA before doing anything, and exits with a distinct nonzero code when they differ.
    - Both platform drivers exit nonzero on any deviation, and the gate that invokes them takes its status from the remote process rather than from a pipeline.
  </behavior>
  <action>Extend the existing packaged lifecycle matrix rather than inventing a parallel harness. That matrix already builds the real binary, pins it to a clean source SHA, seeds a fixture provider, drives slash turns and a tool call, and asserts the generated draft is quarantined. Add the refusal probes the census identified as reachable from the product surface, and add the negative control alongside them so the matrix can distinguish quarantine from breakage. Do not weaken any assertion already there.

Then write the two drivers. Each one takes the expected source SHA from the environment and asserts the checkout matches it as its very first action, exiting with a distinct nonzero code when it does not — this is what prevents a gate from passing against stale code, which is a failure mode that costs more than any test it protects. Each driver then builds the binary, provokes the product's own drafting path to produce a draft, and drives the sequence: attempt to run the draft, observe the refusal, list and show it, and invoke the Skill tool for it. Each step records the exact invocation and the captured output. The driver exits nonzero if any refusal does not happen, and it also exits nonzero if the negative control fails, so a build where skills are simply broken cannot masquerade as a build where quarantine works.

Use the trap-safe environment assignment form on Windows and prove the value took effect before trusting anything downstream of it — the trailing-space form has already produced one confidently wrong conclusion in this repository. Invoke the PowerShell driver with the file form so its own exit status is what the gate reads, and never read an exit code from a block that also emits output.

Run both drivers at one pinned SHA. Record in `23A-01-LIVE-EVIDENCE.md`, per platform and per route: the exact invocation, the observed output, the exit code, and the negative control's result. Note explicitly that macOS is not covered by this plan's live legs, because this Mac may not run Cargo and the macOS runner is an ephemeral, Sean-gated dispatch; 23A-04 owns the macOS disposition.

Closes the "cannot execute before governed promotion" half of Success Criterion 1 for Linux and Windows. Records evidence for F23-01; marks no requirement complete — closure is claimed by 23A-04.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -x scripts/f23a-boundary-drive.sh &amp;&amp; test -f scripts/f23a-boundary-drive.ps1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'WAYLAND_EXPECT_SHA' scripts/f23a-boundary-drive.sh)" -ge 1 &amp;&amp; test "$(/usr/bin/grep -cF 'WAYLAND_EXPECT_SHA' scripts/f23a-boundary-drive.ps1)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -ciF 'negative control' scripts/f23a-boundary-drive.sh)" -ge 1 &amp;&amp; test "$(/usr/bin/grep -ciF 'negative control' scripts/f23a-boundary-drive.ps1)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; bash -n scripts/f23a-boundary-drive.sh</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; WAYLAND_EXPECT_SHA=$SHA bash scripts/f23a-boundary-drive.sh" &gt; /tmp/f23a-01-drive-linux.log 2&gt;&amp;1; rc=$?; tail -80 /tmp/f23a-01-drive-linux.log; exit $rc</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes SeanD@seandesktop "cmd /c \"cd /d C:\ferrox-win &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA\"; \$env:WAYLAND_EXPECT_SHA='$SHA'; powershell -NoProfile -File C:\ferrox-win\scripts\f23a-boundary-drive.ps1; exit \$LASTEXITCODE" &gt; /tmp/f23a-01-drive-win.log 2&gt;&amp;1; rc=$?; /usr/bin/grep -v CLIXML /tmp/f23a-01-drive-win.log | tail -80; exit $rc</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git rev-parse HEAD &amp;&amp; WAYLAND_BUILD_SOURCE_SHA=$SHA cargo build --locked -p wcore-cli --bin wayland-core &amp;&amp; WAYLAND_BUILD_SOURCE_SHA=$SHA WCORE_EVAL_BIN=/root/wayland/target/debug/wayland-core cargo test --locked -p wcore-eval-scenarios --features packaged-driver-gate --test packaged_driver_gate" &gt; /tmp/f23a-01-packaged-linux.log 2&gt;&amp;1; rc=$?; tail -60 /tmp/f23a-01-packaged-linux.log; exit $rc</automated>
  </verify>
  <done>The packaged lifecycle matrix carries the added refusal probes and the negative control, with no pre-existing assertion weakened. Both drivers exist, assert their checkout SHA first with a distinct nonzero code on mismatch, and exit nonzero when a refusal does not happen or when the negative control fails. Both ran at one pinned SHA — Linux on Hetzner and Windows on SEANDESKTOP — and every gate took its status from the remote process rather than from a pipeline. `23A-01-LIVE-EVIDENCE.md` records, per platform and per route, the exact invocation, the observed output, the exit code and the negative control result, and states plainly that macOS is not covered here and is 23A-04's disposition.</done>
</task>

</tasks>

## What this plan does NOT change (scope fence)

- **Promotion.** `run_skills_promote` in `wcore-cli/src/main.rs` still fails closed with its containment diagnostic. 23A-02 owns replacing it with a governed transaction. Do not soften it here, and do not modify `crates/wcore-cli/tests/skills_lifecycle_cmd.rs`.
- **Observe, revoke and rollback.** 23A-03 owns them. The `/skill list` and `/skill show` output is read here as evidence and is not extended here.
- **The journey driver and the phase disposition.** 23A-04 owns them, including the macOS decision.
- **Phase 23B's entire surface.** Operator session lifecycle, memory and user-model controls, cache and compaction economics, the repository index, and the multi-day journey are planned under `.planning/phases/23B-continuous-agency/` and are not touched, referenced as dependencies, or duplicated.
- **The drafting and detection path.** `PatternDetector`, `DraftWriter` and `SkillDrafter` are read as sources of the artifact shape; their behaviour is not changed.
- **The tool permission model and `wcore-config`.** If the forgery hypothesis's fix reaches there, that is ESCALATED, not in-scope.
- **No test is deleted, weakened, re-gated, ignored or allow-attributed.** If the honest outcome is REFUTED-NO-GAP, the plan closes with that finding and no enforcement change.

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| model-visible context ← generated skill content | Anything generated that reaches the system prompt, the catalog listing or a tool result has crossed into the model's influence surface |
| spawned process ← generated skill body | A shell directive inside a generated body is attacker-influenced text that becomes an argv or a shell string when composed |
| quarantine verdict ← files inside the quarantined directory | The classifier reads the manifest and the body from the same directory it is judging, so whoever can write there can influence the verdict |
| operator inspection ← model invocation | A quarantined draft must remain visible to a human and invisible to the model; collapsing the two either blinds the operator or unquarantines the content |

## STRIDE Threat Register

| Threat ID | Category | Component | Severity | Disposition | Mitigation Plan |
|-----------|----------|-----------|----------|-------------|-----------------|
| T-23A-01-01 | Elevation of Privilege | An enumerated route reaches skill execution without the model-facing resolver, so generated content runs before any governed promotion exists | critical | mitigate | Task 1 enumerates every route with a path-and-line citation and resolves each to a named gate or UNGATED; Task 2 drives a run-time nonce at each and asserts absence from the shell's observable effect; a CRITICAL or HIGH gap is closed inside this plan or escalated |
| T-23A-01-02 | Tampering | The quarantine verdict is computed from a manifest and a body that both live inside the quarantined directory, so writing those two files can un-quarantine generated content | high | mitigate | Task 1 resolves the hypothesis as two independent measurements — classifier behaviour and tool-write reachability — and assigns severity from the pair; a reachable defect is closed or escalated, an unreachable one is recorded as a residual risk with its precondition |
| T-23A-01-03 | Information Disclosure | A refusal discloses the quarantined body in its error text, handing the model the content the refusal exists to withhold | high | mitigate | The corpus asserts the tool result contains neither the nonce nor the body, mirroring the discipline the packaged driver gate already holds at its hidden-skill probe |
| T-23A-01-04 | Spoofing | A gate that cannot fail manufactures confidence: a piped ssh status, an exit code read from an output-emitting block, or a run against a stale checkout | high | mitigate | Both shapes are banned by name in the execution rules; every remote gate redirects to a file and exits with the captured status; both drivers assert their checkout SHA first with a distinct nonzero code on mismatch |
| T-23A-01-05 | Repudiation | The corpus passes because skills are broken rather than because quarantine works, and nothing distinguishes the two | high | mitigate | A user-authored negative control must reach the model AND execute, in both the corpus and both live drivers; a run where everything is refused exits nonzero |
| T-23A-01-06 | Tampering | A hardcoded needle matches a stale artifact from an earlier run, making an absence assertion vacuously true | medium | mitigate | The nonce is generated at run time per test and per driver invocation, and it is planted in three distinct sinks |
| T-23A-01-07 | Denial of Service | Scope metastasis: the census leads into the tool permission model, the promotion transaction, or Phase 23B's surface, and the plan follows it | medium | mitigate | The termination criterion caps this plan at one census cycle with three defined exit states; the scope fence names every adjacent surface and requires recording the connection and stopping |
| T-23A-01-08 | Repudiation | The corpus races another suite on the process-global home-directory environment variable and produces a confidently wrong result in either direction | medium | mitigate | Every home-pinning test carries the same serial attribute the existing auto-skill loop suite uses, and the guard restores the prior value on drop |
| T-23A-01-SC | Tampering | npm/pip/cargo installs | low | accept | No dependency is added, removed or updated; no `Cargo.toml` or `Cargo.lock` change; no install task exists in this plan |
</threat_model>

<verification>
Local gates (Mac, source level only — Cargo is never run here): `cargo fmt --all -- --check` clean; the census exists, states a termination state, resolves at least ten routes to a named gate or UNGATED, and carries at least twelve path-and-line citations; the corpus file exists and pins the home directory under the serial attribute; both drivers exist, reference the expected-SHA variable, name a negative control, and the shell driver parses under `bash -n`; the diff over the skills and agent crates is surgical.

Authoritative gates (real hardware, status taken from the remote process and never from a pipeline): on Hetzner at the pinned SHA, the boundary corpus passes and the `wcore-skills` and `wcore-agent` suites show no regression; the Linux driver runs green including its negative control; the packaged driver gate passes with the real binary built from the pinned source SHA. On SEANDESKTOP at the same SHA, the Windows driver runs green including its negative control, invoked through the PowerShell file form so its own exit status is the gate.

Known unknowns to record rather than resolve here: whether a skill-declared MCP server or a skill-declared hook can be registered from a quarantined draft on a path this census did not reach; whether cross-project resolution can surface a sibling project's generated draft under a configuration this plan did not exercise; and whether macOS behaves identically, which this plan does not measure and 23A-04 dispositions.
</verification>

<success_criteria>
- Every route from generated skill content to execution or model-visible context is enumerated with a path-and-line citation and resolved to a named gate or UNGATED, with the router seed path and the cron skill sink each resolved explicitly.
- The forgery hypothesis is resolved as two independent measurements and assigned a severity from the pair, never collapsed into one conclusion.
- A hostile corpus plants a run-time nonce in three sinks and proves absence from the shell's observable effect, the recorded outbound provider body, the tool result and the system prompt.
- The corpus carries a user-authored negative control that reaches the model and executes, so quarantine is proved to discriminate rather than to deny everything.
- Every CRITICAL or HIGH gap is closed by the smallest change inside the four declared source files with its route named in-source, or explicitly disproved, or escalated with its blast radius; MEDIUM and below are in BACKLOG and did not block.
- The shipped `wayland-core` binary is driven to refuse an unpromoted generated draft on Linux and on Windows, with the exact invocation, observed output and exit code recorded per platform.
- Both drivers assert their checkout SHA before acting and exit nonzero when a refusal does not happen or when the negative control fails.
- No gate in this plan derives its status from a pipeline, from an exit code read out of an output-emitting block, or from grepping an evidence file this plan wrote.
- No existing test was modified, renamed, re-gated or deleted; if the honest outcome was REFUTED-NO-GAP, the plan closed with that finding and made no enforcement change.
</success_criteria>

## Artifacts this plan produces
- `.planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md` — the enumerated routes, their dispositions, the forgery measurements and the termination state.
- `crates/wcore-skills/tests/generated_execution_boundary.rs` — the hostile nonce corpus with its negative control.
- `crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs` — the refusal probes added to the existing packaged live matrix.
- `scripts/f23a-boundary-drive.sh` and `scripts/f23a-boundary-drive.ps1` — the SHA-asserting, negative-controlled live drivers.
- `.planning/phases/23A-governed-skills/23A-01-LIVE-EVIDENCE.md` — the recorded live outcome per platform per route.
- `23A-01-SUMMARY.md`.

<output>
Create `.planning/phases/23A-governed-skills/23A-01-SUMMARY.md` using the standard GSD summary template. Record: every enumerated route with its disposition and citation; the two forgery measurements with their commands and outputs and the severity derived from the pair; the corpus cases and which sink each one asserts absence from; whether the corpus failed before any enforcement change and for what reason; every CRITICAL and HIGH finding with its fix or its disproof, and every MEDIUM and below with its BACKLOG entry; the exact live invocations per platform with observed output, exit codes and the negative control results; the Hetzner suite counts as a delta against the pre-change run with every residual failure named; the explicit statement that macOS is uncovered here and is 23A-04's disposition; the recorded unknowns; and which of the three termination states the plan ended in. Mark no requirement complete — F23-01 closure is claimed by 23A-04.
</output>
