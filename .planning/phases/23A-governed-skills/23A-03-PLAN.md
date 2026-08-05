---
phase: 23A-governed-skills
plan: "03"
type: execute
wave: 3
depends_on:
  - "23A-02"
files_modified:
  - crates/wcore-skills/src/governance.rs
  - crates/wcore-skills/src/refs.rs
  - crates/wcore-skills/src/watcher.rs
  - crates/wcore-agent/src/slash/skill.rs
  - crates/wcore-agent/src/engine.rs
  - crates/wcore-cli/src/skills_cmd.rs
  - crates/wcore-skills/tests/governed_revocation.rs
  - crates/wcore-eval-scenarios/src/governed_skill_drive.rs
  - crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs
  - crates/wcore-eval-scenarios/tests/f23a_revocation_drive.rs
  - scripts/f23a-revocation-drive.sh
  - scripts/f23a-revocation-drive.ps1
  - .planning/phases/23A-governed-skills/23A-03-BASE-SHA
  - .planning/phases/23A-governed-skills/23A-03-LIVE-EVIDENCE.md
autonomous: true
requirements:
  - F23-01
domain: code
must_haves:
  truths:
    - "REVOCATION IS THE HARD ONE, AND THE REASON IS STRUCTURAL, NOT INCIDENTAL. `SkillCatalog` holds `refs: Vec<SkillRef>` where `disable_model_invocation` is a VALUE BAKED IN AT LOAD TIME by the loader (`loader.rs:443-449`), plus an LRU of already-resolved `SkillMetadata` and an eager map (`refs.rs:150-215`). A revocation recorded in the governance ledger is a DATABASE write. The catalog does not consult the ledger again after it is built, and `SkillWatcher` (`watcher.rs`) broadcasts its version counter on FILESYSTEM events only — a database write fires nothing. So a revocation taken at the CLI while a session is running does not reach that session at all until the catalog is rebuilt. A revoke that only takes effect after restart is not revocation; it is a note to a future process. This plan must either propagate revocation into the live catalog or state plainly, with evidence, that it does not and record the exposure window as an OPEN clause. It must not quietly ship the restart-only version and call the criterion closed."
    - "REVOKE AND ROLLBACK ARE DIFFERENT OPERATIONS AND CONFLATING THEM LOSES THE HISTORY. Revoke stops a promoted skill from executing going forward and is a forward-only act: the promotion happened, it is on the record, and it has ended. Rollback returns the system to the state before the promotion — the artifact quarantined, the procedure row back at its pre-promotion status, the promotion no longer authorising anything — while the ledger STILL shows that the promotion and the rollback both happened, because the ledger is append-only by construction from 23A-02. A rollback that erases the promotion entry destroys exactly the evidence an operator needs to understand what ran on their machine and when. Success Criterion 1 names observe, revoke AND roll back as three separate capabilities; deliver three."
    - "OBSERVE MEANS AN OPERATOR CAN SEE WHAT RAN, WHY IT WAS ALLOWED TO, AND WHO ALLOWED IT — NOT THAT A FLAG IS QUERYABLE. The product already surfaces a shallow version: `/skill list` tags entries `(hidden)` and prints a visible-versus-hidden summary (`slash/skill.rs:140-170`), `/skill show` prints `visibility: hidden from model` (`:172-204`), and the packaged driver gate already asserts both of those exact strings (`packaged_driver_gate.rs:902-908`), so they are load-bearing and must not be broken. What is missing is the governance dimension: for a promoted artifact, which hash is bound, which review authorised it, what the evaluation found, when it happened, and the full append-only history including any revocation and rollback. Extend those surfaces; do not replace them and do not change the strings the existing gate asserts."
    - "A REVOCATION THAT DOES NOT REACH EVERY SURFACE 23A-01 ENUMERATED IS NOT A REVOCATION. 23A-01 produced a census of every route from generated content to execution or model-visible context and proved each one refuses unpromoted content. After a revoke, the identical set of routes must refuse the identical artifact again. The proof is the same corpus and the same live driver re-run against the revoked artifact, not a new narrower test that happens to pass. If the boundary driver from 23A-01 does not pass against a revoked artifact, the revocation is incomplete regardless of what the ledger says."
    - "ROLLBACK MUST LEAVE NO RESIDUE, AND RESIDUE IS PROVED BY COMPARING BYTES AND ROWS, NOT BY ASSERTING CLEANLINESS. After rollback the on-disk artifact, the procedure row's status, and the catalog's verdict must all equal what they were before the promotion. Capture that state before promoting, capture it again after rolling back, and compare — the artifact bytes, the procedure status, and the loader's classification. Anything that differs is residue and is a finding, whether or not it looks harmful."
    - "THE EXISTING TRANSITION TABLE ALREADY PERMITS WHAT REVOCATION NEEDS, SO DO NOT REWRITE IT. `ProcedureStatus::can_transition_to` (`wcore-memory/src/v2_types.rs:376-390`) permits Active to Archived and Pinned to Active or Archived. Revoking a promoted procedure is Active to Archived, which is already legal, and `--skills-archive` already performs it and prints a stable confirmation. Reuse that machinery through the 23A-02 kernel rather than adding a parallel status path; two ways to leave the Active state will drift and one of them will forget the ledger."
    - "THE LIVE DRIVER'S MECHANISM IS 23A-01'S, AND RE-PROVING THE BOUNDARY MEANS CALLING ITS ASSERTION SET IN-PROCESS, NOT SHELLING OUT TO ITS WRAPPER. 23A-01 established that nothing in `scripts/` can drive `wayland-core` against a scripted provider — `scripts/wayland-e2e-real-workload.sh` reaches only `--version` (line 327) and `--help` (line 329) — and that the working harness is Rust-side: `OpenAiFixtureScript` (`crates/wcore-eval-scenarios/src/fixtures/openai.rs:34`) behind `run_with_binary_in_paths` (`crates/wcore-eval-scenarios/src/runner.rs:462`) spawning the real binary as `--json-stream --provider <id> --model <model> --base-url <fixture>` (`:340` and `:346`), with the draft produced by the PRODUCT's own drafting path (`packaged_driver_gate.rs:691`, `:732`, `:749`). 23A-01 extracted that into `wcore_eval_scenarios::governed_skill_drive`, INCLUDING a public route-refusal assertion set. This plan's drive target `crates/wcore-eval-scenarios/tests/f23a_revocation_drive.rs` CALLS that assertion set against the revoked artifact, in the same process and against the same seeded environment. It must NOT invoke `scripts/f23a-boundary-drive.sh`: that wrapper rebuilds the binary and drafts a fresh artifact, which destroys the revoked state the re-proof exists to test and would turn the strongest claim in this plan into a green that means nothing."
    - "A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN, AND A GATE THAT EXITS 0 UNCONDITIONALLY IS AN ENGINEERED GREEN WEARING A DIAGNOSTIC'S CLOTHES. Never weaken an assertion, add an ignore or allow attribute, raise a timeout, re-gate, or delete an inconvenient test to reach a gate. Never leave `git status --porcelain`, `git diff --stat` or `git diff --name-only` as a bare gate: all three exit 0 whether or not anything changed, and the working-tree leg alone goes permanently green the moment the executor commits — so every untouched-surface claim here is `test -z \"$(...)\"` over a working-tree leg AND a base-SHA diff leg. If live revocation cannot be made to reach a running session inside this plan's declared files, that is an OPEN clause with its measured exposure window — not a redefinition of revocation. Findings at CRITICAL or HIGH must be fixed or disproved; MEDIUM and below go to `.planning/BACKLOG.md` and DO NOT BLOCK."
  artifacts:
    - path: crates/wcore-skills/src/governance.rs
      provides: "The revoke and rollback transactions appended to the existing ledger, the pre-promotion state capture that rollback restores, and the live-propagation signal that tells a running catalog its governance state changed"
    - path: crates/wcore-skills/src/refs.rs
      provides: "The catalog's response to a governance change: invalidating the resolved-metadata cache and re-deriving the model-invocation verdict for the affected artifact, rather than serving a value baked in at load time"
    - path: crates/wcore-agent/src/slash/skill.rs
      provides: "The observe surface extended with the governance dimension — bound hash, authorising review, evaluation outcome and append-only history — without changing the existing visibility strings the packaged gate asserts"
    - path: crates/wcore-cli/src/skills_cmd.rs
      provides: "The list, show, revoke and rollback verbs with stable stdout tokens and distinct exit codes, reaching the same kernel transactions"
    - path: crates/wcore-skills/tests/governed_revocation.rs
      provides: "Coverage of live-session propagation, the revoke-versus-rollback distinction, the append-only history surviving both, and the byte-and-row comparison proving rollback leaves no residue"
    - path: crates/wcore-eval-scenarios/tests/f23a_revocation_drive.rs
      provides: "The Rust live-drive target the shell wrappers execute, built on 23A-01's `governed_skill_drive` module: observe, revoke, then re-run 23A-01's PUBLIC route-refusal assertion set against the revoked artifact in the same process and the same seeded environment, then roll back and compare against the run's opening capture"
    - path: scripts/f23a-revocation-drive.sh
      provides: "The Linux live wrapper: SHA-asserting (exit 3 on mismatch), build-and-pin as justfile:165-181 does, then exit with the status of `cargo test ... --test f23a_revocation_drive` — no assertion logic of its own"
    - path: .planning/phases/23A-governed-skills/23A-03-LIVE-EVIDENCE.md
      provides: "The recorded live outcome per platform including the measured live-propagation latency or the recorded exposure window if propagation is not achieved"
  key_links:
    - from: crates/wcore-skills/src/governance.rs
      to: crates/wcore-skills/src/refs.rs
      via: "the governance-change signal invalidating the catalog's cached verdict so revocation reaches a session that is already running"
      pattern: "revocation-propagation"
    - from: crates/wcore-cli/src/skills_cmd.rs
      to: crates/wcore-skills/src/governance.rs
      via: "the revoke and rollback verbs reaching the same append-only ledger the promote transaction writes"
      pattern: "cli-to-engine"
    - from: crates/wcore-eval-scenarios/tests/f23a_revocation_drive.rs
      to: crates/wcore-eval-scenarios/src/governed_skill_drive.rs
      via: "calling 23A-01's public route-refusal assertion set against the revoked artifact in the same process and the same seeded environment, so revocation is proved at exactly the routes the census enumerated without a rebuild or a re-draft destroying the revoked state"
      pattern: "boundary-reproof"
