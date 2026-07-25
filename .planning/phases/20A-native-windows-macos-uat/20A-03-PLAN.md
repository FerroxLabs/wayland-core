---
phase: 20A-native-windows-macos-uat
plan: "03"
type: execute
wave: 2
depends_on:
  - "20A-01"
files_modified:
  - crates/wcore-swarm/src/worktree_cleanup.rs
  - crates/wcore-swarm/src/worktree/parent.rs
  - crates/wcore-swarm/src/worktree_tests.rs
  - .gitattributes
  - .planning/phases/20A-native-windows-macos-uat/20A-03-EOL-DECISION.md
autonomous: false
requirements:
  - REQ-native-r4
must_haves:
  truths:
    - "THE REPORTED SYMPTOM, measured single-variable this session: the landing reads EVERY normal Windows checkout as dirty. Git for Windows' system default is `core.autocrlf=true`, so a clone lands a CRLF worktree against an LF index; the landing's deliberate `core.autocrlf=false` security scrub then reports ` M README.md`. This fires for ANY real user, not only for fixtures. That makes it a product defect on the platform, not a test artifact."
    - "THE SCRUB IS DELIBERATE AND SECURITY-BEARING, so it cannot simply be removed. `crates/wcore-swarm/src/worktree_cleanup.rs` builds every swarm git invocation with `-c core.hooksPath=<disabled>`, `-c core.fsmonitor=false` and `-c core.autocrlf=false`, plus `GIT_CONFIG_NOSYSTEM=1`, empty system and global config files, `GIT_ATTR_NOSYSTEM=1`, terminal prompts disabled and every ambient `GIT_*` override removed. It is one coherent hostile-config defense, and `crates/wcore-swarm/src/worktree/candidate.rs` carries the matching deny-by-default config allowlist which explicitly admits `autocrlf` and `eol` as benign core keys. Any change here is a change to that defense."
    - "THE BRIEF'S STATED MECHANISM IS CONTRADICTED BY THE REPOSITORY AND MUST BE RE-ESTABLISHED BEFORE IT IS FIXED. This repo HAS a committed `.gitattributes` whose first rule is `* text=auto eol=lf`, with belt-and-suspenders `text eol=lf` entries for `*.rs`, `*.toml`, `*.md`, `*.yml`, `*.yaml`, `*.snap` and the vendored snapshot tree. An in-tree `eol=lf` attribute OVERRIDES `core.autocrlf` on checkout, and `text=auto` normalizes on the clean side, so on the stated mechanism alone a clone with `autocrlf=true` should still land LF and `git status` should still read clean. `GIT_ATTR_NOSYSTEM=1` disables only the SYSTEM attributes file, not the in-tree one. So EITHER the checkout predates the attributes rule and was never renormalized, OR the attributes file is not in effect on that checkout for a reason not yet identified, OR the real mechanism is something else. The plan must determine WHICH before it changes anything."
    - "THIS IS A DESIGN DECISION, NOT A BUG FIX. Reconciling a security scrub that forces deterministic, attacker-independent checkout behavior with end-of-line semantics that every Windows user's environment perturbs is a genuine product decision with user-visible consequences. It must be DECIDED explicitly at a blocking checkpoint, not resolved by a silent workaround in a git argument list."
    - "A SILENT WORKAROUND IS THE FAILURE MODE TO AVOID. Dropping `core.autocrlf=false` to make the dirty check pass would restore attacker influence over checkout content representation inside the exact code path that decides whether a delegated mutation may land. Loosening the dirty check to ignore whitespace-only differences would blind the landing to a real class of modification. Both are cheap and both are wrong; if either is chosen it must be chosen KNOWINGLY at the checkpoint, with its cost stated."
    - "A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, `#[ignore]`, `#[allow]`, raise a global timeout, or delete an inconvenient test to reach a gate. If the honest outcome of this plan is that the premise was refuted and there is no defect, that is a complete and successful result — record it and close."
  artifacts:
    - path: .planning/phases/20A-native-windows-macos-uat/20A-03-EOL-DECISION.md
      provides: "The single-variable determination of the ACTUAL mechanism, the reconciliation options with their security and user-experience costs, and the recorded decision"
    - path: crates/wcore-swarm/src/worktree_cleanup.rs
      provides: "The authorized reconciliation applied to the swarm's git invocation construction, or an untouched file plus the recorded finding that no change was warranted"
  key_links:
    - from: .planning/phases/20A-native-windows-macos-uat/20A-03-EOL-DECISION.md
      to: crates/wcore-swarm/src/worktree_cleanup.rs
      via: "the authorized reconciliation, decided before any change to the hostile-config defense"
      pattern: "decision-record"
