---
phase: 20A-native-windows-macos-uat
plan: "02"
type: execute
wave: 2
depends_on:
  - "20A-01"
files_modified:
  - crates/wcore-sandbox/src/backends/appcontainer.rs
  - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs
  - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs
  - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/handles.rs
  - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/tests.rs
  - crates/wcore-swarm/tests/dispatch_smoke.rs
  - .planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md
autonomous: false
requirements:
  - REQ-native-r2
  - REQ-native-r4
  - REQ-native-r6
must_haves:
  truths:
    - "THE STRUCTURAL BLOCKER, located in source and confirmed pre-existing: `admit_delegated_backend` in `crates/wcore-swarm/src/dispatch.rs` refuses a backend that answers false to `binds_workspace_authority()`, with the message `sandbox backend {} cannot bind retained delegated workspace authority`. In `crates/wcore-sandbox/src/backends/mod.rs` that predicate defaults to `binds_cwd_authority()`, which defaults to `false`, and `execute_with_cwd_authority` defaults to a fail-closed `PolicyNotSupported`. The Windows `AppContainerBackend` (`crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs`) overrides `name`, `owns_descendants_hard`, `is_available`, `enforces_read_deny` and `execute` — and overrides NEITHER cwd-authority method. So on Windows the backend is structurally inadmissible for delegated dispatch. `git diff` over the sandbox backends directory and the swarm dispatch module across the repair commits is EMPTY: this is pre-existing and structural, not a regression."
    - "THIS IS THE SINGLE HIGHEST-VALUE ITEM IN THE PHASE. All 7 remaining `wcore-swarm` failures share this one cause, and it is also behind the 4 `wcore-agent` transactional-delegated-mutation failures and the 4 `dispatch_smoke` failures — 15 failures, one root. It is also what makes the `windows-public-dispatch` and `windows-f20-lifecycle` native proof targets red today while the other four are green."
    - "WHY IT IS HARD, stated precisely so nobody mistakes it for an oversight: the retained-authority contract is that the child's working directory is bound to a retained directory OBJECT, never to a re-openable pathname — that is the anti-swap property the whole Phase-20 delegated-mutation lifecycle rests on. Linux satisfies it because bwrap can hand the retained descriptor into the namespace as `/proc/self/fd/N` and chdir there, so a file descriptor IS the binding. Windows has no equivalent: `CreateProcess` takes `lpCurrentDirectory` as a PATHNAME, not a HANDLE, and there is no `fchdir`. That asymmetry is the whole problem."
    - "THE ANTI-SWAP GUARANTEE MUST NOT BE WEAKENED TO MAKE THE BIND WORK. If the only way to bind is to re-resolve the directory by pathname with no compensating guarantee, THAT IS A SECURITY REGRESSION and it is ESCALATED, NOT SHIPPED. This is a stated constraint, not a preference."
    - "THE RENAME TRUTH TABLE IS ALREADY MEASURED ON THIS EXACT HARDWARE (NTFS, SEANDESKTOP) and is the raw material for a candidate mechanism — RULE 1: a handle on an object NEVER blocks renaming that object provided the share mode includes FILE_SHARE_DELETE, and desired access is irrelevant. RULE 2: WITHOUT FILE_SHARE_DELETE the rename is refused with ERROR_SHARING_VIOLATION (32). RULE 3: ANY open handle to a descendant, file or directory, any share mode, blocks renaming ANY ancestor with ERROR_ACCESS_DENIED (5). Rules 2 and 3 together describe an OS-enforced PIN: while a suitably-opened handle is held, the object's pathname binding cannot be redirected by rename. Whether the retained authority's ACTUAL share mode delivers that pin is UNPROVEN and is the first thing this plan must measure — it must not be assumed in either direction."
    - "THE TRUTH TABLE COVERS RENAME AND ONLY RENAME, AND RENAME IS NOT THE ONLY WAY TO REDIRECT A PATHNAME ON NTFS. Every rule above is about `MoveFile`. At least three other primitives redirect a path to a different object and NONE of them is covered by a rename pin: (a) DELETE-AND-RECREATE of an ancestor, which does not rename anything; (b) converting an ancestor to, or replacing it with, a DIRECTORY JUNCTION or MOUNT POINT — a reparse point that redirects traversal at resolution time; (c) a SYMBOLIC LINK where the host policy permits one. Mechanism A's entire safety argument is `a path-form bind is safe BECAUSE the OS refuses the swap`, so a redirection primitive the pin does NOT defeat is a hole in the exact security property Sean is asked to authorize. Task 1 must ENUMERATE every pathname-redirection primitive the candidate pin defeats and every one it does not, and must probe delete-and-recreate and reparse-point redirection directly — not reason about them."
    - "THE MECHANISM IS A MEASURED DECISION, NOT A GUESS. Three candidate mechanisms are named for evaluation and the executor may add a fourth: (A) share-mode pin — narrow the retained open so rules 2 and 3 make the path-to-object binding immutable while the handle is held, then a path-form bind is safe BECAUSE the OS refuses the swap; (B) post-spawn identity re-proof — spawn suspended, re-open the child's actual working directory, compare 128-bit file id plus volume serial against the retained handle, and refuse to resume on any mismatch; (C) handle transport — inherit the retained handle into the AppContainer and establish the working directory from it, if any supported mechanism does so without a pathname. Each is judged on ONE question: does the delegated child's working directory provably remain the object the authority retained, for the whole execution?"
    - "A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, `#[ignore]`, `#[allow]`, raise a global timeout, or delete an inconvenient test to reach a gate. Making the AppContainer backend answer true to `binds_workspace_authority()` WITHOUT actually binding the authority would turn 15 red tests green while removing the guarantee they exist to prove. That is the worst available outcome and it is forbidden."
  artifacts:
    - path: .planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md
      provides: "The hardware-measured evaluation of each candidate mechanism against the anti-swap requirement, the residual-window analysis, and the recorded decision or escalation"
    - path: crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs
      provides: "The AppContainer cwd/workspace-authority binding under the authorized mechanism, with the predicate answering true only because the binding is real"
    - path: crates/wcore-sandbox/src/backends/appcontainer/windows_impl/tests.rs
      provides: "The anti-swap regression proof — a test that fails if the binding is ever downgraded to an unguarded pathname re-resolve"
  key_links:
    - from: crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs
      to: crates/wcore-swarm/src/dispatch.rs
      via: "the delegated-backend admission predicate, which may answer true only when the child's working directory provably remains the retained object"
      pattern: "native_windows"
    - from: .planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md
      to: crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs
      via: "the authorized mechanism decision, recorded before any production bind is written"
      pattern: "decision-record"
