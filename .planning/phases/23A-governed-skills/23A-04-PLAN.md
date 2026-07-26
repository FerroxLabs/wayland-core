---
phase: 23A-governed-skills
plan: "04"
type: execute
wave: 4
depends_on:
  - "23A-01"
  - "23A-02"
  - "23A-03"
files_modified:
  - scripts/f23a-governed-skill-journey.sh
  - scripts/f23a-governed-skill-journey.ps1
  - .planning/phases/23A-governed-skills/23A-04-MACOS-DECISION.md
  - .planning/phases/23A-governed-skills/23A-04-CONTRACT.md
  - .planning/phases/23A-governed-skills/23A-04-LIVE-EVIDENCE.md
autonomous: false
requirements:
  - F23-01
domain: code
must_haves:
  truths:
    - "THIS IS THE PHASE'S TERMINAL PLAN AND IT ALONE STATES THE PHASE OUTCOME. Following the program's acceptance contract, 23A-01, 23A-02 and 23A-03 each record their own evidence and each explicitly mark no requirement complete. This plan runs the aggregate proof at ONE exact SHA and states the disposition of F23-01 and of Success Criterion 1. It does not re-litigate a predecessor's finding; it authenticates what they recorded. Phase 20's terminal plan correctly recorded a RED aggregate and left every requirement incomplete rather than ticking a box, and Phase 20A closed COMPLETE on its three Success Criteria while leaving four requirements explicitly OPEN. Both were the right call and both are the standard here."
    - "THE ONE-RUN JOURNEY IS THE PROOF THAT THE STAGES COMPOSE, WHICH THREE SEPARATE GREEN PLANS DO NOT ESTABLISH. F23-01 names nine stages — detect, draft, quarantine, evaluate, review and policy, promote, observe, revoke, rollback — and the predecessor plans prove them in three groups against three separately-prepared states. A lifecycle proved in pieces can still be broken end to end: a promotion that works from a hand-seeded ledger can fail from a ledger the drafter's own path produced, and a rollback that restores a fixture can fail to restore an artifact that a real session wrote. One continuous run, from a draft the product itself detected to a rollback that returns the machine to where it started, is what closes that gap."
    - "A JOURNEY DRIVER WHERE EVERY STEP IS EXPECTED TO SUCCEED CANNOT GO RED FOR THE REASON THAT MATTERS. The driver carries at least three negative controls whose SUCCESS is a failure: an unpromoted draft that must be refused, an unreviewed draft that must not promote, and a post-rollback artifact that must be refused again. It also carries one positive control — a user-authored skill that must execute throughout — so a build where skills are simply broken cannot masquerade as a build where governance works. And it asserts its own checkout SHA before doing anything, because a gate that passes against stale code is worse than no gate. Two self-passing shapes were found in this repository in the last 24 hours: a piped ssh whose status was grep's, and an exit code read from a Tee-Object block that returned an array and made every comparison truthy while a 12/12 and 6/6 all-PASS soak reported failure. Neither shape appears here."
    - "MACOS EVIDENCE IS A SEAN-GATED DISPATCH AND THE COST OF EACH OPTION IS REAL, WHICH IS WHY IT IS A BLOCKING DECISION AND NOT AN IMPLEMENTATION DETAIL. This Mac may not run Cargo under this phase's constraints, and the macOS runner at `~/actions-runner-macos/` is EPHEMERAL — it consumes itself per job and must be re-registered for each dispatch. Its `f20-no-ambient-secrets` label is FALSE: it runs as Sean's user with reach over the SSH directory, the AWS directory and an unlocked keychain, and the real fix is a dedicated runner account that does not exist yet. `scripts/f20-native-macos-proof.sh:134` pulls a container image unconditionally, which cost four failed macOS dispatches during Phase 20A. The ROADMAP execution rules make native proof dispatch an explicit Sean authorization. So the choice is between spending a Sean-gated dispatch on a runner with a known-false security label, and closing on Linux and Windows with macOS recorded as owed. Phase 20A closed COMPLETE with four requirements explicitly OPEN; that precedent makes the second option legitimate, and it must still be CHOSEN rather than absorbed."
    - "THE ADMITTED CONTRACT IS THE ACTUAL DELIVERABLE THAT PHASE 23B CONSUMES. The ROADMAP states that Phase 23B begins only from the admitted 23A contract. That contract is not this plan's summary — it is a written statement of what the governed-skill lifecycle guarantees, what it does not, and what a consumer may rely on: which artifact identity a promotion binds, which stores the transaction spans and what a partial failure leaves, what evaluation is computed from and why use counts cannot be part of it, what revocation reaches and within what window, what rollback restores and what it deliberately preserves, and which surfaces refuse unpromoted content. Every one of those is a fact a downstream plan can be wrong about, and 23B was planned before this phase existed, so the contract must be explicit rather than inferred from code."
    - "D2 IS NOT THIS PHASE'S GATE AND THIS PLAN MUST NOT TOUCH IT. The Phase 23 exit gate D2 freezes durable Goal, child, task and wait semantics and requires replay through the REAL Desktop consumer and reducer, whose half cannot be closed from this repository. `.planning/phases/23B-continuous-agency/23B-04-PLAN.md` explicitly owns naming what Core owes D2 and stopping at that boundary. This plan closes 23A only. It does not claim D2, does not restate what Core owes it, and does not mark Phase 23 complete."
    - "A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, add an ignore or allow attribute, raise a timeout, re-gate, or delete an inconvenient test to reach a gate. If a predecessor left an OPEN clause — most likely 23A-03's live-revocation exposure window — this plan records it as OPEN with its measured value and states whether Success Criterion 1 is nonetheless satisfied, giving the reasoning. It does not close an open clause by restating it more favourably."
  artifacts:
    - path: scripts/f23a-governed-skill-journey.sh
      provides: "The one-run Linux journey: detect, draft, quarantine, refuse, evaluate, review, promote, execute, observe, revoke, refuse, roll back, refuse — with three negative controls, one positive control and a checkout-SHA assertion"
    - path: scripts/f23a-governed-skill-journey.ps1
      provides: "The Windows journey with the same contract, invoked through the PowerShell file form so its own exit status is the gate"
    - path: .planning/phases/23A-governed-skills/23A-04-MACOS-DECISION.md
      provides: "The recorded macOS evidence decision with each option's real cost stated, the authorization captured verbatim, and — if macOS is not run — the exact dispatch that would close it"
    - path: .planning/phases/23A-governed-skills/23A-04-CONTRACT.md
      provides: "The admitted 23A governed-skill contract: what the lifecycle guarantees, what it does not, and what Phase 23B may rely on"
    - path: .planning/phases/23A-governed-skills/23A-04-LIVE-EVIDENCE.md
      provides: "The aggregate proof at one exact SHA per platform, the journey outcome per stage, and the disposition of F23-01 and of Success Criterion 1 with every OPEN clause named"
  key_links:
    - from: scripts/f23a-governed-skill-journey.sh
      to: .planning/phases/23A-governed-skills/23A-04-LIVE-EVIDENCE.md
      via: "the per-stage captured transcript promoted into the recorded phase outcome"
      pattern: "live-evidence"
    - from: .planning/phases/23A-governed-skills/23A-04-MACOS-DECISION.md
      to: .planning/phases/23A-governed-skills/23A-04-LIVE-EVIDENCE.md
      via: "the authorized platform coverage bounding which legs the aggregate proof claims"
      pattern: "decision-record"
    - from: .planning/phases/23A-governed-skills/23A-04-CONTRACT.md
      to: .planning/phases/23B-continuous-agency/23B-01-PLAN.md
      via: "the admitted contract Phase 23B begins from, per the ROADMAP's internal ordering for Phase 23"
      pattern: "phase-handoff"
