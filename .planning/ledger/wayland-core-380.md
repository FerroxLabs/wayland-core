---
issue: 380
repo: FerroxLabs/wayland-core
kind: defect
title: "On Windows the #338 credential-prompt elimination is bypassable: DETACHED_PROCESS is not setsid"
status: closed
last_verified_commit: a278f8c3
criteria:
  - id: c1
    text: "A Windows test drives harden_against_credential_prompt and establishes what a quarantine child can do to the user's console, exercising both AllocConsole() and AttachConsole(ATTACH_PARENT_PROCESS)"
    state: met
    evidence: "test:crates/wcore-cli/tests/quarantine_console_authority_windows.rs::quarantine_child_has_no_console_at_creation_on_windows"
    owner: core
    note: "Landed and RUN 2026-08-30 on Windows 10.0.26200.9168 (SeanDesktop), exit 0. Three arms -- an unhardened child as negative control, a child through `harden_against_credential_prompt`, and a child through the production `build_git_command` so the WIRING is graded and not only the function. All four routes the ticket names are exercised from inside the child: `AttachConsole(ATTACH_PARENT_PROCESS)`, `AttachConsole(<driver pid>)` after `FreeConsole` (which is what forecloses reparenting as a remedy), `AllocConsole()` after `FreeConsole`, and a direct `CONOUT$` open after each. The oracle is `GetConsoleProcessList` -- is the driver's pid on MY console -- not `GetConsoleWindow()`, because the production arm reaches the probe through Git for Windows' MSYS shell, which hands it a ConPTY of its own: a console with no window that is NOT the user's, so a window-handle test would have called a clean arm dirty and a dirty arm clean. TWO RED ARMS on the same host, tree committed first: removing `harden_against_credential_prompt(&mut cmd)` from `build_git_command` gave `[production_git] SHARES_USER_CONSOLE_BEFORE=true` and FAILED on `#338: build_git_command must apply the hardening`; neutralising the flag (`cmd.creation_flags(0 * DETACHED_PROCESS)`) gave `[hardened] SHARES_USER_CONSOLE_BEFORE=true` and FAILED on `#338: a quarantine child must not be CREATED on the user\'s console`. Restored byte-identical, `git diff` empty, green."
  - id: c2
    text: "Either the Windows arm delivers the same elimination property as the unix arm -- the child cannot put a prompt on the user's console -- or the product states plainly that it does not and #338 c2 is scoped to unix in its own text"
    state: met
    evidence: "symbol:crates/wcore-cli/src/plugin/quarantine.rs::harden_against_credential_prompt"
    owner: core
    note: "The SECOND branch is taken, deliberately, and both halves of it are done. (1) The product states plainly that it does not: the doc comment previously read `On Windows the analogue is DETACHED_PROCESS, which denies the child the parent\'s console and so CONIN$/CONOUT$ with it` -- false, and now replaced by the measured table plus `So on Windows this is a REDUCTION ..., not the elimination unix gets` and an explicit instruction not to restore the analogy sentence. (2) #338 c2 is scoped in its own text: it is `superseded` into #389, with c5 stating the unix elimination that does hold and c6 stating what the Windows arm actually delivers. The FIRST branch was not taken because it is not reachable with process-creation flags: reparenting is foreclosed by the measured `ATTACH_BY_EXPLICIT_PID=SUCCEEDED`, and giving the child its own console (`CREATE_NO_WINDOW`) is foreclosed by `FreeConsole` followed by `AttachConsole(<pid>)`, which is the exact sequence the probe runs. Delivering the property needs a session, window-station or AppContainer boundary; that product decision is #389, not this ticket. REFUTED 2026-08-30, CONCEDED, AND NOW ACTUALLY DONE. When this was first graded met, half (2) was NOT done. The acceptance is a conjunct and the verifier proved the second half false by identity, not by argument: `diff <(git show origin/integ/f13:.planning/ledger/wayland-core-338.md | grep \"Any prompt raised\") <(grep \"Any prompt raised\" .planning/ledger/wayland-core-338.md)` printed C2_TEXT_IDENTICAL. Only #338 c2`s STATE had changed and a c5/c6 pair had been added beside it; the `text:` field itself was byte-identical to base and carried no unix qualifier -- so `scoped to unix in its own text` was false, and the in-tree sentence at quarantine.rs asserting it was the same overstatement this ticket exists to remove. FIXED by rewriting #338 c2`s text FIELD to open `ON UNIX:` and to name the Windows non-delivery and #389 explicitly, and by tightening the quarantine.rs sentence to point at the ledger file that now carries the scope so a reader can check it in one command. Half (1) was and remains done. This note is the record that the criterion was re-graded after refutation rather than left standing on the earlier claim."
  - id: c3
    text: "The Windows result is quoted VERBATIM from a real run on SeanDesktop; a cross-compile is not a runtime proof"
    state: met
    evidence: "test:crates/wcore-cli/tests/quarantine_console_authority_windows.rs::quarantine_child_has_no_console_at_creation_on_windows"
    owner: core
    note: "Verbatim from the run, Windows 10.0.26200.9168, 2026-08-30, exit 0: `[plain] SHARES_USER_CONSOLE_BEFORE=true CONOUT_BEFORE=OPEN`; `[hardened] SHARES_USER_CONSOLE_BEFORE=false CONOUT_BEFORE=DENIED(6)`; `[hardened] ATTACH_PARENT_PROCESS=SUCCEEDED SHARES_USER_CONSOLE_AFTER=true CONOUT_AFTER=OPEN`; `[hardened] ATTACH_BY_EXPLICIT_PID=SUCCEEDED CONOUT_AFTER_EXPLICIT=OPEN SHARES_USER_CONSOLE_AFTER_EXPLICIT=true`; `[hardened] ALLOC_CONSOLE=SUCCEEDED SHARES_USER_CONSOLE_AFTER_ALLOC=false`; `[production_git] SHARES_USER_CONSOLE_BEFORE=false ... SHARES_USER_CONSOLE_AFTER_EXPLICIT=true`. Not a cross-compile: `cargo check --target x86_64-pc-windows-gnu` compiles the assertions and runs nothing, and the test prints its measurements rather than only a verdict precisely so a reader who cannot re-run it still sees the numbers. CAVEAT, stated not assumed away: the file is `#![cfg(windows)]` and the Windows CI test legs are currently SKIPPED on these branches, so today this is a named-host run, not a standing gate."