---

<objective>
Close the one structural blocker behind fifteen native Windows failures: bind the AppContainer child's working directory to the RETAINED delegated workspace authority, without weakening the anti-swap guarantee that binding exists to provide.

Purpose: `CreateProcess` takes a pathname, not a handle, so the mechanism Linux uses — hand the retained descriptor into the namespace and chdir to it — has no direct Windows equivalent. The AppContainer backend therefore keeps the fail-closed trait defaults, the swarm's delegated-dispatch admission refuses it, and 7 swarm plus 4 agent plus 4 dispatch-smoke tests fail on one cause. Fixing this unblocks all fifteen at once and turns the two red native proof targets green. Getting it wrong — by making the predicate answer true without a real binding — would turn those fifteen green while destroying the property they exist to prove.
Output: A hardware-measured mechanism evaluation, one authorized decision, the production binding under that decision, an anti-swap regression proof that fails if the binding is ever downgraded, and a re-measured delta against the 20A-01 baseline. Or, if no mechanism preserves the guarantee: an escalation with the evidence and no shipped code.
</objective>

<execution_context>
@/Users/seandonahoe/.codex/gsd-core/workflows/execute-plan.md
@/Users/seandonahoe/.codex/gsd-core/templates/summary.md
</execution_context>

<context>
@AGENTS.md
@.planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md
@crates/wcore-swarm/src/dispatch.rs
@crates/wcore-sandbox/src/backends/mod.rs
@crates/wcore-sandbox/src/backends/appcontainer.rs
@crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs
@crates/wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs
@crates/wcore-sandbox/src/backends/appcontainer/windows_impl/handles.rs
@crates/wcore-sandbox/src/backends/bwrap.rs
@crates/wcore-sandbox/src/directory_authority_windows.rs
</context>

<execution_rules>

**THE TWO AMENDED PHASE RULES — stated verbatim, and they bound this plan.**

- Findings at CRITICAL or HIGH must be fixed or disproved. MEDIUM and below are logged to BACKLOG and DO NOT BLOCK execution.
- Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean.

**TERMINATION CRITERION FOR THIS PLAN (hard — the plan STOPS and escalates rather than spawning more work).** This plan evaluates mechanisms ONCE, takes ONE authorized decision, and implements it ONCE. It terminates in exactly one of three states, and in all three it writes its SUMMARY and stops:
1. **Complete** — a mechanism was authorized at the decision checkpoint, implemented, and proven on hardware to bind the retained authority with the anti-swap property intact, with the delta stated against the 20A-01 baseline.
2. **Escalated — no sound mechanism.** Every evaluated mechanism either fails to bind or preserves the binding only by re-resolving a pathname with no compensating OS-enforced guarantee. STOP. Ship NO production code. Record the measured evidence for each mechanism and escalate to Sean. The 15 failures stay red and that is the correct outcome.
3. **Escalated — implementation contradicts measurement.** The authorized mechanism was implemented and hardware disproved the property it was authorized on. STOP. Revert the production change, record the contradiction, escalate. Do NOT try a different mechanism inside this plan; that is a second cycle and it is forbidden.
Under no circumstances does this plan spawn additional plans, extend its own task list, evaluate a mechanism after the decision checkpoint, or start a second implement/measure cycle.

**THE HARD SECURITY CONSTRAINT (this is the reason the plan exists in this shape).** The retained-handle anti-swap guarantee must NOT be weakened to make the bind work. If the only way to bind is to re-resolve the directory by pathname with no compensating guarantee, that is a SECURITY REGRESSION and must be ESCALATED, NOT SHIPPED. Making `binds_workspace_authority()` answer true without a real binding is the specific forbidden move: it converts fifteen honest reds into fifteen dishonest greens and silently deletes the property the delegated-mutation lifecycle rests on.

**NON-NEGOTIABLE.** A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, `#[ignore]`, `#[allow]`, raise a global timeout, or delete an inconvenient test to reach a gate. Four executors in Phase 20 correctly stopped and escalated rather than improvise, and every one of those calls was right.

**MEASURE BEFORE YOU DESIGN.** Every property this plan relies on is measured on SEANDESKTOP with a throwaway probe before it is designed against. In particular: do NOT assume the retained authority's current share mode delivers the rename pin, and do NOT assume it does not. Measure it. Probes are throwaway and are never added to production code and never committed.

**ENVIRONMENT.**
- Windows: `ssh -o BatchMode=yes SeanD@seandesktop` (Tailscale), checkout `C:\ferrox-win`. Invocation shape: `ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "<cmd> 2>&1" }'`, piped through `grep -v CLIXML | grep -v "^<Objs"`. Git on the box MUST be wrapped `cmd /c "git ..."` — PowerShell's Stop preference treats git's stderr chatter as fatal. `cargo fmt --all` FAILS there with os error 206; `justfile:96-98` already skips fmt-check on Windows.
- Linux: `ssh -o BatchMode=yes hetzner-dsm`, `/root/wayland`. Used here to prove no Linux regression.
- Mac CANNOT compile this workspace. `cargo fmt --all` is the only working cargo command there. Use `/usr/bin/git`, and ALWAYS `/usr/bin/grep`.
- Push the WORK BRANCH to `gh` so the hosts can fetch it. NO push to main, merge, PR, tag, release, or issue closure without Sean.

**THE TWO MEASUREMENT TRAPS (both measured; do not simplify these away).**
1. In `cmd`, `set VAR=value && ...` appends a TRAILING SPACE to the value and Rust silently ignores it. Use `set "VAR=x"` or PowerShell `$env:VAR='x'`, and PROVE the value took effect before trusting any run that depends on it. The AppContainer live tests gate on exactly such a variable, so a vacuous green is one trailing space away.
2. Mac `grep` is rtk-proxied and SILENTLY DROPS LINES — measured at 32 returned versus 674 for the same inverted match on the same file. Every gate in this plan invokes `/usr/bin/grep` explicitly and uses `-F` for literals.

