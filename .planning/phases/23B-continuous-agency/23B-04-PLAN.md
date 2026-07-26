---
phase: 23B-continuous-agency
plan: "04"
type: execute
wave: 4
depends_on:
  - "23B-01"
  - "23B-02"
  - "23B-03"
files_modified:
  - crates/wcore-agent/tests/multi_day_journey_test.rs
  - scripts/f23-multi-day-journey.sh
  - scripts/f23-multi-day-journey.ps1
  - .planning/phases/23B-continuous-agency/evidence/
  - .planning/phases/23B-continuous-agency/23B-04-CLOCK-DECISION.md
  - .planning/phases/23B-continuous-agency/23B-04-LIVE-EVIDENCE.md
  - .planning/phases/23B-continuous-agency/23B-04-D2-OWED.md
autonomous: false
requirements:
  - F23-05
domain: code
must_haves:
  truths:
    - "A MULTI-DAY JOURNEY IS PROVED BY ELAPSING DAYS, NOT BY ASSERTING THEM. Success Criterion 5 says a multi-day wait, resume and complete journey preserves cumulative authority, resource, evidence, memory and delivery state. A test that advances an injected clock proves the code reads a clock. It does not prove the process survived a real restart, that a persisted deadline computed on Monday still means the same thing on Thursday, or that the operating system, the filesystem and the session store all behaved across that span. At least one leg of this journey runs against real elapsed wall time with real process restarts."
    - "THE WALL-CLOCK AUTHORITY IS WHY AN ACCELERATED CLOCK IS NOT UNIVERSALLY SAFE HERE. `crates/wcore-agent/src/budget_authority.rs` models budget wall-clock authority as either active-runtime or an absolute deadline. An absolute deadline is meaningful precisely because it is anchored to real time that keeps passing while the process is dead. Accelerating a clock past an absolute deadline proves the comparison operator works; it does not prove the deadline survived three days of the process not existing. The active-runtime form has no such dependency and may legitimately be accelerated. Which legs use which is a real decision with a real evidence cost, and it is taken at a blocking checkpoint rather than absorbed into an implementation detail."
    - "EXACTLY ONE LOOP OWNER IS THE LOAD-BEARING INVARIANT AND THE MOST LIKELY THING TO BREAK ACROSS A RESTART. F22-04 establishes that direct, workflow, fleet, council and forge execution run as explicit strategies under exactly one outer loop owner. A restart is where a second owner appears: the resumed process starts a loop while a supervisor, a scheduled wake, or an orphaned child resumes another. The journey must assert one owner at every resume point, and the assertion must be over observed runtime state, not over a configuration value."
    - "THIS PLAN IS THE PHASE'S TERMINAL PLAN AND ALONE OWNS THE PHASE-LEVEL DISPOSITION. Following the program's acceptance contract, predecessor plans record their own requirement dispositions but no predecessor summary closes the phase. This plan runs the aggregate proof against one exact SHA and states the phase outcome. It does not re-litigate a predecessor's finding; it authenticates what they recorded."
    - "D2 CANNOT BE CLOSED FROM THIS REPOSITORY AND THIS PLAN MUST NOT PRETEND OTHERWISE. The Phase 23 exit gate D2 (CTRL-04) freezes durable Goal, child, task, wait commands, events, cursors, delivery, approval, failure and reconnect semantics AND requires replaying the canonical serialized fixtures through the REAL Desktop consumer and reducer. Deserialization alone is explicitly insufficient. The consumer side lives in the Desktop repository under the linked Desktop lane. Core owes D2 its frozen producer contract and its canonical serialized fixtures — which Phase 22 Success Criterion 1 emits — and nothing more. This plan names what Core owes, records whether it is ready, and STOPS at the boundary."
    - "A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, add an ignore or allow attribute, raise a timeout, re-gate, or delete an inconvenient test to reach a gate. Phase 20's terminal plan correctly recorded a RED aggregate and left every requirement incomplete rather than ticking a box; that was the right call and it is the standard here."
    - "A TERMINAL ACCEPTANCE PLAN WHOSE GATES CANNOT FAIL ACCEPTS NOTHING, AND THE PREVIOUS REVISION HAD THREE SUCH GATES. Task 3's SHA-pin gate ended `git status --porcelain | head -5` — a pipeline whose exit status is HEAD's, which is always zero, so the pin check could not detect a dirty tree OR a wrong checkout, the exact tampering `T-23B04-07` claims to mitigate. Task 2's journey had NO automated step on Windows or macOS at all: two of the three platform claims for Success Criterion 5 rested entirely on grepping an evidence file the executor itself wrote, which is a tautology and not a proof. And `cargo build --locked` ran without ever asserting the host's checkout equalled the pinned SHA. All three are closed here. For every command written into a `<verify>` block, answer 'what makes this go red?' before writing it."
    - "THE macOS LEG HAD NO BINARY, AND THE ARTIFACT IT NAMED DOES NOT EXIST. The previous revision ran the macOS journey against a 'PREBUILT wayland-core artifact only, no local Cargo'. Measured against `.github/workflows/`: `ci.yml` uploads only `nextest-junit-${{ matrix.os }}` JUnit XML and no binary of any kind, and `release.yml` builds Darwin binaries only on a `v*-wayland-*` tag push or an explicit dispatch — both Sean-only, as is pushing. No such artifact is reachable from inside this phase. The macOS journey leg now runs against the binary `scripts/f23-macos-binary.sh` resolves, and every leg's binary must prove its own provenance through `--build-info`."
  artifacts:
    - path: .planning/phases/23B-continuous-agency/23B-04-CLOCK-DECISION.md
      provides: "The recorded decision on which journey legs run against real elapsed wall time and which may use an accelerated clock, with the evidence cost of each option stated and the authorization recorded verbatim"
    - path: scripts/f23-multi-day-journey.sh
      provides: "The self-recording journey driver: it starts the journey, is re-invoked on each subsequent day, resumes, asserts the invariants at every resume point, and appends to an append-only run log"
    - path: crates/wcore-agent/tests/multi_day_journey_test.rs
      provides: "The committed, always-run regression form of the journey's invariants over persisted state, so the multi-day proof is reproducible in CI at accelerated scale"
    - path: .planning/phases/23B-continuous-agency/23B-04-LIVE-EVIDENCE.md
      provides: "The recorded journey outcome per day per platform, the phase-level aggregate proof at one exact SHA, and the disposition of every Phase 23B requirement and Success Criterion"
    - path: .planning/phases/23B-continuous-agency/23B-04-D2-OWED.md
      provides: "What Core owes the D2 exit gate, whether it is ready, and the explicit statement that the Desktop consumer and reducer replay half cannot be closed from this repository"
  key_links:
    - from: .planning/phases/23B-continuous-agency/23B-04-CLOCK-DECISION.md
      to: scripts/f23-multi-day-journey.sh
      via: "the authorized clock policy bounding which legs the driver may accelerate"
      pattern: "decision-record"
    - from: scripts/f23-multi-day-journey.sh
      to: .planning/phases/23B-continuous-agency/23B-04-LIVE-EVIDENCE.md
      via: "the append-only per-day run log promoted into the recorded journey outcome"
      pattern: "live-evidence"
    - from: .planning/phases/23B-continuous-agency/23B-04-D2-OWED.md
      to: .planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md
      via: "the named exit-gate boundary Core cannot cross alone"
      pattern: "gate-boundary"