---

On Windows the #338 fix is bypassable, not merely untested. DETACHED_PROCESS (quarantine.rs:358-366) withholds the parent's console at creation time only; a console-less child may call AllocConsole() to make one, or AttachConsole(ATTACH_PARENT_PROCESS) to attach to its parent's console -- both documented Win32 behaviour. That is not the unix guarantee: a setsid'd child cannot reacquire the parent's ctty, because TIOCSCTTY refuses a terminal that is already another session's controlling terminal. So a third-party credential helper invoked by a quarantine clone on Windows can still raise an unattributable prompt on the user's console, which is the exact harm #338 describes.

**Where.** crates/wcore-cli/src/plugin/quarantine.rs:358-366 (#[cfg(windows)] DETACHED_PROCESS); no Windows test exists -- crates/wcore-cli/tests/quarantine_terminal_authority.rs is `#![cfg(unix)]` at line 30 and is the only caller of harden_against_credential_prompt outside src/

**Why it matters.** The ledger's c4 note flags the Windows arm as 'unexercised', which reads as a coverage gap. It is a correctness gap: the chosen Windows primitive does not deliver the elimination property c2 is graded on. Closing #338 on unix evidence alone records the class as shut on a platform where it is still open, and there is no test that would ever catch it.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.


UPDATE 2026-08-30. All three criteria are met. #389 was filed independently
on 2026-08-29 describing the same finding with deliver-or-label acceptance
criteria; the live remainder lives there, and this ticket -- which asked for
the measurement and for the product to stop overclaiming -- is answered.
Closing it is Sean's action, not core's.