**AGENTS.md discipline.** Surgical diffs only; no drive-by refactors. Centralize the platform difference inside `wcore-sandbox` — `wcore-swarm` gains no `#[cfg]` and no new conditional. `thiserror` for public error types, `anyhow` internally. Clippy-clean. No new crate and no new dependency unless the authorized mechanism genuinely requires a symbol not already reachable, in which case it is called out at the decision checkpoint rather than added quietly.

**Git hygiene.** Use `/usr/bin/git` on the Mac. Stage the exact paths in `files_modified`, never `-A`, never `.`. Never stage `AGENTS.md` or `.ijfw` churn. No `Co-Authored-By` trailers.
</execution_rules>

<tasks>

<task type="auto">
  <name>Task 1: Measure each candidate binding mechanism on hardware against the anti-swap requirement</name>
  <files>.planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md</files>
  <read_first>crates/wcore-swarm/src/dispatch.rs (the delegated-backend admission function and all four refusal predicates), crates/wcore-sandbox/src/backends/mod.rs (the cwd-authority and workspace-authority trait methods and their two `binds_*` predicates, with the doc comments stating what "without reopening the directory's display path" means), crates/wcore-sandbox/src/backends/bwrap.rs (the cwd-authority override and how it hands the retained descriptor into the namespace — the reference semantics this plan must match on Windows), crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs (the full trait impl and the process-creation path including how the working directory is currently passed), crates/wcore-sandbox/src/directory_authority_windows.rs (the relative-open path, the access masks, the share mode and the create-options word used when a directory authority is retained)</read_first>
  <behavior>
    - The exact current share mode and access mask of a retained directory authority on Windows are READ from source and then CONFIRMED by probe, not inferred.
    - For each candidate mechanism EVALUATED, one question is answered with hardware evidence: can a delegated child's working directory be established such that it provably remains the object the authority retained, for the whole execution?
    - The mechanisms are evaluated IN ORDER (A, then B, then C) and evaluation STOPS at the FIRST mechanism whose verdict is QUALIFIES or QUALIFIES WITH RESIDUAL RISK. Every mechanism after that one is recorded as `NOT-EVALUATED-NOT-NEEDED`, which is a PASSING state and not a gap. Evaluating a further mechanism once one has qualified is the loop generator this plan exists to avoid, and it is forbidden.
    - Any residual window between resolution and the child's first filesystem operation is identified, measured, and stated in wall-clock or ordering terms — not hand-waved.
    - Mechanism A is tested by attempt: with a retained handle open, try to rename the directory, try to rename an ancestor, and record the exact refusal or success. A pin that the OS does not actually enforce is not a pin.
    - Mechanism A is ALSO tested against the redirection primitives rename does not cover: ancestor delete-and-recreate, and ancestor replacement by or conversion to a junction, mount point or symbolic link. The evaluation produces an explicit two-column list of what the pin defeats and what it does not, because an unenumerated primitive that succeeds falsifies the mechanism's entire safety argument.
    - Mechanism B is tested by attempt: can a spawned process be held before it runs, can its actual working directory be re-opened and compared by durable object identity rather than by path, and does a mismatch reliably prevent the child from running.
    - Mechanism C is tested by attempt: is there any supported path by which an inherited handle establishes the child's working directory without a pathname.
    - The evaluation ends with a recommendation and its residual risk, or with the finding that no mechanism qualifies.
  </behavior>
  <action>Read the retained-authority open in the Windows directory-authority module and record verbatim what access mask, share mode and create options a retained directory handle actually carries today. Then CONFIRM it by probe on the box — the source and the runtime must agree before anything is designed against either.

Now evaluate the named candidate mechanisms by direct attempt on SEANDESKTOP, using throwaway probes that are never committed and never added to production code. Judge each against exactly one question: does the delegated child's working directory provably remain the object the authority retained, for the whole execution?

**STOP AT THE FIRST MECHANISM THAT QUALIFIES (amended, authorized by Sean).** Evaluate in order — A, then B, then C. The moment a mechanism's verdict is QUALIFIES or QUALIFIES WITH RESIDUAL RISK, the evaluation ENDS. Record every remaining mechanism as `NOT-EVALUATED-NOT-NEEDED` with one sentence saying which mechanism qualified and therefore made it unnecessary. `NOT-EVALUATED-NOT-NEEDED` is a PASSING state: it is a deliberate, recorded decision to stop, not an unmeasured gap, and the gates below accept it as a complete account of that mechanism. Continuing to probe alternatives after one has qualified generates work without changing the decision, and it is forbidden. A mechanism recorded as `NOT-EVALUATED-NOT-NEEDED` carries NO verdict and is therefore NOT selectable at the Task 2 checkpoint. Only when a mechanism's verdict is DOES NOT QUALIFY does the evaluation advance to the next one.

MECHANISM A — the OS-enforced pin. The rename truth table measured on this hardware says a handle blocks renaming its own object only when the share mode omits delete rights, and that any open descendant handle blocks renaming any ancestor. If the retained authority's handle delivers both, then a rename cannot redirect the path-to-object binding while the handle is held. Measure it: with a retained handle held, attempt to rename the directory itself and attempt to rename each ancestor, and record the exact outcome and error for each.

Then — and this is the part that decides whether Mechanism A is sound at all — ENUMERATE EVERY PATHNAME-REDIRECTION PRIMITIVE and probe the ones the rename table does not cover, because rename is not the only way to point a path at a different object. At minimum, with the retained handle held: (a) attempt to DELETE an ancestor and recreate a different directory at the same name, which renames nothing and is therefore entirely outside the truth table; (b) attempt to replace an ancestor with, or convert an ancestor into, a DIRECTORY JUNCTION or MOUNT POINT that redirects traversal at resolution time; (c) attempt the same with a SYMBOLIC LINK if host policy permits creating one. Record for each: whether the retained handle refuses it, with the exact error, or permits it. Produce an explicit two-column list — the primitives this pin DEFEATS, and the primitives it DOES NOT. Mechanism A's entire safety argument is that a path-form bind is safe because the OS refuses the swap; a single unenumerated primitive that succeeds turns that argument into a false one, and it must appear in the verdict rather than be discovered later.

