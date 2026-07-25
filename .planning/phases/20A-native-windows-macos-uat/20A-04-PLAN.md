---
phase: 20A-native-windows-macos-uat
plan: "04"
type: execute
wave: 3
depends_on:
  - "20A-02"
  - "20A-03"
files_modified:
  - .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md
  - .planning/phases/20A-native-windows-macos-uat/20A-04-REVIEW/
  - .planning/REQUIREMENTS.md
  - .planning/BACKLOG.md
autonomous: false
requirements:
  - REQ-native-r4
  - REQ-native-r9
  - REQ-native-r11
  - REQ-native-r12
  - REQ-native-r13
  - REQ-native-r15
must_haves:
  truths:
    - "PHASE 20A CLOSES ON THREE SUCCESS CRITERIA AND THIS PLAN OWNS ALL THREE: (1) the six-target native Windows proof passes on the certified self-hosted runner against ONE EXACT SEALED CANDIDATE; (2) the macOS leg passes against THAT SAME EXACT CANDIDATE; (3) native evidence is bound to ONE NEWLY DISPATCHED, SEAN-AUTHORIZED RUN — never inferred from source, never from cross-compilation, never from Linux proof, and never from a reused run."
    - "THE SIX WINDOWS TARGETS, exactly, from the shared canonical map in `scripts/f20-native-uat-proof.mjs`: `windows-retained-handle`, `windows-appcontainer-acl`, `windows-job-object`, `windows-public-dispatch`, `windows-hard-process-containment`, `windows-f20-lifecycle`. The retained-handle, AppContainer-ACL and Job-Object targets were proven green on the certified runner earlier and MUST REMAIN green. The two that would fail today — `windows-public-dispatch` and `windows-f20-lifecycle` — are both downstream of the AppContainer bind blocker that 20A-02 owns. If 20A-02 escalated instead of shipping, those two stay red and this plan must say so rather than dispatch anyway."
    - "THE EIGHT macOS TARGETS, exactly, from that same shared map: `macos-retained-directory`, `macos-process-tree`, `macos-docker-reject-path-replacement`, `macos-docker-roundtrip-delete`, `macos-public-dispatch`, `macos-docker-cancellation`, `macos-docker-budget`, `macos-f20-lifecycle`."
    - "THE DISPATCH IS A SEAN GATE (PRD D6) AND THIS PLAN DOES NOT FIRE IT. The plan PREPARES one exact, idempotent, candidate-specific request tuple and stops at a blocking human checkpoint. Sean alone authorizes and fires it, switching to the FerroxLabs account and running the soak workflow in candidate mode against the sealed ref with the candidate flag set, the request nonce bound to the sealed SHA, and the macOS runner label supplied. Preparing is not authorizing, and a prepared tuple is not a dispatched run."
    - "EVERY PRIOR AUTHORIZATION DIGEST IS SPENT AND VOID. A new candidate SHA or any RED requires a FRESH authorization bound to the exact new tuple. Reusing a spent digest, or binding evidence to a run dispatched for a different candidate, defeats Success Criterion 3 entirely."
    - "REQ-native-r13 IS A HARD ATTESTATION GATE, added because four claimed reviews had no on-disk artifact at all. Every review gate emits a SCHEMA-VALIDATED review artifact for EVERY claimed reviewer. A prose-only review does not count toward a PASS, and a reviewer with no artifact is a reviewer who did not review."
    - "REQ-native-r15 IS A HARD PRE-BUILD GATE: the working tree is pristine before any candidate build. It exists because a boundary-breaking diagnostic edit was once measured as if it were the product. A candidate sealed from a tainted tree is not a candidate."
    - "UPSTREAM SUITE COUNTS ARE NOT THE SIX-TARGET PROOF, AND THE GAP IS CONCRETE. `windows-f20-lifecycle` selects `-p wcore-agent --test transactional_delegated_mutation_test` with NO ignored-set flag, while every baseline and delta measurement in 20A-01 and 20A-02 runs that same test WITH the ignored set included — the measured set and the proven set are literally different sets. `windows-public-dispatch` and the final platform acceptance marker additionally exercise marker emission and marker ORDERING, which nothing upstream touches at all. Sealing on upstream greens alone would spend a Sean authorization on an inference, so this plan runs the proof script END TO END on SEANDESKTOP — the same self-hosted msvc runner class the certified leg uses — before preparing any tuple."
    - "THE SEAL IS A PINNED REF, NOT LIVE `HEAD`. This plan commits its own review artifacts, candidate document and requirement updates, so `HEAD` moves after the seal is taken. The SHA is pinned once to `refs/f20a/candidate` and recorded in `20A-04-CANDIDATE.md`, and every downstream gate reads that value. `verifyNativeLog` binds each target marker to commit AND tree, so a gate comparing against a drifted `HEAD` would fail for the wrong reason or pass while proving nothing. No planning commit invalidates the seal, because no planning commit touches `crates/`, `scripts/` or `.github/` — and that invariance is gate-checked."
    - "A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, `#[ignore]`, `#[allow]`, raise a global timeout, or delete an inconvenient test to reach a gate. A RED native run leaves EVERY requirement incomplete with an explicit written disposition naming the failures. That is the correct outcome, and it is worth far more than a green bought by narrowing a target selector."
  artifacts:
    - path: .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md
      provides: "The one exact sealed candidate SHA and tree, its per-host pristine and lockfile proof, the aggregate Linux proof bound to it, the verified target map, and the prepared idempotent dispatch tuple"
    - path: .planning/phases/20A-native-windows-macos-uat/20A-04-REVIEW/
      provides: "One schema-validated review artifact per claimed reviewer, bound to the exact sealed candidate (REQ-native-r13)"
    - path: .planning/REQUIREMENTS.md
      provides: "REQ-native-r1..r15 completed and bound to the authorized run's evidence, or an explicit incomplete disposition naming every failure"
  key_links:
    - from: .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md
      to: .planning/REQUIREMENTS.md
      via: "the exact sealed candidate SHA whose Sean-authorized native run completes REQ-native-r1..r15, or leaves them incomplete"
      pattern: "requirement-completion"
    - from: .planning/phases/20A-native-windows-macos-uat/20A-04-REVIEW/
      to: .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md
      via: "the schema-validated per-reviewer artifacts that must belong to the exact sealed candidate before dispatch is prepared"
      pattern: "attestation"
---

<objective>
Seal ONE exact candidate, prosecute it with attested reviews, and PREPARE the Sean-gated native proof dispatch for six Windows targets and eight macOS targets — without firing it.

Purpose: Phase 20A's three Success Criteria all reduce to one thing — native evidence bound to one exact sealed candidate and one newly dispatched, Sean-authorized run. Everything upstream in this phase exists to make that candidate worth dispatching; this plan is where it is sealed, attested and handed to Sean. Firing the dispatch is Sean's action, not this plan's.
Output: One sealed candidate with per-host pristine, lockfile and aggregate-Linux proof bound to it; one schema-validated review artifact per claimed reviewer; one idempotent prepared dispatch tuple; and either the requirement completion bound to an authorized green run, or an explicit incomplete disposition naming every failure.
</objective>