---

<objective>
Make Success Criterion 5 true by actually running a multi-day wait, resume and complete journey across real process restarts and real elapsed time, proving cumulative authority, resource, memory, evidence and delivery state survive with exactly one loop owner; then close Phase 23B with one aggregate proof at one exact SHA and one honest statement of what the D2 exit gate still requires and why Core cannot supply it.

Purpose: F23-05 is the criterion that ties the other four together — it resumes through 23B-01's session verbs, recalls through 23B-02's memory, and carries the authority and budget model Phase 21 established. It is also the criterion most easily faked, because "multi-day" is trivially assertable and expensively provable. This plan is the phase's terminal plan and alone states the phase outcome.
Output: One recorded clock-policy decision; one genuinely multi-day journey run per platform with an append-only per-day log; the committed regression form of its invariants; one aggregate build-and-test proof at one exact SHA on the authoritative Linux host; and one recorded statement of Core's D2 obligation and its boundary.
</objective>

<execution_context>
@$HOME/.codex/gsd-core/workflows/execute-plan.md
@$HOME/.codex/gsd-core/templates/summary.md
</execution_context>

<context>
@AGENTS.md
@.planning/HANDOFF-2026-07-26-phase20-20A-complete.md
@.planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md
@crates/wcore-agent/src/budget_authority.rs
@crates/wcore-agent/src/recovery.rs
@.planning/phases/23B-continuous-agency/23B-01-LIVE-EVIDENCE.md
@.planning/phases/23B-continuous-agency/23B-02-LIVE-EVIDENCE.md
@.planning/phases/23B-continuous-agency/23B-03-LIVE-EVIDENCE.md
</context>

<execution_rules>

**THE TWO AMENDED PHASE RULES — verbatim, and they bound this plan.**

- Findings at CRITICAL or HIGH must be fixed or disproved. MEDIUM and below are logged to BACKLOG and DO NOT BLOCK execution.
- Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean.

**TERMINATION CRITERION FOR THIS PLAN (hard).** This plan takes ONE clock decision, runs ONE journey, and states the phase outcome ONCE. It terminates in exactly one of four states, and in all four it writes its SUMMARY and stops:
1. **Phase complete** — the journey passed on every platform, the aggregate proof is green at one exact SHA, and every Phase 23B requirement is complete.
2. **Phase complete with named open requirements** — the journey and aggregate proof ran honestly and one or more requirements remain open. Record each with its blocking evidence. Phase 20A closed exactly this way on three met Success Criteria with four requirements explicitly open, and that was correct.
3. **Aggregate RED** — the aggregate proof failed. Record the exact failures with their evidence, leave every requirement incomplete, and STOP. Phase 20's terminal plan did precisely this and it was the right call. Do not attempt repairs inside this plan.
4. **Escalated** — the clock decision was declined at the checkpoint, or a CRITICAL or HIGH finding requires work outside this plan's declared files.
Under no circumstances does this plan create additional plans, repair a predecessor's finding, or start a second journey cycle.

**THIS PLAN DOES NOT MODIFY PRODUCT SOURCE.** Its declared files are one test file, two driver scripts and three planning records. If the journey uncovers a product defect, that is a finding recorded with its severity — a CRITICAL or HIGH one puts the phase in termination state 3 or 4. It is not repaired here, because a terminal acceptance plan that also writes the code it accepts collapses the author and reviewer identity the program's acceptance contract keeps separate.

**D2 IS NAMED, NOT CLOSED.** The Phase 23 exit gate D2 (CTRL-04) freezes durable Goal, child, task, wait commands, events, cursors, delivery, approval, failure and reconnect semantics, and requires replaying canonical serialized fixtures through the REAL Desktop consumer and reducer — deserialization alone is explicitly insufficient. That consumer lives in the Desktop repository under the linked Desktop lane and cannot be reached from here. Core's obligation is the frozen producer contract plus the canonical serialized producer fixtures that Phase 22 Success Criterion 1 emits. This plan records that obligation and its readiness and stops at the boundary. Any assertion that this repository alone satisfies the exit gate is false and blocks the phase — and note that Task 3 mechanically rejects such an assertion in `23B-04-D2-OWED.md`, so do not paraphrase this rule INTO that file.

**SUCCESS CRITERION 1 IS PHASE 23A'S.** Governed skill promotion, quarantine, revocation and rollback are not in 23B's scope and are an admitted input. This plan's phase-level statement covers Success Criteria 2 through 6 and says so explicitly; it does not claim Phase 23 as a whole is complete.

**NON-NEGOTIABLE.** A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. The specific temptations here are to accelerate the clock on the leg where it invalidates the proof, to run the journey's days back to back in one afternoon and call it multi-day, and to tick a requirement because its code exists rather than because its criterion was observed. All three are engineered greens and all three are forbidden.

