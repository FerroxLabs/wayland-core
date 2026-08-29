---
issue: 338
repo: FerroxLabs/wayland-core
kind: defect
title: "Untrusted plugin install can make Wayland prompt the user for credentials via /dev/tty"
status: open
last_verified_commit: a278f8c3
criteria:
  - id: c1
    text: "A quarantine clone of untrusted plugin content cannot open /dev/tty to prompt the user directly"
    state: met
    evidence: "test:crates/wcore-cli/tests/quarantine_terminal_authority.rs::quarantine_child_cannot_reach_the_controlling_terminal"
    owner: core
    note: "The test re-execs itself inside a real PTY so the probe HAS a controlling terminal, then asserts PLAIN=OPEN as negative control, PRODUCTION_GIT=DENIED through build_git_command, HARDENED=DENIED and GIT_STILL_RUNS=true as liveness. The fix is setsid(2) in pre_exec on unix / DETACHED_PROCESS on windows, quarantine.rs:334-368."
  - id: c2
    text: "Any prompt raised inside a quarantine operation is distinguishable by the user from a prompt raised by Wayland itself"
    state: superseded
    evidence: "symbol:crates/wcore-cli/src/plugin/quarantine.rs::build_git_command"
    owner: core
    note: "DECOMPOSED 2026-08-30 into c5 (unix, met) and c6 (Windows, measured); the Windows REMAINDER is handed to FerroxLabs/wayland-core#389, which is open and carries deliver-or-label acceptance criteria. (#380 is the sweep's own filing of the same finding; it asked for the MEASUREMENT and the honest statement, and this pass answers both, so its own ledger now reads met and it is Sean's to close as the duplicate.) Why the text is not simply graded met: it is unqualified (`ANY prompt`) and the product ships on two platforms. On unix the elimination is real and c5 proves it. On Windows it is NOT, and that is now measured rather than assumed -- see c6. Grading the unqualified sentence `met` off the cfg(unix) test alone is the easier-adjacent-property substitution this sweep exists to catch, so it is not done here. Why the CODE was not simply fixed instead: there is no cheap Windows primitive that delivers setsid's guarantee. `AttachConsole(ATTACH_PARENT_PROCESS)` puts a DETACHED_PROCESS child back on the user's console (MEASURED: SUCCEEDED); attaching by EXPLICIT pid also succeeds (MEASURED), so reparenting the child onto a console-less process is not a remedy; and giving the child a console of its own (`CREATE_NO_WINDOW`) is defeated by `FreeConsole` followed by `AttachConsole(<pid>)`, which is exactly the sequence the probe already runs. A real remedy needs a session, window-station or AppContainer boundary -- a product decision with its own cost, which is what #389 is for. What DID change in the code: the doc comment on `harden_against_credential_prompt` asserted `On Windows the analogue is DETACHED_PROCESS, which denies the child the parent's console and so CONIN$/CONOUT$ with it`. That sentence is false and is replaced by the measured table, because an overstated security guarantee is worse than an understated one."
  - id: c5
    text: "On unix, no prompt can be raised inside a quarantine operation at all: the child has no controlling terminal and cannot reacquire the parent's"
    state: met
    evidence: "test:crates/wcore-cli/tests/quarantine_terminal_authority.rs::quarantine_child_cannot_reach_the_controlling_terminal"
    owner: core
    note: "The elimination half of c2, stated at the scope it actually holds. The test re-execs itself inside a real PTY so the probe HAS a controlling terminal, then asserts PLAIN=OPEN as negative control, PRODUCTION_GIT=DENIED through build_git_command, HARDENED=DENIED, and GIT_STILL_RUNS=true as liveness. The mechanism is setsid(2) in pre_exec (quarantine.rs); a setsid'd child cannot take the parent's terminal back because TIOCSCTTY refuses a tty that is already another session's controlling terminal. Unchanged by this pass -- it is quoted here so c2's decomposition does not lose the half that does hold."
  - id: c6
    text: "What the Windows arm actually delivers is measured, not assumed, and the residual is pinned so it cannot silently change"
    state: met
    evidence: "test:crates/wcore-cli/tests/quarantine_console_authority_windows.rs::quarantine_child_has_no_console_at_creation_on_windows"
    owner: core
    note: "RUN ON REAL WINDOWS, 10.0.26200.9168, 2026-08-30 -- the ledger's previous `unexercised` caveat is retired. Measured, verbatim from the run: `[plain] SHARES_USER_CONSOLE_BEFORE=true CONOUT_BEFORE=OPEN`; `[hardened] SHARES_USER_CONSOLE_BEFORE=false CONOUT_BEFORE=DENIED(6)`; `[hardened] ATTACH_PARENT_PROCESS=SUCCEEDED SHARES_USER_CONSOLE_AFTER=true`; `[hardened] ATTACH_BY_EXPLICIT_PID=SUCCEEDED CONOUT_AFTER_EXPLICIT=OPEN`; `[production_git] SHARES_USER_CONSOLE_BEFORE=false ... SHARES_USER_CONSOLE_AFTER_EXPLICIT=true`. So DETACHED_PROCESS is a REDUCTION (the child is not created on the user's console) and not the elimination unix gets, and the residual is asserted rather than narrated: four pins fail the day AttachConsole stops working, telling the next reader to re-grade #389 instead of quietly agreeing. The oracle is `GetConsoleProcessList` and not `GetConsoleWindow`, because the production_git arm reaches the probe through Git for Windows' MSYS shell, which hands it a ConPTY of its OWN -- a console with no window that is not the user's. TWO RED ARMS, tree committed first, on the same Windows host: R1 removed `harden_against_credential_prompt(&mut cmd)` from `build_git_command` (verified on executable code) -> `[production_git] SHARES_USER_CONSOLE_BEFORE=true` and FAILED on `#338: build_git_command must apply the hardening`; R2 neutralised the flag (`cmd.creation_flags(0 * DETACHED_PROCESS)`) -> `[hardened] SHARES_USER_CONSOLE_BEFORE=true` and FAILED on `#338: a quarantine child must not be CREATED on the user's console`. Restored byte-identical (`git diff` empty), touched, green. CAVEAT: cfg(windows), and the Windows CI test legs are currently SKIPPED on these branches, so nothing but a SeanDesktop run exercises it today."
  - id: c3
    text: "The fix reasons about /dev/tty access rather than about inherited stdio or GIT_TERMINAL_PROMPT"
    state: met
    evidence: "symbol:crates/wcore-cli/src/plugin/quarantine.rs::harden_against_credential_prompt"
    owner: core
    note: "The doc comment names Route 1 (GIT_TERMINAL_PROMPT / GIT_ASKPASS / SSH_ASKPASS / GCM_INTERACTIVE / GIT_PAGER) and Route 2 (/dev/tty) explicitly, and the ACTUAL fix is Route 2: setsid so open('/dev/tty') returns ENXIO, inherited by every descendant helper. Fail-closed - a setsid failure fails the spawn."
  - id: c4
    text: "The policy is decided before implementation - deny /dev/tty to the clone, clear credential.helper for quarantine clones, or label quarantine-originated prompts"
    state: met
    evidence: "file:.planning/DECISIONS.md"
    owner: maintainer
    note: "TAKEN 2026-08-29 as Q-338c4: deny /dev/tty via setsid, in the SAME change as layer 1, because layer 1 alone makes the acceptance test green while credential.helper stays open. The rejected alternative and its product cost are also recorded in-tree at quarantine.rs:324 - clearing credential.helper would break private plugin sources. The Windows half is no longer unexercised: c6 measures it on a real Windows host. The teardown half MASTER-PLAN.md:202 required is still not decided -- tracked as FerroxLabs/wayland-core#379."
---

An interactive wayland-core session installing an untrusted third-party plugin
can be induced to prompt the user for credentials. The prompt arrives in the
user's own terminal during an operation they started and looks like it came from
Wayland, because git on the quarantine clone path can open /dev/tty directly
rather than using the stdio the product hands it.

READ THIS BEFORE USING THIS FILE. Unlike the other wayland-core entries seeded
in this pass, #338 was not covered by the v0.13.10 verification sweep. Every
criterion above is transcribed from the issue body alone and every one is
recorded not-met because nothing here has been graded against the shipped tree.
A not-met here means unverified, not measured-absent. The obvious first move is
to read crates/wcore-cli/src/plugin/quarantine.rs and establish what the clone
inherits before anyone designs a remedy.

The related unbounded-join hang on the same function was fixed separately and
does not address this.