---

<objective>
Prove the nine stages of the governed-skill lifecycle compose in one continuous run against the shipped `wayland-core` binary, decide the macOS evidence question explicitly, publish the admitted 23A contract that Phase 23B begins from, and state the phase's disposition at one exact SHA.

Purpose: 23A-01 proved unpromoted content is inert, 23A-02 built the governed promotion transaction, and 23A-03 built observe, revoke and rollback. Each proved its own stages against its own prepared state. Nothing yet proves they compose from a draft the product itself detected through to a rollback that returns the machine to where it started. That composition is what Success Criterion 1 actually asserts, and the ROADMAP makes the admitted contract the precondition for Phase 23B.
Output: One journey driver per platform with negative and positive controls; one recorded macOS decision with its cost accepted; one aggregate build-and-test proof at one exact SHA; the admitted 23A contract; and the phase disposition with every OPEN clause named and carried forward rather than closed by restatement.
</objective>

<execution_context>
@$HOME/.codex/gsd-core/workflows/execute-plan.md
@$HOME/.codex/gsd-core/templates/summary.md
</execution_context>

<context>
@AGENTS.md
@.planning/HANDOFF-2026-07-26-phase20-20A-complete.md
@.planning/ROADMAP.md
@.planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md
@.planning/phases/23A-governed-skills/23A-01-LIVE-EVIDENCE.md
@.planning/phases/23A-governed-skills/23A-02-LIVE-EVIDENCE.md
@.planning/phases/23A-governed-skills/23A-03-LIVE-EVIDENCE.md
@.planning/phases/23B-continuous-agency/23B-01-PLAN.md
</context>

<execution_rules>

**THE TWO AMENDED PHASE RULES — verbatim, and they bound this plan.**
- Findings at CRITICAL or HIGH must be fixed or disproved. MEDIUM and below are logged to `.planning/BACKLOG.md` and DO NOT BLOCK execution.
- Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean.

**TERMINATION CRITERION (hard).** This plan ends in exactly one of three states and writes its SUMMARY in all three:
1. **PHASE COMPLETE** — the journey passes on every authorized platform, the aggregate is green at one exact SHA, the contract is published, and Success Criterion 1 is satisfied. Any predecessor OPEN clause is carried forward by name with its measured value and an explicit statement of why it does not block, mirroring how Phase 20A closed COMPLETE with four requirements OPEN.
2. **PHASE INCOMPLETE** — a stage of the lifecycle does not compose, or a negative control succeeds, or the aggregate is red. Record the phase as incomplete with the exact failing stage and its evidence. This is a legitimate and correct outcome; Phase 20's terminal plan recorded a RED aggregate rather than ticking a box.
3. **BLOCKED ON SEAN** — the macOS decision is not given. Record the open decision and stop. Do NOT pick a platform coverage on Sean's behalf.
Under no circumstances does this plan create additional plans, fix a defect a predecessor left, or extend its own task list. If the journey exposes a defect, that is termination state 2 with the defect recorded — not a repair.