---

<objective>
Force an explicit, recorded decision on how the landing's deterministic-checkout security scrub reconciles with Windows end-of-line semantics — after first establishing what the actual mechanism is, because the reported one is contradicted by the repository.

Purpose: The landing reads every normal Windows checkout as dirty, which fires for any real user and not only for fixtures. The stated cause is that Git for Windows defaults `core.autocrlf=true` while the scrub forces it false. But this repository commits a `.gitattributes` whose `* text=auto eol=lf` rule should override `core.autocrlf` on checkout and normalize on the clean side, which means the stated mechanism should not produce the reported symptom. Something in that chain is not what it appears, and fixing the wrong link would either leave the defect in place or weaken a security control for no benefit.
Output: A single-variable determination of the actual mechanism; a decision taken at a blocking checkpoint with each option's security and user-experience cost stated; the authorized change implemented and proven on real Windows; or a recorded finding that the premise was refuted and no change is warranted.
</objective>

<execution_context>
@/Users/seandonahoe/.codex/gsd-core/workflows/execute-plan.md
@/Users/seandonahoe/.codex/gsd-core/templates/summary.md
</execution_context>

<context>
@AGENTS.md
@.planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md
@.gitattributes
@crates/wcore-swarm/src/worktree_cleanup.rs
@crates/wcore-swarm/src/worktree/candidate.rs
@crates/wcore-swarm/src/worktree_manager.rs
@crates/wcore-swarm/src/worktree/parent.rs
</context>

<execution_rules>

**THE TWO AMENDED PHASE RULES — stated verbatim, and they bound this plan.**

- Findings at CRITICAL or HIGH must be fixed or disproved. MEDIUM and below are logged to BACKLOG and DO NOT BLOCK execution.
- Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean.

**TERMINATION CRITERION FOR THIS PLAN (hard — the plan STOPS and escalates rather than spawning more work).** This plan determines the mechanism ONCE, takes ONE decision, and implements it ONCE. It terminates in exactly one of four states, and in all four it writes its SUMMARY and stops:
1. **Complete** — the mechanism was determined, a reconciliation was authorized at the checkpoint, implemented, and proven on real Windows with no Linux or macOS regression.
2. **Premise refuted, no defect** — the determination shows a normal Windows checkout does NOT read dirty at this SHA. STOP. Record the refutation with its evidence, make NO code change, and close. This is a complete and successful outcome, not a failure to find something.
3. **Premise refuted, different defect** — a real dirty-checkout defect exists but has a different cause than the stated one. Record it with severity. If it is CRITICAL or HIGH and the fix is confined to this plan's declared files, proceed to the checkpoint with the corrected options. If it is not, or the fix reaches outside those files, STOP and escalate to Sean.
4. **Escalated** — every reconciliation option costs a security guarantee, and Sean declines all of them at the checkpoint. STOP. Ship nothing and record the open decision.
Under no circumstances does this plan spawn additional plans, extend its own task list, or start a second determine/decide cycle.

**SCOPE BOUNDARY (hard).** This plan touches only the end-of-line reconciliation. The AppContainer retained-workspace-authority bind is 20A-02's, the CI wiring is 20A-01's, and the sealed candidate and native dispatch are 20A-04's. If the determination leads toward any of those surfaces, record the connection and stop rather than following it.

**NON-NEGOTIABLE.** A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, `#[ignore]`, `#[allow]`, raise a global timeout, or delete an inconvenient test to reach a gate. Four executors in Phase 20 correctly stopped and escalated rather than improvise, and every one of those calls was right. Here the specific temptation is to loosen the dirty check until it stops complaining; that is an engineered green over a real signal and it is forbidden.

**SINGLE-VARIABLE DISCIPLINE (hard).** The determination in Task 1 changes ONE thing at a time and records the observation after each. A determination that changes the clone, the config and the attributes together proves nothing about which one mattered, and it is exactly how the stated mechanism came to be believed without being isolated.