<execution_context>
@/Users/seandonahoe/.codex/gsd-core/workflows/execute-plan.md
@/Users/seandonahoe/.codex/gsd-core/templates/summary.md
</execution_context>

<context>
@AGENTS.md
@.planning/ROADMAP.md
@.planning/REQUIREMENTS.md
@.planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md
@.planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md
@.planning/phases/20A-native-windows-macos-uat/20A-03-EOL-DECISION.md
@.planning/phases/20-transactional-delegated-mutation/20-NATIVE-REPAIR-PRD.md
@scripts/f20-native-uat-proof.mjs
@scripts/f20-native-windows-proof.ps1
@scripts/f20-native-macos-proof.sh
@.github/workflows/nightly-windows-soak.yml
</context>

<execution_rules>

**THE TWO AMENDED PHASE RULES — stated verbatim, and they bound this plan.**

- Findings at CRITICAL or HIGH must be fixed or disproved. MEDIUM and below are logged to BACKLOG and DO NOT BLOCK execution.
- Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean.

**TERMINATION CRITERION FOR THIS PLAN (hard — the plan STOPS and escalates rather than spawning more work).** This plan seals ONE candidate, runs AT MOST TWO review rounds, prepares ONE dispatch tuple, and stops at the Sean gate. It terminates in exactly one of four states, and in all four it writes its SUMMARY and stops:
1. **Complete** — Sean authorized at the checkpoint, the dispatched run returned green across all six Windows targets and all eight macOS targets bound to the exact sealed candidate, and REQ-native-r1..r15 are completed and bound to that run.
2. **Prepared, awaiting Sean** — the tuple is prepared and idempotent and the checkpoint is open. STOP. Record the explicit incomplete disposition leaving every requirement incomplete. Do NOT fire, do NOT poll in a loop, do NOT re-seal while waiting.
3. **RED** — the authorized run returned red on any target. STOP. Record the explicit incomplete disposition naming every failing target with its output. Do NOT re-seal, do NOT re-dispatch, do NOT narrow a selector. A new candidate needs a new plan and a fresh authorization.
4. **Blocked upstream** — the candidate cannot pass the six-target proof. This fires on EITHER of two conditions: 20A-02 escalated instead of shipping the bind, OR the mandatory local six-target dry-run in Task 1 did not go green on all six. STOP before sealing (or, if the dry-run failed after sealing, before preparing the tuple). Record that the candidate is not worth a dispatch and escalate. Dispatching a candidate known to be red spends a Sean authorization for nothing. A CRITICAL or HIGH review finding in Task 2 also lands here — see that task.
Two review rounds is the hard cap. A third round is NOT permitted; it escalates to Sean. Under no circumstances does this plan spawn additional plans, extend its own task list, re-seal after a red, or begin a second seal/review/dispatch cycle.

**THE SEAN GATES (unchanged, and this plan honors all of them).** Source push to main, main merge, issue closure, release, deployment, canary promotion, NATIVE PROOF DISPATCH, and deletion of a retained candidate UAT evidence ref all require Sean's explicit authorization. This plan pushes the WORK BRANCH so hosts can fetch it, and does nothing else on that list. It PREPARES the dispatch; it does not fire it.

**AUTHORIZATION FRESHNESS (hard).** Every prior authorization digest is spent and void. A new candidate SHA or any RED requires a FRESH authorization bound to the exact new tuple. Evidence from a run dispatched for a different candidate is not evidence for this one, and re-binding it would defeat Success Criterion 3 outright.

**THE SEAL IS AN IMMUTABLE RECORDED VALUE, NOT LIVE `HEAD` (hard, and every gate depends on it).** Task 1 seals ONE SHA and immediately writes it into `20A-04-CANDIDATE.md` AND pins it to a local ref `refs/f20a/candidate`. Tasks 2, 3 and 4 then commit review artifacts, the candidate document and requirement updates, so `HEAD` MOVES while the candidate does not. Every downstream gate therefore reads the sealed value from `20A-04-CANDIDATE.md` or from `refs/f20a/candidate` and NEVER recomputes `git rev-parse HEAD`. This is not hygiene: `verifyNativeLog` binds every target marker to commit AND tree, so a gate that silently compares against a drifted `HEAD` would either fail for the wrong reason or pass while proving nothing about the sealed candidate. The candidate SHA is a planning-artifact seal — planning commits after it do NOT invalidate it, because no planning commit changes `crates/`, `scripts/` or `.github/`, and that invariance is itself gate-checked.

**NON-NEGOTIABLE.** A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, `#[ignore]`, `#[allow]`, raise a global timeout, or delete an inconvenient test to reach a gate. Four executors in Phase 20 correctly stopped and escalated rather than improvise, and every one of those calls was right. The specific temptation here is to narrow a native target selector so it stops selecting the test that fails; the wrong-OS anti-drift guard exists because that has happened before, and it stays.

**ENVIRONMENT.**
- Windows: `ssh -o BatchMode=yes SeanD@seandesktop` (Tailscale), checkout `C:\ferrox-win`. Invocation shape: `ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "<cmd> 2>&1" }'`, piped through `grep -v CLIXML | grep -v "^<Objs"`. Git on the box MUST be wrapped `cmd /c "git ..."` — PowerShell's Stop preference treats git's stderr chatter as fatal. `cargo fmt --all` FAILS there with os error 206; `justfile:96-98` already skips fmt-check on Windows.
- Linux: `ssh -o BatchMode=yes hetzner-dsm`, `/root/wayland`. This is where the aggregate candidate proof runs.
- Mac CANNOT compile this workspace. `cargo fmt --all` is the only working cargo command there. Use `/usr/bin/git`, and ALWAYS `/usr/bin/grep`.
- Push the WORK BRANCH to `gh` so the hosts can fetch it. NO push to main, merge, PR, tag, release, or issue closure without Sean. Switch to the FerroxLabs account before every `gh` operation.
- The certified Windows leg runs on an AppContainer-capable self-hosted msvc runner, NOT a hosted image (REQ-native-r11). Confirm the runner the candidate job actually targets before preparing the tuple.

**THE TWO MEASUREMENT TRAPS (both measured; do not simplify these away).**
1. In `cmd`, `set VAR=value && ...` appends a TRAILING SPACE to the value and Rust silently ignores it. Use `set "VAR=x"` or PowerShell `$env:VAR='x'`, and PROVE it took effect. The native proof scripts set the live-acceptance flag, and a trailing space there would skip every acceptance test and produce a vacuously green six-target run — the single most dangerous failure mode in this plan.
2. Mac `grep` is rtk-proxied and SILENTLY DROPS LINES — measured at 32 returned versus 674 for the same inverted match on the same file. Every gate in this plan invokes `/usr/bin/grep` explicitly and uses `-F` for literals.

**Git hygiene.** Use `/usr/bin/git` on the Mac. Stage the exact paths in `files_modified`, never `-A`, never `.`. Never stage `AGENTS.md` or `.ijfw` churn. No `Co-Authored-By` trailers.
</execution_rules>

<tasks>