If the current share mode does NOT deliver the pin, determine what narrowing would, and what that narrowing costs elsewhere — specifically whether it would break the handle-bound destructive cleanup, the loan accounting, or any existing test. Cap that narrowing analysis at 30 minutes of wall clock or 6 probes, whichever comes first; if it is not settled by then, record what is known, mark the mechanism QUALIFIES WITH RESIDUAL RISK or DOES NOT QUALIFY on the evidence in hand, and move on. A narrowing that breaks an existing guarantee is not a free win and must be reported as such.

MECHANISM B — post-spawn identity re-proof. EVALUATE ONLY IF MECHANISM A'S VERDICT WAS `DOES NOT QUALIFY`; otherwise record it `NOT-EVALUATED-NOT-NEEDED` and skip this paragraph entirely. Determine whether the child can be created in a held state, whether its actual working directory can be re-opened and compared to the retained handle by DURABLE OBJECT IDENTITY (the volume serial plus the 128-bit file id, not the path and not the short name), and whether a mismatch reliably prevents the child from performing any filesystem operation. Measure the ordering: identify precisely what the child can and cannot do between creation and the identity check. If the child can touch the filesystem before the check completes, the mechanism does not close the window and that must be stated plainly.

MECHANISM C — handle transport. EVALUATE ONLY IF BOTH MECHANISM A AND MECHANISM B carry the verdict `DOES NOT QUALIFY`; otherwise record it `NOT-EVALUATED-NOT-NEEDED` and skip this paragraph entirely. Determine whether the retained handle can be inherited into the AppContainer, and whether any supported mechanism establishes the child's working directory from an inherited handle rather than from a pathname. Record what the AppContainer's own restrictions do to handle inheritance. If no such mechanism exists, say so — a clean negative is a real result and shortens the decision.

You may evaluate AT MOST ONE fourth mechanism, and ONLY if all three named mechanisms carry the verdict `DOES NOT QUALIFY` and the measurements actively suggest it — capped at 60 minutes of wall clock or 10 probes, whichever comes first, after which it is recorded with whatever verdict the evidence supports and the evaluation ends. There is no fifth. You may not evaluate any mechanism at all after the decision checkpoint in Task 2.

For each mechanism EVALUATED record: whether it binds at all; whether the anti-swap property holds and by what enforcement; for Mechanism A specifically, the two-column list of pathname-redirection primitives the pin defeats and does not defeat; the residual window if any, in ordering or wall-clock terms; what it costs elsewhere; and its verdict as QUALIFIES, QUALIFIES WITH RESIDUAL RISK, or DOES NOT QUALIFY. Write it all into `20A-02-BIND-MECHANISM.md` with the probe output inline.

If EVERY mechanism is DOES NOT QUALIFY, or the only qualifying one is an unguarded pathname re-resolve, STOP HERE. Ship no production code, write the SUMMARY recording the escalation, and hand it to Sean. That is termination state 2 and it is a correct outcome, not a failure.

Records evidence for REQ-native-r2 and REQ-native-r6; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -f .planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cE 'QUALIFIES|DOES NOT QUALIFY|NOT-EVALUATED-NOT-NEEDED' .planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md)" -ge "3"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'QUALIFIES' .planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md)" -ge "1" || test "$(/usr/bin/grep -cF 'DOES NOT QUALIFY' .planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md)" -ge "3"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'Mechanism A' .planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md)" -ge "1" &amp;&amp; test "$(/usr/bin/grep -cF 'Mechanism B' .planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md)" -ge "1" &amp;&amp; test "$(/usr/bin/grep -cF 'Mechanism C' .planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md)" -ge "1"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; for p in rename delete-and-recreate junction "mount point" symbolic; do test "$(/usr/bin/grep -ciF "$p" .planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md)" -ge "1" || { echo "redirection primitive not enumerated: $p"; exit 1; }; done; echo "every pathname-redirection primitive is enumerated with a probed verdict"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git status --porcelain -- crates/</automated>
  </verify>
  <done>The retained authority's actual access mask, share mode and create options are recorded from source and confirmed by probe. Each of the three named mechanisms is ACCOUNTED FOR: either it carries a hardware-measured verdict with its enforcement basis, its residual window and its cost elsewhere, or it is recorded `NOT-EVALUATED-NOT-NEEDED` because an earlier mechanism already qualified — which is a passing state, not a gap. If a mechanism qualified, the evaluation stopped there. Mechanism A, if evaluated, additionally carries the two-column enumeration of pathname-redirection primitives it defeats and does not defeat, with delete-and-recreate and reparse-point redirection PROBED rather than reasoned about. At most one fourth mechanism was evaluated, within its cap, and the narrowing analysis respected its cap. No probe was added to production code and `crates/` is unmodified at the end of this task. If no mechanism qualified, the plan terminated here with an escalation and no shipped code.</done>
</task>

<task type="checkpoint:decision" gate="blocking">
  <name>Task 2 (BLOCKING DECISION): Authorize the binding mechanism, or escalate</name>
  <files>.planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md</files>
  <action>Present Task 1's hardware-measured verdict table to Sean and obtain ONE authorization. Do not write a line of production code before it is given. If the selected mechanism's Task 1 verdict was QUALIFIES WITH RESIDUAL RISK, record the exact residual risk Sean named and accepted, in his words, in `20A-02-BIND-MECHANISM.md`. If Sean selects escalation, Task 3 does not run: write the SUMMARY recording termination state 2 and stop. No mechanism may be evaluated after this checkpoint.</action>
  <decision>Which mechanism binds the AppContainer child's working directory to the retained delegated workspace authority — or does this escalate?</decision>
  <context>