---

<objective>
Make the second half of Success Criterion 1 true: a promoted generated skill can be observed with its full governance provenance, revoked so that it stops executing — including in a session that is already running — and rolled back to exactly its pre-promotion state, with the append-only history of all of it still readable; and prove each of the three through the shipped `wayland-core` binary on Linux and Windows.

Purpose: 23A-02 made promotion possible. A promotion that cannot be undone is a one-way door, and F23-01 explicitly requires observe, revoke and rollback as distinct capabilities. The hardest of the three is structural: the catalog bakes the model-invocation verdict in at load time and the skill watcher only fires on filesystem events, so a ledger write does not reach a live session by any existing path. That is the thing this plan must either fix or measure and declare.
Output: Observe extended with the governance dimension; revoke and rollback as distinct append-only transactions; live propagation into a running catalog or a measured, recorded exposure window; a byte-and-row proof that rollback leaves no residue; and 23A-01's boundary driver re-run green against the revoked artifact on both platforms.
</objective>

<execution_context>
@$HOME/.codex/gsd-core/workflows/execute-plan.md
@$HOME/.codex/gsd-core/templates/summary.md
</execution_context>

<context>
@AGENTS.md
@.planning/HANDOFF-2026-07-26-phase20-20A-complete.md
@.planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md
@.planning/phases/23A-governed-skills/23A-02-LIVE-EVIDENCE.md
@crates/wcore-skills/src/refs.rs
@crates/wcore-skills/src/watcher.rs
@crates/wcore-skills/src/loader.rs
@crates/wcore-agent/src/slash/skill.rs
@crates/wcore-memory/src/v2_types.rs
@crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs
</context>

<execution_rules>

**THE TWO AMENDED PHASE RULES — verbatim, and they bound this plan.**
- Findings at CRITICAL or HIGH must be fixed or disproved. MEDIUM and below are logged to `.planning/BACKLOG.md` and DO NOT BLOCK execution.
- Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean.

**DEPENDENCY.** This plan begins from 23A-02's ledger, which is append-only by construction specifically so revoke and rollback need no further migration. If a seventh migration turns out to be required, that is a 23A-02 design miss: record it as a finding with its reason before adding one, and add it additively.

**TERMINATION CRITERION (hard).** This plan ends in exactly one of three states and writes its SUMMARY in all three:
1. **COMPLETE** — observe carries the governance dimension, revoke and rollback are distinct append-only transactions, revocation reaches a running session, rollback leaves no residue proved by comparison, and both live drivers pass on both platforms including the re-run boundary driver.
2. **PARTIAL-WITH-OPEN-CLAUSE** — live propagation into a running session cannot be achieved inside this plan's declared files. Record it as an OPEN clause WITH THE MEASURED EXPOSURE WINDOW — how long a revoked artifact remains executable in an already-running session, measured, not estimated — and ship the rest. Do not redefine revocation to mean "next boot" in order to close the criterion.
3. **ESCALATED** — the propagation fix reaches outside the declared files, for example into the engine's catalog ownership or the session lifecycle. Record the blast radius and stop.
Under no circumstances does this plan create additional plans or extend its own task list.

