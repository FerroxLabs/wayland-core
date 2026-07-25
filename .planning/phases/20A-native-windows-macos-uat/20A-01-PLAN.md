---
phase: 20A-native-windows-macos-uat
plan: "01"
type: execute
wave: 1
depends_on: []
files_modified:
  - .github/workflows/ci.yml
  - .github/workflows/nightly-windows-soak.yml
  - scripts/wayland-e2e-windows-soak.ps1
  - scripts/f20-native-windows-proof.ps1
  - .planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md
  - .planning/BACKLOG.md
autonomous: true
requirements:
  - REQ-native-r1
  - REQ-native-r2
  - REQ-native-r3
  - REQ-native-r4
  - REQ-native-r5
  - REQ-native-r7
  - REQ-native-r8
  - REQ-native-r10
  - REQ-native-r14
  - REQ-native-r15
must_haves:
  truths:
    - "THE UNVERIFIED PRECONDITION THIS PLAN EXISTS TO CLOSE (`.planning/TEST-AUDIT.md` §'What I could NOT determine'): nobody has confirmed that the 155 Windows-only and 23 macOS-only tests even COMPILE at `01a5b0ae`/`70ccd708`. The Mac cannot build this workspace, the Windows host is on a different commit, and `gh run list -R FerroxLabs/wayland-core --branch plan/f20-unified-audit-repair` returns `[]` — CI has NEVER run on this branch, and the last `ci.yml` run of any kind was 2026-07-13. Compilation is the precondition for every claim about those 178 tests, and it is exactly the failure mode that hid 133 `wcore-sandbox` tests for two weeks. Establish it FIRST, before any repair plan spends effort on a tree that may not build."
    - "THE BLIND SPOT, measured (TEST-AUDIT §1.0/§1.3): the only RECURRING Windows automation is `nightly-windows-soak.yml` (cron, `main` only), and `scripts/wayland-e2e-windows-soak.ps1` runs nextest over exactly five crates — `wcore-cron`, `wcore-config`, `wcore-providers`, `wcore-tools`, `wcore-swarm`. `wcore-sandbox` is NOT among them, and `wcore-sandbox` alone holds 105 Windows-only tests including every retained-handle security proof. 283 tests have no execution evidence at this SHA in normal CI; roughly 145 run in no automatic workflow on any branch."
    - "THE TEN ORPHANED ACL SECURITY TESTS (TEST-AUDIT §1.3 #4): `crates/wcore-sandbox/tests/live_fs_acl.rs` carries twelve Windows-only `#[ignore]`d ACL tests. `scripts/f20-native-windows-proof.ps1` names exactly two of them (`one_execution_grant_never_leaks_to_another_identity`, `granted_path_is_readable_then_revoked`). The other ten run NOWHERE — including `deny_ace_still_blocks_granted_read` and `normal_sid_only_grant_is_denied`, the two that would have caught the `fs_read_deny` silent no-op found and fixed this session. They are triple-gated: `#![cfg(windows)]` plus `#[ignore]` plus absent from every selector. A test with no runner is not a control."
    - "STARTING STATE — HARDWARE-MEASURED THIS SESSION, do not re-derive: on SEANDESKTOP at `c39f7254`, `wcore-sandbox` was 135 run / 135 passed / 0 failed / 45 skipped (green twice consecutively); `wcore-swarm` was 90 run / 83 passed / 7 failed; `wcore-agent --test transactional_delegated_mutation_test --run-ignored all` was 9 / 5 passed / 4 failed; `wcore-swarm --test dispatch_smoke` was 7 / 3 passed / 4 failed. `git diff --stat c39f7254 70ccd708 -- crates/ .github/ scripts/ justfile .config/` touches ONLY `.config/nextest.toml` (+32 lines of timeout overrides) and five desktop contract fixtures — NO `crates/**/*.rs` change — so those counts should carry to the phase base, but the nextest.toml delta makes that a PREDICTION, not a measurement. Re-measure."
    - "ALL 7 REMAINING `wcore-swarm` FAILURES SHARE ONE CAUSE — `sandbox backend appcontainer cannot bind retained delegated workspace authority` (`crates/wcore-swarm/src/dispatch.rs:52-57`, reached because `AppContainerBackend` in `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs` overrides neither `binds_cwd_authority` nor `execute_with_cwd_authority`, so both keep their fail-closed trait defaults in `crates/wcore-sandbox/src/backends/mod.rs:299-350`). `git diff` over `backends/` and `dispatch.rs` across the repair commits is EMPTY: it is pre-existing and STRUCTURAL, not a regression. THIS PLAN DOES NOT FIX IT — plan 20A-02 owns it. This plan must not be drawn into it."
    - "THIS PLAN MEASURES AND WIRES; IT DOES NOT REPAIR BEHAVIOUR. The only code repair in scope is a COMPILE error in a Windows-only or macOS-only body. Every other defect found is RECORDED as a severity-classified finding and ROUTED — never fixed here. That boundary is what stops this plan from metastasizing the way Phase 20 did."
    - "A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Wiring a test into CI so that it can fail is the entire point; a test that is wired and then goes red has done its job. Never weaken an assertion, `#[ignore]`, `#[allow]`, raise a global timeout, or delete an inconvenient test to reach a gate."
  artifacts:
    - path: .planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md
      provides: "The compile verdict for the 155 Windows-only and 23 macOS-only tests at one exact SHA, the re-measured four-suite Windows baseline with every failure named, and the severity-classified finding register with each item's route"
    - path: scripts/wayland-e2e-windows-soak.ps1
      provides: "`wcore-sandbox` added to the recurring Windows nextest surface, closing the 105-test recurring-automation gap"
    - path: .github/workflows/ci.yml
      provides: "A CI trigger that actually fires on this working branch, so the macOS and self-hosted-Windows legs compile this tree"
    - path: scripts/f20-native-windows-proof.ps1
      provides: "The ten previously unselected `live_fs_acl` ACL security tests wired into a real runner without weakening the wrong-OS anti-drift guard"
    - path: .planning/BACKLOG.md
      provides: "Every MEDIUM-and-below finding, logged and explicitly non-blocking"
  key_links:
    - from: scripts/wayland-e2e-windows-soak.ps1
      to: crates/wcore-sandbox/tests/live_fs_acl.rs
      via: "the recurring Windows automation selecting the crate and the ACL security tests it has never run"
      pattern: "native_windows"
    - from: .planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md
      to: .planning/phases/20A-native-windows-macos-uat/20A-02-PLAN.md
      via: "the re-measured failure set that 20A-02's delta is stated against"
      pattern: "measured-baseline"