This is the trust boundary the entire Phase-20 delegated-mutation lifecycle rests on, on the platform where it has never worked. `CreateProcess` takes a pathname, not a handle, and there is no `fchdir` on Windows, so the mechanism Linux uses cannot be copied. Fifteen tests are red on this one cause and two native proof targets are red downstream of it. The forbidden shortcut — making the admission predicate answer true without a real binding — would turn all fifteen green while deleting the property they exist to prove, so the mechanism choice is a security decision and not an implementation detail. Task 1's `20A-02-BIND-MECHANISM.md` carries the hardware-measured verdict, enforcement basis, residual window and cost for each option; read it before choosing.
  </context>
  <options>
    <option id="mechanism-a-pin">
      <name>OS-enforced pin — narrow the retained open so the path-to-object binding cannot be redirected while the handle is held, then bind by path safely</name>
      <pros>The guarantee is enforced by the operating system rather than by our own re-check, so there is no window at all; it reuses the rename semantics already measured on this exact hardware; the production change is small and local to the retained open</pros>
      <cons>Requires narrowing a share mode that other code paths may depend on; Task 1 must have proven the pin actually holds for the directory AND every ancestor, and must have proven the narrowing breaks no existing guarantee — if either is unproven this option is not available. CRITICALLY: a rename pin defeats rename and nothing else, so this option is available ONLY if Task 1's two-column enumeration shows the pin also defeats ancestor delete-and-recreate and reparse-point redirection (junction, mount point, symbolic link). Any primitive in the DOES-NOT-DEFEAT column is a live redirection route and must be named and accepted here, or this option declined</cons>
    </option>
    <option id="mechanism-b-reproof">
      <name>Post-spawn identity re-proof — create the child held, compare its actual working directory to the retained handle by durable object identity, and refuse to release it on any mismatch</name>
      <pros>Does not disturb any existing share mode or handle semantics; the comparison is on durable object identity rather than on a path, so a substitution is detected rather than assumed away</pros>
      <cons>Leaves a window between creation and the check unless Task 1 proved the child cannot touch the filesystem before the check completes; adds a failure mode to the spawn path; the refusal path must be proven to actually stop the child rather than merely report</cons>
    </option>
    <option id="mechanism-c-handle">
      <name>Handle transport — establish the child's working directory from an inherited handle, with no pathname involved</name>
      <pros>Closest in spirit to the Linux mechanism and to the retained-authority contract as written; if it works, no compensating guarantee is needed because no pathname is ever resolved</pros>
      <cons>Likely unavailable — Task 1 must have found a supported mechanism, and AppContainer restrictions on handle inheritance may rule it out entirely; if Task 1 recorded a clean negative this option is not available</cons>
    </option>
    <option id="escalate">
      <name>Escalate — no mechanism preserves the anti-swap guarantee, so ship nothing</name>
      <pros>Honors the stated constraint exactly; the fifteen failures stay honestly red rather than dishonestly green; Sean decides whether to accept a residual risk, fund a deeper mechanism, or scope Windows delegated dispatch differently</pros>
      <cons>Phase 20A cannot reach the six-target Windows proof on this candidate; the public dispatch and F20 lifecycle native targets stay red</cons>
    </option>
  </options>
  <resume-signal>Select: mechanism-a-pin, mechanism-b-reproof, mechanism-c-handle, or escalate. If selecting a mechanism whose Task 1 verdict was QUALIFIES WITH RESIDUAL RISK, state that the residual risk is accepted and name it.</resume-signal>
  <verify>
    <human-check>The authorized mechanism and any accepted residual risk are recorded verbatim in `20A-02-BIND-MECHANISM.md`, and the selected option's Task 1 verdict was not DOES NOT QUALIFY and not `NOT-EVALUATED-NOT-NEEDED` — an unevaluated mechanism carries no verdict and is therefore not selectable.</human-check>
  </verify>
  <done>One mechanism is authorized and recorded, with any residual risk named and accepted in Sean's own words — or escalation was selected and Task 3 does not run.</done>
</task>