**SCOPE BOUNDARY (hard).** This plan builds observe, revoke and rollback. It does NOT revisit promotion, the policy or the ledger schema — 23A-02 owns them. It does NOT take the phase disposition or run the end-to-end journey — 23A-04 owns those. It does not touch Phase 23B's surface: operator session lifecycle, memory and user-model controls, cache and compaction economics, the repository index and the multi-day journey are planned under `.planning/phases/23B-continuous-agency/` and are not duplicated, referenced as dependencies, or contradicted. In particular, session checkpoint and rewind are 23B-01's verbs over SESSIONS; skill rollback here is over a governance record and they are not the same mechanism.

**FOUR-PLAN CAP.** This phase has exactly 4 plans. Do not propose a fifth.

**DO NOT BREAK THE STRINGS THE PACKAGED GATE ASSERTS.** `packaged_driver_gate.rs` asserts on the `(hidden)` tag and on the `visibility: hidden from model` line produced by the slash surface. Those are contract text for an existing live gate. Extend the output around them; do not reword them.

**ENVIRONMENT.**
- Repository: `/Users/seandonahoe/dev/waylandcore-ferrox`, branch `plan/f20-unified-audit-repair`. NEVER touch `/Users/seandonahoe/dev/waylandcore`.
- NEVER run Cargo on this Mac. `cargo fmt --all -- --check` is the only cargo command used locally.
- Linux authority: `ssh -o BatchMode=yes hetzner-dsm`, `/root/wayland`.
- Windows: `ssh -o BatchMode=yes SeanD@seandesktop`, checkout `C:\ferrox-win`, PowerShell default shell. `cargo fmt --all` FAILS there with os error 206. Windows CI runs clippy `-D warnings` BEFORE tests, so a lint failure means tests never execute.
- The PTY harness in `crates/wcore-cli/tests/support/pty.rs` is UNIX-ONLY by construction — ConPTY cannot surface stdout in headless runners, and the module says so. The live-session leg of this plan therefore runs its interactive form on Linux and its non-interactive form on both platforms. Do not claim a Windows PTY result that cannot be produced.
- Both hosts' fetch refspecs are pinned to an unrelated branch. ALWAYS `git fetch origin plan/f20-unified-audit-repair`.
- Mac `grep` is rtk-proxied and SILENTLY DROPS LINES. ALWAYS `/usr/bin/grep`, `-F` for literals. Use `/usr/bin/git` on the Mac.
- In `cmd`, `set VAR=x && ...` appends a TRAILING SPACE. Use `set "VAR=x"` or `$env:VAR='x'` and PROVE it took effect.
- Push the work branch to `gh`. NO push to main, merge, PR, tag, release, deployment or issue closure — Sean-only.
- No git write commands in this repository beyond the executor's commit discipline.

**THE SELF-PASSING GATE BAN (hard).**
- `ssh host 'cmd' | grep -v CLIXML` is FORBIDDEN as a gate; the pipeline's status is grep's. Filter for READING only.
- Reading an exit code from a block that also emits output is FORBIDDEN; read it on the line AFTER the pipeline.
- `git status --porcelain`, `git diff --stat` and `git diff --name-only` are FORBIDDEN as bare gates: all three EXIT 0 UNCONDITIONALLY. Wrap them in `test -z "$(...)"`, and pair a working-tree leg with a base-SHA diff leg so neither an untracked file nor an already-committed change can hide.
- Every remote gate redirects to a file, captures the status on the next line, and exits with that status.
- Do NOT close any behaviour by grepping an evidence file this plan wrote, and do not treat `grep -c '<word>' <file-this-plan-authored>` as a verification — it counts typing, not behaviour.