**ENVIRONMENT.**
- Linux (authoritative aggregate proof AND the long-lived journey host): `ssh -o BatchMode=yes hetzner-dsm`, `/root/wayland`. Full workspace aggregate is 11,519 tests in roughly 194 seconds on this host. It is the only host that stays up unattended for days.
- Windows (native journey leg): `ssh -o BatchMode=yes SeanD@seandesktop`, checkout `C:\ferrox-win`, cargo at `C:\Users\seand\.cargo\bin\cargo.exe`. The remote default shell is PowerShell, so an `ssh` command string is PowerShell source and must end with an explicit `exit $LASTEXITCODE` for the status to propagate. This box reboots and is shared; the journey must survive that, which is a feature of the test rather than a problem with it.
- macOS (native journey leg): THIS Mac. See the macOS binary decision below.

**GATE DISCIPLINE — every command in a `<verify>` block must be able to go RED. Three hard rules, each closing a defect this plan actually shipped.**

1. **A gate is NEVER a pipeline into a filter, and never ends in a command that always succeeds.** The previous revision's SHA-pin gate ended `git status --porcelain | head -5`; a pipeline reports its LAST command's status and `head` is always zero, so the gate could not detect a dirty tree or a wrong checkout — precisely what `T-23B04-07` claims to mitigate. The same class covers `ssh host 'cmd' | grep -v CLIXML`, which reports grep's status, not ssh's. Redirect, capture the status on the NEXT line, assert on it, and only then read the log:
   `ssh -o BatchMode=yes HOST "…; exit \$LASTEXITCODE" > LOG 2>&1; rc=$?; test "$rc" -eq 0 && /usr/bin/grep -qF "MARKER" LOG`
2. **Never read an exit code from a block that also emits output.** In PowerShell, `$x = & { cargo … | Tee-Object …; $LASTEXITCODE }` returns an ARRAY of every output line plus the code, so `if ($x -ne 0)` is an always-truthy array filter. That bug made an all-PASS 12/12 + 6/6 Windows soak report failure; the fix and its post-mortem are in `scripts/wayland-e2e-windows-soak.ps1:174-190` and `:244-255`. Read `$LASTEXITCODE` on the line AFTER the pipeline, and always end a driver with an explicit `exit`.
3. **Every platform claim needs a command that actually runs on that platform.** Success Criterion 5 is a three-platform claim; the previous revision automated only Linux and closed Windows and macOS by grepping the evidence file this plan itself writes. Each platform's journey now ends in a real `--verify` invocation on that platform whose process exit status is the gate, with a caller-generated nonce as a second, independent check.

**macOS BINARY SOURCE — DECIDED IN 23B-01 AND CARRIED HERE, WITH ITS BASIS AND ITS MEASUREMENTS.** The previous revision ran the macOS journey against a "PREBUILT `wayland-core` artifact only". That artifact does not exist and cannot be produced from inside this phase. Measured, not assumed: `.github/workflows/ci.yml:204-208` uploads only `nextest-junit-${{ matrix.os }}` — JUnit XML, no binary of any kind, on any branch; `.github/workflows/release.yml:1-24` fires only on a `v*-wayland-*` tag push, a `workflow_call`, or an explicit `workflow_dispatch`, and its Darwin targets at `:70-74` therefore never build for `plan/f20-unified-audit-repair`. Tagging, releasing, dispatching and pushing are all Sean-only, so no CI run producing a macOS binary can be triggered from inside plan execution. **Decision: the macOS leg builds its own binary on this Mac, through `scripts/f23-macos-binary.sh`, which 23B-01 owns and this plan consumes unchanged.** Basis: HANDOFF §3 item 7 — "This Mac CAN compile the workspace. The old 'never compiles on Mac' note is a workflow convention, not a fact" — plus the pinned toolchain `1.95.0-aarch64-apple-darwin` present under `~/.rustup/toolchains` and matching `rust-toolchain.toml`. **`hetzner-dsm` remains the sole authority for the aggregate proof in Task 3; the Mac build produces a JOURNEY TARGET only, isolated in `--target-dir target/f23-macos`, which the existing `/target/` ignore rule already covers.** The resolver asserts the binary's `--build-info` source SHA equals the commit under test, so a multi-day journey cannot silently switch binaries mid-span. If the Mac build fails, that is a RED to record: the macOS journey rows go OPEN with the compiler's exact error under termination state 2. It is never a silent skip. If `scripts/f23-macos-binary.sh` is absent because 23B-01 did not land it, STOP and record that as a blocking dependency rather than improvising a second resolver.
- ALWAYS `/usr/bin/grep` on the Mac with `-F` for literals.
- Always `git fetch origin plan/f20-unified-audit-repair` explicitly. In the Mac repo `origin` is a stale local worktree; the real remote is `gh`.
- The aggregate proof pins ONE exact SHA and verifies the host's checked-out HEAD equals it BEFORE any build step, the way Phase 20A's candidate jobs asserted the authorized SHA against the real checkout.
- NO push to main, merge, PR, tag, release, deployment, canary or issue closure. Those are Sean-only.

**AGENTS.md discipline.** Surgical diffs. Clippy-clean with warnings denied for the one test file added. No `Co-Authored-By` trailers. Stage the exact paths in `files_modified`, never `-A`, never `.`.
</execution_rules>

<tasks>

<task type="checkpoint:decision" gate="blocking">
  <name>Task 1 (BLOCKING DECISION): Authorize the clock policy for the multi-day journey</name>
  <files>.planning/phases/23B-continuous-agency/23B-04-CLOCK-DECISION.md</files>
  <action>Before writing the driver, present the clock policy to Sean and obtain one authorization. Record the selected option and the evidence cost accepted with it verbatim in `23B-04-CLOCK-DECISION.md`. The journey driver may not be written until this is recorded, because the policy determines what the driver is allowed to simulate.

Record, each on its own line and in exactly this machine-readable form, the minimum REAL elapsed span each platform's leg must show before that platform may be claimed: `linux_required_real_span_seconds=<n>`, `macos_required_real_span_seconds=<n>`, `windows_required_real_span_seconds=<n>`. A leg the policy permits to accelerate records `0` and is labelled a weaker claim in the published evidence. Task 2's span gate reads these three lines and compares them against the span the run log's own first and last timestamps produce; without them the elapsed span can only be gated on the driver's own say-so, which is what "multi-day" being trivially assertable means in practice.