---

<objective>
Close the CI blind spot and establish the TRUE native baseline before any repair work is planned against it.

Purpose: Nine production defects hid behind a green suite because the green suite was Linux-only. This branch has never been through CI at all; the recurring Windows automation excludes the one crate holding every retained-handle security proof; ten ACL boundary tests run nowhere; and nobody has confirmed the 178 Windows-only/macOS-only tests even compile. Every downstream plan in this phase states its delta against a baseline — so the baseline must be MEASURED at the phase SHA, not inherited from a prediction.
Output: One compile verdict per platform bound to one exact SHA; the four Windows suites re-measured with every failure named; `wcore-sandbox` and the ten orphaned ACL tests wired into runners that actually execute; a CI trigger that fires on this branch; and a severity-classified finding register with each item routed.
</objective>

<execution_context>
@/Users/seandonahoe/.codex/gsd-core/workflows/execute-plan.md
@/Users/seandonahoe/.codex/gsd-core/templates/summary.md
</execution_context>

<context>
@AGENTS.md
@.planning/TEST-AUDIT.md
@.planning/ROADMAP.md
@scripts/wayland-e2e-windows-soak.ps1
@scripts/f20-native-windows-proof.ps1
@.github/workflows/ci.yml
@.github/workflows/nightly-windows-soak.yml
@crates/wcore-sandbox/tests/live_fs_acl.rs
</context>

<execution_rules>

**THE TWO AMENDED PHASE RULES — stated verbatim, and they bound this plan.**

- Findings at CRITICAL or HIGH must be fixed or disproved. MEDIUM and below are logged to BACKLOG and DO NOT BLOCK execution.
- Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean.

**TERMINATION CRITERION FOR THIS PLAN (hard — the plan STOPS and escalates rather than spawning more work).** This plan performs at most ONE wire-then-re-measure cycle. It terminates in exactly one of three states, and in all three it writes its SUMMARY and stops:
1. **Complete** — compile verdict recorded for both platforms, four suites re-measured, wiring landed and proven to select what it claims, findings classified and routed.
2. **Compile-blocked** — a Windows-only or macOS-only body fails to compile and the repair is NOT a mechanical module-path/import/cfg fix confined to the failing file. STOP. Record the exact compiler diagnostics and escalate to Sean. Do not redesign anything to make it compile.
3. **Finding-blocked** — the re-measure surfaces a CRITICAL or HIGH finding that is neither the known AppContainer bind blocker (20A-02) nor the checkout-dirty item (20A-03). STOP. Record it with severity and evidence and escalate to Sean. Do NOT open a fourth plan and do NOT fix it here.
Under no circumstances does this plan spawn additional plans, extend its own task list, or start a second measure/fix cycle.

**SCOPE BOUNDARY (hard).** The ONLY code repair in scope is a compile error in a Windows-only or macOS-only test body. Behavioural repair is out of scope: the AppContainer retained-workspace-authority bind belongs to 20A-02, the checkout-dirty/eol decision belongs to 20A-03, and the sealed-candidate native dispatch belongs to 20A-04.