**ENVIRONMENT.**
- Windows: `ssh -o BatchMode=yes SeanD@seandesktop` (Tailscale), checkout `C:\ferrox-win`. Invocation shape: `ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "<cmd> 2>&1" }'`, piped through `grep -v CLIXML | grep -v "^<Objs"`. Git on the box MUST be wrapped `cmd /c "git ..."` — PowerShell's Stop preference treats git's stderr chatter as fatal, and this plan runs a great deal of git. `cargo fmt --all` FAILS there with os error 206; `justfile:96-98` already skips fmt-check on Windows.
- Linux: `ssh -o BatchMode=yes hetzner-dsm`, `/root/wayland`. Used here to prove no Linux regression.
- Mac CANNOT compile this workspace. `cargo fmt --all` is the only working cargo command there. Use `/usr/bin/git`, and ALWAYS `/usr/bin/grep`.
- Push the WORK BRANCH to `gh` so the hosts can fetch it. NO push to main, merge, PR, tag, release, or issue closure without Sean.
- Do the determination in a SCRATCH clone on the box, never in `C:\ferrox-win`. The phase's other plans measure against that checkout and a mutated worktree there would poison their baselines.

**THE TWO MEASUREMENT TRAPS (both measured; do not simplify these away).**
1. In `cmd`, `set VAR=value && ...` appends a TRAILING SPACE to the value and Rust silently ignores it. Use `set "VAR=x"` or PowerShell `$env:VAR='x'`, and PROVE the value took effect before trusting any run that depends on it. This plan sets `GIT_*` environment variables during the determination, and a trailing space there produces a confidently wrong conclusion.
2. Mac `grep` is rtk-proxied and SILENTLY DROPS LINES — measured at 32 returned versus 674 for the same inverted match on the same file. Every gate in this plan invokes `/usr/bin/grep` explicitly and uses `-F` for literals.

**AGENTS.md discipline.** Surgical diffs only; every changed line traces to the authorized reconciliation. No drive-by refactor of the git-invocation builder or the config allowlist. Clippy-clean.

**Git hygiene.** Use `/usr/bin/git` on the Mac. Stage the exact paths in `files_modified`, never `-A`, never `.`. Never stage `AGENTS.md` or `.ijfw` churn. No `Co-Authored-By` trailers.
</execution_rules>

<tasks>