**SCOPE BOUNDARY (hard).**
- This plan closes Phase 23A ONLY. It does NOT claim the Phase 23 exit gate D2, does not restate what Core owes D2, and does not mark Phase 23 complete. `.planning/phases/23B-continuous-agency/23B-04-PLAN.md` owns the D2 boundary.
- This plan does NOT touch Phase 23B's surface: operator session lifecycle, memory and user-model controls, cache and compaction economics, the repository index and the multi-day journey are planned under `.planning/phases/23B-continuous-agency/`.
- This plan writes ONLY inside its own phase directory and `scripts/`. It does NOT edit `ROADMAP.md`, `STATE.md`, `REQUIREMENTS.md` or `PROJECT.md`; updating the F23-01 and Phase 23A status rows at phase close is the orchestrator's action, and this plan supplies the disposition it acts on. The sibling terminal plan `23B-04` observes the same boundary.
- No push, merge, PR, tag, release, deployment, canary promotion, native proof dispatch or GitHub issue closure. Every one of those is Sean's explicit authorization under the ROADMAP execution rules.

**FOUR-PLAN CAP.** This phase has exactly 4 plans and this is the fourth. Do not propose a fifth.

**ENVIRONMENT.**
- Repository: `/Users/seandonahoe/dev/waylandcore-ferrox`, branch `plan/f20-unified-audit-repair`. NEVER touch `/Users/seandonahoe/dev/waylandcore`.
- NEVER run Cargo on this Mac. `cargo fmt --all -- --check` is the only cargo command used locally.
- Linux authority: `ssh -o BatchMode=yes hetzner-dsm`, `/root/wayland`. The full aggregate runs there in roughly 194 seconds.
- Windows: `ssh -o BatchMode=yes SeanD@seandesktop`, checkout `C:\ferrox-win`, PowerShell default shell, cargo at `C:\Users\seand\.cargo\bin\cargo.exe`. `cargo fmt --all` FAILS there with os error 206. Windows CI runs clippy `-D warnings` BEFORE tests, so a lint failure means tests never execute. A second Windows runner, `ferrox-win-msvc`, exists and was idle for the whole 20A effort; if both are used, give each its own worktree — concurrent compile load on one shared box corrupted a proof run.
- macOS: the ephemeral self-hosted runner at `~/actions-runner-macos/` consumes itself per job and must be re-registered for each dispatch. Dispatching it is Sean's authorization, which is why Task 2 exists.
- Both hosts' fetch refspecs are pinned to an unrelated branch. ALWAYS `git fetch origin plan/f20-unified-audit-repair`.
- Mac `grep` is rtk-proxied and SILENTLY DROPS LINES. ALWAYS `/usr/bin/grep`, `-F` for literals. Use `/usr/bin/git` on the Mac.
- In `cmd`, `set VAR=x && ...` appends a TRAILING SPACE. Use `set "VAR=x"` or `$env:VAR='x'` and PROVE it took effect.
- The PTY harness is UNIX-ONLY by construction; do not claim a Windows PTY result.

**THE SELF-PASSING GATE BAN (hard).**
- `ssh host 'cmd' | grep -v CLIXML` is FORBIDDEN as a gate; the pipeline's status is grep's. Filter for READING only.
- Reading an exit code from a block that also emits output — for example around a `Tee-Object` pipeline — is FORBIDDEN. Read it on the line AFTER the pipeline.
- Every remote gate redirects to a file, captures the status on the next line, and exits with that status.
- The phase disposition is NOT closed by grepping an evidence file this plan wrote. It is closed by the journey drivers' exit statuses, the aggregate's exit status, and the negative controls.

**AGENTS.md discipline.** Surgical diffs. No production source is modified by this plan — it proves and records. If a proof requires a source change, that is termination state 2. Stage exact paths, never `-A`, never `.`. No `Co-Authored-By` trailers.
</execution_rules>

<tasks>