**NON-NEGOTIABLE.** A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, `#[ignore]`, `#[allow]`, raise a global timeout, or delete an inconvenient test to reach a gate. Four executors in Phase 20 correctly stopped and escalated rather than improvise, and every one of those calls was right. A newly wired test that goes RED is a SUCCESS of this plan, not a failure of it — record it and route it, do not silence it.

**ENVIRONMENT.**
- Windows: `ssh -o BatchMode=yes SeanD@seandesktop` (Tailscale), checkout `C:\ferrox-win`. Invocation shape: `ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "<cmd> 2>&1" }'`, piped through `grep -v CLIXML | grep -v "^<Objs"`. Git on the box MUST be wrapped `cmd /c "git ..."` — PowerShell's Stop preference treats git's stderr chatter as fatal. `cargo fmt --all` FAILS on the box with os error 206; `justfile:96-98` already skips fmt-check on Windows, so do not re-introduce it there.
- Linux: `ssh -o BatchMode=yes hetzner-dsm`, `/root/wayland`.
- Mac CANNOT compile this workspace. `cargo fmt --all` is the only working cargo command there. Use `/usr/bin/git`, and ALWAYS `/usr/bin/grep`.
- Push the WORK BRANCH to `gh` so the hosts can fetch it. NO push to main, merge, PR, tag, release, or issue closure without Sean.

**THE TWO MEASUREMENT TRAPS (both measured; do not simplify these away).**
1. In `cmd`, `set VAR=value && ...` appends a TRAILING SPACE to the value and Rust silently ignores it. Use `set "VAR=x"` or PowerShell `$env:VAR='x'`, and PROVE the value took effect (echo it back, or assert on the behaviour it gates) before trusting any run that depends on it. This matters directly here: `live_fs_acl.rs` gates on `WAYLAND_SANDBOX_LIVE_WINDOWS`.
2. Mac `grep` is rtk-proxied and SILENTLY DROPS LINES — measured at 32 returned versus 674 for the same inverted match on the same file. Every gate in this plan invokes `/usr/bin/grep` explicitly and uses `-F` for literals.

**BOX SHA IS UNKNOWN — READ IT, DO NOT ASSUME.** `.planning/TEST-AUDIT.md` records `C:\ferrox-win` at `ce9a11a6`; this session's measurements were taken at `c39f7254`. Both are ancestors of the branch HEAD and the two records disagree. The executor READS the SHA the box prints and records it; it never assumes either value.

**Git hygiene.** Use `/usr/bin/git` on the Mac. Stage the exact paths in `files_modified`, never `-A`, never `.`. Never stage `AGENTS.md` or `.ijfw` churn. No `Co-Authored-By` trailers.
</execution_rules>

<tasks>

<task type="auto">
  <name>Task 1: Move the box to the exact phase SHA and settle the compile question for all 178 platform-only tests</name>
  <files>.planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md</files>
  <read_first>.planning/TEST-AUDIT.md (§1.1, §1.2, §1.3, and the "What I could NOT determine" list), .github/workflows/ci.yml (the `on:` block and the `ci` job matrix)</read_first>
  <behavior>
    - The exact SHA under test is pinned once and every result in this plan is bound to it.
    - The Windows host's ACTUAL current SHA is read and recorded before it is moved, so the record of where it stood is not lost.
    - Whether the 155 Windows-only test bodies compile at that SHA is answered YES or NO with the compiler's own output, never inferred.
    - Whether the 23 macOS-only test bodies compile at that SHA is answered YES or NO. The Mac cannot compile, so this answer can only come from a machine that can — it is obtained in Task 2 and cross-recorded here.
    - The working tree is pristine before any measurement, and `Cargo.lock` is consistent under `--locked`, so no measurement is taken against a tainted tree.
    - A compile failure that is a mechanical module-path, import or cfg-gate defect confined to the failing file is repaired. Anything larger stops the plan.
  </behavior>
  <action>Pin ONE exact source SHA for this whole plan and record it. Push the work branch so the hosts can fetch it.

FIRST, before moving anything, read and record the SHA the Windows box is actually standing on — `.planning/TEST-AUDIT.md` says `ce9a11a6` and this session's measurements say `c39f7254`, and those disagree. Record what the box prints. Then confirm the box's tree is pristine (no local modifications) before moving it; if it is not, record exactly what was dirty and restore it to pristine, because REQ-native-r15 exists precisely because a tainted tree was measured once already.

Move the box to the pinned SHA by fetching and detaching, and confirm the SHA it prints matches the pin. Regenerate and verify `Cargo.lock` consistency by requiring the build to succeed under `--locked` (REQ-native-r10); if `--locked` fails, that is itself a finding, recorded, not worked around.