**THE BASE SHA (hard, and it is this plan's FIRST action).** Before reading or writing anything else, record the untouched starting commit: `/usr/bin/git rev-parse HEAD > .planning/phases/23A-governed-skills/23A-03-BASE-SHA`. Every untouched-surface gate reads it. Never rewrite it later — a base SHA that advances with the work turns every blast-radius gate green by construction.

**DRIVER SELF-TESTS (hard).** Both wrappers honour two externally-triggered self-tests, implemented once in the Rust drive target so both platforms inherit them. `WAYLAND_EXPECT_SHA` set to a SHA the checkout does not match MUST exit exactly 3 — a pinned code, because "distinct nonzero" is not gateable. `WAYLAND_F23A_SELFTEST=revoked-still-runs` MUST make the drive target skip the revocation it is about to assert on — leaving the artifact promoted — and then run the route-refusal assertion set anyway, so the assertions are handed content that legitimately DOES execute; the target must then fail, print `F23A-SELFTEST-TRIPPED: revoked-still-runs`, and exit nonzero. A driver that stays green under that injection has a revocation re-proof that never actually fires, which is the single most load-bearing claim in this plan.

**AGENTS.md discipline.** Surgical diffs. Clippy-clean at `-D warnings`. `thiserror` for public errors, `anyhow` internally, no `unwrap()` in production code. Do not refactor the catalog while adding invalidation to it. Stage exact paths, never `-A`, never `.`. No `Co-Authored-By` trailers.
</execution_rules>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: Revoke and rollback as distinct append-only transactions, with revocation reaching a session that is already running</name>
  <files>crates/wcore-skills/src/governance.rs, crates/wcore-skills/src/refs.rs, crates/wcore-skills/src/watcher.rs, crates/wcore-agent/src/engine.rs, crates/wcore-skills/tests/governed_revocation.rs</files>
  <read_first>crates/wcore-skills/src/refs.rs (the catalog's three storage layers — the refs vector carrying the load-time model-invocation verdict, the async LRU of resolved metadata, and the eager map — and every method that reads them: find, find_metadata_sync, resolve, resolve_for_model and visible), crates/wcore-skills/src/watcher.rs (the debounced filesystem watcher and its version counter, to establish precisely what it does and does not fire on), crates/wcore-skills/src/loader.rs (the governance-aware entry points 23A-02 added and the oracle they consult), crates/wcore-skills/src/governance.rs (the ledger, the promote transaction and the state it captures), crates/wcore-memory/src/v2_types.rs (the transition table: Active to Archived is already legal and is what revocation uses), crates/wcore-agent/src/engine.rs (where the catalog handle lives on the engine and what already invalidates cached prompt sections, since the system-prompt skills section is cached and event-invalidated)</read_first>
  <behavior>
    - Test 1: revoking a promoted artifact makes the loader classify it as quarantined again on the next load, and the ledger shows promotion followed by revocation in order.
    - Test 2: revocation reaches a catalog that was ALREADY BUILT — the same catalog instance stops treating the artifact as model-invocable without being rebuilt from scratch. If this cannot be achieved, the test instead measures how long the stale verdict persists and that measurement becomes the recorded exposure window.
    - Test 3: revocation does not disturb any other artifact, promoted or otherwise, in the same catalog.
    - Test 4: rollback restores the pre-promotion state exactly — artifact bytes, procedure status and loader classification all equal the values captured before promotion, compared field by field.
    - Test 5: after rollback the ledger still shows the promotion, the revocation if one occurred, and the rollback, in order — rollback appends, it never erases.
    - Test 6: revoke and rollback are distinguishable in the ledger and in what they restore; a revoke alone does not restore the procedure row's pre-promotion status and a rollback does.
    - Test 7: rolling back an artifact that was never promoted, and revoking one that was never promoted, each fail closed with their own distinct reasons and change nothing.
    - Test 8: the system-prompt skills section, which is cached and event-invalidated, no longer lists a revoked artifact once the invalidation the revocation triggers has run.
  </behavior>
  <action>Establish the propagation problem before trying to solve it. Read the catalog's three storage layers and write down which of them carries the model-invocation verdict and when it was computed. The verdict is a value the loader baked into each ref at load time; the LRU holds already-resolved metadata; the watcher fires on filesystem events and a ledger write is not one. Confirm that by reading, and record it — because if the executor assumes the watcher will carry the signal, the revocation will appear to work in a fresh-catalog test and silently fail in every real session.

Then choose the narrowest propagation mechanism that reaches a live catalog from inside the declared files, and prefer reusing what exists to inventing a channel. The catalog already owns a lock-protected cache that can be invalidated. The system prompt already has an event-invalidated skills section, which means an invalidation path for skill state already exists and can be triggered. Wire the governance change into those rather than adding a second notification system beside the watcher.

If no mechanism inside the declared files reaches a running session, do NOT redefine revocation. Measure the exposure instead: how long, in wall time or in turns, a revoked artifact remains executable in a session that was already running when the revocation was recorded. Record that number as the OPEN clause's exposure window. A measured window is actionable; "takes effect on restart" is a redefinition.

Build revoke and rollback as distinct transactions on the existing append-only ledger. Revoke is forward-only: the artifact stops being executable and the ledger gains a revocation entry. Rollback restores the pre-promotion state — which means the promote transaction must have captured that state, so if 23A-02 did not capture enough to restore from, capture it now and say so in the summary. Reuse the existing Active-to-Archived transition through the 23A-02 kernel rather than adding a parallel status path; two ways to leave the Active state will drift and one will forget the ledger.

Prove no-residue by comparison, not by assertion. Capture the artifact bytes, the procedure row status and the loader's classification before promoting. Capture them again after rolling back. Compare field by field and fail on any difference, including differences that look harmless — a timestamp that moved is still residue and is still worth knowing about, even if the disposition is to accept it.

Log every MEDIUM and below finding to `.planning/BACKLOG.md`.

Implements the revoke and rollback stages of F23-01; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -f crates/wcore-skills/tests/governed_revocation.rs</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -ciE 'revoke|revocation' crates/wcore-skills/src/governance.rs)" -ge 1 &amp;&amp; test "$(/usr/bin/grep -ciF 'rollback' crates/wcore-skills/src/governance.rs)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox; F=crates/wcore-skills/tests/governed_revocation.rs; test -f "$F" || { echo "the revocation suite was not written"; exit 1; }; N=$(/usr/bin/grep -cE '^[[:space:]]*#\[(tokio::)?test' "$F"); test "$N" -ge 8 || { echo "the revocation suite carries $N cases against the eight the behaviour enumerates"; exit 1; }; test "$(/usr/bin/grep -cF 'can_transition_to' crates/wcore-memory/src/v2_types.rs)" -ge 1 || { echo "REGRESSION GUARD (not coverage): the existing transition table that revoke reuses instead of adding a parallel status path is gone"; exit 1; }</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox; B=$(cat .planning/phases/23A-governed-skills/23A-03-BASE-SHA); test -n "$B" || { echo "base SHA not recorded"; exit 1; }; OUT=$(/usr/bin/git status --porcelain -- crates/ ':(exclude)crates/wcore-skills/src/governance.rs' ':(exclude)crates/wcore-skills/src/refs.rs' ':(exclude)crates/wcore-skills/src/watcher.rs' ':(exclude)crates/wcore-agent/src/engine.rs' ':(exclude)crates/wcore-skills/tests/governed_revocation.rs'; /usr/bin/git diff --name-only "$B"..HEAD -- crates/ ':(exclude)crates/wcore-skills/src/governance.rs' ':(exclude)crates/wcore-skills/src/refs.rs' ':(exclude)crates/wcore-skills/src/watcher.rs' ':(exclude)crates/wcore-agent/src/engine.rs' ':(exclude)crates/wcore-skills/tests/governed_revocation.rs'); test -z "$OUT" || { printf 'this task touched crate files outside its five declared sources:\n%s\n' "$OUT"; exit 1; }</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; git rev-parse HEAD &amp;&amp; cargo nextest run --profile ci -p wcore-skills --no-fail-fast" &gt; /tmp/f23a-03-kernel-linux.log 2&gt;&amp;1; rc=$?; tail -60 /tmp/f23a-03-kernel-linux.log; exit $rc</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git rev-parse HEAD &amp;&amp; cargo build --locked --workspace --all-features &amp;&amp; cargo nextest run --profile ci --workspace --no-fail-fast" &gt; /tmp/f23a-03-aggregate-linux.log 2&gt;&amp;1; rc=$?; tail -40 /tmp/f23a-03-aggregate-linux.log; exit $rc</automated>
  </verify>
  <done>The propagation problem is established from the code before it is addressed, and the finding is recorded. Revoke and rollback are distinct transactions on the existing append-only ledger, reusing the already-legal Active-to-Archived transition through the 23A-02 kernel rather than a parallel status path. Revocation either reaches an already-built catalog — proved against the same catalog instance, not a fresh one — or the exposure window is MEASURED and recorded as an OPEN clause, never redefined away. Rollback restores artifact bytes, procedure status and loader classification to captured pre-promotion values, compared field by field. The ledger still shows every transition in order after both operations. Revoking or rolling back something never promoted fails closed with distinct reasons. The `wcore-skills` suite and the full workspace aggregate are green on Hetzner at the pinned SHA.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 2: The observe surface — governance provenance and append-only history, without breaking the strings the packaged gate asserts</name>
  <files>crates/wcore-agent/src/slash/skill.rs, crates/wcore-cli/src/skills_cmd.rs, crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs</files>
  <read_first>crates/wcore-agent/src/slash/skill.rs (the runtime list and show renderers at 140-204: the exact `(hidden)` tag, the visible-versus-hidden summary line and the `visibility: hidden from model` line — all three are asserted by an existing live gate and are contract text), crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs (lines 902-908, the assertions that pin those strings, so the extension is proved not to break them), crates/wcore-cli/src/skills_cmd.rs (the verbs and stable tokens 23A-02 established, which this task extends rather than reshapes), crates/wcore-skills/src/governance.rs (the ledger read surface: what an operator can be shown about a promotion, a review, a revocation and a rollback), crates/wcore-cli/src/main.rs (the existing skills audit path near 2378-2403 — the established shape for a machine-readable report beside a human-readable one)</read_first>
  <behavior>
    - Test 1: showing a promoted artifact displays the bound hash, the authorising review with its actor and time, the evaluation outcome, and the promotion time.
    - Test 2: showing a revoked artifact displays the revocation and still displays the promotion that preceded it, in order.
    - Test 3: showing a rolled-back artifact displays the full history — promotion, any revocation, and the rollback — and reports the artifact as quarantined now.
    - Test 4: showing a never-promoted generated draft reports it as quarantined with no governance history, and does not fabricate an empty record that reads like a promotion.
    - Test 5: listing distinguishes quarantined-generated, promoted-generated and user-authored entries, so an operator can see at a glance what the product is allowed to run and why.
    - Test 6: the existing visibility strings are unchanged — the packaged gate's assertions still pass byte-for-byte.
    - Test 7: the machine-readable form of the report contains the same governance facts as the human-readable form, so an operator and a script see the same truth.
  </behavior>
  <action>Extend the two existing renderers rather than replacing them. The `(hidden)` tag, the visible-versus-hidden summary and the visibility line are asserted by a live packaged gate; rewording any of them turns a green gate red for a reason that has nothing to do with governance. Add the governance block around and beneath them.

Show enough that an operator can answer the question that matters after an incident: what ran on this machine, when was it allowed to, who allowed it, and against which exact bytes. That means the bound hash, the authorising review with actor and time, the evaluation outcome, the promotion time, and the ordered history including any revocation and rollback. A record that shows only current state cannot answer that question, which is why the ledger was made append-only in 23A-02.

Do not fabricate empty records. A generated draft that was never promoted has no governance history, and rendering it as an empty promotion record is worse than rendering nothing — it reads like a promotion with missing fields.

Give the report a machine-readable form alongside the human-readable one, following the shape the existing skills audit already uses. Assert that both carry the same governance facts; a script and a human reading different truths from the same product is how an operator ends up trusting the wrong one.

Extend the packaged driver gate with observe probes so the governance surface is proved through the real binary, not only in unit tests. Do not weaken any assertion already there.

Implements the observe stage of F23-01; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox; S=crates/wcore-agent/src/slash/skill.rs; test "$(/usr/bin/grep -ciF 'governance' "$S")" -ge 1 || { echo "the list and show renderers carry no governance provenance, so an operator cannot answer what was allowed to run, when, and against which bytes"; exit 1; }; for T in 'visibility: ' 'hidden from model' '(hidden)'; do test "$(/usr/bin/grep -cF "$T" "$S")" -ge 1 || { echo "REGRESSION GUARD (not coverage): the contract string [$T] that the live packaged gate pins was reworded"; exit 1; }; done</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox; P=crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs; test "$(/usr/bin/grep -ciF 'governance' "$P")" -ge 1 || { echo "the packaged gate gained no observe probe, so the governance surface is proved only in unit tests and never through the real binary"; exit 1; }; test "$(/usr/bin/grep -cF 'visibility: hidden from model' "$P")" -ge 1 || { echo "REGRESSION GUARD (not coverage): the packaged gate's existing visibility assertion was weakened or removed"; exit 1; }</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; git rev-parse HEAD &amp;&amp; cargo nextest run --profile ci -p wcore-agent -p wcore-cli --no-fail-fast" &gt; /tmp/f23a-03-observe-linux.log 2&gt;&amp;1; rc=$?; tail -60 /tmp/f23a-03-observe-linux.log; exit $rc</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git rev-parse HEAD &amp;&amp; WAYLAND_BUILD_SOURCE_SHA=$SHA cargo build --locked -p wcore-cli --bin wayland-core &amp;&amp; WAYLAND_BUILD_SOURCE_SHA=$SHA WCORE_EVAL_BIN=/root/wayland/target/debug/wayland-core cargo test --locked -p wcore-eval-scenarios --features packaged-driver-gate --test packaged_driver_gate" &gt; /tmp/f23a-03-packaged-linux.log 2&gt;&amp;1; rc=$?; tail -60 /tmp/f23a-03-packaged-linux.log; exit $rc</automated>
  </verify>
  <done>Showing a promoted artifact displays the bound hash, the authorising review with actor and time, the evaluation outcome and the promotion time; showing a revoked or rolled-back one displays the ordered history including what preceded it. A never-promoted draft shows no fabricated record. Listing distinguishes quarantined-generated, promoted-generated and user-authored entries. The three existing visibility strings are unchanged and the packaged gate's assertions still pass byte-for-byte, proved by running that gate against the real binary on Hetzner. The machine-readable and human-readable reports carry the same governance facts.</done>
</task>

<task type="auto">
  <name>Task 3: Prove observe, revoke and rollback through the shipped binary, and re-prove 23A-01's boundary against the revoked artifact</name>
  <files>crates/wcore-eval-scenarios/tests/f23a_revocation_drive.rs, crates/wcore-eval-scenarios/src/governed_skill_drive.rs, scripts/f23a-revocation-drive.sh, scripts/f23a-revocation-drive.ps1, .planning/phases/23A-governed-skills/23A-03-LIVE-EVIDENCE.md</files>
  <read_first>crates/wcore-eval-scenarios/src/governed_skill_drive.rs (23A-01's shared harness and specifically its PUBLIC route-refusal assertion set — the function this task calls against the revoked artifact; also the seeding helper, the drafting helper, the operator-verb helper and the two self-test switches), crates/wcore-eval-scenarios/tests/f23a_promotion_drive.rs (23A-02's drive target: the review-promote-execute arc whose end state this target begins from, and its control style), crates/wcore-eval-scenarios/src/fixtures/openai.rs (`OpenAiFixtureScript` at 34, `tool_call` at 105, `base_url` at 407), crates/wcore-eval-scenarios/src/runner.rs (`run_with_binary_in_paths` at 462 and the argv at 338-346), scripts/f23a-promotion-drive.sh (23A-02's thin wrapper: the expected-SHA assertion with its pinned exit 3 and the build-and-pin sequence this wrapper mirrors), crates/wcore-cli/tests/support/pty.rs (the Unix-only PTY harness and its module note at line 7 explaining why ConPTY cannot surface stdout in headless runners — the constraint that decides which live-session leg runs where), crates/wcore-cli/src/skills_cmd.rs (the verbs, their stable tokens and their exit codes), justfile (the f01-packaged-driver-gate recipes at 165-181 for unix and 183-184 for windows)</read_first>
  <behavior>
    - The live mechanism is 23A-01's, inherited: the drive target is `crates/wcore-eval-scenarios/tests/f23a_revocation_drive.rs` built on `wcore_eval_scenarios::governed_skill_drive`, and the two shell files are thin wrappers whose exit status is `cargo test -p wcore-eval-scenarios --features packaged-driver-gate --test f23a_revocation_drive`.
    - The driver starts from a promoted, executing artifact, observes its full governance provenance through the shipped binary, and captures it.
    - The driver revokes through the shipped binary and then proves the artifact is refused again at every route 23A-01 enumerated, by CALLING 23A-01's public route-refusal assertion set against the revoked artifact IN THE SAME PROCESS and against the SAME seeded environment — never by invoking `scripts/f23a-boundary-drive.sh`, which would rebuild the binary and draft a fresh artifact, destroying the revoked state the re-proof exists to test.
    - On Linux the driver additionally proves live-session revocation: a session is running when the revocation is recorded, and the driver observes either that the session stops executing the artifact or exactly how long it continues to.
    - The driver rolls back through the shipped binary and proves the pre-promotion state restored by comparing captured bytes and status, not by asserting cleanliness.
    - The driver proves the history survives: after rollback, the shipped binary still reports the promotion, the revocation and the rollback in order.
    - A control artifact that was promoted and NOT revoked continues to execute throughout, so the run distinguishes revocation from a global switch that re-quarantined everything.
    - The wrapper asserts its own checkout SHA before acting and exits EXACTLY 3 on mismatch; the run exits nonzero on any deviation including the control failing.
    - `WAYLAND_F23A_SELFTEST=revoked-still-runs` makes the drive target skip the revocation, leave the artifact promoted, run the route-refusal assertion set anyway, print `F23A-SELFTEST-TRIPPED: revoked-still-runs` and exit nonzero — proving the re-proof actually fires rather than passing vacuously.
  </behavior>
  <action>Continue from 23A-02's drive target rather than rebuilding its setup: this target's starting state is a promoted, executing artifact, and its job is to take that state apart in the two ways the criterion names. Build it as `crates/wcore-eval-scenarios/tests/f23a_revocation_drive.rs` on 23A-01's shared `governed_skill_drive` harness, feature-gated on `packaged-driver-gate`, with the `.sh` and `.ps1` files reduced to thin wrappers: assert `WAYLAND_EXPECT_SHA` and exit EXACTLY 3 on mismatch, export `WAYLAND_BUILD_SOURCE_SHA`, build the binary, derive an absolute `WCORE_EVAL_BIN` (with `.exe` on Windows) exactly as `justfile:165-181` and `justfile:183-184` do, then run the drive target and exit with ITS status. No assertion logic in a wrapper.

Observe first and capture what the product says, because that capture is what the rollback comparison is checked against later in the same run. Then revoke, and prove the refusal returned by CALLING 23A-01's public route-refusal assertion set against the revoked artifact, in-process, against the same seeded environment. Calling the census-derived assertion set is the point: a fresh, narrower check written here would prove revocation at whichever routes the author happened to think of, and the whole reason 23A-01 produced a census was so that set is not left to memory. Do NOT shell out to `scripts/f23a-boundary-drive.sh` to get this — that wrapper rebuilds the binary and drafts a NEW artifact, so it would assert refusal on an artifact that was never promoted and never revoked, and would pass no matter what this plan's revocation did. If 23A-01's assertion set is not shaped to be callable against a caller-supplied environment and skill name, extending it to be so is in scope here and `governed_skill_drive.rs` is declared for exactly that.

Then prove the re-proof itself can fail. `WAYLAND_F23A_SELFTEST=revoked-still-runs` skips the revocation, leaves the artifact promoted, and runs the assertion set anyway; the target must fail, print `F23A-SELFTEST-TRIPPED: revoked-still-runs` and exit nonzero. Without that, "the boundary driver passed against the revoked artifact" is a sentence, not a measurement — and it is the most load-bearing sentence in this plan.

Prove the live-session behaviour on Linux, where the PTY harness actually works. Start a session, record the revocation from outside it, and observe what the running session does. If it stops executing the artifact, record how quickly. If it does not, measure how long it continues to and record that as the exposure window — the same number Task 1 measured, now observed at the product surface rather than in a test. Do not claim a Windows PTY result; the harness is Unix-only by construction and the module says why.

Roll back and compare against the capture taken at the start of the run — the artifact bytes, the reported status and the loader's verdict. Then confirm the history is still readable, because an append-only ledger that loses entries on rollback is not append-only.

Keep a promoted-and-not-revoked control alive for the whole run. Without it, a bug that re-quarantines every generated artifact looks exactly like working revocation.

Exit nonzero on any deviation, including the control failing. On Windows use the trap-safe environment assignment form, prove it took effect, invoke the PowerShell driver through the file form, and never read an exit code from a block that also emits output.

Record in `23A-03-LIVE-EVIDENCE.md`, per platform: every invocation with its output and exit code; the observed governance provenance before and after each operation; the boundary driver's result against the revoked artifact; the live-session outcome or the measured exposure window; the rollback comparison field by field; the control's result; and the explicit statement that macOS is 23A-04's disposition.

Closes the observe, revoke and rollback stages of F23-01 at the product surface for Linux and Windows; marks no requirement complete — closure is claimed by 23A-04.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -x scripts/f23a-revocation-drive.sh &amp;&amp; test -f scripts/f23a-revocation-drive.ps1 &amp;&amp; test -f crates/wcore-eval-scenarios/tests/f23a_revocation_drive.rs &amp;&amp; bash -n scripts/f23a-revocation-drive.sh</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'f23a_revocation_drive' scripts/f23a-revocation-drive.sh)" -ge 1 &amp;&amp; test "$(/usr/bin/grep -cF 'f23a_revocation_drive' scripts/f23a-revocation-drive.ps1)" -ge 1 &amp;&amp; test "$(/usr/bin/grep -cF 'governed_skill_drive' crates/wcore-eval-scenarios/tests/f23a_revocation_drive.rs)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox; HITS=$(/usr/bin/grep -nE '^[^#]*(bash|sh|pwsh|powershell|source|\.|&amp;)[^#]*f23a-boundary-drive' scripts/f23a-revocation-drive.sh scripts/f23a-revocation-drive.ps1); test -z "$HITS" || { printf 'the revocation wrapper INVOKES the boundary wrapper, which rebuilds and re-drafts and would assert refusal on an artifact that was never revoked; the re-proof must be an in-process call to the shared route-refusal assertion set:\n%s\n' "$HITS"; exit 1; }</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; WAYLAND_EXPECT_SHA=$SHA bash scripts/f23a-revocation-drive.sh" &gt; /tmp/f23a-03-drive-linux.log 2&gt;&amp;1; rc=$?; tail -80 /tmp/f23a-03-drive-linux.log; exit $rc</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; WAYLAND_EXPECT_SHA=0000000000000000000000000000000000000000 bash scripts/f23a-revocation-drive.sh; echo SHA_RC=\$?; WAYLAND_EXPECT_SHA=$SHA WAYLAND_F23A_SELFTEST=revoked-still-runs bash scripts/f23a-revocation-drive.sh; echo SELFTEST_RC=\$?" &gt; /tmp/f23a-03-selftest-linux.log 2&gt;&amp;1; /usr/bin/grep -qF 'SHA_RC=3' /tmp/f23a-03-selftest-linux.log &amp;&amp; /usr/bin/grep -qE 'SELFTEST_RC=[1-9]' /tmp/f23a-03-selftest-linux.log &amp;&amp; /usr/bin/grep -qF 'F23A-SELFTEST-TRIPPED: revoked-still-runs' /tmp/f23a-03-selftest-linux.log || { echo "revocation driver self-tests did not fire — the boundary re-proof passes vacuously"; tail -40 /tmp/f23a-03-selftest-linux.log; exit 1; }</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes SeanD@seandesktop "cmd /c \"cd /d C:\ferrox-win &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; cargo clippy --workspace --all-targets -- -D warnings\"; \$env:WAYLAND_EXPECT_SHA='$SHA'; powershell -NoProfile -File C:\ferrox-win\scripts\f23a-revocation-drive.ps1; exit \$LASTEXITCODE" &gt; /tmp/f23a-03-drive-win.log 2&gt;&amp;1; rc=$?; /usr/bin/grep -v CLIXML /tmp/f23a-03-drive-win.log &gt; /tmp/f23a-03-drive-win.clean 2&gt;/dev/null; tail -80 /tmp/f23a-03-drive-win.clean; test $rc -eq 0 || { echo "the Windows clippy-then-revocation-driver leg exited $rc; the CLIXML-filtered transcript is above and the raw capture is /tmp/f23a-03-drive-win.log"; exit $rc; }</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git rev-parse HEAD &amp;&amp; WAYLAND_EXPECT_SHA=$SHA bash scripts/f23a-promotion-drive.sh" &gt; /tmp/f23a-03-promotion-regress.log 2&gt;&amp;1; rc=$?; tail -40 /tmp/f23a-03-promotion-regress.log; exit $rc</automated>
  </verify>
  <done>The drive target `crates/wcore-eval-scenarios/tests/f23a_revocation_drive.rs` exists and reuses 23A-01's `governed_skill_drive` harness; both shell files are thin wrappers that name it, carry no assertion logic, and — gate-checked — do NOT shell out to the boundary wrapper. One continuous run per platform observes, revokes, re-proves the boundary against the revoked artifact by calling 23A-01's own assertion set in-process against the same seeded environment, rolls back, compares against the run's own opening capture field by field, and confirms the history is still readable. The promoted-and-not-revoked control executes throughout and its failure fails the run. Both self-tests fired: a wrong `WAYLAND_EXPECT_SHA` exited exactly 3, and `WAYLAND_F23A_SELFTEST=revoked-still-runs` printed the tripped marker and exited nonzero, so the boundary re-proof is proved not to pass vacuously. On Linux the live-session leg either shows revocation reaching a running session or records the measured exposure window; no Windows PTY result is claimed. 23A-02's promotion driver still passes at this SHA. `23A-03-LIVE-EVIDENCE.md` records every invocation, output, exit code, the provenance before and after each operation, the rollback comparison, the control result and both self-test outcomes, and states that macOS is 23A-04's disposition. No gate took its status from a pipeline, and no untouched-surface claim rests on a bare `git status --porcelain` or `git diff --stat`.</done>
</task>

</tasks>

## What this plan does NOT change (scope fence)

- **Promotion, the evaluation policy and the ledger schema.** 23A-02 owns them. This plan appends to the ledger; it does not reshape it, and a seventh migration is a recorded finding before it is an action.
- **The three visibility strings the packaged driver gate asserts.** They are contract text for a live gate; the governance block goes around them.
- **The procedure transition table.** Active to Archived is already legal and is what revocation uses; no new status and no parallel status path.
- **23A-01's enforcement boundary.** Revocation is proved by calling that plan's route-refusal assertion set in-process against the revoked artifact, not by relaxing it, re-deriving it, or shelling out to its wrapper (which rebuilds and re-drafts, and would assert refusal on an artifact that was never revoked).
- **The phase disposition, the journey driver and the macOS decision.** 23A-04 owns them.
- **Phase 23B's entire surface** — operator session lifecycle including checkpoint and rewind, memory and user-model controls, cache and compaction economics, the repository index and the multi-day journey, planned under `.planning/phases/23B-continuous-agency/`. Session rewind and skill rollback are different mechanisms over different objects and are not merged.
- **The watcher's filesystem semantics.** If the governance signal needs a channel, it is added beside the watcher's existing behaviour, not by making the watcher pretend a database write is a file event.
- **No test is deleted, weakened, re-gated, ignored or allow-attributed.** If live propagation cannot be achieved, that is an OPEN clause with a measured exposure window, not a redefinition.

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| running session ← governance state change | A session that started before a revocation holds its own view of what may execute, and that view is where a revoked artifact keeps running |
| operator understanding ← observe surface | Whatever the observe surface omits is what an operator cannot reason about after an incident |
| restored state ← captured state | Rollback is only as trustworthy as the pre-promotion state that was captured to restore from |
| history ← rollback | An operation that undoes an effect must not also undo the record that the effect happened |

## STRIDE Threat Register

| Threat ID | Category | Component | Severity | Disposition | Mitigation Plan |
|-----------|----------|-----------|----------|-------------|-----------------|
| T-23A-03-01 | Elevation of Privilege | Revocation is recorded in the ledger but the running session keeps executing the artifact, because the catalog baked the verdict in at load time and the watcher only fires on filesystem events | critical | mitigate | The propagation problem is established from the code first; the narrowest mechanism inside the declared files is wired into the catalog's existing invalidation; if none reaches a live session the exposure window is MEASURED, recorded as an OPEN clause and proved again at the product surface by the Linux live-session leg |
| T-23A-03-02 | Repudiation | Rollback erases the promotion entry, destroying the evidence of what ran and when | high | mitigate | Rollback appends to the append-only ledger and is asserted to leave the promotion and any revocation readable in order, both in the kernel tests and through the shipped binary in the live driver |
| T-23A-03-03 | Tampering | Rollback leaves residue — different bytes, a different status, or a stale cached verdict — so the pre-promotion state is claimed rather than restored | high | mitigate | The pre-promotion state is captured before promoting and compared field by field after rolling back, in the kernel and again in the live driver against that run's own opening capture; any difference is a finding regardless of how harmless it looks |
| T-23A-03-04 | Repudiation | Revocation appears to work because a bug re-quarantined every generated artifact | high | mitigate | A promoted-and-not-revoked control executes throughout every kernel test and every live run, and its failure fails the run |
| T-23A-03-05 | Information Disclosure | The observe surface fabricates an empty governance record for a never-promoted draft, which reads like a promotion with missing fields | medium | mitigate | A never-promoted draft is asserted to show no governance history at all, distinctly from a promoted one |
| T-23A-03-06 | Repudiation | The machine-readable and human-readable reports disagree, so an operator and a script trust different truths | medium | mitigate | Both forms are asserted to carry the same governance facts |
| T-23A-03-07 | Denial of Service | Rewording an existing visibility string turns a live packaged gate red for a reason unrelated to governance, and the gate gets softened to compensate | medium | mitigate | The three strings are gate-checked present unchanged and the packaged driver gate is run against the real binary as part of this plan's own verification |
| T-23A-03-08 | Spoofing | A gate that cannot fail: a piped ssh status, an exit code read from an output-emitting block, a bare `git status --porcelain` or `git diff --stat` (both exit 0 unconditionally), or a driver run against a stale checkout | high | mitigate | All shapes are banned by name; every remote gate redirects and exits with the captured status; every untouched-surface claim is `test -z` over a working-tree leg AND a base-SHA diff leg; both wrappers exit exactly 3 on an expected-SHA mismatch and that pinned code is gate-checked out of captured remote stdout |
| T-23A-03-09 | Spoofing | The boundary re-proof passes vacuously — the revocation driver shells out to 23A-01's wrapper, which rebuilds the binary and drafts a NEW artifact, so refusal is asserted on something that was never promoted and never revoked and the gate is green regardless of what revocation did | critical | mitigate | The re-proof CALLS 23A-01's public route-refusal assertion set in-process against the same seeded environment and the revoked artifact; both shell files are gate-checked to contain zero references to the boundary wrapper; and `WAYLAND_F23A_SELFTEST=revoked-still-runs` leaves the artifact promoted and runs the assertion set anyway, which must fail with a tripped marker |
| T-23A-03-SC | Tampering | npm/pip/cargo installs | low | accept | No dependency is added, removed or updated; no `Cargo.toml` or `Cargo.lock` change; no install task exists in this plan |
</threat_model>

<verification>
Local gates (Mac, source level only — Cargo is never run here): `cargo fmt --all -- --check` clean; the revocation test binary exists; the kernel carries both revoke and rollback; the three visibility strings are present unchanged in the slash renderer and still asserted in the packaged gate; the Rust drive target exists, names the shared `governed_skill_drive` harness, and both wrappers name the drive target; neither wrapper references `f23a-boundary-drive` at all, since the re-proof is an in-process call; the shell wrapper parses under `bash -n`; nothing under `crates/` was touched outside each task's declared files, measured with `test -z` over a working-tree leg AND a base-SHA diff leg so neither an untracked file nor an already-committed change can hide.

Authoritative gates (real hardware, status taken from the remote process and never from a pipeline): on Hetzner at the pinned SHA, the `wcore-skills` suite passes, the `wcore-agent` and `wcore-cli` suites pass, the full workspace builds and the aggregate passes, the packaged driver gate passes against the real binary, the revocation driver runs green including the control and the live-session leg, and 23A-02's promotion driver still passes. On SEANDESKTOP at the same SHA, clippy at `-D warnings` passes FIRST, then the revocation driver through the PowerShell file form, with no PTY result claimed.

Known unknowns to record rather than resolve here: whether a revocation propagates to a session running in another process or another profile, which this plan does not exercise; whether the measured exposure window differs materially under load; and whether macOS behaves identically, which this plan does not measure and 23A-04 dispositions.
</verification>

<success_criteria>
- Revoke and rollback exist as distinct append-only transactions, reusing the already-legal Active-to-Archived transition through the 23A-02 kernel rather than a parallel status path.
- Revocation either reaches a catalog that was already built — proved against the same instance, not a fresh one — or the exposure window is measured in the kernel AND observed at the product surface, and recorded as an OPEN clause. Revocation is never redefined to mean "next boot".
- Rollback restores artifact bytes, procedure status and loader classification to values captured before promotion, compared field by field, with any difference reported as residue.
- After rollback the ledger still shows the promotion, any revocation and the rollback, in order.
- The observe surface shows the bound hash, the authorising review with actor and time, the evaluation outcome, the promotion time and the ordered history, in both a human-readable and a machine-readable form carrying the same facts, and shows no fabricated record for a never-promoted draft.
- The three visibility strings the packaged driver gate asserts are unchanged, and that gate passes against the real binary at this plan's SHA.
- Revocation is proved at exactly the routes 23A-01's census enumerated, by CALLING that plan's public route-refusal assertion set against the revoked artifact in-process — never by shelling out to its wrapper, which would rebuild and re-draft and assert on an artifact that was never revoked. That prohibition is gate-checked on both shell files.
- The live mechanism is 23A-01's `governed_skill_drive` harness, inherited not forked; the `.sh` and `.ps1` files are thin wrappers around `cargo test ... --test f23a_revocation_drive` and carry no assertion logic.
- Both wrappers exit exactly 3 on an expected-SHA mismatch, and `WAYLAND_F23A_SELFTEST=revoked-still-runs` makes both fail with a tripped marker — so the boundary re-proof is proved not to pass vacuously.
- A promoted-and-not-revoked control executes throughout every kernel test and every live run, so revocation is proved to be targeted rather than global.
- 23A-02's promotion driver still passes at this plan's SHA.
- No gate derives its status from a pipeline, from an exit code read out of an output-emitting block, from a bare `git status --porcelain`, `git diff --stat` or `git diff --name-only` (all of which exit 0 unconditionally), or from grepping an evidence file this plan wrote; every untouched-surface claim is `test -z` over both a working-tree leg and a base-SHA leg; no Windows PTY result is claimed.
</success_criteria>

## Artifacts this plan produces
- `crates/wcore-skills/src/governance.rs` — revoke and rollback transactions plus the live-propagation signal.
- `crates/wcore-skills/src/refs.rs` — the catalog's invalidation response to a governance change.
- `crates/wcore-agent/src/slash/skill.rs` and `crates/wcore-cli/src/skills_cmd.rs` — the observe surface with governance provenance and history.
- `crates/wcore-skills/tests/governed_revocation.rs` — propagation, distinction, history and no-residue coverage.
- `crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs` — observe probes added to the existing live matrix.
- `.planning/phases/23A-governed-skills/23A-03-BASE-SHA` — the untouched starting commit every untouched-surface gate measures against.
- `crates/wcore-eval-scenarios/tests/f23a_revocation_drive.rs` — the Rust drive target that calls 23A-01's route-refusal assertion set against the revoked artifact in-process.
- `scripts/f23a-revocation-drive.sh` and `scripts/f23a-revocation-drive.ps1` — the SHA-asserting thin wrappers around that drive target.
- `.planning/phases/23A-governed-skills/23A-03-LIVE-EVIDENCE.md` — the recorded live outcome per platform.
- `23A-03-SUMMARY.md`.

<output>
Create `.planning/phases/23A-governed-skills/23A-03-SUMMARY.md` using the standard GSD summary template. Record: the propagation problem as established from the code, with the citations; the mechanism chosen or the measured exposure window with how it was measured, in the kernel and at the product surface; the revoke-versus-rollback distinction and what each restores; the field-by-field rollback comparison including any residue found and its disposition; the ledger's ordered contents after both operations; the governance facts the observe surface shows and the proof that the three existing visibility strings are unchanged; the packaged driver gate's result against the real binary; the boundary driver's result against the revoked artifact; the control's behaviour throughout; the exact live invocations per platform with outputs and exit codes; the confirmation that 23A-02's promotion driver still passes at this SHA; the Hetzner aggregate counts with every residual failure named; the Windows clippy-then-drive result and the explicit note that no PTY result is claimed there; the explicit statement that macOS is 23A-04's disposition; the recorded unknowns; and which of the three termination states the plan ended in. Mark no requirement complete — F23-01 closure is claimed by 23A-04.
</output>