<task type="auto">
  <name>Task 1: Determine the ACTUAL mechanism single-variable, in a scratch clone, before proposing any reconciliation</name>
  <files>.planning/phases/20A-native-windows-macos-uat/20A-03-EOL-DECISION.md</files>
  <read_first>.gitattributes (all of it — note that the first rule normalizes every text file and that several extension rules restate it), crates/wcore-swarm/src/worktree_cleanup.rs (the git-invocation builder: the protected `-c` arguments it always prepends, the environment it sets and clears, and the working directory it runs in), crates/wcore-swarm/src/worktree/candidate.rs (the deny-by-default core-key allowlist and which end-of-line keys it admits as benign), crates/wcore-swarm/src/worktree_manager.rs (the dispatch-time dirty check and the integration-checkout dirty check), crates/wcore-swarm/src/worktree/parent.rs (the landing's own dirty refusal and the clean-status record it binds)</read_first>
  <behavior>
    - The reported symptom is reproduced, or shown not to reproduce, on a FRESH clone made the way a real user would make one, in a scratch location that does not disturb the phase's measurement checkout.
    - Each variable is changed alone and the observation recorded after each: the system autocrlf default, the presence and effect of the in-tree attributes file, the scrub's forced config value, and the environment the scrub sets.
    - The determination answers one question directly: does the in-tree `* text=auto eol=lf` rule take effect on this checkout, and if not, why not.
    - The determination distinguishes a fresh clone from a checkout that predates the attributes rule and was never renormalized, because those two have different fixes and only one of them is a product defect.
    - Whether the symptom reaches the LANDING specifically is established, not assumed from a bare `git status` — the landing runs its status through the scrub's own argument list and environment, which is not the same thing as a plain status.
    - The outcome is one of: the stated mechanism is confirmed; a different mechanism is identified; or no defect reproduces.
  </behavior>
  <action>Work in a SCRATCH clone on the box, never in `C:\ferrox-win`, and remove it when done. The other plans measure against that checkout and mutating it would poison their baselines.

Record the box's git configuration first: the system, global and local `core.autocrlf` and `core.eol` values as git itself reports them with their origin, so the starting conditions are on the record rather than assumed. Then make a fresh clone of the work branch the way a real user would — no special flags, no pre-set config — and immediately, before touching anything, record whether a plain status reports the tree clean or dirty, and if dirty, exactly which paths and what the byte difference is.

Now vary ONE thing at a time, recording the observation after each, and never bundling two changes into one measurement:
  - Does the in-tree attributes file take effect at all on this checkout? Ask git directly what end-of-line attributes it resolves for a representative tracked text file such as the repository README, and compare that answer against what the committed rules say it should be.
  - Does the reported dirtiness survive when the scrub's forced `core.autocrlf=false` is applied, and does it disappear when it is not? That isolates the scrub as cause or bystander.
  - Does a renormalization change the answer? If the worktree bytes disagree with the index only because the checkout predates the attributes rule, that is a stale-checkout condition rather than a defect that fires for every new user, and the distinction changes the fix entirely.
  - Does the symptom reach the LANDING, as opposed to a plain status? Reproduce the status the way the swarm's git-invocation builder does — the same forced config arguments, the same cleared and set environment, the same working directory — because that is the invocation whose verdict actually blocks a landing. Set every environment variable using the trap-safe form and prove it took effect before trusting the observation.

Then state the determination plainly as ONE of:
  - CONFIRMED — the stated mechanism reproduces for a fresh, normal clone. Proceed to the checkpoint.
  - REFUTED, NO DEFECT — a fresh normal clone reads clean through the landing's own invocation at this SHA. STOP. Record the refutation with its evidence and every observation that supports it, make no code change, and close. Note explicitly whether the box's current checkout is a stale pre-attributes tree, since that would explain the original observation without there being a product defect.
  - REFUTED, DIFFERENT CAUSE — a real defect exists with a different mechanism. Record it, assign a severity, and if it is CRITICAL or HIGH and confined to this plan's declared files, carry the corrected options into the checkpoint. Otherwise stop and escalate.

Write every observation, with the exact command and its output, into `20A-03-EOL-DECISION.md`. Remove the scratch clone. Records evidence for REQ-native-r4; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -f .planning/phases/20A-native-windows-macos-uat/20A-03-EOL-DECISION.md</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cE 'CONFIRMED|REFUTED' .planning/phases/20A-native-windows-macos-uat/20A-03-EOL-DECISION.md)" -ge "1"</automated>
    <automated>ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "git status --porcelain"; cmd /c "git rev-parse HEAD" }' | grep -v CLIXML | grep -v "^&lt;Objs"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git status --porcelain -- crates/ .gitattributes</automated>
  </verify>
  <done>The determination was made in a scratch clone that was removed afterwards, and `C:\ferrox-win` is unmodified so the other plans' baselines are intact. The box's starting git configuration is recorded with origins. Each variable was changed alone with its observation recorded, including whether the in-tree attributes rule resolves as committed and whether the symptom reaches the landing's own invocation rather than only a plain status. The determination is stated as CONFIRMED, REFUTED-NO-DEFECT or REFUTED-DIFFERENT-CAUSE with the evidence for it. No production file was modified in this task.</done>
</task>

<task type="checkpoint:decision" gate="blocking">
  <name>Task 2 (BLOCKING DECISION): Authorize the end-of-line reconciliation, or escalate</name>
  <files>.planning/phases/20A-native-windows-macos-uat/20A-03-EOL-DECISION.md</files>
  <action>Present Task 1's determination and the option costs to Sean and obtain ONE authorization. Do not change a line of the git-invocation builder, the attributes file or the dirty check before it is given. If Task 1 determined REFUTED-NO-DEFECT, this checkpoint does not run at all: record the refutation, make no code change, and close. Record the authorized option and the cost accepted with it verbatim in `20A-03-EOL-DECISION.md`. If Sean selects escalation, Task 3 does not run: write the SUMMARY recording termination state 4 and stop.</action>
  <decision>How does the landing's deterministic-checkout security scrub reconcile with Windows end-of-line semantics?</decision>
  <context>