Now settle the compile question, which is the reason this task runs first. On the box, compile the whole workspace WITH all test targets so the Windows-only bodies are actually type-checked — a check that omits test targets proves nothing about them. Capture the full output. Record the verdict explicitly for `wcore-sandbox` (which holds 105 of the 155, across the appcontainer `windows_impl` test module, the retained-handle Windows test module mounted by path from `directory_authority.rs`, the live ACL integration test, the acl_lease modules, the hard-process-containment Windows integration test, and the live-integrity test) and for `wcore-agent` (REQ-native-r3, whose recorded 2026-07-22 failure was a Windows COMPILE defect — REQ-native-r14 requires that fix re-proven on real hardware, not asserted from source).

If a compile error appears: repair it ONLY when it is a mechanical module-path, import or cfg-gate defect confined to the failing file, and record both the diagnostic and the repair. If the repair would require a design change, an API change, or edits across more than the failing file, STOP — this plan is compile-blocked. Write the diagnostics into the baseline document, write the SUMMARY, and escalate to Sean. Do not redesign anything to reach a compile.

Record everything in a new `20A-01-BASELINE.md`: the pinned SHA, the box's prior SHA, the pristine-tree confirmation, the `--locked` result, and the per-crate compile verdict with the exact command used. Records evidence for REQ-native-r3, r10, r14 and r15; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "git rev-parse HEAD"; cmd /c "git status --porcelain"; cmd /c "git fetch --all --prune 2>&amp;1"; cmd /c "git checkout --detach '"$SHA"' 2>&amp;1"; cmd /c "git rev-parse HEAD" }' | grep -v CLIXML | grep -v "^&lt;Objs"</automated>
    <automated>ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "cargo build --locked --workspace --all-targets 2>&amp;1" }' | grep -v CLIXML | grep -v "^&lt;Objs" | tail -40</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -f .planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md &amp;&amp; test "$(/usr/bin/grep -cF 'pinned SHA' .planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md)" -ge "1"</automated>
  </verify>
  <done>One exact SHA is pinned and the box is detached on it, with the box's prior SHA recorded rather than assumed. The tree is pristine and `--locked` is satisfied or the failure is recorded as a finding. The compile question for the 155 Windows-only bodies is answered with the compiler's own output, per crate, in `20A-01-BASELINE.md`. Any compile repair was a mechanical single-file fix, or the plan stopped as compile-blocked with the diagnostics recorded and escalated.</done>
</task>

<task type="auto">
  <name>Task 2: Wire the never-executing Windows surface into runners that actually run — CI branch trigger, wcore-sandbox in the soak, and the ten orphaned ACL security tests</name>
  <files>.github/workflows/ci.yml, .github/workflows/nightly-windows-soak.yml, scripts/wayland-e2e-windows-soak.ps1, scripts/f20-native-windows-proof.ps1</files>
  <read_first>scripts/wayland-e2e-windows-soak.ps1 (the phase that invokes nextest over the five crates, and its surrounding phase structure), scripts/f20-native-windows-proof.ps1 (the target array, the wrong-OS anti-drift guard and its `os` field semantics, and the environment variable that enables live Windows sandbox acceptance), .github/workflows/ci.yml (the `on:` block and the self-hosted Windows matrix entry), crates/wcore-sandbox/tests/live_fs_acl.rs (the live-acceptance gate helper at the top of the file and all twelve ignored test names)</read_first>
  <behavior>
    - This working branch is reachable by CI, so the macOS leg and the self-hosted Windows leg compile this tree instead of never seeing it. That is what supplies the macOS-only compile verdict Task 1 cannot obtain locally.
    - The recurring Windows automation runs `wcore-sandbox`, closing the 105-test gap. Its existing five-crate surface is preserved, not replaced.
    - All twelve `live_fs_acl` ACL tests are selected by a runner that actually executes, not just the two currently named. The ten that ran nowhere — including the two that would have caught the read-deny silent no-op — become capable of failing.
    - The `hard_process_containment_windows` gate marker that no gate selects becomes selected. It is wired, never deleted.
    - The wrong-OS anti-drift guard still fails closed for every OS-specific target, and no target's `os` classification is relaxed to make wiring easier.
    - Newly selected tests that go RED are recorded as findings, not silenced. Nothing is `#[ignore]`d, `#[allow]`ed, re-gated or removed to keep a runner green.
  </behavior>
  <action>Three wirings, each with a gate proving it selects what it claims.

WIRING A — make CI fire on this branch. `ci.yml` triggers only on `pull_request → main` and `push → main`, which is why `gh run list -R FerroxLabs/wayland-core --branch plan/f20-unified-audit-repair` returns an empty array and neither the macOS nor the self-hosted Windows leg has ever compiled this tree. Extend the trigger so the working branch is covered — either by adding the branch to the push trigger or by adding a manual dispatch trigger, whichever is the smaller diff against the existing shape. Do NOT change the job matrix, the runner labels, the concurrency group or the test command. Then RUN it and record the result: the macOS job's compile outcome is the only available answer to Task 1's macOS-only question (REQ-native-r14 also requires the macOS proof-harness test-mapping fix re-proven, and this is the first machine that can see it). Note in the SUMMARY that dispatching this workflow is normal CI, NOT the Sean-gated native proof dispatch — that is a different workflow and belongs to 20A-04.