<task type="auto">
  <name>Task 1: The one-run governed-skill journey with three negative controls and one positive control</name>
  <files>scripts/f23a-governed-skill-journey.sh, scripts/f23a-governed-skill-journey.ps1</files>
  <read_first>scripts/f23a-boundary-drive.sh (23A-01's refusal driver and its SHA assertion and exit discipline), scripts/f23a-promotion-drive.sh (23A-02's review-promote-execute arc and its unreviewed and post-edit controls), scripts/f23a-revocation-drive.sh (23A-03's observe-revoke-rollback arc, its boundary re-run and its promoted-and-not-revoked control), .planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md (the enumerated routes the journey's refusal stages must cover), crates/wcore-cli/src/skills_cmd.rs (the verbs, their stable stdout tokens and their distinct exit codes, so the journey observes outcomes rather than parsing prose), justfile (the packaged-driver-gate recipes near 163-185: how the real binary is built and pinned to a clean source SHA on each platform)</read_first>
  <behavior>
    - Stage 1 (detect and draft): the product's own drafting path produces a generated skill. The journey does not hand-write the artifact, because a hand-written fixture proves the lifecycle over a fixture.
    - Stage 2 (quarantine and refuse): the shipped binary refuses to run it, and refuses it at the routes the census enumerated.
    - Stage 3 (evaluate and review): the evaluation is computed and an operator review is recorded, each observable in captured output.
    - Stage 4 (promote): governed promotion succeeds with its stable token and a zero exit.
    - Stage 5 (execute): the SAME artifact now executes, with its observable effect captured.
    - Stage 6 (observe): the shipped binary reports the bound hash, the authorising review, the evaluation outcome and the history.
    - Stage 7 (revoke and refuse): revocation succeeds and the artifact is refused again at the same routes.
    - Stage 8 (rollback and refuse): rollback restores the pre-promotion state, the artifact is still refused, and the history still shows every transition in order.
    - Negative control A: a draft that was never reviewed does not promote at any point in the run.
    - Negative control B: a draft that was never promoted never executes at any point in the run.
    - Negative control C: after rollback, the artifact does not execute.
    - Positive control: a user-authored skill executes at every stage of the run.
    - The journey asserts its own checkout SHA first and exits with a distinct nonzero code on mismatch; it exits nonzero on any stage deviation and on any control violation, and it names which stage or control failed.
  </behavior>
  <action>Compose the three predecessor drivers into one continuous run rather than rewriting their logic. Each of them already established a fragment of the arc with its own controls; the value this driver adds is that all nine stages run against ONE artifact that the product itself detected and drafted, in one process lifetime's worth of state, with no hand-seeded fixture between stages. If a stage needs a fixture to work, that fact is itself the finding — record it and let the run go red rather than seeding around it.

Start with the SHA assertion, exit with a distinct code on mismatch, and print the SHA. Then build the binary the way the packaged gate does, from a clean tree pinned to the source SHA.

Carry all four controls for the entire run, not just at the moment they are checked. The unreviewed draft and the never-promoted draft must be present from stage 1 and probed at several points, because a defect that unquarantines everything at stage 4 would be invisible to a control that is only probed at stage 8. The positive control must execute at every stage, because that is what distinguishes governance from breakage.

On failure, name the stage and the control. A driver that exits 1 with no indication of which of thirteen checks failed costs an hour of rediscovery per failure, and this driver will be re-run on three surfaces.

On Windows, use the trap-safe environment assignment form and prove it took effect before trusting anything downstream. Invoke the PowerShell driver through the file form so its own exit status is the gate. Never read an exit code from a block that also emits output.

Records evidence for F23-01; marks no requirement complete — Task 3 states the disposition.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -x scripts/f23a-governed-skill-journey.sh &amp;&amp; test -f scripts/f23a-governed-skill-journey.ps1 &amp;&amp; bash -n scripts/f23a-governed-skill-journey.sh</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'WAYLAND_EXPECT_SHA' scripts/f23a-governed-skill-journey.sh)" -ge 1 &amp;&amp; test "$(/usr/bin/grep -cF 'WAYLAND_EXPECT_SHA' scripts/f23a-governed-skill-journey.ps1)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -ciE 'control' scripts/f23a-governed-skill-journey.sh)" -ge 4 &amp;&amp; test "$(/usr/bin/grep -ciE 'control' scripts/f23a-governed-skill-journey.ps1)" -ge 4</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git status --porcelain -- crates/</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; git rev-parse HEAD &amp;&amp; WAYLAND_EXPECT_SHA=$SHA bash scripts/f23a-governed-skill-journey.sh" &gt; /tmp/f23a-04-journey-linux.log 2&gt;&amp;1; rc=$?; tail -100 /tmp/f23a-04-journey-linux.log; exit $rc</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes SeanD@seandesktop "cmd /c \"cd /d C:\ferrox-win &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; git rev-parse HEAD\"; \$env:WAYLAND_EXPECT_SHA='$SHA'; powershell -NoProfile -File C:\ferrox-win\scripts\f23a-governed-skill-journey.ps1; exit \$LASTEXITCODE" &gt; /tmp/f23a-04-journey-win.log 2&gt;&amp;1; rc=$?; /usr/bin/grep -v CLIXML /tmp/f23a-04-journey-win.log | tail -100; exit $rc</automated>
  </verify>
  <done>Both journey drivers exist, assert their checkout SHA first with a distinct nonzero code on mismatch, and run all nine stages against one artifact the product itself drafted with no hand-seeded fixture between stages. All four controls are present from stage 1 and probed at several points, and any control violation fails the run with the stage and control named. Both ran green at one pinned SHA — Linux on Hetzner and Windows on SEANDESKTOP through the PowerShell file form. No production source was modified. No gate took its status from a pipeline.</done>
</task>

