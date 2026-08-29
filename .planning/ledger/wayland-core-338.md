---
issue: 338
repo: FerroxLabs/wayland-core
title: "Untrusted plugin install can make Wayland prompt the user for credentials via /dev/tty"
status: open
last_verified_commit: 43848f75
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
    evidence: "symbol:crates/wcore-cli/src/plugin/quarantine.rs::build_git_command"
    owner: core
    note: "Satisfied by ELIMINATION, not by labelling. build_git_command is the sole Command::new in the file and it both redirects stdio away from the terminal and removes the ctty, so no quarantine-originated prompt can reach the user at all. The criterion asked for distinguishability; the chosen policy makes the ambiguous prompt unreachable. Wording deviation flagged rather than papered over."
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