<task type="auto">
  <name>Task 3: Implement the authorized binding, prove the anti-swap property survives it, and re-measure the delta</name>
  <files>crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs, crates/wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs, crates/wcore-sandbox/src/backends/appcontainer/windows_impl/handles.rs, crates/wcore-sandbox/src/backends/appcontainer/windows_impl/tests.rs, crates/wcore-sandbox/src/backends/appcontainer.rs, crates/wcore-swarm/tests/dispatch_smoke.rs</files>
  <read_first>.planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md (the authorized mechanism and its residual-risk statement), crates/wcore-sandbox/src/backends/bwrap.rs (its cwd-authority override and the live acceptance test that proves the retained object really is the child's working directory — the shape to mirror), crates/wcore-sandbox/src/backends/appcontainer/windows_impl/tests.rs (existing fixture idiom, so nothing is duplicated or displaced), .planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md (the measured baseline this task states its delta against)</read_first>
  <behavior>
    - The AppContainer backend binds the delegated child's working directory to the retained authority under the authorized mechanism, and answers true to the admission predicate ONLY because that binding is real.
    - The delegated-backend admission in the swarm's dispatch path stops refusing on Windows, and it does so without any change to the swarm crate — the platform difference stays inside the sandbox crate.
    - A regression test proves the anti-swap property directly: with a substitution attempted against the bound working directory, the child does not end up operating on the substituted object. It fails if the binding is ever downgraded to an unguarded pathname re-resolve.
    - The regression test asserts the INVARIANT — that the retained object is what the child works in — never a specific OS error code or the shape of today's failure.
    - `dispatch_smoke` carries no directory rename performed while a handle to that directory or a descendant is held — the non-portable construction REQ-native-r6 names — or the absence of any such construction at this SHA is recorded with evidence.
    - The fifteen previously-failing tests are re-run and each one's outcome is recorded by name against the 20A-01 baseline. A residual failure is diagnosed and named, never annotated away.
    - Repair is bounded: at most two repair iterations across the ordered gate, then escalation.
    - Linux and macOS behaviour is unchanged, proven by the aggregate suite on Hetzner rather than asserted.
  </behavior>
  <action>Implement ONLY the mechanism authorized at the checkpoint, exactly as authorized. If the checkpoint selected escalation, this task does not run: write the SUMMARY and stop.

In the Windows AppContainer backend, override the cwd-authority execution method and its predicate so the delegated child's working directory is bound to the retained authority under the authorized mechanism, and let the workspace-authority predicate follow the trait's existing derivation rather than being overridden separately — the trait already derives one from the other, and adding a second independent answer is how the two drift apart. Whatever the mechanism, the predicate must answer true because the binding is real; if the binding fails at runtime the execution must fail closed, never fall back to an unbound spawn. Document the mechanism at the override with the measured basis for its guarantee, so a future reader cannot downgrade it without deleting the reason. Keep the platform difference inside `wcore-sandbox`: `wcore-swarm` gains no `#[cfg]` and no conditional, and the dispatch admission function is not touched.

Add ONE anti-swap regression test to the Windows AppContainer test module. It must construct the substitution the guarantee exists to defeat — attempt to redirect the bound working directory's pathname to a different object while the authority is held — and assert that the child provably operated on the RETAINED object, not the substituted one. Assert the invariant, never an error code, an error kind or a numeric OS status: encoding today's failure shape into an assertion enshrines the defect. Give the test a doc comment stating which mechanism it proves and that it fails if the binding is downgraded to an unguarded pathname re-resolve. Do not modify, rename, re-gate or delete any existing test.

Close REQ-native-r6's OTHER clause while you are in `crates/wcore-swarm/tests/dispatch_smoke.rs`, because it is the same file and the same failing suite. REQ-native-r6 as written in `.planning/REQUIREMENTS.md` is "`dispatch_smoke` Windows-portable (no `fs::rename` of open dir)" — the binding work above unblocks the ADMISSION half of that suite, but the portability half is a separate, small, concrete defect: a directory rename performed while a handle to that directory or a descendant is open is refused on Windows by the measured rename truth table, so any such construction in that test file is non-portable by inspection. Locate every such construction in `dispatch_smoke.rs`, and replace it with a portable equivalent that preserves the assertion's meaning exactly. If there is NO such construction left at this SHA — it may already have been repaired — record that determination with the evidence and treat r6's portability clause as already satisfied. Do not invent a change to justify the requirement. Do not touch any other file for this clause, and do not extend it into `worktree_tests.rs`, which is 20A-03's surface.

Then measure on the box, in this order, stopping at the first failure and repairing before advancing: compile the sandbox and swarm crates with all test targets; run the `wcore-sandbox` crate suite; run the `wcore-swarm` crate suite; run the `wcore-agent` transactional delegated-mutation integration test with the ignored set included; run the `wcore-swarm` dispatch smoke integration test; then clippy across the workspace with all targets. State every count as a DELTA against the counts `20A-01-BASELINE.md` actually measured, never against any number written in this plan. Name every residual failure and diagnose it. A residual failure that this plan did not target and that was not in the baseline is a NEW regression — diagnose and fix it if it is caused by this change, or record it with severity and escalate if it is not. Do not annotate it away.

REPAIR ITERATION CAP (hard, M1): "repairing before advancing" is bounded at TWO repair iterations across the whole ordered gate. An iteration is one edit-commit-resync-remeasure cycle. If the gate is still not clean after the second, STOP — record every remaining failure by name with its output and escalate to Sean. This plan's termination criterion permits exactly one implementation; an unbounded repair loop inside it is that criterion defeated by the back door, and it is how Phase 20 reached seventy-four plans.

Prove no Linux regression on Hetzner with the aggregate build and test at this SHA (REQ-native-r4), and record the counts.

If hardware disproves the property the authorized mechanism was chosen on, STOP: revert the production change, record the contradiction with its evidence, and escalate. Do not try a different mechanism here — that is a second cycle and it is forbidden.

Implements REQ-native-r6; records evidence for REQ-native-r2 and REQ-native-r4; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'binds_cwd_authority' crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs)" -ge "1"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'execute_with_cwd_authority' crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs)" -ge "1"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'cfg(windows)' crates/wcore-swarm/src/dispatch.rs)" = "$(/usr/bin/git show HEAD:crates/wcore-swarm/src/dispatch.rs | /usr/bin/grep -cF 'cfg(windows)')"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/grep -n 'rename' crates/wcore-swarm/tests/dispatch_smoke.rs || echo "no rename construction remains in dispatch_smoke (REQ-native-r6 portability clause)"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git diff --stat -- crates/wcore-swarm/src/dispatch.rs | /usr/bin/grep -c . | /usr/bin/grep -qx 0 &amp;&amp; echo "dispatch.rs untouched"</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "git fetch --all --prune 2>&amp;1"; cmd /c "git checkout --detach '"$SHA"' 2>&amp;1"; cmd /c "git rev-parse HEAD"; cmd /c "cargo check -p wcore-sandbox -p wcore-swarm --all-targets 2>&amp;1" }' | grep -v CLIXML | grep -v "^&lt;Objs" | tail -30</automated>
    <automated>ssh -o BatchMode=yes SeanD@seandesktop 'powershell -NoProfile -Command { Set-Location C:\ferrox-win; cmd /c "cargo nextest run -p wcore-swarm --no-fail-fast 2>&amp;1"; cmd /c "cargo nextest run -p wcore-agent --test transactional_delegated_mutation_test --run-ignored all --no-fail-fast 2>&amp;1"; cmd /c "cargo nextest run -p wcore-swarm --test dispatch_smoke --no-fail-fast 2>&amp;1" }' | grep -v CLIXML | grep -v "^&lt;Objs" | tail -60</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch --all --prune &amp;&amp; git checkout --detach $SHA &amp;&amp; git rev-parse HEAD &amp;&amp; cargo build --locked --workspace --all-features &amp;&amp; cargo nextest run --profile ci --no-fail-fast" 2&gt;&amp;1 | tail -30</automated>
  </verify>
  <done>The AppContainer backend binds the delegated child's working directory to the retained authority under the mechanism authorized at the checkpoint, and the admission predicate answers true only because that binding is real. `crates/wcore-swarm/src/dispatch.rs` is untouched and `wcore-swarm` gained no platform conditional. REQ-native-r6's portability clause is closed in `dispatch_smoke.rs` or its absence at this SHA is recorded with evidence. One anti-swap regression test exists, constructs the substitution, asserts the invariant rather than any error shape, and no existing test was modified or re-gated. Every one of the fifteen previously-failing tests has its outcome recorded by name as a delta against the measured 20A-01 baseline, with any residual failure diagnosed, within at most two repair iterations. The Hetzner aggregate proves no Linux regression. If hardware disproved the authorized property, the production change was reverted and the contradiction escalated.</done>
</task>

</tasks>

## What this plan does NOT change (scope fence)