<task type="checkpoint:decision" gate="blocking">
  <name>Task 2 (BLOCKING DECISION): Authorize macOS native evidence for Phase 23A, or close on Linux and Windows with macOS recorded as owed</name>
  <files>.planning/phases/23A-governed-skills/23A-04-MACOS-DECISION.md</files>
  <action>Present the options and their real costs to Sean and obtain ONE authorization before Task 3 states any platform coverage. Record the selected option and the cost accepted with it verbatim in `23A-04-MACOS-DECISION.md`. Do not dispatch anything before the authorization is given — native proof dispatch is an explicit Sean authorization under the ROADMAP execution rules — and do not pick a coverage on Sean's behalf. If the decision is not given, that is termination state BLOCKED ON SEAN: record the open decision and stop.</action>
  <decision>Does Phase 23A's governed-skill lifecycle require native macOS evidence before the phase closes?</decision>
  <context>
Task 1's journey passes on Linux and Windows at one pinned SHA. macOS is uncovered, and covering it is not free. This Mac may not run Cargo under this phase's constraints. The macOS runner at `~/actions-runner-macos/` is EPHEMERAL — it consumes itself per job and must be re-registered for every dispatch. Its `f20-no-ambient-secrets` label is FALSE: it runs as Sean's user with reach over the SSH directory, the AWS directory and an unlocked keychain, and the real fix is a dedicated runner account that does not exist. `scripts/f20-native-macos-proof.sh:134` pulls a container image unconditionally, which caused four failed macOS dispatches during Phase 20A and is an open follow-up. Against that: nothing in this phase's lifecycle is obviously platform-specific — it is SQLite, content hashing, and file classification — but Phase 20A's whole existence is evidence that "obviously not platform-specific" has been wrong here before, and the Windows defect classes it found were path representation, handle semantics and mandatory locking, all of which this lifecycle touches. Phase 20A closed COMPLETE on its three Success Criteria with four requirements explicitly OPEN, so closing with a named, recorded gap is an established and legitimate outcome in this program. The point of this checkpoint is that the gap is chosen, not absorbed.
  </context>
  <options>
    <option id="dispatch-macos">
      <name>Dispatch the ephemeral macOS runner and run the journey natively before closing the phase</name>
      <pros>Closes Success Criterion 1 on all three platforms with no owed evidence; exercises the lifecycle against a case-insensitive filesystem, which is the most plausible place a content-hash-and-path-classification design behaves differently; produces the third leg the later native certification phase will want anyway</pros>
      <cons>Spends a Sean-gated dispatch on a runner whose no-ambient-secrets label is known false, so the run has reach over real credentials; requires re-registering the ephemeral runner; the existing macOS proof script's unconditional image pull already burned four dispatches and is unfixed; adds wall-clock time to a phase whose Linux and Windows evidence is already complete</cons>
    </option>
    <option id="close-two-platforms">
      <name>Close Phase 23A on Linux and Windows, and record macOS as owed with the exact dispatch that would close it</name>
      <pros>The precedent exists and was correct — Phase 20A closed COMPLETE with four requirements explicitly OPEN; spends no Sean-gated dispatch and takes no credential exposure; keeps Phase 23B unblocked, which the ROADMAP's internal ordering makes the point of finishing 23A; the owed leg is recorded precisely enough to be run later without rediscovery</cons>
      <cons>Success Criterion 1 carries a named platform gap into 23B and beyond; if the lifecycle does behave differently on a case-insensitive filesystem, the discovery moves to a later and more expensive phase</cons>
    </option>
    <option id="defer-to-certification">
      <name>Close on Linux and Windows and explicitly assign the macOS leg to the native cross-platform certification phase rather than owing it here</name>
      <pros>Puts the macOS proof where the three-platform matrix already lives instead of creating a one-off dispatch now; avoids re-registering an ephemeral runner twice for the same evidence; the certification phase's matrix would cover this lifecycle more thoroughly than a single journey run would</pros>
      <cons>The gap stays open far longer, and a defect found there is discovered after several phases have built on this contract; requires the assignment to be recorded somewhere the certification phase will actually read, which this plan cannot guarantee because it does not write shared files</cons>
    </option>
  </options>
  <resume-signal>Select: dispatch-macos, close-two-platforms, or defer-to-certification. If selecting dispatch-macos, confirm explicitly that the runner's false no-ambient-secrets label and its credential reach are accepted for this run. If selecting close-two-platforms or defer-to-certification, confirm explicitly that Success Criterion 1 closes with a named macOS gap.</resume-signal>
  <verify>
    <human-check>The selected option and the cost accepted with it are recorded verbatim in `23A-04-MACOS-DECISION.md`, and if macOS is not run the exact dispatch that would close it is written down precisely enough to execute later without rediscovery.</human-check>
  </verify>
  <done>One platform coverage is authorized and recorded with its accepted cost — or the decision was not given and the plan stopped in termination state BLOCKED ON SEAN. Nothing was dispatched before the authorization.</done>
</task>