The scrub in the swarm's git-invocation builder forces `core.autocrlf=false` alongside disabled hooks, disabled fsmonitor, emptied system and global config, and cleared ambient `GIT_*` overrides. It is one coherent defense whose purpose is that the content a delegated mutation is judged against cannot be perturbed by anything outside the repository. End-of-line handling is exactly such a perturbation, which is why it is in the list — and it is also the thing every Windows user's environment perturbs, which is why the two collide. Task 1's `20A-03-EOL-DECISION.md` carries the determination and the evidence; read it before choosing, and note that if the determination was REFUTED-NO-DEFECT this checkpoint does not run at all. Each option below costs something real; the point of this checkpoint is that the cost is chosen rather than absorbed silently.
  </context>
  <options>
    <option id="attributes-authoritative">
      <name>Make the in-tree attributes rule authoritative and prove it — keep the scrub exactly as it is, and ensure a checkout normalized by the committed rules reads clean through the landing's own invocation</name>
      <pros>Changes no security control at all; the repository already commits the rule, so this makes reality match stated intent rather than adding policy; a normalized checkout is deterministic for every user on every platform, which is what the scrub wants; the fix is a checkout-hygiene concern rather than a loosening</pros>
      <cons>Only available if Task 1 showed the attributes rule is not currently taking effect, and requires understanding why; users with an existing stale checkout need a renormalization step, which is a real migration cost that must be documented rather than assumed away</cons>
    </option>
    <option id="scrub-normalizes">
      <name>Have the scrub force deterministic end-of-line handling explicitly rather than merely disabling the ambient one — set the end-of-line policy the landing requires instead of only refusing the user's</name>
      <pros>Keeps the defense's purpose intact and strengthens it: the landing stops depending on the user's checkout being normalized and starts guaranteeing the representation it judges against; the config allowlist already admits both end-of-line keys as benign, so the surface is already accounted for</pros>
      <cons>Adds a forced value to a security-bearing argument list, which must be justified in-source and cannot be a quiet addition; must be proven not to change what the landing considers a real modification on any platform</cons>
    </option>
    <option id="relax-dirty-check">
      <name>Loosen the dirty check so end-of-line-only differences do not block a landing</name>
      <pros>Smallest possible change and it makes the symptom disappear immediately on every Windows checkout, stale or fresh</pros>
      <cons>Blinds the landing to a real class of modification: an end-of-line-only difference is still a content difference, and the check exists to refuse landing against a tree that is not what it claims to be; this is the engineered-green option and it should be chosen only with that stated plainly</cons>
    </option>
    <option id="escalate">
      <name>Escalate — every option costs a guarantee Sean is not willing to spend, so record the decision as open and ship nothing</name>
      <pros>Leaves the security defense untouched and the decision visible rather than absorbed into an implementation detail</pros>
      <cons>The dirty-checkout symptom stays, and it fires for every real Windows user, not just for fixtures</cons>
    </option>
  </options>
  <resume-signal>Select: attributes-authoritative, scrub-normalizes, relax-dirty-check, or escalate. If selecting relax-dirty-check, confirm explicitly that the loss of end-of-line-difference detection at the landing is accepted.</resume-signal>
  <verify>
    <human-check>The authorized option and the cost accepted with it are recorded verbatim in `20A-03-EOL-DECISION.md`, and the selected option is one Task 1's determination actually left available.</human-check>
  </verify>
  <done>One reconciliation is authorized and recorded with its accepted cost — or escalation was selected, or Task 1 determined REFUTED-NO-DEFECT, and Task 3 does not run.</done>
</task>

<task type="auto">
  <name>Task 3: Implement the authorized reconciliation and prove it on real Windows with no Linux or macOS regression</name>
  <files>crates/wcore-swarm/src/worktree_cleanup.rs, crates/wcore-swarm/src/worktree/parent.rs, crates/wcore-swarm/src/worktree_tests.rs, .gitattributes</files>
  <read_first>.planning/phases/20A-native-windows-macos-uat/20A-03-EOL-DECISION.md (the determination and the authorized option), crates/wcore-swarm/src/worktree_cleanup.rs (the git-invocation builder as it stands), crates/wcore-swarm/src/worktree_tests.rs (the existing status-based tests and the fixture idiom, so nothing is duplicated or displaced), .planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md (the measured baseline this task states its delta against)</read_first>
  <behavior>
    - Only the authorized option is implemented, exactly as authorized. No second option is partially applied as a hedge.
    - A fresh, normal Windows clone reads clean through the LANDING'S OWN invocation — the same forced config, the same environment, the same working directory — not merely through a plain status.
    - The hostile-config defense is intact: hooks still disabled, fsmonitor still disabled, system and global config still emptied, ambient overrides still cleared, and the deny-by-default config allowlist still deny-by-default.
    - A regression test proves the reconciliation, and it asserts the INVARIANT — that a normally-cloned tree is judged clean, or that an end-of-line difference is judged the way the authorized option says it should be — never a specific git message or exit code.
    - Linux and macOS behaviour is unchanged, proven by the aggregate suite rather than asserted.
    - If the authorized option carries a user migration cost, that cost is documented in-source where a maintainer will see it, not only in a planning artifact.
  </behavior>
  <action>Implement ONLY the option authorized at the checkpoint. If the checkpoint selected escalation, or Task 1 determined REFUTED-NO-DEFECT, this task does not run: write the SUMMARY recording that outcome and stop.

