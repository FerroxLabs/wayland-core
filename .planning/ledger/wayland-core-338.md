---
issue: 338
repo: FerroxLabs/wayland-core
kind: defect
title: "Untrusted plugin install can make Wayland prompt the user for credentials via /dev/tty"
status: open
last_verified_commit: 431d21ed
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
    evidence: "test:crates/wcore-cli/tests/quarantine_credential_helper_policy.rs::the_quarantine_builder_applies_this_platforms_credential_policy"
    owner: core
    note: "Satisfied by ELIMINATION, not by labelling, and now on BOTH platforms rather than one. Wording deviation flagged rather than papered over. UNIX: build_git_command redirects stdio away from the terminal and setsid removes the ctty, so no quarantine-originated prompt can reach the user. WINDOWS: DETACHED_PROCESS is NOT that guarantee and the claim that it was has been removed from the source. MEASURED on Windows 11 build 26200 - a DETACHED_PROCESS child reached the launcher console via AttachConsole(ATTACH_PARENT_PROCESS), a console-less grandchild reached it via AttachConsole(<launcher pid>), and a grandchild holding its own console reached it after FreeConsole(); all three writes were read back out of the launcher screen buffer with ReadConsoleOutputCharacterW, against a control marker proving the read-back works. Windows therefore takes the SECOND policy on this criterion own menu (c4): credential.helper is reset in the argv so no third-party helper is spawned to prompt at all. The named test drives a real git against a loopback endpoint that answers 401, with a CONTROL arm proving a plain git DOES invoke the configured helper and a served-request counter proving the arm reached a credential lookup."
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
    note: "TAKEN 2026-08-29 as Q-338c4 (unix: deny /dev/tty via setsid, in the SAME change as layer 1) and EXTENDED 2026-08-29 as Q-338d6 (windows: clear credential.helper, because the first option is measurably unavailable there). The earlier CAVEAT - the DETACHED_PROCESS arm was cfg(windows) while every test was cfg(unix) - is closed: the unit module is now cfg(test), and two of its tests plus one integration test run on Windows."
  - id: c5
    text: "A quarantine git that fails does not leave the helpers it spawned running"
    state: met
    evidence: "test:crates/wcore-cli/src/plugin/quarantine.rs::a_timed_out_git_reaps_the_whole_detached_tree"
    owner: core
    note: "ADDED - not in the issue body, obliged by the fix. The setsid hardening put every quarantine git child in a session wayland-core does not own while the timeout path still SIGKILLed one pid, so the fix converted a bounded wedge into an unreaped detached tree (.planning/MASTER-PLAN.md:144 predicted exactly this and MASTER-PLAN.md:202 obliged the teardown to land in the same change). symbol:crates/wcore-cli/src/plugin/quarantine.rs::GitProcessTree now owns the tree - kill(-pgid) behind a verified group-leadership check on unix, a kill-on-close Job Object on Windows - reaped from EVERY error exit of run_git via Drop and disarmed only on success. Both failure shapes are graded, not just the one the defect named: the wall-clock timeout and the drain guard. RED ARM: the three new tests grafted onto a clean pre-fix checkout of origin/integ/f13 fail 3/3 (2 passed, 3 failed) with the defect wording - the background worker N that the timed-out git spawned is STILL ALIVE - while the two pre-existing tests stay green."
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

## What the Windows arm can and cannot promise (measured 2026-08-29)

Do not restore the sentence "on Windows the analogue is DETACHED_PROCESS". It
is false, and it is what let the Windows half ship unexercised. `DETACHED_PROCESS`
withholds the parent console at CREATION time and is not a boundary: on Windows
11 build 26200 every re-acquisition route tested succeeded (direct child via
`AttachConsole(ATTACH_PARENT_PROCESS)`; console-less grandchild via
`AttachConsole(<pid>)`; grandchild via `FreeConsole()` then `AttachConsole(<pid>)`).
The unix guarantee is different in kind - `TIOCSCTTY` refuses a terminal that is
already another session's ctty - so it cannot be ported.

What closes #338 on Windows is therefore that no third-party credential helper
is spawned at all. The residual, stated rather than hidden: an `ssh://` or
`git@` source still reaches `ssh`, whose Windows passphrase prompt is not a
credential helper and is not covered by that reset. `SSH_ASKPASS=""` plus
`SSH_ASKPASS_REQUIRE=never` are pinned, but they are route-1 knobs, and route 1
is exactly the class Windows cannot enforce.