WIRING B — add `wcore-sandbox` to the recurring Windows soak. In `scripts/wayland-e2e-windows-soak.ps1`, extend the nextest crate selection to include `wcore-sandbox` alongside the five it already carries, and update the phase's own description and its count so the script does not describe itself falsely. Keep every existing crate. This is the single highest-leverage line in the plan: it is what gives 105 Windows-only tests, including every retained-handle security proof, a recurring execution path for the first time.

WIRING C — give the ten orphaned ACL security tests a runner. Twelve Windows-only ignored tests live in `crates/wcore-sandbox/tests/live_fs_acl.rs`; the proof script names two. Wire ALL of them. Prefer the smallest change that makes the whole file's ignored set selectable rather than enumerating names one by one — an enumerated list is exactly how ten of them fell out, and a file-level selection cannot silently lose a test that is added later. Whichever mechanism you choose, it must run the ignored set and it must fail closed if a selector matches nothing, matching the discipline the proof script already applies. The live acceptance helper at the top of that file gates on an environment variable: set it using the trap-safe form and PROVE it took effect before trusting the run, because the trailing-space form silently disables every one of these tests and would produce a vacuous green. Apply the same treatment to the `hard_process_containment_windows` gate marker that no gate currently selects. Do NOT delete it — the audit's "FIX or DELETE" is resolved as FIX, because deleting a test to tidy a gate is precisely the move this phase forbids.

Preserve the wrong-OS anti-drift guard exactly (REQ-native-r8): every OS-specific target's selected source must still be affirmatively cfg-gated for its own OS and must still fail closed otherwise, and no target may be reclassified as cross-platform to dodge the guard. If a newly wired test cannot pass the guard, that is a finding, not a reason to relax the guard.

Run the newly wired surface on the box and record the outcome per test. Tests that go RED here are the plan working as intended: name them, classify them, route them. Do not fix them — behavioural repair is 20A-02's and 20A-03's scope. Records evidence for REQ-native-r1, r2, r5, r7 and r8; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'wcore-sandbox' scripts/wayland-e2e-windows-soak.ps1)" -ge "1"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; for c in wcore-cron wcore-config wcore-providers wcore-tools wcore-swarm; do test "$(/usr/bin/grep -cF "$c" scripts/wayland-e2e-windows-soak.ps1)" -ge "1" || exit 1; done</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'live_fs_acl' scripts/f20-native-windows-proof.ps1)" -ge "2"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'plan/f20-unified-audit-repair' .github/workflows/ci.yml)" -ge "1" || test "$(/usr/bin/grep -cF 'workflow_dispatch' .github/workflows/ci.yml)" -ge "1"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/grep -c "os = 'windows'" scripts/f20-native-windows-proof.ps1</automated>
    <automated>ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; $env:WAYLAND_SANDBOX_LIVE_WINDOWS='"'"'1'"'"'; cmd /c "echo LIVEFLAG=%WAYLAND_SANDBOX_LIVE_WINDOWS%"; cmd /c "cargo nextest run -p wcore-sandbox --test live_fs_acl --run-ignored all --no-fail-fast 2>&amp;1" }' | grep -v CLIXML | grep -v "^&lt;Objs" | tail -40</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; gh auth switch --user FerroxLabs &gt;/dev/null 2&gt;&amp;1; gh run list -R FerroxLabs/wayland-core --branch plan/f20-unified-audit-repair --limit 5</automated>
  </verify>
  <done>CI fires on this branch and has produced at least one run whose macOS leg compile outcome is recorded. `wcore-sandbox` is in the recurring Windows soak's crate list and all five original crates survive. All twelve `live_fs_acl` ACL tests and the previously unselected containment gate marker are selected by a runner that executes, with the live-acceptance environment variable set in the trap-safe form and PROVEN to have taken effect. The wrong-OS anti-drift guard is intact and no target was reclassified. Every newly wired test's outcome is recorded by name. No test was deleted, re-gated, `#[ignore]`d or `#[allow]`ed.</done>
</task>