Apply the change surgically. If the authorized option adds or changes a forced git configuration value in the swarm's invocation builder, add it beside the existing protected arguments in the same style and justify it in-source: state what perturbation it removes and why the landing needs the representation to be determined rather than inherited. If the authorized option makes the in-tree attributes rule authoritative, make the smallest change that achieves it and record in-source what a user with a stale pre-attributes checkout must do, because that migration cost is real and will otherwise be rediscovered by a user. If the authorized option loosens the dirty check, record in-source exactly what class of difference the landing no longer detects — a loosening whose cost is not written down where a maintainer sees it will be widened later by someone who does not know what it bought.

Do NOT touch the rest of the hostile-config defense: hooks, fsmonitor, the emptied system and global config, the cleared ambient overrides, and the deny-by-default core-key allowlist all stay exactly as they are. Do not refactor the invocation builder while you are in it.

Add ONE regression test that proves the reconciliation holds. It asserts the invariant the authorized option establishes — that a normally-cloned tree is judged clean through the landing's own invocation, or that an end-of-line difference is judged exactly as the authorized option says — and it must not assert a git message, an exit code, or any other shape of today's behavior. Do not modify, rename, re-gate or delete any existing test.

Prove it on real Windows: make a fresh normal clone in a scratch location, apply nothing to it, and confirm the landing's own invocation judges it clean. Then run the `wcore-swarm` crate suite on the box and state its counts as a DELTA against what `20A-01-BASELINE.md` actually measured, naming every residual failure. Remember that the AppContainer bind failures belong to 20A-02 and are expected here unless that plan has already landed — attribute them, do not absorb them.

Prove no Linux or macOS regression: run the aggregate build and test on Hetzner at this SHA and record the counts (REQ-native-r4). If the CI wiring from 20A-01 is live, record the macOS leg's result too.

Remove the scratch clone. Implements the authorized reconciliation; records evidence for REQ-native-r4; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'core.hooksPath' crates/wcore-swarm/src/worktree_cleanup.rs)" -ge "1" &amp;&amp; test "$(/usr/bin/grep -cF 'core.fsmonitor=false' crates/wcore-swarm/src/worktree_cleanup.rs)" -ge "1"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'GIT_CONFIG_NOSYSTEM' crates/wcore-swarm/src/worktree_cleanup.rs)" -ge "1" &amp;&amp; test "$(/usr/bin/grep -cF 'GIT_ATTR_NOSYSTEM' crates/wcore-swarm/src/worktree_cleanup.rs)" -ge "1"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'BENIGN_CORE_KEYS' crates/wcore-swarm/src/worktree/candidate.rs)" -ge "1"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git diff --stat -- crates/wcore-swarm/ .gitattributes</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "git fetch --all --prune 2>&amp;1"; cmd /c "git checkout --detach '"$SHA"' 2>&amp;1"; cmd /c "git rev-parse HEAD"; cmd /c "git status --porcelain"; cmd /c "cargo nextest run -p wcore-swarm --no-fail-fast 2>&amp;1" }' | grep -v CLIXML | grep -v "^&lt;Objs" | tail -40</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch --all --prune &amp;&amp; git checkout --detach $SHA &amp;&amp; git rev-parse HEAD &amp;&amp; cargo build --locked --workspace --all-features &amp;&amp; cargo nextest run --profile ci --no-fail-fast" 2&gt;&amp;1 | tail -30</automated>
  </verify>
  <done>Only the authorized option is implemented, with its cost written in-source where a maintainer will see it. The hostile-config defense is intact — hooks, fsmonitor, emptied system and global config, cleared ambient overrides and the deny-by-default allowlist are all gate-checked present. A fresh normal Windows clone is judged clean through the landing's own invocation, proven on the box. One regression test exists, asserts the invariant rather than any git message or exit code, and no existing test was modified or re-gated. The `wcore-swarm` counts are stated as a delta against the measured baseline with every residual failure named and attributed. The Hetzner aggregate proves no Linux regression. The scratch clone is removed and `C:\ferrox-win` is at the pinned SHA.</done>
</task>

</tasks>

## What this plan does NOT change (scope fence)

