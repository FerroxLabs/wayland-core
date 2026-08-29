---
issue: 338
repo: FerroxLabs/wayland-core
title: "Untrusted plugin install can make Wayland prompt the user for credentials via /dev/tty"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "A quarantine clone of untrusted plugin content cannot open /dev/tty to prompt the user directly"
    state: not-met
    owner: core
    note: "seeded from the issue body and NOT graded against the tree. The report is that git opens /dev/tty directly, so it does not need the stdio we hand it, and an interactive session installing a third-party plugin can be induced to prompt mid-install"
  - id: c2
    text: "Any prompt raised inside a quarantine operation is distinguishable by the user from a prompt raised by Wayland itself"
    state: not-met
    owner: core
    note: "seeded from the issue body and NOT graded against the tree. The consent argument does not carry - the user consented to installing a plugin, not to handing credentials to its author, and the UI gives them nothing to tell the two apart"
  - id: c3
    text: "The fix reasons about /dev/tty access rather than about inherited stdio or GIT_TERMINAL_PROMPT"
    state: not-met
    owner: core
    note: "seeded from the issue body and NOT graded against the tree. The report states GIT_TERMINAL_PROMPT=0 was separately measured to be a no-op against helper-driven behaviour on this path, so suppressing git's own prompting does not close it"
  - id: c4
    text: "The policy is decided before implementation - deny /dev/tty to the clone, clear credential.helper for quarantine clones, or label quarantine-originated prompts"
    state: not-met
    owner: maintainer
    note: "seeded from the issue body and NOT graded against the tree. The reporter explicitly asks that this not be closed by adding an env var; clearing credential.helper breaks private plugin sources, which is a real product cost"
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