<task type="auto">
  <name>Task 3: Re-measure the four Windows suites at the pinned SHA and classify every finding with its route</name>
  <files>.planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md, .planning/BACKLOG.md</files>
  <read_first>.planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md (as written by Tasks 1-2), crates/wcore-swarm/src/dispatch.rs (the delegated-backend admission function and its four refusal messages), crates/wcore-sandbox/src/backends/mod.rs (the cwd-authority and workspace-authority trait defaults and the two `binds_*` predicates)</read_first>
  <behavior>
    - The four suites measured this session are re-run at the pinned SHA and their ACTUAL counts recorded, so 20A-02 and 20A-03 state their deltas against a measurement rather than a prediction.
    - Every failure is named. A count without names is not a baseline.
    - Each failure is attributed to one of exactly three buckets: the known AppContainer bind blocker, the known checkout-dirty item, or NEW. A NEW failure at CRITICAL or HIGH stops the plan.
    - Every finding carries a severity, and the two amended rules are applied to it: CRITICAL/HIGH must be fixed or disproved; MEDIUM and below go to BACKLOG and do not block.
    - The three already-green native proof targets are confirmed still green, so the phase knows what it must not break.
    - No finding is fixed in this plan.
  </behavior>
  <action>Re-run, on the box at the pinned SHA, the four suites this session measured, capturing full output: the `wcore-sandbox` crate suite; the `wcore-swarm` crate suite; the `wcore-agent` transactional delegated-mutation integration test with the ignored set included; and the `wcore-swarm` dispatch smoke integration test. Record the ACTUAL run/passed/failed/skipped counts. The session's measurements at `c39f7254` were 135/135/0/45, 90/83/7, 9/5/4 and 7/3/4 respectively — treat those as a PREDICTION to compare against, never as the result. The only tree delta between that SHA and the phase base is `.config/nextest.toml` timeout overrides and five desktop contract fixtures, so a material divergence in the counts is itself a finding worth naming.

Name EVERY failing test. Then attribute each one to exactly one bucket:
  - **Known blocker A** — the delegated-backend admission refusal `sandbox backend appcontainer cannot bind retained delegated workspace authority`, raised in `wcore-swarm`'s dispatch admission because the AppContainer backend keeps the fail-closed trait defaults for cwd-authority binding. This is the single cause behind the 7 swarm, 4 agent and 4 dispatch-smoke failures. It is pre-existing and structural — the diff over the sandbox backends directory and the swarm dispatch module across the repair commits is empty. Route: 20A-02. Do not fix it here.
  - **Known blocker B** — the checkout-dirty item. Route: 20A-03. Do not fix it here.
  - **NEW** — anything else. Assign a severity. A NEW finding at CRITICAL or HIGH terminates this plan: record it with its evidence, write the SUMMARY, and escalate to Sean. A NEW finding at MEDIUM or below is appended to `.planning/BACKLOG.md` with its evidence and explicitly marked non-blocking.

Separately, confirm the three native proof targets already proven green on the certified runner — the retained-handle target, the AppContainer ACL target and the Job Object target — are STILL green at this SHA, and record that they are the invariant the rest of the phase must not break. Record that the two that would fail today are the public dispatch target and the F20 lifecycle target, both of which are downstream of blocker A.

Write all of it into `20A-01-BASELINE.md` as the authoritative baseline for the phase, with the pinned SHA at the top of every table. Records evidence for REQ-native-r1, r2, r4, r5 and r7; marks no requirement complete.</action>
  <verify>
    <automated>ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "cargo nextest run -p wcore-sandbox --no-fail-fast 2>&amp;1" }' | grep -v CLIXML | grep -v "^&lt;Objs" | tail -30</automated>
    <automated>ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "cargo nextest run -p wcore-swarm --no-fail-fast 2>&amp;1" }' | grep -v CLIXML | grep -v "^&lt;Objs" | tail -40</automated>
    <automated>ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "cargo nextest run -p wcore-agent --test transactional_delegated_mutation_test --run-ignored all --no-fail-fast 2>&amp;1" }' | grep -v CLIXML | grep -v "^&lt;Objs" | tail -30</automated>
    <automated>ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "cargo nextest run -p wcore-swarm --test dispatch_smoke --no-fail-fast 2>&amp;1" }' | grep -v CLIXML | grep -v "^&lt;Objs" | tail -30</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'Severity' .planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md)" -ge "1"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -f .planning/BACKLOG.md</automated>
  </verify>
  <done>All four suites are re-measured at the pinned SHA with actual counts and every failure named. Each failure is attributed to blocker A, blocker B, or NEW-with-severity. The three already-green native targets are confirmed still green. MEDIUM-and-below findings are in BACKLOG and explicitly non-blocking; any NEW CRITICAL or HIGH finding terminated the plan with an escalation to Sean. No finding was fixed in this plan, and no test was weakened, re-gated or deleted.</done>
</task>

</tasks>

## What this plan does NOT change (scope fence)