- **The rest of the hostile-config defense — untouched and gate-checked.** Disabled hooks, disabled fsmonitor, `GIT_CONFIG_NOSYSTEM`, the emptied system and global config files, `GIT_ATTR_NOSYSTEM`, disabled terminal prompts and every cleared ambient `GIT_*` override stay exactly as they are. Only the end-of-line question is on the table.
- **The deny-by-default core-key allowlist in the candidate module — untouched.** It already admits the two end-of-line keys as benign; it is not widened, and no other key is added to it.
- **The AppContainer retained-workspace-authority bind — 20A-02 owns it.** Its failures are expected in this plan's `wcore-swarm` run until that plan lands; they are attributed, never absorbed and never fixed here.
- **The CI wiring, the soak crate list and the proof-script target map — 20A-01 owns them.**
- **The sealed candidate and the native proof dispatch — 20A-04 owns them.**
- **`C:\ferrox-win` is never used as a scratch surface.** Every determination runs in a scratch clone that is removed afterwards, so the phase's measurement checkout stays trustworthy.
- **No refactor of the git-invocation builder.** Every changed line traces to the authorized reconciliation.
- **No test is deleted, weakened, re-gated, `#[ignore]`d or `#[allow]`ed.** If the honest outcome is that the premise was refuted, the plan closes with that finding rather than manufacturing a change to justify itself.

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| landing verdict ← checkout content representation | The landing judges whether a delegated mutation may land against the bytes in the tree; anything that perturbs those bytes outside the repository is inside this boundary |
| scrub ← ambient user/system configuration | The forced config and cleared environment exist so a user's or an attacker's ambient git configuration cannot change what the landing sees |
| dirty check ← class of detected difference | Whatever difference the dirty check stops detecting is a difference a mutation can carry past the landing unnoticed |
| product behavior ← fixture behavior | A symptom that only fixtures see is a test problem; one that every real user sees is a product defect, and the two have different acceptable fixes |

## STRIDE Threat Register

| Threat ID | Category | Component | Severity | Disposition | Mitigation Plan |
|-----------|----------|-----------|----------|-------------|-----------------|
| T-20A-03-01 | Tampering | The scrub's forced deterministic-checkout setting is dropped to make the dirty check pass, restoring ambient and attacker influence over the content representation the landing judges | high | mitigate | Removing the forced setting is not among the authorized options; the rest of the defense is gate-checked present after the change; any option that spends a guarantee must be chosen knowingly at the blocking checkpoint with its cost stated |
| T-20A-03-02 | Tampering | The dirty check is loosened to ignore a class of difference, so a real modification lands unnoticed | high | mitigate | That option exists but is labeled as the engineered-green option, requires an explicit acceptance at the checkpoint, and requires the lost detection class to be written in-source where a maintainer sees it before it is widened later |
| T-20A-03-03 | Spoofing | The wrong mechanism is fixed: the stated cause is contradicted by the committed attributes rule, so a change made on the stated cause could leave the real defect in place while claiming a fix | high | mitigate | Task 1 determines the mechanism single-variable in a scratch clone before any change, distinguishes a fresh clone from a stale pre-attributes checkout, and must reproduce the symptom through the landing's own invocation rather than a plain status |
| T-20A-03-04 | Repudiation | A stale pre-attributes checkout is mistaken for a product defect, producing a code change that fixes nothing for real users while adding surface | medium | mitigate | The determination explicitly separates the stale-checkout condition from the fresh-clone condition, and REFUTED-NO-DEFECT is a defined and complete termination state that makes no code change |
| T-20A-03-05 | Denial of Service | The determination mutates `C:\ferrox-win`, poisoning the baselines the other plans in this phase measure against | medium | mitigate | All determination work happens in a scratch clone that is removed afterwards; the measurement checkout's status and SHA are gate-checked at the end of Task 1 and Task 3 |
| T-20A-03-06 | Spoofing | A confidently wrong conclusion from a trailing-space environment variable during the determination, since the reproduction sets several `GIT_*` variables | medium | mitigate | The trap-safe assignment form is mandated and each value is proven to have taken effect before the observation that depends on it is trusted |
| T-20A-03-07 | Denial of Service | Scope metastasis — the determination leads into the sandbox or dispatch surfaces and the plan follows it | medium | mitigate | The scope boundary requires recording the connection and stopping; the termination criterion caps the plan at ONE determine/decide/implement cycle with four defined exit states |
| T-20A-03-08 | Repudiation | The regression test asserts a git message or exit code, so it passes for the wrong reason and stops failing when git's output changes | medium | mitigate | The test is required to assert the invariant the authorized option establishes; asserting a git message, an exit code or any shape of today's behavior is forbidden explicitly |
| T-20A-03-SC | Tampering | npm/pip/cargo installs | low | accept | No dependency is added, removed or updated; no `Cargo.toml` or `Cargo.lock` change; no install task exists in this plan |
</threat_model>