- **`crates/wcore-swarm/src/dispatch.rs` — untouched, and gate-checked to be so.** The admission function and its four refusal predicates are correct: a backend that cannot bind the retained authority SHOULD be refused. The defect is that the Windows backend cannot bind, not that the swarm asks. Relaxing the admission check would be the forbidden shortcut in its purest form.
- **The trait defaults in `crates/wcore-sandbox/src/backends/mod.rs` — untouched.** They fail closed by design: a backend opts in by overriding AND actually implementing. Changing a default to `true` would silently qualify every stub.
- **`binds_workspace_authority` is not overridden independently.** It already derives from the cwd-authority predicate; a second independent answer is how the two drift apart and how a backend comes to claim a workspace binding it does not have.
- **The bwrap and Docker backends, and every non-Windows path — untouched.** bwrap's descriptor-into-the-namespace mechanism is the reference semantics this plan matches, not a surface it modifies.
- **The CI wiring, the soak crate list and the proof-script target map — 20A-01 owns them.** This plan consumes 20A-01's baseline; it does not re-wire anything.
- **The checkout-dirty / eol reconciliation — 20A-03 owns it.** Not touched here.
- **The sealed candidate and the native proof dispatch — 20A-04 owns them.** This plan does not seal, does not prepare a tuple, and does not dispatch.
- **No test is deleted, weakened, re-gated, `#[ignore]`d or `#[allow]`ed**, and no assertion is loosened to reach a green. The anti-swap regression test asserts the invariant, never an error code.
- **No probe reaches production code.** Every measurement in Task 1 is a throwaway and `crates/` is gate-checked clean at the end of that task.
- **No new crate and no new dependency** unless the authorized mechanism genuinely requires a symbol that is not already reachable, in which case it is raised at the decision checkpoint rather than added quietly.

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| delegated child ← retained workspace authority | The child may mutate ONLY the workspace object the authority retained; if the binding is by pathname and the pathname can be redirected, the child mutates something else entirely |
| admission predicate ← real binding | `binds_workspace_authority()` is the sole gate between a backend and delegated dispatch; a predicate that answers true without a binding admits an unbound child |
| path resolution ← rename/substitution | Any pathname resolved between validation and use is a substitution opportunity unless the OS refuses the swap or the object identity is re-proven |
| spawn ordering ← identity check | Under a post-spawn re-proof mechanism, anything the child can do before the check completes is outside the guarantee |
| repair claim ← real hardware | The Mac cannot compile; only SEANDESKTOP can prove or disprove any of this, and only Hetzner can prove the absence of a Linux regression |

## STRIDE Threat Register

| Threat ID | Category | Component | Severity | Disposition | Mitigation Plan |
|-----------|----------|-----------|----------|-------------|-----------------|
| T-20A-02-01 | Elevation of Privilege | The forbidden shortcut: the admission predicate is made to answer true without a real binding, turning 15 honest reds into 15 dishonest greens and deleting the anti-swap property the delegated-mutation lifecycle rests on | critical | mitigate | Forbidden verbatim in the execution rules; the predicate may answer true only because the binding is real; a dedicated anti-swap regression test constructs the substitution and asserts the retained object was operated on; `dispatch.rs` is gate-checked untouched so the admission check itself cannot be relaxed |
| T-20A-02-02 | Spoofing | Directory substitution against a pathname-bound working directory: the delegated child is spawned into an object the authority never proved, so every downstream receipt, merge and rollback attributes work to the wrong workspace | critical | mitigate | The mechanism is chosen ONLY if hardware showed the binding survives a substitution attempt, either by OS-enforced pin or by durable-object-identity re-proof; if neither holds, the plan escalates and ships nothing |
| T-20A-02-03 | Tampering | A residual window between resolution and the child's first filesystem operation is left unmeasured and assumed benign | high | mitigate | Task 1 must state each mechanism's residual window in ordering or wall-clock terms; a mechanism verdict of QUALIFIES WITH RESIDUAL RISK cannot be authorized without Sean naming and accepting that risk at the checkpoint |
| T-20A-02-13 | Spoofing | Mechanism A is authorized on a rename-only pin while an UNENUMERATED redirection primitive still succeeds — ancestor delete-and-recreate, or an ancestor replaced by or converted to a junction, mount point or symbolic link — so the path-form bind is redirected by a route the measured truth table never covered and the safety argument Sean authorized is simply false | critical | mitigate | Task 1 must produce a two-column enumeration of every pathname-redirection primitive the pin defeats and does not defeat, with delete-and-recreate and reparse-point redirection PROBED on hardware rather than reasoned about; the enumeration is gate-checked for each primitive by name, and any primitive that succeeds against the pin downgrades Mechanism A's verdict before it reaches the checkpoint |
| T-20A-02-04 | Denial of Service | Narrowing the retained handle's share mode to obtain the pin breaks the handle-bound destructive cleanup, the loan accounting, or an existing guarantee elsewhere | high | mitigate | Mechanism A's evaluation must state what the narrowing costs elsewhere and prove it breaks no existing guarantee; the full sandbox and swarm suites plus the Hetzner aggregate must be green after the change, with every residual failure named |
| T-20A-02-05 | Spoofing | A false green claimed from source inspection or from the Mac, which cannot compile this workspace | high | mitigate | Task 1 ends with `crates/` gate-checked unmodified; every behavioural claim comes from SEANDESKTOP or Hetzner; deltas are stated against the counts `20A-01-BASELINE.md` actually measured, never against a number written in this plan |
| T-20A-02-06 | Spoofing | A vacuous green: the AppContainer live tests gate on an environment variable set with the trailing-space `cmd` form, so they all skip while the runner reports success | high | mitigate | The trap-safe assignment form is mandated and the value is proven to have taken effect before any run that depends on it is trusted |
| T-20A-02-07 | Repudiation | The anti-swap regression test asserts today's error code instead of the invariant, so it passes for the wrong reason and stops failing the moment the failure shape changes | high | mitigate | The test is required to assert that the child operated on the RETAINED object; asserting an error code, error kind or numeric OS status is forbidden explicitly |
| T-20A-02-08 | Denial of Service | Scope metastasis — mechanism evaluation loops, a second mechanism is tried after the first fails in implementation, and the plan grows without bound as Phase 20 did | high | mitigate | The termination criterion caps the plan at ONE evaluation, ONE decision and ONE implementation, with three defined exit states; evaluating a mechanism after the checkpoint is forbidden; an implementation contradiction reverts and escalates rather than retrying |
| T-20A-02-09 | Tampering | The platform difference leaks into `wcore-swarm` as a `#[cfg]`, making the Windows path unexercisable by any Linux test and recreating the condition that hid this defect | medium | mitigate | AGENTS.md centralization is restated as a hard rule and the `cfg(windows)` count in `dispatch.rs` is diffed against the pre-task tree |
| T-20A-02-10 | Information Disclosure | A throwaway diagnostic probe is left in production code, widening the sandbox's own surface | medium | mitigate | Probes are explicitly throwaway, never added to production code and never committed; Task 1's gate requires `crates/` to be unmodified at its end |
| T-20A-02-SC | Tampering | npm/pip/cargo installs | low | accept | No dependency is added, removed or updated and no `Cargo.toml` change is expected; if the authorized mechanism genuinely requires an unreachable symbol, that is raised at the decision checkpoint rather than added quietly, and no install task exists in this plan |
</threat_model>