<task type="auto">
  <name>Task 3: The aggregate proof at one exact SHA, the admitted 23A contract, and the phase disposition</name>
  <files>.planning/phases/23A-governed-skills/23A-04-CONTRACT.md, .planning/phases/23A-governed-skills/23A-04-LIVE-EVIDENCE.md</files>
  <read_first>.planning/phases/23A-governed-skills/23A-04-MACOS-DECISION.md (the authorized platform coverage, which bounds what this task may claim), .planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md and the three predecessor LIVE-EVIDENCE files and SUMMARYs (the recorded findings and any OPEN clause this task carries forward by name), .planning/ROADMAP.md (the Phase 23 Success Criteria, the internal ordering that makes 23B begin from the admitted 23A contract, and the exit-gate boundary this plan must not cross), .planning/phases/23B-continuous-agency/23B-01-PLAN.md (the first consumer of this contract, so the contract states what that plan can actually rely on)</read_first>
  <behavior>
    - The aggregate build and test run at ONE exact SHA on every authorized platform, with counts recorded and every residual failure named and attributed to its owning plan or to the known-red list.
    - The SHA the aggregate ran at is the same SHA the journey drivers ran at, stated explicitly, because a phase closed on evidence from two different trees is not closed.
    - The admitted contract states what the governed-skill lifecycle guarantees and what it does not, in terms a downstream plan can rely on without reading the source.
    - Every OPEN clause a predecessor recorded is carried forward by name with its measured value and an explicit statement of whether it blocks Success Criterion 1 and why.
    - The disposition of F23-01 and of Success Criterion 1 is stated plainly, bounded by the authorized platform coverage.
    - D2 is not claimed, restated or advanced, and Phase 23 is not marked complete.
  </behavior>
  <action>Run the aggregate at one exact SHA on every authorized platform and state the counts as a delta against the last recorded aggregate, naming every residual failure and attributing it — to a predecessor plan, to a Phase 23B surface, or to the known-red list the handoff already records. The known reds include a Windows sandbox descendant-reaping case, two private-DACL snapshot cases, and a worker-runtime-limits case whose timeout was deliberately not raised. Do not absorb a residual failure into a total, and do not raise a timeout to make one pass.

State the SHA once and use it everywhere. The journey drivers, the aggregate and the contract all refer to the same tree, and that identity is stated explicitly — a phase closed on evidence from two trees is not closed, and the drivers' own SHA assertions exist precisely so this claim is checkable rather than asserted.

Write the contract for its actual consumer. Phase 23B was planned before this phase existed, so it cannot have inferred anything from this code; the contract must be explicit. State: which artifact identity a promotion binds and what happens when the bytes change; which stores the promote transaction spans and what a partial failure leaves behind; what the evaluation is computed from and why production use counts cannot be part of it; what revocation reaches, within what window, and what it does not reach; what rollback restores and what it deliberately preserves; which surfaces refuse unpromoted content and where the enumerated list of them lives; and what a consumer must NOT assume. The last item matters most — a contract that only lists guarantees invites a downstream plan to assume the complement.

Carry every OPEN clause forward by name. The most likely one is 23A-03's live-revocation exposure window if propagation into a running session was not achieved. Record its measured value and state explicitly whether Success Criterion 1 is nonetheless satisfied and why. Do not close an open clause by restating it more favourably; Phase 20A carried four requirements OPEN into its COMPLETE disposition and named each one.

State the disposition of F23-01 and of Success Criterion 1, bounded by the authorized platform coverage. Do not claim, restate or advance D2 — it is Phase 23's exit gate, its Desktop consumer half cannot be closed from this repository, and `23B-04` owns naming what Core owes it. Do not mark Phase 23 complete; this plan closes 23A.

Do not edit `ROADMAP.md`, `REQUIREMENTS.md`, `STATE.md` or `PROJECT.md`. Updating the F23-01 and Phase 23A status rows is the orchestrator's action at phase close, and this task supplies the disposition it acts on.

States the phase disposition for F23-01 and Success Criterion 1.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -f .planning/phases/23A-governed-skills/23A-04-CONTRACT.md &amp;&amp; test -f .planning/phases/23A-governed-skills/23A-04-MACOS-DECISION.md &amp;&amp; test -f .planning/phases/23A-governed-skills/23A-04-LIVE-EVIDENCE.md</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -ciE 'must not assume|does not guarantee' .planning/phases/23A-governed-skills/23A-04-CONTRACT.md)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git status --porcelain -- .planning/ROADMAP.md .planning/REQUIREMENTS.md .planning/STATE.md .planning/PROJECT.md</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git status --porcelain -- crates/</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; git rev-parse HEAD &amp;&amp; cargo build --locked --workspace --all-features &amp;&amp; cargo nextest run --profile ci --workspace --no-fail-fast" &gt; /tmp/f23a-04-aggregate-linux.log 2&gt;&amp;1; rc=$?; tail -60 /tmp/f23a-04-aggregate-linux.log; exit $rc</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes SeanD@seandesktop "cmd /c \"cd /d C:\ferrox-win &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; git rev-parse HEAD &amp;&amp; cargo clippy --workspace --all-targets -- -D warnings &amp;&amp; cargo nextest run --profile ci -p wcore-skills -p wcore-cli -p wcore-agent -p wcore-memory --no-fail-fast\"; exit \$LASTEXITCODE" &gt; /tmp/f23a-04-aggregate-win.log 2&gt;&amp;1; rc=$?; /usr/bin/grep -v CLIXML /tmp/f23a-04-aggregate-win.log | tail -60; exit $rc</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git rev-parse HEAD &amp;&amp; WAYLAND_BUILD_SOURCE_SHA=$SHA cargo build --locked -p wcore-cli --bin wayland-core &amp;&amp; WAYLAND_BUILD_SOURCE_SHA=$SHA WCORE_EVAL_BIN=/root/wayland/target/debug/wayland-core cargo test --locked -p wcore-eval-scenarios --features packaged-driver-gate --test packaged_driver_gate" &gt; /tmp/f23a-04-packaged-linux.log 2&gt;&amp;1; rc=$?; tail -60 /tmp/f23a-04-packaged-linux.log; exit $rc</automated>
  </verify>
  <done>The aggregate ran at one exact SHA on every authorized platform, with counts stated as a delta and every residual failure named and attributed — none absorbed and no timeout raised. The journey, the aggregate and the packaged driver gate all refer to that same SHA and the identity is stated explicitly. The contract states what the lifecycle guarantees, what it does not, and what a consumer must not assume, in terms 23B can use without reading the source. Every predecessor OPEN clause is carried forward by name with its measured value and an explicit blocking judgement. The disposition of F23-01 and Success Criterion 1 is stated and bounded by the authorized platform coverage. D2 is neither claimed nor restated, Phase 23 is not marked complete, and no shared planning file and no production source was modified.</done>