<task type="auto">
  <name>Task 1: Answer the upstream go/no-go, then seal ONE exact candidate with per-host pristine, lockfile and aggregate Linux proof</name>
  <files>.planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md</files>
  <read_first>.planning/phases/20A-native-windows-macos-uat/20A-02-SUMMARY.md (which termination state 20A-02 ended in — this decides whether a candidate is worth sealing at all), .planning/phases/20A-native-windows-macos-uat/20A-03-SUMMARY.md (the eol determination and whether it changed anything), .planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md (the measured baseline every count is a delta against), .planning/phases/20-transactional-delegated-mutation/20-NATIVE-REPAIR-PRD.md (the evidence rules and the dispatch gate), scripts/f20-native-uat-proof.mjs (the shared canonical target map and the wrong-OS anti-drift expectations both proof scripts mirror)</read_first>
  <behavior>
    - The upstream go/no-go is answered FIRST: if 20A-02 escalated rather than shipping the bind, two of the six Windows targets cannot pass and the candidate is not worth a Sean authorization.
    - The six-target proof script is run END TO END on SEANDESKTOP at the sealed SHA before any dispatch is prepared, and all six targets green locally is the go/no-go precondition — because the upstream suite measurements use a different test selection than the proof does, and prove nothing about marker emission or ordering.
    - Exactly ONE source SHA and its tree hash are sealed, pinned to a named ref, and every gate afterwards reads that recorded value rather than live `HEAD`, which moves as this plan commits its own artifacts.
    - For each requirement whose fix may predate this candidate, whether that fix is PRESENT at the sealed SHA is recorded, so no requirement is later reported as completed by a run that did not deliver it.
    - Every host that will build the candidate is confirmed pristine first — no diagnostic edit, no uncommitted change, nothing salvaged from a probe.
    - The lockfile is consistent, proven by a build that refuses to update it.
    - The aggregate Linux proof runs against the sealed SHA and its ACTUAL counts are recorded, compared against the immediately preceding run rather than against a historical figure.
    - The six Windows and eight macOS target ids are confirmed unchanged and the wrong-OS anti-drift guard is confirmed intact, without either being modified.
    - The certified Windows runner is confirmed AppContainer-capable and self-hosted, and the macOS harness's own re-validation status is recorded.
    - No requirement is completed here. Sealing is not proving.
  </behavior>
  <action>FIRST, the go/no-go. Read the termination state 20A-02 ended in. If it escalated rather than shipping the AppContainer binding, then the public-dispatch and F20-lifecycle Windows targets cannot pass, and dispatching a candidate known to be red spends a Sean authorization for nothing. STOP HERE: record that the candidate is not worth sealing, write the SUMMARY, and escalate. That is termination state 4 and it is a correct outcome.

Otherwise, seal the candidate. Pin ONE exact source SHA and record both it and its tree hash; every artifact, review and piece of evidence in this plan binds to that pair and to nothing else. Confirm the working tree is pristine on every host that will build it — the Mac, Hetzner and the Windows box — and record the confirmation per host (REQ-native-r15). This gate exists because a boundary-breaking diagnostic edit was once measured as if it were the product; a candidate sealed from a tainted tree is not a candidate.

Make the sealed SHA available on `hetzner-dsm:/root/wayland` and on `C:\ferrox-win` by fetching and detaching, and confirm each host prints the sealed SHA back. Then run the aggregate proof on Hetzner against that exact SHA, capturing full output for the workspace build with the lockfile pinned and all features enabled, and for the full test run under the CI profile without fail-fast. Record the ACTUAL run, passed, failed and skipped counts. Do NOT assert a historical expected total — compare against the immediately preceding run on this branch and explain any divergence. A lockfile inconsistency surfaced by the pinned build is a finding, recorded, not worked around.

Then verify the native target map has not drifted, WITHOUT changing it. Confirm the six Windows target ids and the eight macOS target ids are exactly those the shared canonical map declares; confirm the wrong-OS anti-drift guard is present and still fails closed for every target it classifies as OS-specific; and confirm the candidate Windows job targets an AppContainer-capable self-hosted msvc runner rather than a hosted image (REQ-native-r11). Record the macOS harness's re-validation status (REQ-native-r9) — the harness must be confirmed real and green against real macOS, and if 20A-01's CI wiring produced a macOS run at or near this SHA, cite it.

NOW THE MANDATORY LOCAL SIX-TARGET DRY-RUN, and it is the real go/no-go — everything above is necessary and none of it is sufficient. Run `scripts/f20-native-windows-proof.ps1` END TO END on SEANDESKTOP at the sealed SHA, with `WAYLAND_SANDBOX_LIVE_WINDOWS` set in the trap-safe form and PROVEN to have taken effect first. SEANDESKTOP is the same self-hosted msvc runner class the certified CI leg uses, so this is the closest available rehearsal of the run Sean is being asked to authorize.

This dry-run exists because the upstream measurements DO NOT COVER what the proof actually runs, and the gap is concrete rather than theoretical. The `windows-f20-lifecycle` target selects `-p wcore-agent --test transactional_delegated_mutation_test` with NO ignored-set flag, while every baseline and delta measurement in 20A-01 and 20A-02 runs that same test WITH the ignored set included — the measured set and the proven set are different sets, and a green on one is not a green on the other. Separately, `windows-public-dispatch` and the final platform acceptance marker exercise marker emission and marker ORDERING, which nothing upstream touches at all. Sealing on upstream suite counts alone would spend a Sean authorization on an inference.

ALL SIX TARGETS GREEN LOCALLY IS THE GO/NO-GO PRECONDITION. Anything less — a red target, a missing target marker, a marker out of order, an absent final acceptance marker, or a live-acceptance flag that did not take effect — is termination state 4: record it with the full output, do NOT seal or do NOT proceed to the tuple, and escalate. Also confirm the proof script's target array is EXACTLY the canonical six in canonical order; a seventh entry or a reordering would make the native verifier fail closed on the real run, and 20A-01 was fenced off this file precisely to keep that array intact.

PIN THE SEAL SO IT CANNOT DRIFT (H1). Write the sealed SHA and tree into `20A-04-CANDIDATE.md` and pin the SHA to the local ref `refs/f20a/candidate`. From this point on, every gate in this plan reads the sealed value from that document or that ref and NEVER recomputes `git rev-parse HEAD` — Tasks 2, 3 and 4 all commit planning artifacts, so `HEAD` will move while the candidate must not.

RECORD PER-REQUIREMENT CODE PRESENCE (M4). For each of REQ-native-r1, r2, r5 and r7, record one line stating whether its named code fix is already PRESENT at the sealed SHA, with the evidence — several of these were repaired in earlier work and this plan must not later report as "completed by this run" a requirement whose fix predates the candidate. A requirement whose fix is absent AND unproven is a finding, not a completion.

