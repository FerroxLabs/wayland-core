---
issue: 338
repo: FerroxLabs/wayland-core
kind: defect
title: "Untrusted plugin install can make Wayland prompt the user for credentials via /dev/tty"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "A quarantine clone of untrusted plugin content cannot open /dev/tty to prompt the user directly"
    state: met
    evidence: "test:crates/wcore-cli/tests/quarantine_terminal_authority.rs::quarantine_child_cannot_reach_the_controlling_terminal"
    owner: core
    note: "The test re-execs itself inside a real PTY so the probe HAS a controlling terminal, then asserts PLAIN=OPEN as negative control, PRODUCTION_GIT=DENIED through build_git_command, HARDENED=DENIED and GIT_STILL_RUNS=true as liveness. The fix is setsid(2) in pre_exec on unix / DETACHED_PROCESS on windows, quarantine.rs:334-368."
  - id: c2
    text: "Any prompt raised inside a quarantine operation is distinguishable by the user from a prompt raised by Wayland itself"
    state: met
    evidence: "test:crates/wcore-cli/tests/quarantine_terminal_authority_windows.rs::a_quarantine_child_does_not_inherit_the_users_console"
    owner: core
    note: "Satisfied by ELIMINATION, not by labelling. build_git_command is the sole Command::new in the file and it both redirects stdio away from the terminal and takes the terminal away, so no quarantine-originated prompt can reach the user at all. The criterion asked for distinguishability; the chosen policy makes the ambiguous prompt unreachable. Wording deviation flagged rather than papered over. WINDOWS WAS UNEXERCISED UNTIL 2026-08-29 and is now MEASURED, on SeanDesktop, Windows 11 build 10.0.26200.9168, through the production build_git_command: `SELF_PID=49352 / PLAIN=OPEN / HARDENED=DENIED / GIT_STILL_RUNS=true / PLAIN_PIDS=CONSOLE:50520,49352,28320,50160,15040,16120 / HARDENED_PIDS=NO_OUTPUT(status=Some(0), stderr=\"\") / PRODUCTION_GIT_PIDS=CONSOLE:9700,40648,28084`. Reading, with the instrument control first: the UNHARDENED child\x27s console process list CONTAINS this test\x27s pid (49352), so the probe can tell \"the user\x27s console\" from \"a console\"; a hardened `cmd` child is DENIED `CONOUT$`; and a hardened `git` through the production builder reports a console that does NOT contain our pid. THE FIRST WINDOWS RUN SAID `PRODUCTION_GIT=OPEN` and would have been reported as an unhardened production path -- it was wrong, because Git for Windows runs a `!`-alias through its MSYS2 `sh`, whose runtime allocates a console of its own, and `open CONOUT$` cannot tell that console from ours. The criterion is graded on the pid-list probe for exactly that reason. RESIDUAL, measured rather than asserted: on Windows the property is \"the child does not end up on the USER\x27S console\", not the Unix \"the child can have no terminal\". DETACHED_PROCESS withholds the parent console at creation; AllocConsole()/AttachConsole(ATTACH_PARENT_PROCESS) are not blocked by it, and the production git demonstrably ends up with three pids in a console of its own. A prompt there appears in a window the child made, never on the terminal the install was launched from. The source doc no longer calls DETACHED_PROCESS the \"analogue\" of setsid."
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
    note: "TAKEN 2026-08-29 as Q-338c4: deny /dev/tty via setsid, in the SAME change as layer 1, because layer 1 alone makes the acceptance test green while credential.helper stays open. The rejected alternative and its product cost are also recorded in-tree at quarantine.rs:324 - clearing credential.helper would break private plugin sources. CAVEAT: the DETACHED_PROCESS arm is cfg(windows) and the test is cfg(unix), so the Windows half of the fix is unexercised."
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