</task>

</tasks>

## What this plan does NOT change (scope fence)

- **Production source.** This plan proves and records. If a proof requires a source change, that is termination state PHASE INCOMPLETE with the defect recorded — not a repair, and not a fifth plan.
- **Any predecessor's finding.** This plan authenticates what 23A-01, 23A-02 and 23A-03 recorded; it does not re-litigate, re-measure or soften them.
- **The Phase 23 exit gate D2.** Not claimed, not restated, not advanced. `23B-04` owns naming what Core owes it, and the Desktop consumer half cannot be closed from this repository at all.
- **Phase 23 completion.** This plan closes 23A. Phase 23 closes when 23B closes.
- **Phase 23B's surface** — operator session lifecycle, memory and user-model controls, cache and compaction economics, the repository index and the multi-day journey.
- **Shared planning files.** `ROADMAP.md`, `REQUIREMENTS.md`, `STATE.md` and `PROJECT.md` are untouched and gate-checked untouched; the status-row update at phase close is the orchestrator's action.
- **Sean-only actions.** No push, merge, PR, tag, release, deployment, canary promotion, native proof dispatch or issue closure. The macOS dispatch specifically waits on Task 2's authorization.
- **No test is deleted, weakened, re-gated, ignored or allow-attributed, and no timeout is raised.** A residual failure is named and attributed, never absorbed.

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| phase disposition ← evidence identity | A disposition is only as sound as the claim that every piece of evidence came from the same tree |
| downstream assumption ← published contract | Whatever the contract omits is what Phase 23B will assume, having been planned before this phase existed |
| authorized coverage ← claimed coverage | A phase that claims platforms it did not run has manufactured its own completion |
| open clause ← restatement | An OPEN clause can be closed by evidence or hidden by wording, and the two look identical in a summary |

## STRIDE Threat Register

| Threat ID | Category | Component | Severity | Disposition | Mitigation Plan |
|-----------|----------|-----------|----------|-------------|-----------------|
| T-23A-04-01 | Spoofing | The phase closes on evidence gathered from more than one tree, so the disposition describes a state that never existed | high | mitigate | One SHA is stated once and used everywhere; each driver asserts its own checkout SHA with a distinct nonzero code on mismatch; the aggregate, the journey and the packaged gate are all run against that SHA and the identity is stated explicitly |
| T-23A-04-02 | Spoofing | A journey driver where nothing can fail manufactures the phase's completion | critical | mitigate | Three negative controls whose success is a failure and one positive control whose failure is a failure, all present from stage 1 and probed at several points; the driver names the failing stage and control; neither banned self-passing shape appears in any gate |
| T-23A-04-03 | Repudiation | An OPEN clause is closed by restating it more favourably rather than by evidence | high | mitigate | Every predecessor OPEN clause is carried forward BY NAME with its measured value and an explicit blocking judgement; Phase 20A's precedent of closing COMPLETE with four named OPEN requirements is the standard |
| T-23A-04-04 | Elevation of Privilege | A native macOS dispatch is run without authorization, on a runner whose no-ambient-secrets label is known false and which has reach over real credentials | high | mitigate | The dispatch is gated behind a blocking decision; nothing is dispatched before the authorization is recorded; the false label and the credential reach are stated in the decision context so the cost is chosen rather than absorbed |
| T-23A-04-05 | Information Disclosure | The contract lists only guarantees, so Phase 23B assumes the complement of what is written | high | mitigate | The contract is required to state what a consumer must NOT assume, and that section is gate-checked present |
| T-23A-04-06 | Repudiation | A residual test failure is absorbed into an aggregate count instead of being named and attributed | medium | mitigate | Counts are stated as a delta with every residual failure named and attributed to its owning plan or to the recorded known-red list; raising a timeout to clear one is forbidden by name |
| T-23A-04-07 | Denial of Service | The plan drifts into repairing a defect the journey exposed, becoming a fifth plan in disguise | medium | mitigate | Termination state PHASE INCOMPLETE is defined as the correct response to an exposed defect; the scope fence forbids production source changes and the four-plan cap is restated |
| T-23A-04-08 | Tampering | A shared planning file is rewritten wholesale during phase close, destroying entries other phases own | medium | mitigate | This plan writes only inside its own phase directory and `scripts/`; the four shared planning files are gate-checked untouched; the status-row update is the orchestrator's scoped action |
| T-23A-04-SC | Tampering | npm/pip/cargo installs | low | accept | No dependency is added, removed or updated; no `Cargo.toml` or `Cargo.lock` change; no install task exists in this plan |
</threat_model>