<verification>
Local gates (Mac, source level only — the Mac cannot compile this workspace): `cargo fmt --all -- --check` clean; `20A-03-EOL-DECISION.md` exists and states the determination as CONFIRMED or REFUTED with its evidence; after Task 3 the hostile-config defense is gate-checked present (disabled hooks, disabled fsmonitor, `GIT_CONFIG_NOSYSTEM`, `GIT_ATTR_NOSYSTEM`) and the deny-by-default core-key allowlist still exists; the diff over the swarm crate and the attributes file is surgical.

Authoritative gates (real hardware): the determination ran in a scratch clone which was removed, and `C:\ferrox-win` is unmodified and at the pinned SHA; a fresh normal Windows clone is judged clean through the landing's own invocation; the `wcore-swarm` suite on the box is stated as a delta against the counts `20A-01-BASELINE.md` measured, with every residual failure named and attributed — the AppContainer bind failures explicitly attributed to 20A-02 rather than absorbed. Hetzner gate: `cargo build --locked --workspace --all-features` plus `cargo nextest run --profile ci --no-fail-fast` at the same SHA, proving no Linux regression (REQ-native-r4). If the 20A-01 CI wiring is live, the macOS leg's result is recorded too.

Known unknowns to record, not to resolve here: whether other filesystems or non-NTFS volumes change the observation; whether Git for Windows version differences on other users' machines alter which default applies; and whether any other repository consumed by the swarm carries attributes rules that conflict with this one.
</verification>

<success_criteria>
- The ACTUAL mechanism is determined single-variable in a scratch clone, with the in-tree attributes rule's effect established directly rather than assumed, and with the symptom reproduced through the landing's own invocation rather than a plain status.
- A fresh clone is distinguished from a stale pre-attributes checkout, because only one of those is a product defect.
- The reconciliation is DECIDED at a blocking checkpoint with each option's security and user-experience cost stated — never resolved by a silent workaround in a git argument list.
- Only the authorized option is implemented, and whatever it costs is written in-source where a maintainer will see it before someone widens it.
- The rest of the hostile-config defense is gate-checked intact and the deny-by-default allowlist is not widened.
- One regression test asserts the invariant the authorized option establishes, never a git message or exit code, and no existing test was modified or re-gated.
- The `wcore-swarm` counts are stated as a delta against the measured 20A-01 baseline with every residual failure named and attributed to its owning plan (REQ-native-r4).
- The Hetzner aggregate proves no Linux regression at the same SHA.
- `C:\ferrox-win` is unmodified and at the pinned SHA, so the other plans' baselines remain trustworthy.
- If the premise was refuted and no defect exists, the plan closed with that recorded finding and made NO code change — a complete and successful outcome.
</success_criteria>

## Artifacts this phase produces
- `.planning/phases/20A-native-windows-macos-uat/20A-03-EOL-DECISION.md` — the single-variable determination, the reconciliation options with their costs, and the recorded decision or refutation.
- `crates/wcore-swarm/src/worktree_cleanup.rs` and `.gitattributes` — the authorized reconciliation with its cost recorded in-source, or untouched if the premise was refuted.
- `crates/wcore-swarm/src/worktree_tests.rs` — the regression proof for the authorized reconciliation.
- `20A-03-SUMMARY.md` recording the determination, the decision, the implementation and the two-platform non-regression proof.

<output>
Create `.planning/phases/20A-native-windows-macos-uat/20A-03-SUMMARY.md` using the standard GSD summary template. Record: the box's starting git configuration with origins; the fresh-clone observation before anything was touched; each single-variable observation with its exact command and output, including what end-of-line attributes git resolves for a representative tracked text file and whether that matches the committed rules; whether the symptom reaches the landing's own invocation or only a plain status; the explicit distinction between a fresh clone and a stale pre-attributes checkout; the determination as CONFIRMED, REFUTED-NO-DEFECT or REFUTED-DIFFERENT-CAUSE; the option authorized at the checkpoint and the cost accepted with it; the implementation and where its cost is recorded in-source; the regression test and the invariant it asserts; the `wcore-swarm` delta against the measured baseline with every residual failure attributed to its owning plan; the Hetzner aggregate; confirmation that the scratch clone was removed and `C:\ferrox-win` is unmodified; the recorded unknowns; and which of the four termination states the plan ended in. Mark no requirement complete — closure is claimed by the downstream native proof under 20A-04.
</output>