- **The AppContainer retained-workspace-authority bind — 20A-02 owns it, entirely.** This plan reads the admission refusal and names it as the single cause of the 7 swarm + 4 agent + 4 dispatch-smoke failures. It does not touch `crates/wcore-sandbox/src/backends/`, `crates/wcore-swarm/src/dispatch.rs`, or any trait default.
- **The checkout-dirty / eol reconciliation — 20A-03 owns it.** This plan does not touch `.gitattributes`, `crates/wcore-swarm/src/worktree_cleanup.rs`, or any git configuration scrub.
- **The sealed candidate and the native proof dispatch — 20A-04 owns them.** Running normal CI on this branch is NOT the native proof dispatch; the two are different workflows and the latter is Sean-gated.
- **Phase 20.** It closed green at `01a5b0ae` with F20-01..F20-06 / GATE-01 / GATE-02 complete. Nothing here reopens it.
- **No test is deleted, weakened, re-gated, `#[ignore]`d or `#[allow]`ed.** The audit's "FIX or DELETE" verdicts are all resolved as FIX. Wiring a test so it CAN fail is the deliverable.
- **The wrong-OS anti-drift guard and every target's `os` classification.** No target is reclassified as cross-platform to make wiring easier.
- **The soak's existing five crates, the CI job matrix, the runner labels and the test command.** Only the trigger surface and the crate list grow.
- **The Linux and macOS behaviour of any production code.** This plan adds no production code beyond a possible mechanical compile fix.

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| security control ← execution evidence | A test with no runner provides zero coverage; the ten orphaned ACL tests are the boundary between a claimed ACL control and a proven one |
| repair decision ← measured baseline | Every downstream plan states a delta; if the baseline is predicted rather than measured, every delta is unattributable |
| platform claim ← real hardware | The Mac cannot compile; only SEANDESKTOP and CI can answer whether the 178 platform-only tests build or pass |
| green suite ← platform coverage | An 11,519-test green run that is Linux-only is not evidence about Windows, and treating it as such is what hid nine production defects |
| runner selection ← test enumeration | A hand-enumerated selector silently loses tests as they are added; that is exactly how ten of twelve ACL tests fell out |

## STRIDE Threat Register

| Threat ID | Category | Component | Severity | Disposition | Mitigation Plan |
|-----------|----------|-----------|----------|-------------|-----------------|
| T-20A-01-01 | Repudiation | 105 Windows-only `wcore-sandbox` tests, including every retained-handle security proof, have no recurring execution path on any branch | critical | mitigate | Task 2 Wiring B adds `wcore-sandbox` to the recurring Windows soak; Task 3 records the resulting outcome per test |
| T-20A-01-02 | Elevation of Privilege | Ten `live_fs_acl` ACL boundary tests run nowhere, including the read-deny and normal-SID-grant proofs that would have caught the read-deny silent no-op | critical | mitigate | Task 2 Wiring C selects the whole ignored set with a file-level mechanism that cannot silently lose a later addition, and fails closed on an empty selector |
| T-20A-01-03 | Spoofing | The 178 Windows-only and macOS-only test bodies may not COMPILE at this SHA, making every claim about them vacuous — the exact failure mode that hid 133 tests for two weeks | high | mitigate | Task 1 compiles the workspace with all test targets on real Windows; Task 2 Wiring A obtains the macOS answer from a machine that can compile; both verdicts are recorded per crate |
| T-20A-01-04 | Spoofing | A vacuous green: the live-acceptance environment variable is set with the trailing-space `cmd` form, Rust silently ignores it, and every newly wired ACL test skips while the runner reports success | high | mitigate | The trap-safe assignment form is mandated and the value is echoed back and PROVEN to have taken effect before any run that depends on it is trusted |
| T-20A-01-05 | Tampering | A newly wired test goes RED and is silenced with an `#[ignore]`, a widened `#[allow]`, a relaxed assertion or a deletion, converting a discovered defect into a hidden one | high | mitigate | Forbidden verbatim in the execution rules and in every task's `<done>`; RED is defined as a success of this plan; the audit's "FIX or DELETE" verdicts are all resolved as FIX |
| T-20A-01-06 | Spoofing | A measurement is taken against a tainted working tree or an inconsistent lockfile, so the baseline every downstream delta rests on is not reproducible | high | mitigate | Task 1 confirms the box's tree is pristine before measuring and requires the build to succeed under `--locked` (REQ-native-r10 / r15); a `--locked` failure is recorded as a finding |
| T-20A-01-07 | Denial of Service | Scope metastasis — the plan starts fixing what it measures and grows without bound, as Phase 20 did to 74 plans | high | mitigate | The termination criterion caps the plan at ONE wire-then-re-measure cycle with three defined exit states; behavioural repair is fenced to 20A-02 / 20A-03; a NEW CRITICAL or HIGH finding stops the plan and escalates rather than opening a fourth |
| T-20A-01-08 | Tampering | The wrong-OS anti-drift guard is relaxed, or a target reclassified as cross-platform, to make a newly wired test pass the guard — reintroducing the defect where a target silently mapped to a wrong-OS test | high | mitigate | REQ-native-r8 is restated as a hard constraint; a test that cannot pass the guard is recorded as a finding rather than accommodated; the OS-specific target count is gate-checked |
| T-20A-01-09 | Information Disclosure | Extending the CI trigger to a working branch exposes workflow steps to a branch that has not been reviewed for main | medium | accept | The added surface is `ci.yml` only (fmt / clippy / test / audit) on an existing self-hosted runner and the hosted macOS runner; the Sean-gated candidate jobs in the soak workflow are untouched and still require the nonce; no secret is added and no permission is widened |
| T-20A-01-10 | Denial of Service | Adding `wcore-sandbox` to the nightly soak lengthens the recurring run and may push it past its window | low | accept | The soak is a nightly cron with no downstream consumer waiting on it; a longer run is the cost of covering 105 previously unexecuted tests, and any timeout is a finding rather than a reason to drop the crate |
| T-20A-01-SC | Tampering | npm/pip/cargo installs | low | accept | No dependency is added, removed or updated; no `Cargo.toml` change; `Cargo.lock` is verified for consistency, not modified for content; no install task exists in this plan |
</threat_model>

