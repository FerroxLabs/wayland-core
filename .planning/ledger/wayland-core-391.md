---
issue: 391
repo: FerroxLabs/wayland-core
kind: defect
title: "Windows: the local-operator shell applies no OS read-deny, so the VCS content store is readable to Bash (split from #244 c4)"
status: open
last_verified_commit: a278f8c3
criteria:
  - id: c1
    text: "Whether the Windows local-operator shell is expected to confine the VCS content store is a DECIDED question, recorded with its reason, rather than an unexamined gap"
    state: met
    evidence: "file:.planning/DECISIONS.md"
    owner: core
    note: "TAKEN 2026-08-30 as Q-391: NO, and say so everywhere the product speaks. Owner is core and not maintainer, per Sean`s standing decide-do-not-park rule and per DECISIONS.md`s own header. The decision is not a new product call -- it is an existing standing ruling being written down where a reader can find it: delivering the confinement needs a Windows FILESYSTEM sandbox and Windows gets none (AppContainer is CLOSED and must not be reopened). The alternative of removing the local-operator exemption is refused for a reason with evidence behind it, not by preference: the exemption exists because refusing left every fresh Windows clone with no shell at all, and the product`s own printed remedy --trust-workspace hands back a policy with secret_read_deny_required == false and therefore the IDENTICAL uncontained shell. A gate whose documented one-command bypass grants the same capability is a usability cost, not a boundary."
  - id: c2
    text: "The product claims no more than it delivers, anywhere a user or a reader can see it"
    state: met
    evidence: "symbol:crates/wcore-tools/src/workspace_policy.rs::is_vcs_content_store_static"
    owner: core
    note: "Three overclaims found by grep and all three removed in the same change, with a known-positive control in the same query so the search could not read clean by failing. (1) #244 c4`s criterion text said `a Bash subprocess cannot read the store` unqualified -- rewritten to `wherever the OS sandbox enforces read-deny ... where it cannot, the shell is REFUSED`. (2) The cited doc comment said `BashTool`s subprocess is confined by the OS sandbox, which consumes vcs_content_stores as fs_read_deny` full stop -- now qualified with WHERE the backend enforces read-deny, with the local-operator exemption, the Windows job-object default and this issue named. (3) DECISIONS.md carries Q-391 and its reasoning, so the position is discoverable without reading either. Graded `met` at the surfaces that exist today: docs/tools.md`s object-store passages are about Write recovery and make no confinement claim -- checked, not assumed."
  - id: c3
    text: "The gap is pinned by a test that fails if it ever closes, and that test is not confined to a Windows host"
    state: met
    evidence: "test:crates/wcore-tools/tests/bash_vcs_store_local_operator_gap.rs::a_local_operator_shell_reads_the_store_on_a_non_enforcing_backend"
    owner: core
    note: "MEASURED TWO WAYS THAT AGREE. On real Windows 10.0.26200.9168: PLATFORM_ENFORCES_READ_DENY=false, CONTROL_PLAIN_OK=true, CONTROL_HEAD_OK=true, ROOT_STORE_LEAKED=true, NESTED_STORE_LEAKED=true, RECURSIVE_LEAKED=true. Independently on Linux (hetzner), verbatim: `LOCAL_OPERATOR_STORE_READ: Exit code: 0 / STDOUT: ROOT-OBJECT-BYTES-244`. NOT cfg(windows), deliberately: WindowsJobObjectBackend compiles on every target and really spawns -- it delegates to NoSandboxBackend -- which is the property local_operator_shell_gate.rs already relies on, so this is a STANDING gate on the build host rather than a named-host observation nothing re-checks. The fail-closed arm is reproduced FIRST in the sibling test: the same backend and command with a non-local principal returns `Refused: shell is unavailable because the active sandbox`. Both preconditions are asserted rather than assumed -- the backend must NOT claim read-deny and the policy must still REQUIRE it -- so neither test can go vacuous, and an ordinary working-tree read is the positive control in the open arm. The test asserts the gap IS THERE and says in its failure message that a failure means the gap has CLOSED: re-grade #244 c4 and this issue, do not delete the test."
  - id: c4
    text: "If Q-391 is ever revisited and the store SHOULD be confined for the local operator on Windows, the mechanism does not depend on AppContainer"
    state: blocked
    owner: maintainer
    handoff: "FerroxLabs/wayland-core#254"
    note: "BLOCKED, with the reason stated rather than suppressed: this criterion is conditional on a decision that has been taken the other way (c1, Q-391 = no), so there is nothing for core to build and nothing for core to grade. It is kept rather than deleted because the condition is a real future branch, and the AppContainer bar is the part a future reader most needs to inherit -- that route cost months and is closed. #254 is the open ticket that owns the Windows sandbox-backend question end to end, so it is where this lands if the decision is ever reopened."
---

Split out of #244 c4 while re-grading it after a verifier refuted the
unqualified text. #244 c4 asserted "a Bash subprocess cannot read the store at
the root or at any nested depth"; on the Windows shipping default, for the
ordinary interactive user, that is FALSE. The ledger previously recorded only
that the Linux test "skips by construction" there -- untested is not the
finding, false-on-the-shipping-platform is.

This ticket is the honest statement plus the pin, and it is largely DONE: what
remains is that a defect ticket stays open until someone with the authority to
close it agrees the position is right. The live harm, if the position is ever
judged wrong, moves to #254.

Related and deliberately NOT folded in: #388 (GitTool reconstructs store bytes
by porcelain, every platform, inside the sandbox) is a different mechanism, and
#390 is the in-process predicate's own nested-gitfile miss.
