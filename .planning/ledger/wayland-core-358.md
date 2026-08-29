---
issue: 358
repo: FerroxLabs/wayland-core
kind: defect
title: "OwnedTree owns only the LEAF on Windows: the grandchild case #1156 was filed about is still open on all 49 swept sites"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "OwnedTree kills the process TREE on Windows, not just the direct child"
    state: not-met
    owner: core
    note: "child_pids under cfg(windows) returns Vec::new() at crates/wcore-cli/tests/support/owned_tree.rs:97-100, so reap() snapshots an empty descendant set. The degradation is deliberate and documented at the definition; it is not yet closed."
  - id: c2
    text: "A test grades the grandchild case ON WINDOWS: a direct child with a detached grandchild, guard dropped while unwinding, both gone afterwards"
    state: not-met
    owner: core
    note: "crates/wcore-cli/tests/harness_owns_spawned_trees.rs is the Unix twin and the shape to mirror; it is cfg(unix) only because the mechanism is. No Windows twin exists."
  - id: c3
    text: "The red arm is quoted VERBATIM from a real Windows run, showing the grandchild surviving before the change"
    state: not-met
    owner: core
    note: "Nobody has watched anything fail on this platform. A test nobody watched fail is not evidence."
  - id: c4
    text: "A negative control passes in both arms, so a change that kills too much fails here"
    state: not-met
    owner: core
    note: "Guards against a Job Object or snapshot walk that reaps the runner's own agent process or a sibling job."
  - id: c5
    text: "The CI run that executed the Windows arm is cited by URL"
    state: not-met
    owner: core
    note: "The only feedback loop is a [ci-windows] push to a single contended self-hosted runner. No run is cited."
  - id: c6
    text: "clippy --target x86_64-pc-windows-msvc -p wcore-cli --all-targets -D warnings is clean"
    state: not-met
    owner: core
    note: "The mechanism is unsafe FFI (CreateToolhelp32Snapshot or a Job Object) and needs a target-gated dev-dependency plus a lockfile change. -gnu is not -msvc and does not substitute."
---

The fourth of `#352`'s four asks, split out because it is the only one that
cannot be executed from the Linux build host: it is not a test change but a new
platform capability plus a dependency, in `unsafe` FFI, iterable only through a
`[ci-windows]` push.

On Windows `OwnedTree::reap()` snapshots an empty descendant set, kills the
direct child and reaps it — so the grandchild case `#1156` was filed about is
still open there, on every swept site at once.

The contract is that the GUARD owns the tree, not the call sites, and that there
is no silent fallback: the Windows arm must be as loud about what it cannot do as
the Linux arm is. Deleting the Windows arm and having `OwnedTree` refuse to
compile there is an acceptable outcome — an honest "not supported" beats a guard
that looks present and owns nothing.