First establish, single-variable and on the record, whether the absolute-deadline form of the budget wall-clock authority derives its comparison from the system clock at evaluation time or from a value captured at reservation time. That determination decides whether an accelerated clock can honestly exercise it at all, and it must be measured against the code rather than assumed from the type name. Record the finding with its evidence before presenting the options.</action>
  <decision>Which legs of the multi-day wait, resume and complete journey run against real elapsed wall time, and which may use an accelerated clock?</decision>
  <context>
Success Criterion 5 requires that a multi-day journey preserves cumulative authority, resource, memory, evidence and delivery state. The budget model distinguishes an active-runtime wall-clock authority, which accumulates only while the process runs, from an absolute deadline, which is anchored to real time that keeps passing while the process is dead. Those two have genuinely different evidence requirements: accelerating a clock past an absolute deadline proves a comparison operator, not that a deadline computed on Monday still means the same thing on Thursday after three restarts. Real elapsed time costs days of calendar and occupies three hosts; an accelerated clock costs evidence strength. The point of this checkpoint is that the cost is chosen rather than absorbed. Task 1's determination of how the absolute deadline is actually evaluated is recorded in `23B-04-CLOCK-DECISION.md` and must be read before choosing, because it may remove one of these options.
  </context>
  <options>
    <option id="real-time-full">
      <name>Full real elapsed time — the journey spans at least three real calendar days on all three platforms with real process restarts and no clock manipulation anywhere</name>
      <pros>The strongest possible evidence and the only one that proves a persisted deadline, a session store, a journal and an operating system all behaved across days of the process not existing; it also incidentally exercises the Windows box's real reboots, which is the environment a user actually has; no argument about what was simulated</pros>
      <cons>Costs at least three calendar days of elapsed wall time before the phase can close, and occupies the Linux host, the Windows box and this Mac for that span; a defect found on day three costs another full cycle to re-prove</cons>
    </option>
    <option id="real-time-linux-accelerated-elsewhere">
      <name>Real elapsed time on the long-lived Linux host, accelerated clock on the macOS and Windows legs</name>
      <pros>Keeps the strongest evidence on the one host that genuinely stays up unattended for days, while the two attended machines finish in an afternoon; the accelerated legs still perform real process restarts and real persistence, so only the time span is simulated; total calendar cost is the same three days but only one host is occupied</pros>
      <cons>The macOS and Windows platform claims are weaker than the Linux one and must be labelled that way in the evidence rather than presented as equivalent; a platform-specific time or persistence defect could hide on exactly the two platforms that were accelerated</cons>
    </option>
    <option id="accelerated-except-absolute-deadline">
      <name>Accelerated clock everywhere except the absolute-deadline leg, which always runs against real elapsed time on every platform</name>
      <pros>Targets the cost precisely at the one authority whose meaning depends on real time passing while the process is dead, and leaves everything with no real-time dependency free to run fast; finishes in roughly one day of calendar</pros>
      <cons>Depends entirely on Task 1's determination being correct about which behaviors have a genuine real-time dependency; if that determination is wrong, an accelerated leg silently proves nothing and the error is invisible in the evidence</cons>
    </option>
    <option id="escalate">
      <name>Escalate — none of the above buys evidence worth its cost right now, so record the decision as open and do not run the journey</name>
      <pros>Spends no calendar time on a proof whose shape is not yet agreed, and leaves the criterion visibly open rather than closed on evidence Sean does not accept</pros>
      <cons>Success Criterion 5 stays open, Phase 23B cannot close, and the criterion most likely to reveal a cross-restart defect goes unexercised</cons>
    </option>
  </options>
  <resume-signal>Select: real-time-full, real-time-linux-accelerated-elsewhere, accelerated-except-absolute-deadline, or escalate. If selecting an option with any accelerated leg, confirm explicitly that the weaker platform claim will be labelled as such in the published evidence.</resume-signal>
  <verify>
    <human-check>`23B-04-CLOCK-DECISION.md` records the single-variable determination of how the absolute-deadline authority is actually evaluated, with the evidence for it; the authorized option; and the evidence cost accepted with it, stated verbatim. The selected option is one the determination actually left available.</human-check>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cE 'real-time-full|real-time-linux-accelerated-elsewhere|accelerated-except-absolute-deadline|escalate' .planning/phases/23B-continuous-agency/23B-04-CLOCK-DECISION.md)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; for P in linux macos windows; do /usr/bin/grep -qE "^${P}_required_real_span_seconds=[0-9]+$" .planning/phases/23B-continuous-agency/23B-04-CLOCK-DECISION.md || exit 1; done</automated>
  </verify>
  <done>One clock policy is authorized and recorded with its accepted evidence cost, on top of a measured determination of how the absolute-deadline authority is evaluated. The three `*_required_real_span_seconds=` lines are present and machine-readable, so Task 2's span gate has an authorized threshold to compare against rather than the driver's own claim. If escalate was selected, Tasks 2 and 3 do not run: write the SUMMARY recording termination state 4 and stop.</done>
</task>