Record the sealed SHA and tree, the per-host pristine confirmations, the aggregate counts, the local six-target dry-run result per target, the target-map verification and the runner confirmation in `20A-04-CANDIDATE.md`. Records evidence for REQ-native-r4, r9, r11 and r15; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check &amp;&amp; /usr/bin/git status --porcelain -- crates/ scripts/ .github/</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git update-ref refs/f20a/candidate "$(/usr/bin/git rev-parse HEAD)" &amp;&amp; echo "SEALED=$(/usr/bin/git rev-parse refs/f20a/candidate) TREE=$(/usr/bin/git rev-parse refs/f20a/candidate^{tree})"</automated>
    <automated>SEALED=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse refs/f20a/candidate); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch --all --prune &amp;&amp; git checkout --detach $SEALED &amp;&amp; git rev-parse HEAD &amp;&amp; git status --porcelain &amp;&amp; cargo build --locked --workspace --all-features &amp;&amp; cargo nextest run --profile ci --no-fail-fast" 2&gt;&amp;1 | tail -40</automated>
    <automated>SEALED=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse refs/f20a/candidate); ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "git fetch --all --prune 2>&amp;1"; cmd /c "git checkout --detach '"$SEALED"' 2>&amp;1"; cmd /c "git rev-parse HEAD"; cmd /c "git status --porcelain" }' | grep -v CLIXML | grep -v "^&lt;Objs"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; for t in windows-retained-handle windows-appcontainer-acl windows-job-object windows-public-dispatch windows-hard-process-containment windows-f20-lifecycle; do test "$(/usr/bin/grep -cF "$t" scripts/f20-native-uat-proof.mjs)" -ge "1" || exit 1; done; echo "six windows targets present"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; for t in macos-retained-directory macos-process-tree macos-docker-reject-path-replacement macos-docker-roundtrip-delete macos-public-dispatch macos-docker-cancellation macos-docker-budget macos-f20-lifecycle; do test "$(/usr/bin/grep -cF "$t" scripts/f20-native-uat-proof.mjs)" -ge "1" || exit 1; done; echo "eight macos targets present"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -c "^    @{ id = '" scripts/f20-native-windows-proof.ps1)" = "6" &amp;&amp; /usr/bin/grep -o "id = '[a-z0-9-]*'" scripts/f20-native-windows-proof.ps1 | /usr/bin/sed "s/id = //;s/'//g" | tr '\n' ',' | /usr/bin/grep -qxF 'windows-retained-handle,windows-appcontainer-acl,windows-job-object,windows-public-dispatch,windows-hard-process-containment,windows-f20-lifecycle,' &amp;&amp; echo "target array is EXACTLY the canonical six in canonical order"</automated>
    <automated>ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; $env:WAYLAND_SANDBOX_LIVE_WINDOWS='"'"'1'"'"'; cmd /c "echo LIVEFLAG=%WAYLAND_SANDBOX_LIVE_WINDOWS%"; powershell -NoProfile -File scripts\f20-native-windows-proof.ps1 2>&amp;1 }' | grep -v CLIXML | grep -v "^&lt;Objs" | tail -60</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -f .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md &amp;&amp; SEALED=$(/usr/bin/git rev-parse refs/f20a/candidate) &amp;&amp; test "$(/usr/bin/grep -cF "$SEALED" .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md)" -ge "1" &amp;&amp; echo "sealed SHA recorded and pinned: $SEALED"</automated>
  </verify>
  <done>The upstream go/no-go was answered first, and the plan either stopped as blocked-upstream or proceeded. One exact SHA and tree are sealed, recorded in `20A-04-CANDIDATE.md` and pinned to `refs/f20a/candidate`, so no later planning commit can drift it. Every build host is confirmed pristine and detached on the sealed SHA. The aggregate Hetzner proof ran against it with actual counts recorded and compared against the immediately preceding run rather than a historical figure. `scripts/f20-native-windows-proof.ps1` was run END TO END on SEANDESKTOP at the sealed SHA with the live-acceptance flag proven effective, and ALL SIX targets went green with markers in canonical order — anything less terminated the plan as blocked-upstream. The target array is exactly the canonical six in canonical order. The eight macOS target ids are confirmed unchanged, the wrong-OS anti-drift guard is intact and unmodified, and the candidate Windows job is confirmed to target an AppContainer-capable self-hosted msvc runner. Per-requirement code presence at the sealed SHA is recorded for r1, r2, r5 and r7. No requirement is completed.</done>
</task>

<task type="auto">
  <name>Task 2: Prosecute the sealed candidate with attested reviews — one schema-validated artifact per claimed reviewer, two rounds maximum</name>
  <files>.planning/phases/20A-native-windows-macos-uat/20A-04-REVIEW/, .planning/BACKLOG.md</files>
  <read_first>.planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md (the sealed SHA and tree every review must bind to), .planning/phases/20-transactional-delegated-mutation/20-16-PLAN.md (the established review-gate shape this phase already uses, so it is followed rather than reinvented), .planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md and .planning/phases/20A-native-windows-macos-uat/20A-03-EOL-DECISION.md (the two decisions the review must prosecute, since both traded something for something)</read_first>
  <behavior>
    - Every claimed reviewer produces an on-disk, schema-validated artifact bound to the sealed SHA and tree. A reviewer with no artifact is a reviewer who did not review, and their claim does not count toward a PASS.
    - Each artifact names its reviewer, the exact SHA and tree reviewed, and every finding with a severity and a disposition.
    - The review specifically prosecutes the two decisions this phase made and the test surface it wired.
    - A CRITICAL or HIGH finding is disproved with evidence, or the plan terminates and escalates — it is never fixed inside this plan, which modifies no production source and gate-checks the tree clean. MEDIUM and below go to BACKLOG and do not block.
    - At most TWO review rounds run. A third is not permitted and escalates.
    - A self-referential review — author and reviewer identity collapsed into one executor — does not count, and a review bound to a different SHA is stale and does not count.
  </behavior>
  <action>Run the review gate against the EXACT sealed candidate, following the review-gate shape this phase already uses rather than inventing a new one.

For EVERY claimed reviewer, emit an on-disk, schema-validated review artifact under `20A-04-REVIEW/` (REQ-native-r13). This requirement exists because four claimed reviews once had no on-disk artifact at all. Each artifact must carry the reviewer identity, the exact sealed SHA and tree it reviewed, every finding with a severity of critical, high, medium or low, and an explicit disposition. A prose-only review does not count toward a PASS, and a reviewer whose artifact is missing or bound to a different SHA is treated as not having reviewed. Reject any artifact that is stale or self-referential — author and reviewer identity may not collapse into one executor.

Direct the review at the two decisions this phase actually made, because both traded something for something and both are where a defect would hide. First: does the authorized AppContainer binding mechanism genuinely preserve the anti-swap property it was authorized on, and is the admission predicate answering true because the binding is REAL rather than because it was made to say so? Second: does the authorized end-of-line reconciliation genuinely preserve the hostile-config defense, and is whatever it cost recorded where a maintainer will see it? Third, prosecute the newly wired test surface from 20A-01: are the ten previously orphaned ACL tests genuinely selected AND genuinely executing, or are they selected and silently skipping because a live-acceptance flag did not take effect.

Apply the two amended rules exactly. Findings at CRITICAL or HIGH must be fixed or disproved. MEDIUM and below are logged to BACKLOG and DO NOT BLOCK execution. Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean.