<verification>
Local gates (Mac, source level only — the Mac cannot compile): `cargo fmt --all -- --check` clean; `scripts/wayland-e2e-windows-soak.ps1` names `wcore-sandbox` and still names all five original crates; `scripts/f20-native-windows-proof.ps1` selects the whole `live_fs_acl` ignored set and the previously unselected containment gate marker; `ci.yml` carries a trigger that covers this branch; the OS-specific target count in the proof script is recorded and unreduced; `20A-01-BASELINE.md` exists and carries the pinned SHA, the compile verdicts and a severity column.

Authoritative gates (real hardware, in order, at the pinned SHA): the box's prior SHA is recorded and the box is detached on the pin; the tree is pristine; `cargo build --locked --workspace --all-targets` compiles on Windows, with the per-crate verdict recorded for `wcore-sandbox` and `wcore-agent`; at least one CI run exists on this branch and its macOS leg's compile outcome is recorded; the whole `live_fs_acl` ignored set executes with the live-acceptance flag proven to have taken effect; the four suites are re-measured with actual counts and every failure named and bucketed; the retained-handle, AppContainer-ACL and Job-Object native targets are confirmed still green.

Known unknowns to record, not to resolve here: whether any newly wired test that goes RED is a real defect or a fixture problem (that determination belongs to whichever plan owns the surface); whether the `.config/nextest.toml` timeout deltas between `c39f7254` and the phase base shift any count; whether the macOS-only bodies compile under the ephemeral candidate image as well as under hosted `macos-latest`.
</verification>

<success_criteria>
- The compile question for the 155 Windows-only and 23 macOS-only tests is ANSWERED at one exact SHA with each platform's own compiler output, closing the audit's top "could not determine" item (REQ-native-r3, r14).
- `wcore-sandbox` has a recurring Windows execution path for the first time, and the soak's original five crates are all preserved.
- All twelve `live_fs_acl` ACL tests and the previously unselected containment gate marker are selected by a runner that executes, with the live-acceptance flag proven effective rather than assumed (REQ-native-r1, r2, r5).
- CI fires on this branch and has produced at least one run, so the macOS and self-hosted Windows legs have compiled this tree.
- The wrong-OS anti-drift guard is intact and no target was reclassified (REQ-native-r8); the retained-handle, AppContainer-ACL and Job-Object targets are confirmed still green (REQ-native-r7).
- The four Windows suites are re-measured with actual counts, every failure named and bucketed as blocker A, blocker B, or NEW-with-severity, giving 20A-02 and 20A-03 a measured baseline to state their deltas against (REQ-native-r4).
- The tree was pristine and `--locked`-consistent before any measurement (REQ-native-r10, r15).
- MEDIUM-and-below findings are logged to BACKLOG and explicitly do not block; any NEW CRITICAL or HIGH finding terminated the plan and escalated to Sean rather than being absorbed.
- No test was deleted, weakened, re-gated, `#[ignore]`d or `#[allow]`ed, and no behavioural repair was attempted in this plan.
</success_criteria>

## Artifacts this phase produces
- `.planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md` — the pinned SHA, the two compile verdicts, the four re-measured suites with every failure named and bucketed, and the severity-classified finding register.
- `scripts/wayland-e2e-windows-soak.ps1`, `scripts/f20-native-windows-proof.ps1`, `.github/workflows/ci.yml` — the three wirings that end the blind spot.
- `.planning/BACKLOG.md` — the MEDIUM-and-below findings, explicitly non-blocking.

<output>
Create `.planning/phases/20A-native-windows-macos-uat/20A-01-SUMMARY.md` using the standard GSD summary template. Record: the pinned SHA and the box's prior SHA as actually read; the pristine-tree and `--locked` results; the per-crate compile verdict for the Windows-only bodies and the macOS-only bodies with the exact commands and the machine each ran on; the three wirings with the gate output proving each selects what it claims; the live-acceptance flag proof; the four re-measured suite counts with every failure named and bucketed as blocker A / blocker B / NEW-with-severity, stated against the `c39f7254` prediction and noting any divergence; confirmation that the retained-handle, AppContainer-ACL and Job-Object native targets are still green; the BACKLOG entries created; and which of the three termination states the plan ended in. Mark no requirement complete — this plan measures and wires; closure is claimed by the downstream native proof.
</output>