<verification>
Local gates (Mac, source level only — the Mac cannot compile this workspace): `cargo fmt --all -- --check` clean; `20A-02-BIND-MECHANISM.md` exists and accounts for each of the three named mechanisms — a measured verdict for every mechanism evaluated, and `NOT-EVALUATED-NOT-NEEDED` for every mechanism after the first to qualify; `crates/` is unmodified at the end of Task 1; after Task 3 the AppContainer Windows backend overrides the cwd-authority method and its predicate; `crates/wcore-swarm/src/dispatch.rs` shows no diff and its `cfg(windows)` count is unchanged from the pre-task tree.

Authoritative gates (SEANDESKTOP, real hardware), in order, on the exact final SHA: the sandbox and swarm crates compile with all test targets; the `wcore-sandbox` suite is green with the new anti-swap regression test passing; the `wcore-swarm` suite, the `wcore-agent` transactional delegated-mutation test with the ignored set included, and the `wcore-swarm` dispatch smoke test are each stated as a delta against the counts `20A-01-BASELINE.md` measured, with every residual failure named and diagnosed; workspace clippy with all targets is clean. Hetzner gate: `cargo build --locked --workspace --all-features` plus `cargo nextest run --profile ci --no-fail-fast` at the same SHA, proving no Linux regression (REQ-native-r4).

Known unknowns to record, not to resolve here: whether the authorized mechanism holds on filesystems other than NTFS under the box's default volume — the rename-rule probes behind mechanism A are NTFS-local, and a ReFS, FAT or SMB workspace may behave differently; whether AppContainer handle-inheritance restrictions differ across Windows builds; and whether any residual window accepted at the checkpoint is reachable by a real adversary or only by a cooperating test.

The six-target native proof dispatch is a DEPENDENCY, not a task here: it runs only after the phase's repair set has landed and remains a Sean gate under 20A-04.
</verification>

<success_criteria>
- Each of the three named binding mechanisms is accounted for before any production code is written: the ones evaluated carry a hardware-measured verdict with its enforcement basis, its residual window and its cost elsewhere, and any mechanism after the first one to qualify is recorded `NOT-EVALUATED-NOT-NEEDED` — a passing state, because evaluating alternatives to a mechanism that already qualified changes no decision.
- The mechanism actually shipped is the one authorized at the blocking decision checkpoint, and any residual risk was named and accepted there rather than discovered afterwards.
- The AppContainer backend binds the delegated child's working directory to the retained authority, and the admission predicate answers true ONLY because that binding is real — the forbidden shortcut is gate-checked against (REQ-native-r6).
- One anti-swap regression test constructs the substitution and asserts the child operated on the RETAINED object, never on an error code, so it fails if the binding is ever downgraded to an unguarded pathname re-resolve.
- `crates/wcore-swarm/src/dispatch.rs` is untouched and `wcore-swarm` gained no platform conditional — the platform difference lives entirely in `wcore-sandbox` per AGENTS.md.
- Mechanism A's safety argument, if authorized, is backed by an ENUMERATED two-column list of pathname-redirection primitives it defeats and does not defeat, with ancestor delete-and-recreate and reparse-point redirection probed rather than reasoned about — because a rename pin says nothing about either.
- REQ-native-r6's portability clause is closed in `dispatch_smoke.rs` — no directory rename while a handle to it or a descendant is held — or its absence at this SHA is recorded with evidence rather than left ambiguous (REQ-native-r6).
- The fifteen previously-failing tests each have their outcome recorded by name as a delta against the measured 20A-01 baseline, with every residual failure diagnosed rather than annotated, within at most two repair iterations (REQ-native-r2).
- The Hetzner aggregate proves no Linux regression at the same SHA (REQ-native-r4).
- If no mechanism preserved the anti-swap guarantee, the plan shipped NO production code and escalated with the evidence — and the fifteen failures stayed honestly red.
- No test was deleted, weakened, re-gated, `#[ignore]`d or `#[allow]`ed, and no probe reached production code.
</success_criteria>

## Artifacts this phase produces
- `.planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md` — the hardware-measured mechanism evaluation, the residual-window analysis, and the authorized decision or the escalation.
- `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/` — the retained-authority binding under the authorized mechanism, documented with its measured guarantee.
- `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/tests.rs` — the anti-swap regression proof.
- `20A-02-SUMMARY.md` recording the mechanism decision, the implementation, the per-suite delta against the measured baseline, and the Linux non-regression proof.

<output>
Create `.planning/phases/20A-native-windows-macos-uat/20A-02-SUMMARY.md` using the standard GSD summary template. Record: the fix commit SHA and tree; the retained authority's actual access mask, share mode and create options as read from source and confirmed by probe; the per-mechanism hardware verdict with enforcement basis, residual window and cost elsewhere; the authorized decision and any residual risk Sean named and accepted; the implementation with the measured basis documented at the override; the anti-swap regression test and exactly what substitution it constructs; the per-suite SEANDESKTOP counts stated as a delta against the counts `20A-01-BASELINE.md` measured, with every residual failure named and diagnosed; the Hetzner aggregate proving no Linux regression; the recorded unknowns (NTFS-local rename probes, AppContainer handle-inheritance variation across Windows builds, reachability of any accepted residual window); and which of the three termination states the plan ended in. Mark no requirement complete — closure is claimed by the downstream native proof under 20A-04.
</output>