A CRITICAL OR HIGH REVIEW FINDING IS TERMINATION STATE 4, NOT AN IN-PLAN FIX (H2). This plan modifies no production source: its `files_modified` contains no `crates/` path, its scope fence forbids production changes, and three of its gates assert the source tree is clean. So it has no legal way to land a code fix, and pretending otherwise would either violate its own fence or produce a silent, ungated edit. If a review surfaces a CRITICAL or HIGH finding: STOP, record it with its severity and evidence, leave every requirement incomplete, and escalate to Sean. The repair belongs to whichever plan owns that surface — 20A-02 for the bind, 20A-03 for the reconciliation, 20A-01 for the wiring — under a fresh candidate and a fresh authorization. MEDIUM and below go to BACKLOG and do not block, exactly as the amended rules say.

The two-round cap therefore bounds REVIEW rounds, not fix-and-re-seal cycles: a second round exists only to re-review after new evidence or a corrected reviewer set, never to re-review a mutated candidate. Each fix would invalidate the prior round's evidence anyway, and an uncapped loop is how a phase reaches seventy-four plans.

Records evidence for REQ-native-r12 and REQ-native-r13; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -d .planning/phases/20A-native-windows-macos-uat/20A-04-REVIEW &amp;&amp; ls -1 .planning/phases/20A-native-windows-macos-uat/20A-04-REVIEW/ | /usr/bin/grep -c .</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SEALED=$(/usr/bin/git rev-parse refs/f20a/candidate); for f in .planning/phases/20A-native-windows-macos-uat/20A-04-REVIEW/*; do test "$(/usr/bin/grep -cF "$SEALED" "$f")" -ge "1" || { echo "NOT BOUND TO SEALED SHA: $f"; exit 1; }; done; echo "all review artifacts bound to the sealed SHA $SEALED"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; for f in .planning/phases/20A-native-windows-macos-uat/20A-04-REVIEW/*; do test "$(/usr/bin/grep -ciE 'critical|high|medium|low' "$f")" -ge "1" || { echo "NO SEVERITY IN: $f"; exit 1; }; done; echo "every artifact carries severities"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -f .planning/BACKLOG.md</automated>
  </verify>
  <done>Every claimed reviewer has an on-disk artifact under `20A-04-REVIEW/`, each bound to the sealed SHA read from `refs/f20a/candidate` and each carrying severities and a disposition; no prose-only, stale or self-referential review counted toward the PASS. The two phase decisions and the newly wired ACL test surface were all prosecuted explicitly. Any CRITICAL or HIGH finding was disproved with evidence, or the plan terminated as blocked-upstream and escalated — none was fixed in-plan. MEDIUM and below are in BACKLOG and explicitly non-blocking. At most two review rounds ran, and a third was not attempted.</done>
</task>

<task type="auto">
  <name>Task 3: Prepare ONE idempotent, candidate-specific dispatch tuple — and do not fire it</name>
  <files>.planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md</files>
  <read_first>.github/workflows/nightly-windows-soak.yml (the candidate-mode dispatch inputs, what each one is for, and which jobs candidate mode actually runs), scripts/f20-native-windows-proof.ps1 (the six-target array, the live-acceptance flag it sets, and the anti-drift guard), scripts/f20-native-macos-proof.sh (the eight-target array and its guard), .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md (the sealed SHA, tree and review status the tuple must bind to)</read_first>
  <behavior>
    - The prepared tuple is bound to the exact sealed SHA and tree and to nothing else, and it is idempotent — preparing it twice yields the same tuple and mutates nothing external.
    - The exact command Sean will run is written out in full and ready to paste, with every input's value resolved rather than left as a placeholder.
    - The macOS runner label is resolved to the actual pinned value, or the tuple explicitly records that it is unresolved and why — an unresolved label is a reason to stop, not a blank to leave empty and hope.
    - No external mutation occurs: no workflow is dispatched, no ref is pushed to main, no issue is touched, no tag or release is created.
    - The tuple records that every prior authorization digest is spent and void, so nobody reuses one.
    - The plan is ready to stop here if Sean is not present.
  </behavior>
  <action>Prepare exactly ONE candidate-specific dispatch tuple and persist it in `20A-04-CANDIDATE.md`. It must be idempotent: preparing it twice yields the same tuple and mutates nothing outside the planning directory.

Resolve every input the candidate-mode dispatch takes, and write the resulting command out in full so it can be pasted without editing: the account switch that must precede every operation against the org; the workflow to run and the exact ref that resolves to the sealed SHA; the candidate-mode flag; the request nonce, bound to the sealed SHA so the tuple cannot be reused for a different candidate; and the pinned macOS runner label. Resolve that label to its actual value — if it cannot be resolved, record why and treat it as a reason to stop rather than a blank to leave empty, because an unresolvable macOS runner means Success Criterion 2 cannot be met by this dispatch.

Record alongside the tuple: the six Windows target ids and the eight macOS target ids the run will prove; which of them are already green on the certified runner and must remain so; which are expected to change state as a result of this phase's repairs; the review status from Task 2 with the artifact count; and the aggregate Linux counts from Task 1. Record explicitly that every prior authorization digest is spent and void, and that any RED or any new candidate SHA requires a fresh authorization bound to a new tuple.

Confirm and record that the candidate-mode Windows job targets an AppContainer-capable self-hosted msvc runner (REQ-native-r11), and that the live-acceptance flag the proof script sets is set in the trap-safe form — a trailing space there would skip every acceptance test and hand back a vacuously green six-target run, which is the single most dangerous failure mode in this plan.

DO NOT DISPATCH. Do not run the workflow, do not push to main, do not open a PR, do not tag, do not release, do not close an issue. Preparing is not authorizing.

Records evidence for REQ-native-r11 and REQ-native-r12; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SEALED=$(/usr/bin/git rev-parse refs/f20a/candidate); test "$(/usr/bin/grep -cF "$SEALED" .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md)" -ge "1"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'nightly-windows-soak.yml' .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md)" -ge "1" &amp;&amp; test "$(/usr/bin/grep -cF 'f20_request_nonce' .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md)" -ge "1" &amp;&amp; test "$(/usr/bin/grep -cF 'f20_macos_runner_label' .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md)" -ge "1"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; gh auth switch --user FerroxLabs &gt;/dev/null 2&gt;&amp;1; gh run list -R FerroxLabs/wayland-core --workflow nightly-windows-soak.yml --limit 5</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git status --porcelain -- crates/ scripts/ .github/</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git log origin/main..HEAD --oneline | /usr/bin/grep -c . &amp;&amp; /usr/bin/git rev-parse --abbrev-ref HEAD</automated>
  </verify>
  <done>One idempotent tuple is persisted in `20A-04-CANDIDATE.md`, bound to the sealed SHA and tree, with every dispatch input resolved to an actual value and the full command ready to paste. The macOS runner label is resolved or its unresolvability is recorded as a stop condition. The six Windows and eight macOS target ids are recorded with their expected state changes, alongside the review artifact count and the aggregate Linux counts. Every prior authorization digest is recorded as spent and void. No workflow was dispatched, nothing was pushed to main, no PR, tag, release or issue action occurred, and the source tree is unmodified.</done>
</task>

<task type="checkpoint:human-action" gate="blocking">
  <name>Task 3b (BLOCKING SEAN GATE): Authorize and fire the native proof dispatch — executor must not fire it</name>
  <files>.planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md</files>
  <action>Hand the prepared tuple to Sean and STOP. The native proof dispatch is a Sean-only action under PRD D6: the authorization IS the human decision and it cannot be automated or inferred. The executor does not run the dispatch command, does not poll in a loop waiting for one, and does not re-seal while waiting. If Sean declines or does not respond, Task 4 records termination state 2 — the tuple stays prepared and idempotent and every requirement stays incomplete. If Sean fires it, record the run id and url verbatim in `20A-04-CANDIDATE.md` and proceed to Task 4.</action>
  <what-built>
One exact candidate is sealed at a recorded SHA and tree, with per-host pristine confirmations, a lockfile-pinned build, and an aggregate Linux proof bound to it. Every claimed reviewer has an on-disk, schema-validated review artifact bound to that same SHA, with CRITICAL and HIGH findings fixed or disproved and MEDIUM-and-below routed to BACKLOG. One idempotent, candidate-specific dispatch tuple is prepared and persisted in `20A-04-CANDIDATE.md`, with every input resolved and the full command ready to paste. Nothing has been dispatched.
  </what-built>
  <how-to-verify>
The native proof dispatch is a Sean-only action (PRD D6) — it cannot be automated, because the authorization IS the human decision. To authorize and fire:

1. Read `.planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md`. Confirm the sealed SHA and tree, the aggregate Linux counts, the review artifact count, and the resolved macOS runner label.
2. Confirm the request nonce is bound to THIS sealed SHA. Every prior authorization digest is spent and void; a reused digest would bind this phase's evidence to a different candidate and defeat Success Criterion 3.
3. Run the prepared command exactly as written in that file — switch to the FerroxLabs account first, then dispatch the soak workflow in candidate mode against the sealed ref, with the candidate flag set, the nonce bound to the sealed SHA, and the resolved macOS runner label supplied.
4. Report back the run id and url.

Expected on success: all six Windows targets and all eight macOS targets pass, each emitting its per-target marker bound to the sealed commit, tree and nonce. Expected on failure: at least one target reports a failure, and every requirement stays incomplete.

To decline: say so, and the plan records the explicit incomplete disposition and stops. Declining is a legitimate outcome — a prepared tuple that is never fired costs nothing, while a dispatch against a candidate that is not ready spends an authorization for nothing.
  </how-to-verify>
  <resume-signal>Report the dispatched run id and url, or say "decline" to stop with the tuple prepared and every requirement left incomplete.</resume-signal>
  <verify>
    <human-check>Either a run id and url are recorded in `20A-04-CANDIDATE.md` and that run's head SHA equals the value in `refs/f20a/candidate`, or a decline is recorded. No dispatch was fired by the executor.</human-check>
  </verify>
  <done>Sean has either authorized and fired the dispatch — with the run id, url and head SHA recorded and the head SHA matching the sealed candidate — or declined, in which case the tuple stays prepared and idempotent and every requirement stays incomplete.</done>
</task>

<task type="auto">
  <name>Task 4: Bind the authorized run's evidence to the sealed candidate — complete the requirements, or record the explicit incomplete disposition</name>
  <files>.planning/REQUIREMENTS.md, .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md</files>
  <read_first>.planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md (the sealed SHA, tree and nonce every marker must match), scripts/f20-native-uat-proof.mjs (the per-target marker format and the verification the proof harness itself performs), .planning/REQUIREMENTS.md (the native requirement list and the traceability table)</read_first>
  <behavior>
    - Evidence is accepted ONLY from the one newly dispatched, Sean-authorized run. Never from source inspection, never from cross-compilation, never from the Linux proof, and never from a reused or prior run.
    - Every per-target marker is checked to bind to the sealed commit, the sealed tree and the authorized nonce. A marker that does not match all three is not evidence for this candidate.
    - All six Windows targets and all eight macOS targets must be present and passing with complete markers. A missing target is a failure, not an absence.
    - A green run completes REQ-native-r1..r15 bound to that run; anything else leaves EVERY requirement incomplete with an explicit written disposition naming each failure with its output.
    - No re-seal, no re-dispatch, no selector narrowing, and no requirement completed on partial evidence.
  </behavior>
  <action>If the checkpoint was declined, or no run was dispatched, this task records termination state 2: write the explicit incomplete disposition leaving every requirement incomplete, note that the tuple remains prepared and idempotent, and stop. Do not poll in a loop and do not re-seal while waiting.

Otherwise, retrieve the authorized run's logs and verify the evidence binds to the sealed candidate. Check EVERY per-target marker against three things at once: the sealed commit, the sealed tree, and the authorized nonce. A marker matching only some of them is not evidence for this candidate. Confirm all six Windows target ids and all eight macOS target ids are present with complete markers — a target that is absent from the log is a FAILURE, not an absence, because a target that never ran proves nothing and a silently dropped target is exactly what the anti-drift guard exists to prevent.

Confirm the run is NEW: its dispatch time is after the tuple was prepared, and its id is not one previously used for any candidate. Success Criterion 3 requires native evidence bound to one newly dispatched, Sean-authorized run, so reusing a run defeats the criterion no matter how green it is.

If all fourteen targets pass with complete markers bound to the sealed commit, tree and nonce, complete REQ-native-r1 through REQ-native-r15 in `.planning/REQUIREMENTS.md`, record the completion SHA and the run id against each, and update the traceability table so the native requirements point at Phase 20A rather than at Phase 20's native path.

If ANY target is red, absent, or bound to a different commit, tree or nonce, EVERY requirement stays incomplete and an explicit written disposition names each failure with its output. Do NOT re-seal, do NOT re-dispatch, do NOT narrow a selector, and do NOT complete a subset of the requirements on partial evidence. A new candidate needs a new plan and a fresh authorization. That is termination state 3 and it is the correct outcome.

Implements REQ-native-r12; completes REQ-native-r1..r15 on green evidence only.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; gh auth switch --user FerroxLabs &gt;/dev/null 2&gt;&amp;1; gh run list -R FerroxLabs/wayland-core --workflow nightly-windows-soak.yml --limit 3 --json databaseId,headSha,conclusion,createdAt</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SEALED=$(/usr/bin/git rev-parse refs/f20a/candidate); test "$(/usr/bin/grep -cF "$SEALED" .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md)" -ge "1" &amp;&amp; echo "evidence is being bound to the sealed value $SEALED, not to live HEAD"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cE 'REQ-native-r(1|15)\b' .planning/REQUIREMENTS.md)" -ge "1"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -ciE 'incomplete disposition|Complete @' .planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md)" -ge "1"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git status --porcelain -- crates/ scripts/ .github/</automated>
  </verify>
  <done>Either every one of the six Windows and eight macOS targets passed with complete markers bound to the sealed commit, tree and authorized nonce from ONE newly dispatched run — in which case REQ-native-r1..r15 are completed with the SHA and run id recorded and the traceability table updated — or an explicit incomplete disposition names every failing, absent or mismatched target with its output and leaves EVERY requirement incomplete. No evidence was accepted from source, cross-compilation, the Linux proof or a reused run. Nothing was re-sealed, re-dispatched or narrowed, and no requirement was completed on partial evidence.</done>
</task>

</tasks>

## What this plan does NOT change (scope fence)

- **The native target map and the wrong-OS anti-drift guard — verified, never modified.** The six Windows and eight macOS target ids are checked for drift, the Windows target array is gate-checked for EXACTLY six entries in canonical order (absence of a seventh, not merely presence of the six), and the guard is checked for presence; none of it is edited. Narrowing a selector so it stops selecting a failing test is the forbidden move this guard exists to prevent, and 20A-01 was fenced entirely off this file to keep the array intact.
- **No production source is fixed here, including in response to a review finding.** A CRITICAL or HIGH review finding terminates the plan and escalates; the repair belongs to the plan that owns the surface, under a fresh candidate and a fresh authorization. This plan has no `crates/` path in `files_modified` and gate-checks the tree clean three times, so an in-plan fix would be an ungated edit against its own fence.
- **The proof scripts' assertions, timeouts and gates.** Nothing in them is relaxed to reach a green.
- **The AppContainer bind — 20A-02 owns it.** If that plan escalated, this plan stops at the go/no-go rather than dispatching a candidate known to be red.
- **The end-of-line reconciliation — 20A-03 owns it.** This plan reviews the decision; it does not revisit it.
- **The CI wiring and the soak crate list — 20A-01 owns them.** This plan cites 20A-01's macOS CI result as evidence; it does not re-wire anything.
- **Phase 20.** It closed green at `01a5b0ae`. Its requirements are complete and are not reopened, re-proven or renegotiated here.
- **Every other Sean gate.** No push to main, no merge, no PR, no tag, no release, no deployment, no canary promotion, no issue closure, and no deletion of a retained candidate UAT evidence ref. The work branch is pushed so hosts can fetch it, and that is all.
- **No production source is modified by this plan at all.** Its `files_modified` are planning artifacts only, and the source tree is gate-checked unmodified at three points.
- **No re-seal after a red, and no third review round.** Both are how Phase 20 reached seventy-four plans; both terminate and escalate here.

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| requirement completion ← Sean-authorized run | Completion is gated by one exact-tuple authorization and by evidence from that one newly dispatched run; every prior digest is spent and void |
| native evidence ← exact candidate identity | A marker is evidence only if it binds to the sealed commit AND tree AND authorized nonce; any looser match lets a different candidate's run be claimed as this one's |
| six-target proof ← target selection | A narrowed selector, a dropped target or a wrong-OS mapping turns a proof into a ceremony that proves nothing |
| acceptance test execution ← live-acceptance flag | If the flag does not take effect, every acceptance test skips and the run is vacuously green — the single most dangerous failure in this plan |
| review PASS ← on-disk attestation | A claimed reviewer with no artifact is a reviewer who did not review; four such claims have already occurred |
| candidate ← pristine tree | A candidate sealed from a tree carrying a diagnostic edit is not the product that was reviewed |

## STRIDE Threat Register

| Threat ID | Category | Component | Severity | Disposition | Mitigation Plan |
|-----------|----------|-----------|----------|-------------|-----------------|
| T-20A-04-01 | Spoofing | Native evidence is inferred from source, cross-compilation, the Linux proof or a REUSED run, defeating Success Criterion 3 outright | critical | mitigate | Task 4 accepts evidence only from one newly dispatched authorized run, checks each marker against the sealed commit AND tree AND nonce, and confirms the run is new by dispatch time and id |
| T-20A-04-02 | Spoofing | A vacuously green six-target run: the live-acceptance flag is set with the trailing-space form, every acceptance test skips, and the proof reports success having proven nothing | critical | mitigate | The trap-safe assignment form is mandated and the flag is confirmed and recorded at tuple-preparation time; Task 2's review explicitly prosecutes whether the wired ACL tests are executing rather than silently skipping |
| T-20A-04-03 | Elevation of Privilege | A spent or reused authorization digest is accepted, binding this phase's evidence to a different candidate | high | mitigate | The tuple records every prior digest as spent and void; the nonce is bound to the sealed SHA; any RED or new SHA requires a fresh authorization bound to a new tuple |
| T-20A-04-04 | Tampering | A native target selector is narrowed, or a target silently dropped, so a failing test stops being selected | high | mitigate | The Windows target array is gate-checked for EXACTLY six entries in canonical order — absence of a seventh, not merely presence of the six — the eight macOS ids are checked present, the wrong-OS anti-drift guard is verified intact and never edited, and an ABSENT target is treated as a FAILURE rather than an absence |
| T-20A-04-12 | Spoofing | The candidate is sealed and a Sean authorization spent on an INFERENCE: the upstream suites are green but the six-target proof was never run end to end, and the two are not the same evidence — `windows-f20-lifecycle` selects the lifecycle test WITHOUT the ignored set while every upstream measurement includes it, and marker emission and ordering are exercised nowhere upstream | critical | mitigate | Task 1 requires a MANDATORY end-to-end run of the six-target proof script on SEANDESKTOP — the same self-hosted msvc runner class the certified leg uses — at the sealed SHA with the live-acceptance flag proven effective, and makes all six green locally the go/no-go precondition; anything less is termination state 4 and no authorization is spent |
| T-20A-04-13 | Tampering | The seal drifts under its own plan: the SHA is captured as live `HEAD`, then Tasks 2-4 commit planning artifacts and every downstream gate recomputes `HEAD` — so gates fail for the wrong reason or silently pass against a candidate that no longer matches the markers, which bind commit AND tree | high | mitigate | The seal is pinned once to `refs/f20a/candidate` in Task 1's first gate and recorded in `20A-04-CANDIDATE.md`; every downstream gate reads that ref and never recomputes `HEAD`; the source tree's invariance across the plan's own commits is itself gate-checked |
| T-20A-04-14 | Repudiation | A requirement whose code fix predates this candidate is reported as completed BY this run, so the completion record misattributes what the native proof actually delivered | medium | mitigate | Task 1 records one line per requirement stating whether its named code fix is PRESENT at the sealed SHA with the evidence; a requirement whose fix is absent and unproven is a finding rather than a completion |
| T-20A-04-05 | Repudiation | A claimed review with no on-disk artifact counts toward a PASS, exactly as happened four times before | high | mitigate | REQ-native-r13 is enforced mechanically: every artifact must exist on disk, bind to the sealed SHA and carry severities; stale, prose-only and self-referential reviews are rejected and gate-checked |
| T-20A-04-06 | Spoofing | The candidate is sealed from a tainted tree, so what is proven is not what was reviewed | high | mitigate | REQ-native-r15 is enforced per host — Mac, Hetzner and the Windows box are each confirmed pristine before the candidate build, and the source tree is gate-checked unmodified at three points |
| T-20A-04-07 | Elevation of Privilege | The Sean-gated dispatch is fired by the executor rather than by Sean, or another gated action (push to main, merge, release, issue closure) is taken alongside it | high | mitigate | The dispatch is a blocking human-action checkpoint; the plan prepares and stops; the gated-action list is restated in the execution rules and the branch state is gate-checked |
| T-20A-04-08 | Denial of Service | Scope metastasis — a red run triggers a re-seal, a third review round, or a new dispatch, and the phase grows without bound as Phase 20 did to 74 plans | high | mitigate | The termination criterion caps the plan at ONE seal, TWO review rounds and ONE tuple, with four defined exit states; a red terminates rather than re-seals; a third round is not permitted and escalates to Sean |
| T-20A-04-09 | Tampering | A subset of the requirements is completed on partial evidence, so the phase reads closed while some targets never passed | high | mitigate | Completion requires all six Windows and all eight macOS targets green with complete markers bound to the sealed identity; anything else leaves EVERY requirement incomplete with a named disposition |
| T-20A-04-10 | Denial of Service | The macOS runner label cannot be resolved, so the macOS leg silently does not run and Success Criterion 2 goes unmet while the Windows leg reports green | medium | mitigate | An unresolvable label is recorded as a stop condition at tuple preparation rather than left blank; Task 4 treats an absent macOS target as a failure |
| T-20A-04-11 | Repudiation | The aggregate Linux counts are compared against a historical figure rather than the immediately preceding run, hiding a real regression behind a familiar number | medium | mitigate | Task 1 forbids asserting a historical expected total and requires comparison against the immediately preceding run on this branch with any divergence explained |
| T-20A-04-SC | Tampering | npm/pip/cargo installs | low | accept | No dependency is added, removed or updated; no `Cargo.toml` change; `Cargo.lock` consistency is verified by a pinned build, not modified; no install task exists in this plan |
</threat_model>

<verification>
Local gates (Mac, source level only — the Mac cannot compile this workspace): `cargo fmt --all -- --check` clean and the source tree gate-checked unmodified across `crates/`, `scripts/` and `.github/` at Task 1, Task 3 and Task 4; the sealed SHA and tree recorded; the six Windows and eight macOS target ids all present in the shared canonical map; `20A-04-CANDIDATE.md` carrying the sealed SHA, the resolved dispatch inputs and either a completion or an explicit incomplete disposition; every file under `20A-04-REVIEW/` bound to the sealed SHA and carrying severities.

Authoritative gates: Hetzner runs `cargo build --locked --workspace --all-features` plus `cargo nextest run --profile ci --no-fail-fast` against the sealed SHA with actual counts recorded and compared to the immediately preceding run; both build hosts print the sealed SHA back and report a clean status; the candidate Windows job is confirmed to target an AppContainer-capable self-hosted msvc runner. The DECISIVE gate is the Sean-authorized native run: all six Windows targets and all eight macOS targets passing with complete per-target markers bound to the sealed commit, tree and authorized nonce, from ONE newly dispatched run.

Known unknowns to record, not to resolve here: whether the ephemeral macOS image the pinned label resolves to matches the one the harness was last validated against; whether any target's runtime on the certified runner has drifted enough to approach a workflow timeout; and whether any MEDIUM-or-below finding routed to BACKLOG in this phase would have been a HIGH under a different threat model — that reclassification is Sean's call, not this plan's.
</verification>

<success_criteria>
- The upstream go/no-go was answered before sealing, so no Sean authorization was spent on a candidate known to be red (termination state 4 available and honored).
- The six-target proof script was run END TO END on SEANDESKTOP at the sealed SHA with the live-acceptance flag proven effective, and ALL SIX went green before any dispatch was prepared — closing the gap between the upstream suites' test selection and what the proof actually runs, and covering marker emission and ordering that nothing upstream touches.
- ONE exact candidate is sealed with its SHA and tree, pinned to `refs/f20a/candidate` so the plan's own later commits cannot drift it, every build host confirmed pristine (REQ-native-r15), a lockfile-pinned build, and an aggregate Linux proof whose counts are compared against the immediately preceding run rather than a historical figure (REQ-native-r4).
- The Windows target array is gate-checked for exactly six entries in canonical order and the eight macOS ids are confirmed unchanged; the wrong-OS anti-drift guard is intact and unmodified; the certified Windows leg targets an AppContainer-capable self-hosted msvc runner (REQ-native-r11); the macOS harness's re-validation status is recorded (REQ-native-r9).
- Per-requirement code presence at the sealed SHA is recorded for r1, r2, r5 and r7, so no requirement is misattributed to a run that did not deliver it.
- Every claimed reviewer has an on-disk, schema-validated artifact bound to the sealed SHA carrying severities and a disposition; no prose-only, stale or self-referential review counted (REQ-native-r13).
- CRITICAL and HIGH review findings are disproved with evidence or terminate the plan and escalate — never fixed in-plan, since this plan modifies no production source; MEDIUM and below are in BACKLOG and explicitly non-blocking; at most two review rounds ran and a third was not attempted.
- ONE idempotent, candidate-specific dispatch tuple is prepared with every input resolved and the full command ready to paste, and the dispatch was NOT fired by this plan — it stopped at the Sean gate (PRD D6).
- On authorization, native evidence is accepted only from that one newly dispatched run, with every marker bound to the sealed commit, tree and nonce, and an absent target treated as a failure (REQ-native-r12).
- REQ-native-r1..r15 are completed and bound to that run, or an explicit incomplete disposition names every failure and leaves EVERY requirement incomplete — with no re-seal, no re-dispatch, no selector narrowing and no partial completion.
- No production source was modified, and no Sean gate other than the prepared-and-authorized dispatch was touched.
</success_criteria>

## Artifacts this phase produces
- `.planning/phases/20A-native-windows-macos-uat/20A-04-CANDIDATE.md` — the sealed SHA and tree, the per-host pristine confirmations, the aggregate Linux counts, the verified target map and runner, the prepared idempotent dispatch tuple, and the completion or explicit incomplete disposition.
- `.planning/phases/20A-native-windows-macos-uat/20A-04-REVIEW/` — one schema-validated review artifact per claimed reviewer, bound to the sealed candidate.
- `.planning/REQUIREMENTS.md` — REQ-native-r1..r15 completed and bound to the authorized run, or left incomplete with a named disposition.
- `20A-04-SUMMARY.md` recording the seal, the reviews, the prepared tuple, the authorization outcome and the phase's closing position.

<output>
Create `.planning/phases/20A-native-windows-macos-uat/20A-04-SUMMARY.md` using the standard GSD summary template. Record: the upstream go/no-go and its basis; the sealed SHA and tree; the per-host pristine confirmations; the aggregate Hetzner build and test counts with the comparison against the immediately preceding run and any divergence explained; the target-map and anti-drift verification and the confirmed Windows runner; the macOS harness re-validation status; the review artifact inventory with each reviewer, its bound SHA and its findings by severity, plus the round count and what was fixed or disproved; the BACKLOG entries created; the prepared dispatch tuple with every input resolved and the note that all prior digests are spent and void; the checkpoint outcome; if a run was dispatched, its id, url, per-target markers and the commit/tree/nonce each one bound to; the requirement completion bound to that run OR the explicit incomplete disposition naming every failing, absent or mismatched target; the recorded unknowns; and which of the four termination states the plan ended in.
</output>