<task type="auto">
  <name>Task 2: Run the multi-day wait, resume and complete journey for real, under the authorized clock policy</name>
  <files>scripts/f23-multi-day-journey.sh, scripts/f23-multi-day-journey.ps1, crates/wcore-agent/tests/multi_day_journey_test.rs, .planning/phases/23B-continuous-agency/evidence/, .planning/phases/23B-continuous-agency/23B-04-LIVE-EVIDENCE.md</files>
  <read_first>.planning/phases/23B-continuous-agency/23B-04-CLOCK-DECISION.md (the authorized policy — it bounds what this driver may simulate), crates/wcore-agent/src/budget_authority.rs (the wall-clock authority forms, how a reservation is recorded, and how a refund or an exhaustion is attributed), crates/wcore-agent/src/recovery.rs (how a resume reconstructs state after a kill, and the eight reconcile reasons a resume may surface), crates/wcore-agent/src/session_journal/reducer.rs entry points only (how recorded events fold into resumed state — the file is 164 KB, read the entry points and the snapshot boundary), crates/wcore-agent/src/durable_child.rs and durable_spawner.rs (what survives a restart on the delegation side and who owns the loop after one), scripts/f23-macos-binary.sh and scripts/f23-session-operator-drive.sh (23B-01's macOS binary resolver, consumed unchanged, and 23B-01's driver, whose resume, inspect and reconcile verbs this journey uses rather than reimplements, and whose `--binary` / `--sha` / `--nonce` contract, `--build-info` provenance assertion and nonce-bound terminal marker this driver reuses), scripts/wayland-e2e-windows-soak.ps1 lines 174-190 and 244-255 (the worked example of PowerShell exit-code capture and the post-mortem on the `$x = &amp; { … ; $LASTEXITCODE }` array-filter bug that reported a fully passing run as a failure), .planning/phases/23B-continuous-agency/23B-02-LIVE-EVIDENCE.md (the memory controls whose state must survive the journey)</read_first>
  <behavior>
    - The driver takes `--binary <path>`, `--sha <commit>` and `--nonce <hex>` — the same contract 23B-01's driver established — and asserts the binary's own `--build-info` source SHA equals `--sha` on EVERY invocation, so a multi-day journey cannot silently switch binaries mid-span. On macOS the binary comes from `scripts/f23-macos-binary.sh`, which 23B-01 owns and this plan consumes unchanged.
    - The driver supports a `--verify --nonce <hex>` mode that runs on the platform whose journey it is checking. It re-reads the append-only run log and the persisted state, recomputes the elapsed span from the log's own first and last timestamps, re-asserts every invariant, writes one `F23_04_SPAN_SECONDS=<n>` line, and emits `F23_04_JOURNEY=PASS platform=&lt;linux|macos|windows&gt; nonce=&lt;the given nonce&gt;` ONLY if the journey reached its terminal Goal transition and every invariant held. Any failure exits non-zero and emits no PASS marker. This mode is what each platform's gate runs, so every platform claim rests on a command that actually executed on that platform.
    - The recomputed span is compared against that platform's `*_required_real_span_seconds` value from `23B-04-CLOCK-DECISION.md`; a span shorter than the authorized threshold fails, so a journey run back to back in one afternoon cannot be reported as multi-day.
    - The driver is invoked once to START the journey and once per subsequent day to RESUME it, and it is idempotent per day so a double invocation does not double-count.
    - Every invocation appends to an append-only run log carrying the wall-clock timestamp, the host, the process identity, the session and Goal identity, and the invariant results — so the elapsed span is evidenced by the log's own timestamps rather than by a claim.
    - Between days the process genuinely does not exist: the driver exits and nothing of it remains resident.
    - At every resume point the driver asserts, over observed runtime state rather than configuration: exactly one loop owner exists; cumulative token and cost reservations carry forward rather than resetting; the authority envelope is no wider than it was on day one; memory written on day one is still recalled; the journal and its evidence chain are continuous across the gap; and no delivery is duplicated or lost.
    - A wait that was pending on day one is still pending on day two and completes on the day its condition is met, not on the first resume after it.
    - The journey completes through a real terminal Goal transition, not by the driver declaring it complete.
    - Any resume that surfaces an unknown-effect reconcile item is recorded with which reason was reported and how it was resolved, using 23B-01's reconcile verb.
    - The Windows leg survives at least one real reboot of the shared box, or records that no reboot occurred during the span.
    - The driver exits non-zero if any invariant fails, and a failed invariant halts the journey rather than being logged and stepped over.
    - The committed test file reproduces every invariant over persisted state at accelerated scale, so the proof is repeatable in CI without a multi-day wait.
  </behavior>
  <action>Write `scripts/f23-multi-day-journey.sh` and its PowerShell port, obeying the authorized clock policy exactly — the driver may accelerate only the legs the decision permits, and it must record for each leg whether that leg's span was real or simulated.

Start the journey with a durable objective that has a wait condition satisfiable only after the span: a real Goal with a pending wait, a memory fact written on day one, a budget reservation under the wall-clock authority form the decision named, and a delegated child whose result must be delivered exactly once. Exit the process fully.

On each subsequent day, re-invoke the driver. It resumes through 23B-01's session verbs rather than reimplementing resume, asserts every invariant over observed runtime state, appends its results to the run log, and exits. Assert the single loop owner by observing what is actually running, not by reading a configuration value — a second owner is the defect this journey exists to catch and it will not appear in configuration.

Run the journey on all three platforms under the authorized policy: Linux on `hetzner-dsm`, which is the only host that stays up unattended; Windows on `SeanDesktop`, recording whether a real reboot occurred inside the span, with every ssh command string ending in an explicit `exit $LASTEXITCODE` and never piped into a filter; macOS on this Mac against the binary `scripts/f23-macos-binary.sh` resolves. Where the policy permits acceleration, label that leg's claim as weaker in the evidence rather than presenting it as equivalent. When each platform's journey is complete, close it with that platform's own `--verify --nonce` invocation: its process exit status is the platform gate, and the nonce-bound PASS marker is the second, independent check. A platform whose `--verify` did not run is OPEN, never inferred from another platform's result.

Add `crates/wcore-agent/tests/multi_day_journey_test.rs` reproducing every invariant over persisted state at accelerated scale so the proof is repeatable in CI. This test is the regression form, not the proof — the proof is the run log with its real timestamps, and the SUMMARY must say so plainly so a future reader does not mistake a green CI test for the multi-day evidence.

