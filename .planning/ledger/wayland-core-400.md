---
issue: 400
repo: FerroxLabs/wayland-core
kind: defect
title: "sandbox status never says the backend refuses PowerShell, while four production sites silently downgrade the shell"
status: open
last_verified_commit: bb8706ee
criteria:
  - id: c1
    text: "sandbox status states, in both the human and the --json arm, that the active backend refuses PowerShell whenever blocks_powershell() is true"
    state: not-met
    owner: core
    note: "FILED AND NOT TAKEN 2026-08-30 by lane w3-windows-honesty, which found it while closing #368 c6 and is recording WHY it stopped rather than shipping it. This adds a field to `sandbox status --json`, an operator surface a host integration reads, and the lane's assignment was #368/#369/#370/#389 -- a new field on a shipped surface at RC time, on a lane already answering a refutation, is not work that traces to any of those four. It is filed rather than deferred so it has a carrier: a deferral with no ticket decays into nothing. VERIFIED AGAINST THE TREE, not taken from the issue prose: `SandboxRegistry::blocks_powershell` exists at crates/wcore-sandbox/src/lib.rs:424; `SandboxStatus::project` at crates/wcore-cli/src/sandbox_cmd.rs:150-190 projects eight fields and this is not one of them; the four rewriting call sites are crates/wcore-tools/src/bash.rs:715, :800, :1018, :1180, each passing it to `downgrade_unsupported_shell_for_sandbox`."
  - id: c2
    text: "The statement is graded through backend -> SandboxRegistry -> SandboxStatus -> both arms, and a red arm making blocks_powershell return false reddens a test"
    state: not-met
    owner: core
    note: "The property, and it is the one #368 c6 was first graded WITHOUT. c6 was anchored to a test asserting two constants were well formed; replacing `AppContainerBackend::known_limitations` with `Vec::new()` compiled, emptied the disclosure on real Windows, and left every test green because nothing called the method. Grading a boolean here would repeat that exactly. The machinery to do it right already exists on this branch -- `BACKENDS_THAT_DECLARE_LIMITATIONS` plus `every_declaring_backends_disclosure_reaches_both_operator_arms` -- so whoever takes this extends that projection rather than writing a new assertion on `blocks_powershell()`."
  - id: c3
    text: "The disclosure names the consequence (the command is downgraded to another shell) and not only the fact"
    state: not-met
    owner: core
    note: "Separated from c1 so it cannot be met by inattention. A row reading `blocks powershell true` is the same failure mode #368 c6 identified in the capability booleans: accurate, and unreadable as a posture. What the operator SEES is a powershell command running under a different shell, so the disclosure has to name that or it does not let them attribute what they observe."
---

Filed 2026-08-30 by lane w3-windows-honesty as the N+1 of the class #368 c6
closed, and filed rather than fixed for the reason recorded on c1.

#368 c6 asked that the product state a defect where an operator reads the
containment posture, and closed that for `known_limitations`. `blocks_powershell`
is the same shape one trait method over and is worse: `known_limitations` was
information withheld, whereas this one silently REWRITES the operator's argv at
four production sites while the only surface describing the sandbox's posture
says nothing about it.

Both trackers were searched before filing, with a control that returned hits so
an empty result could not read as absence: FerroxLabs/wayland-core#252 and
FerroxLabs/wayland#737 / #754 are the PowerShell EXECUTION failures, and none of
them is about the posture surface.

A SECOND INSTANCE OF THE SAME CLASS, found in the same sweep and recorded here
rather than filed separately, because the remedy is the same one and splitting it
would give two tickets one fix. `SessionSandboxBackend`
(crates/wcore-agent/src/orchestration/anvil/forge.rs:167) is a DECORATOR over a
real `SandboxRegistry`. It delegates `execute`, `name`, `is_available`,
`enforces_read_deny` and `blocks_powershell` -- and does NOT delegate
`known_limitations` or `unavailable_reason`, so through it both fall back to the
trait defaults `vec![]` and `None`. Any future status read taken through that
decorator would report a backend with no known limitations and no reason for
being unavailable, which is precisely the reassurance #368 c6 was filed about.

TOTAL OVER THE TRAIT, so those are not two somebody stopped at. Re-derived
2026-08-30 by the resuming fix lane, which read every default rather than
assuming the unnamed ones were harmless: `SandboxBackend` declares 17 items,
the decorator forwards 5, and the other 12 inherit their defaults. THREE of
those inherit in a NON-CONSERVATIVE direction, not two.

The third is `availability_probe_is_startup_safe`, whose default is `true`,
and it is behavioural rather than disclosure. `AppContainerBackend` overrides
it to `false` (crates/wcore-sandbox/src/backends/appcontainer/windows_impl/
process.rs:382) precisely because its probe is a 15s wall-clock-guarded real
spawn, and `select_without_startup_probe` (crates/wcore-sandbox/src/lib.rs:731)
reads it to keep that spawn off the `--json-stream` readiness path. Through the
decorator that answer flips to `true`, which would put the guarded spawn back
on a startup path -- the #125 hang class. It is latent for the same reason the
other two are (nothing selects a backend through this decorator), and it cannot
be delegated today even by an author who wanted to: `SandboxRegistry` exposes no
`availability_probe_is_startup_safe`, so closing it needs a registry accessor as
well as a forwarding method.

The remaining nine are safe, checked individually and not by assumption:
`execute_with_cwd_authority` returns `PolicyNotSupported`; `probe_hard_containment`
returns `PolicyNotSupported`; `execute_with_workspace_authority` delegates to the
former; `hard_containment_identity` defaults `None`, which the trait documents as
structurally incapable of minting hard containment; `confines_filesystem`,
`owns_descendants_hard`, `binds_cwd_authority` and `binds_workspace_authority`
default `false`, i.e. they UNDER-claim containment; and `execute_streaming`'s
default drives `self.execute`, which the decorator does forward. So the hazard
through this decorator is the disclosure pair PLUS the startup-probe answer, and
that is a statement about all 17 items rather than about the ones that were
noticed first.

It is NOT reached today: the decorator is Anvil's gate-closure executor and
nothing projects `SandboxStatus` through it, so this is a latent hole, not a live
one -- and it is stated as latent rather than as a bug, because overstating it
here would be the same failure the ticket is about. It is NOT fixed here for one
stated reason: `forge.rs` is in the generated Desktop contract corpus's
`SOURCE_INPUTS`, so a six-line delegation forces a corpus regeneration, and
churning that at RC time to close an unreached path is a worse trade than
recording it. Whoever takes c1 delegates both methods in the same change.

Note also what the scanner in
`crates/wcore-sandbox/tests/declared_limitations_are_registered.rs` can and
cannot see: it walks `wcore-sandbox/src` only, so it is TOTAL over that crate and
BLIND to a `SandboxBackend` implementation in any other -- which is how the
decorator above escapes it. Widening it to the workspace would sweep in the many
test doubles in `wcore-tools` and turn a decidable check into a maintained
denylist, so the boundary is deliberate and is written down here rather than
discovered later.

Nothing here reopens the Windows filesystem-sandbox or AppContainer decision.
This is disclosure only.