<verification>
Local gates (Mac, source level only — Cargo is never run here): `cargo fmt --all -- --check` clean; both journey drivers exist, reference the expected-SHA variable, name at least four controls, and the shell driver parses under `bash -n`; the decision record, the contract and the live-evidence file all exist; the contract contains an explicit must-not-assume section; the four shared planning files are unmodified; no production source is modified.

Authoritative gates (real hardware, status taken from the remote process and never from a pipeline): on Hetzner at the pinned SHA, the journey runs green including all four controls, `cargo build --locked --workspace --all-features` succeeds, the full aggregate passes, and the packaged driver gate passes against the real binary built from that source SHA. On SEANDESKTOP at the same SHA, clippy at `-D warnings` passes FIRST, then the four governance-bearing crate suites, then the journey through the PowerShell file form. macOS runs only if Task 2 authorized it, and is otherwise recorded as owed with its exact dispatch.

Known unknowns to record rather than resolve here: whether the lifecycle behaves identically on a case-insensitive filesystem, which is unmeasured unless macOS was authorized; whether a revocation propagates across processes or profiles, which no plan in this phase exercised; and whether the evaluation thresholds hold for corpora other than the drafts this workspace produces.
</verification>

<success_criteria>
- One continuous run per authorized platform drives all nine F23-01 stages against a single artifact the product itself detected and drafted, with no hand-seeded fixture between stages.
- Three negative controls and one positive control are present from stage 1, probed at several points, and any violation fails the run with the stage and control named.
- The macOS evidence question is DECIDED at a blocking checkpoint with each option's real cost stated — the ephemeral runner, the known-false security label, the unconditional image pull that burned four dispatches, and the Phase 20A precedent for closing with named gaps — and nothing is dispatched before the authorization is recorded.
- The aggregate build and test pass at ONE exact SHA on every authorized platform, that SHA is the same one the journey and the packaged gate ran at, and the identity is stated explicitly.
- Every residual failure is named and attributed; none is absorbed into a total and no timeout is raised.
- The admitted 23A contract states what the lifecycle guarantees, what it does not, and what a consumer must NOT assume — in terms Phase 23B can rely on without reading the source.
- Every predecessor OPEN clause is carried forward by name with its measured value and an explicit blocking judgement.
- The disposition of F23-01 and Success Criterion 1 is stated and bounded by the authorized platform coverage.
- D2 is neither claimed, restated nor advanced; Phase 23 is not marked complete; no shared planning file and no production source is modified.
- No gate derives its status from a pipeline, from an exit code read out of an output-emitting block, or from grepping an evidence file this plan wrote.
</success_criteria>

## Artifacts this plan produces
- `scripts/f23a-governed-skill-journey.sh` and `scripts/f23a-governed-skill-journey.ps1` — the one-run nine-stage journey with four controls and a checkout-SHA assertion.
- `.planning/phases/23A-governed-skills/23A-04-MACOS-DECISION.md` — the authorized platform coverage with its accepted cost and, if macOS is owed, the exact dispatch that would close it.
- `.planning/phases/23A-governed-skills/23A-04-CONTRACT.md` — the admitted 23A governed-skill contract Phase 23B begins from.
- `.planning/phases/23A-governed-skills/23A-04-LIVE-EVIDENCE.md` — the aggregate at one exact SHA, the per-stage journey outcome, and the phase disposition.
- `23A-04-SUMMARY.md`.

<output>
Create `.planning/phases/23A-governed-skills/23A-04-SUMMARY.md` using the standard GSD summary template. Record: the exact SHA every piece of evidence was gathered at and the explicit statement that the journey, the aggregate and the packaged gate all refer to it; the per-stage journey outcome per platform with each control's result; the authorized platform coverage and the cost accepted with it, verbatim; the aggregate counts as a delta with every residual failure named and attributed; the packaged driver gate's result against the real binary; every predecessor OPEN clause carried forward by name with its measured value and blocking judgement; the contract's guarantees, non-guarantees and must-not-assume list; the disposition of F23-01 and of Success Criterion 1 bounded by the authorized coverage; the explicit statement that D2 is not claimed and Phase 23 is not marked complete; the confirmation that no shared planning file and no production source was modified; the recorded unknowns; and which of the three termination states the plan ended in. If the outcome is PHASE COMPLETE, state F23-01's disposition so the orchestrator can update the ROADMAP and REQUIREMENTS status rows; this plan does not edit them.
</output>