Write `23B-04-LIVE-EVIDENCE.md` carrying, per platform and per day: the wall-clock timestamp, whether the span was real or simulated, each invariant's result, the reconcile items surfaced and how they were resolved, and the terminal Goal transition. Include the elapsed span computed from the log's own first and last timestamps. Marks F23-05 complete only if every invariant passed on every platform under the authorized policy; otherwise record the disposition and leave it incomplete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -x scripts/f23-multi-day-journey.sh &amp;&amp; test -f scripts/f23-multi-day-journey.ps1 &amp;&amp; bash -n scripts/f23-multi-day-journey.sh</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch -q origin plan/f20-unified-audit-repair &amp;&amp; git checkout -q --detach $SHA &amp;&amp; test -f crates/wcore-agent/tests/multi_day_journey_test.rs &amp;&amp; cargo nextest run -p wcore-agent --profile ci --test multi_day_journey_test --no-tests=fail --no-fail-fast"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; NONCE=$(/usr/bin/openssl rand -hex 8) &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; REQ=$(/usr/bin/grep -oE '^linux_required_real_span_seconds=[0-9]+' .planning/phases/23B-continuous-agency/23B-04-CLOCK-DECISION.md | cut -d= -f2) &amp;&amp; test -n "$REQ" &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-04-linux-verify.log &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo build --release -p wcore-cli --bin wayland-core &amp;&amp; bash scripts/f23-multi-day-journey.sh --verify --binary target/release/wayland-core --sha $SHA --nonce $NONCE --min-span-seconds $REQ" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0 &amp;&amp; /usr/bin/grep -qF "F23_04_JOURNEY=PASS platform=linux nonce=$NONCE" "$L"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; test "$(uname -s)" = Darwin &amp;&amp; NONCE=$(/usr/bin/openssl rand -hex 8) &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; REQ=$(/usr/bin/grep -oE '^macos_required_real_span_seconds=[0-9]+' .planning/phases/23B-continuous-agency/23B-04-CLOCK-DECISION.md | cut -d= -f2) &amp;&amp; test -n "$REQ" &amp;&amp; BIN=$(bash scripts/f23-macos-binary.sh) &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-04-macos-verify.log &amp;&amp; bash scripts/f23-multi-day-journey.sh --verify --binary "$BIN" --sha "$SHA" --nonce "$NONCE" --min-span-seconds "$REQ" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0 &amp;&amp; /usr/bin/grep -qF "F23_04_JOURNEY=PASS platform=macos nonce=$NONCE" "$L"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; NONCE=$(/usr/bin/openssl rand -hex 8) &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; REQ=$(/usr/bin/grep -oE '^windows_required_real_span_seconds=[0-9]+' .planning/phases/23B-continuous-agency/23B-04-CLOCK-DECISION.md | cut -d= -f2) &amp;&amp; test -n "$REQ" &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-04-windows-verify.log &amp;&amp; ssh -o BatchMode=yes SeanD@seandesktop "Set-Location C:\ferrox-win; git fetch -q origin plan/f20-unified-audit-repair; git checkout -q --detach $SHA; if (\$LASTEXITCODE -ne 0) { exit 91 }; cargo build --release -p wcore-cli --bin wayland-core; if (\$LASTEXITCODE -ne 0) { exit 90 }; powershell -NoProfile -ExecutionPolicy Bypass -File scripts\f23-multi-day-journey.ps1 -Verify -Binary target\release\wayland-core.exe -Sha $SHA -Nonce $NONCE -MinSpanSeconds $REQ; exit \$LASTEXITCODE" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0 &amp;&amp; /usr/bin/grep -qF "F23_04_JOURNEY=PASS platform=windows nonce=$NONCE" "$L"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; for P in linux macos windows; do N=$(/usr/bin/grep -oE "nonce=[0-9a-f]{16}" ".planning/phases/23B-continuous-agency/evidence/23B-04-$P-verify.log" | tail -1) &amp;&amp; test -n "$N" &amp;&amp; /usr/bin/grep -qF "$N" .planning/phases/23B-continuous-agency/23B-04-LIVE-EVIDENCE.md || exit 1; done</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cE 'day-[0-9]' .planning/phases/23B-continuous-agency/23B-04-LIVE-EVIDENCE.md)" -ge 9</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cE 'loop-owner|cumulative-budget|authority-envelope|memory-recall|evidence-chain|delivery-once' .planning/phases/23B-continuous-agency/23B-04-LIVE-EVIDENCE.md)" -ge 18</automated>
  </verify>
  <done>The journey ran on all three platforms under the authorized clock policy, and each platform closed with its OWN `--verify --nonce` invocation executed on that platform, exiting zero and echoing its caller-generated nonce in the PASS marker — Linux over ssh to `hetzner-dsm`, Windows over ssh to `SeanDesktop` with the status carried by an explicit `exit $LASTEXITCODE` and never through a pipeline, and macOS locally against the binary `scripts/f23-macos-binary.sh` resolved. Each recomputed span met that platform's authorized `*_required_real_span_seconds` threshold. The Linux run log's own first and last timestamps evidence the elapsed span, and the process did not exist between days. Every invariant — one loop owner, cumulative budget carry-forward, unwidened authority, day-one memory recall, continuous evidence chain, exactly-once delivery — is recorded per day per platform. The wait completed on its condition rather than on the first resume. Any accelerated leg is labelled as a weaker claim. The committed regression test passes and the SUMMARY states plainly that it is the regression form and not the multi-day evidence. F23-05's disposition is recorded.</done>
</task>

<task type="auto">
  <name>Task 3: Aggregate proof at one exact SHA, phase disposition, and the D2 boundary statement</name>
  <files>.planning/phases/23B-continuous-agency/evidence/23B-04-pinned-sha.txt, .planning/phases/23B-continuous-agency/evidence/, .planning/phases/23B-continuous-agency/23B-04-LIVE-EVIDENCE.md, .planning/phases/23B-continuous-agency/23B-04-D2-OWED.md</files>
  <read_first>.planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md in full (both D1 and D2 clauses, and the closing statement that Core Phase 22 proves producer fixtures and standalone and host protocol behavior while the linked Desktop lane proves consumer replay and control behavior, that both receipts are required for a whole-Wayland claim, and that neither blocks Core-only engine claims outside the shared contract), .planning/phases/23B-continuous-agency/23B-01-LIVE-EVIDENCE.md and 23B-02-LIVE-EVIDENCE.md and 23B-03-LIVE-EVIDENCE.md (the predecessor dispositions this task authenticates rather than re-derives), .planning/ROADMAP.md Phase 23 section (the six Success Criteria and the note that 23A owns Criterion 1 and that D2 is the exit gate), the Phase 20 terminal record of an aggregate RED left honestly incomplete (the standard for what a RED disposition looks like)</read_first>
  <behavior>
    - One exact SHA is pinned, and the authoritative Linux host's checked-out HEAD is verified equal to it BEFORE any build step runs.
    - The aggregate build runs with the lockfile locked and the workspace and all features selected, and a regenerated lockfile is a failure rather than a fixup.
    - The aggregate test run uses the continuous-integration profile with no fail-fast, and its full counts — run, passed, flaky, failed, timed out, skipped — are recorded verbatim.
    - Every Phase 23B Success Criterion, numbers two through six, receives an explicit verdict traced to the predecessor evidence file that established it.
    - Every Phase 23B requirement, F23-02 through F23-06, receives an explicit disposition. A requirement whose code exists but whose criterion was not observed is INCOMPLETE.
    - The phase statement covers Success Criteria 2 through 6 only and says so; Criterion 1 is named as Phase 23A's and is not claimed here.
    - The D2 record names what Core owes, states whether it is ready, and states without hedging that the Desktop consumer and reducer replay half cannot be closed from this repository.
    - No requirement is marked complete on the basis of a predecessor's summary alone; the evidence file it cites must exist and carry the verdict.
    - An aggregate RED leaves every requirement incomplete and the phase in termination state 3, with no repair attempted here.
  </behavior>
  <action>Pin one exact SHA and write it, alone on one line, into `.planning/phases/23B-continuous-agency/evidence/23B-04-pinned-sha.txt`, so every gate in this task reads the same pin from an artifact rather than from an ephemeral shell variable that could differ between gates. On `hetzner-dsm`, fetch the branch explicitly, check out that SHA detached, and verify the host's HEAD EQUALS it and that no tracked file is modified before running anything — Phase 20A's candidate jobs asserted the authorized SHA against the real checkout before any build step and that discipline is why its evidence is trustworthy. The previous revision's version of this gate ended `git status --porcelain | head -5`, whose exit status is `head`'s and therefore always zero: it printed the state but could not fail on a wrong or dirty checkout, which is exactly the tampering `T-23B04-07` claims to mitigate. Assert with `git diff --quiet && git diff --cached --quiet` instead, which reddens on a modified tracked file and ignores untracked build residue.

Then run the locked full-workspace all-features build and the continuous-integration-profile test run with no fail-fast, both re-asserting the pin first, and record the full counts verbatim. A regenerated lockfile or any nonzero exit is a RED, not something to fix here.

Authenticate the predecessor dispositions rather than re-deriving them: for each of Success Criteria 2 through 6, cite the evidence file and the specific rows that establish it, and state a verdict. For each of F23-02 through F23-06, state a disposition. A requirement whose implementation landed but whose criterion was never observed against the shipped product is INCOMPLETE — that distinction is the whole point of this phase's live mandate and Phase 20A's four honestly open requirements are the precedent.

State the phase outcome for Success Criteria 2 through 6 only, and name Criterion 1 as Phase 23A's rather than claiming or disclaiming it.

Write `23B-04-D2-OWED.md`. It names the D2 exit gate as freezing durable Goal, child, task, wait commands, events, cursors, delivery, approval, failure and reconnect semantics AND requiring replay of canonical serialized fixtures through the real Desktop consumer and reducer, with deserialization alone explicitly insufficient. It states Core's obligation: the frozen producer contract and the canonical serialized producer fixtures that Phase 22 Success Criterion 1 emits. It records whether that obligation is currently met, citing the actual state of the producer contract and the Desktop contract corpus rather than asserting it. And it states plainly that the consumer and reducer half lives in the Desktop repository under the linked Desktop lane and cannot be closed from here, so Phase 23's exit gate remains open regardless of this phase's outcome. Do not attempt to close it, do not create a task to close it, and do not describe the Core half as sufficient.

Marks F23-02 through F23-06 complete only where the recorded criterion verdict supports it.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; PIN=$(cat .planning/phases/23B-continuous-agency/evidence/23B-04-pinned-sha.txt) &amp;&amp; test -n "$PIN" &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch -q origin plan/f20-unified-audit-repair &amp;&amp; git checkout -q --detach $PIN &amp;&amp; test \"\$(git rev-parse HEAD)\" = \"$PIN\" &amp;&amp; git diff --quiet &amp;&amp; git diff --cached --quiet"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; PIN=$(cat .planning/phases/23B-continuous-agency/evidence/23B-04-pinned-sha.txt) &amp;&amp; test -n "$PIN" &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; test \"\$(git rev-parse HEAD)\" = \"$PIN\" &amp;&amp; cargo build --locked --workspace --all-features &amp;&amp; git diff --quiet -- Cargo.lock"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; PIN=$(cat .planning/phases/23B-continuous-agency/evidence/23B-04-pinned-sha.txt) &amp;&amp; test -n "$PIN" &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-04-aggregate.log &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; test \"\$(git rev-parse HEAD)\" = \"$PIN\" &amp;&amp; cargo nextest run --profile ci --no-tests=fail --no-fail-fast" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cE 'F23-0[23456]' .planning/phases/23B-continuous-agency/23B-04-LIVE-EVIDENCE.md)" -ge 5</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'Desktop' .planning/phases/23B-continuous-agency/23B-04-D2-OWED.md)" -ge 3</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -v '^ *#' .planning/phases/23B-continuous-agency/23B-04-D2-OWED.md | /usr/bin/grep -ciE 'core closed d2|d2 is closed|d2 complete')" -eq 0</automated>
  </verify>
  <done>One exact SHA is pinned in `evidence/23B-04-pinned-sha.txt` and every gate re-asserted the host's HEAD equalled it, with no tracked file modified, before any build step — proved by `test "$(git rev-parse HEAD)" = "$PIN" && git diff --quiet && git diff --cached --quiet`, not by a pipeline ending in a command that always succeeds. The locked all-features build and the continuous-integration test run are recorded with verbatim counts. Success Criteria 2 through 6 each carry a verdict traced to the predecessor evidence rows that establish it, and F23-02 through F23-06 each carry a disposition where implementation-without-observation reads INCOMPLETE. Criterion 1 is named as Phase 23A's. `23B-04-D2-OWED.md` names Core's D2 obligation, records its readiness against actual state, and states without hedging that the Desktop consumer and reducer replay half cannot be closed from this repository — and contains no claim that D2 is closed.</done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| day N process → day N+1 process | All state crosses through disk while no process exists; a same-UID actor or a partial write can shape what the resumed process believes |
| persisted budget reservation → resumed authority | A reservation recorded days earlier is re-honoured under a clock that has moved |
| resumed session → delegated child result delivery | A result produced before a restart is delivered after one |
| planning evidence records → phase completion claim | Recorded verdicts determine whether requirements are marked complete |

## STRIDE Threat Register

| Threat ID | Category | Component | Severity | Disposition | Mitigation Plan |
|-----------|----------|-----------|----------|-------------|-----------------|
| T-23B04-01 | Elevation of Privilege | authority envelope widening across a restart | critical | mitigate | Every resume point asserts the envelope is no wider than day one, over observed runtime state rather than configuration (Task 2) |
| T-23B04-02 | Elevation of Privilege | cumulative budget resetting across a restart | high | mitigate | Cumulative token and cost reservations are asserted to carry forward at every resume point; an absolute deadline is exercised against real elapsed time per the authorized clock policy (Task 1, Task 2) |
| T-23B04-03 | Tampering | a second loop owner appearing after a resume | high | mitigate | Single-owner assertion at every resume point over observed running state; a second owner halts the journey rather than being logged and stepped over (Task 2) |
| T-23B04-04 | Repudiation | duplicated or lost delivery of a child result across the gap | high | mitigate | Exactly-once delivery asserted at every resume point and recorded per day per platform (Task 2) |
| T-23B04-05 | Spoofing | a phase completion claim resting on code existence rather than observed behavior | high | mitigate | Every requirement disposition must cite the predecessor evidence rows that establish its criterion; implementation-without-observation reads INCOMPLETE (Task 3) |
| T-23B04-06 | Spoofing | a claim that Core closed the D2 exit gate | high | mitigate | `23B-04-D2-OWED.md` states the Desktop half cannot be closed from here, and a mechanical gate rejects any closure claim in that file (Task 3) |
| T-23B04-07 | Tampering | the aggregate proof running against a different tree than the pinned SHA | high | mitigate | The host's HEAD is verified equal to the pinned SHA before any build step, and the build runs locked so a regenerated lockfile is a failure (Task 3) |
| T-23B04-08 | Tampering | an accelerated leg presented as equivalent evidence to a real-time leg | medium | mitigate | The clock decision is taken at a blocking checkpoint with its evidence cost recorded, and every accelerated leg is labelled as a weaker claim in the published evidence (Task 1, Task 2) |
| T-23B04-SC | Tampering | package-manager installs | low | accept | This plan adds NO dependency and modifies no product source. A newly required crate would trigger the Package Legitimacy Gate and a blocking human checkpoint, and this plan STOPS rather than installing |
</threat_model>

<verification>
- `23B-04-CLOCK-DECISION.md` records the measured determination of how the absolute-deadline authority is evaluated, the authorized option, and the accepted evidence cost verbatim.
- The journey run log's own first and last timestamps evidence the elapsed span on at least the Linux leg, and the process did not exist between days.
- `cargo nextest run -p wcore-agent --profile ci --test multi_day_journey_test` green on `hetzner-dsm`.
- `cargo build --locked --workspace --all-features` exits zero with no lockfile regeneration at the pinned SHA.
- `cargo nextest run --profile ci --no-fail-fast` counts recorded verbatim at the pinned SHA.
- Each of the three platforms closed its own journey with a `--verify --nonce` invocation THAT RAN ON THAT PLATFORM, whose process exit status is the gate and whose caller-generated nonce appears in the PASS marker. No platform claim is inferred from another platform's result or from the evidence file alone.
- Each platform's recomputed span met that platform's `*_required_real_span_seconds` threshold from `23B-04-CLOCK-DECISION.md`, so the elapsed span is gated on an authorized number rather than on the driver's own claim.
- The pinned SHA lives in `evidence/23B-04-pinned-sha.txt`; every Task 3 gate re-asserts the host's HEAD equals it, and the cleanliness check is `git diff --quiet && git diff --cached --quiet` — never a pipeline ending in `head`, whose status is always zero.
- No gate in this plan is a pipeline into a filter, and no exit code is read from a block that also emits output.
- `23B-04-LIVE-EVIDENCE.md` carries at least nine day-by-platform rows and eighteen invariant results, the three verify nonces, plus a verdict for each of Success Criteria 2 through 6 and a disposition for each of F23-02 through F23-06.
- `23B-04-D2-OWED.md` contains no claim that D2 is closed, asserted mechanically.

<human-check>The elapsed span recorded in `23B-04-LIVE-EVIDENCE.md` is computed from the run log's own timestamps, not stated by the driver. If the first and last Linux timestamps are less than the span the authorized clock policy required for a real-time leg, the journey did not run and must be re-run rather than re-described.</human-check>
</verification>

<success_criteria>
- Success Criterion 5: a multi-day wait, resume and complete journey ran under an authorized and recorded clock policy on Linux, macOS and Windows, with real process restarts, and preserved cumulative authority, resource, memory, evidence and delivery state with exactly one loop owner at every resume point.
- Phase 23B's Success Criteria 2 through 6 each carry an explicit verdict traced to the live evidence that established it, and F23-02 through F23-06 each carry an explicit disposition.
- One aggregate locked all-features build and one continuous-integration test run are recorded at one exact SHA whose identity was verified against the host's checkout before any build step.
- Core's D2 obligation is named and its readiness recorded, and the Desktop consumer and reducer replay half is stated as unclosable from this repository — Phase 23's exit gate remains open regardless of this phase's outcome.
- Nothing was weakened, ignored, re-gated, timed out differently, or deleted to reach a gate; an aggregate RED leaves every requirement incomplete and is a successful, honest termination.
</success_criteria>

<output>
Create `.planning/phases/23B-continuous-agency/23B-04-SUMMARY.md` when done, recording the termination state, the authorized clock policy and its accepted evidence cost, the journey's elapsed span computed from the run log's own timestamps, the per-platform invariant results with accelerated legs labelled, the aggregate proof's verbatim counts at the pinned SHA, the verdict for Success Criteria 2 through 6, the disposition of F23-02 through F23-06, and the D2 boundary statement. State plainly that the committed regression test is the reproducible form and NOT the multi-day evidence.
</output>
