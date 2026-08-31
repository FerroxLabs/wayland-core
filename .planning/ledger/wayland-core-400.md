---
issue: 400
repo: FerroxLabs/wayland-core
kind: defect
title: "sandbox status never says the backend refuses PowerShell, while four production sites silently downgrade the shell"
status: open
last_verified_commit: cfcf97d0
criteria:
  - id: c1
    text: "sandbox status states, in both the human and the --json arm, that the active backend refuses PowerShell whenever blocks_powershell() is true"
    state: met
    evidence: "symbol:crates/wcore-cli/src/sandbox_cmd.rs::powershell_downgrade_note"
    owner: core
    note: "MET cfcf97d0. `SandboxStatus` gained two fields, both projected from the live registry in `project` and both serialised in `to_json`: `blocks_powershell` (the fact, read through SandboxRegistry::blocks_powershell -> SandboxBackend::blocks_powershell) and `powershell_downgrade` (the consequence sentence, Some iff the fact is true). The human arm prints the boolean beside its eight siblings and, when it is true, a headed NOTE block carrying the SAME string the --json arm carries -- one string projected into both arms, so the terminal and the desktop app cannot say different things. LIVE-PROVEN, not only unit-tested: the debug binary built from this commit was run on hetzner. `wayland-core sandbox status` prints the row 'blocks powershell         false' and `--json` carries both keys (...,'blocks_powershell':false,...,'powershell_downgrade':null,...) on the real bubblewrap backend. WHAT THAT LIVE RUN DOES NOT SHOW, said rather than left implied: no backend on Linux answers true, so the true arm was not exercised by the binary. It is exercised through the whole chain by c2's test on every target, and through the REAL AppContainer backend on Windows by `every_declaring_backends_disclosure_reaches_both_operator_arms`, whose appcontainer row now declares blocks_powershell. Verified on hetzner at this commit, clean tree: cargo check --workspace --all-targets = 0; cargo clippy -p wcore-config -p wcore-sandbox -p wcore-cli --all-targets -- -D warnings = 0; cargo nextest run -p wcore-config -p wcore-sandbox -p wcore-cli --retries 0 = 4970 tests run, 4970 passed, 0 failed."
  - id: c2
    text: "The statement is graded through backend -> SandboxRegistry -> SandboxStatus -> both arms, and a red arm making blocks_powershell return false reddens a test"
    state: met
    evidence: "test:crates/wcore-cli/src/sandbox_cmd.rs::a_backend_that_refuses_powershell_says_so_and_names_the_consequence"
    owner: core
    note: "MET cfcf97d0. Graded through backend -> SandboxRegistry -> SandboxStatus -> both arms, exactly as this ledger's earlier note asked, and NOT by asserting the boolean. A sentinel backend answering blocks_powershell() = true is wrapped in a real SandboxRegistry, projected with SandboxStatus::project, and read back out of to_json() and render_status_human(). RED ARM on the PRODUCTION path: SandboxRegistry::blocks_powershell's delegate replaced with `false` (MUTATION_SITES=1), cargo check -p wcore-cli --tests CHECK_EXIT=0 FIRST so the red is behaviour and not a build break, then TESTS_EXIT=100 -- 'the registry dropped the backend's answer before the status was even built'. Re-run against the FINAL artifact after the follow-up commit, same result; restore blob-verified equal to the HEAD blob (c639dcc195f8e782cd739f3e1c257abda1bd1ba8). WHY A SENTINEL AND NOT THE REAL BACKEND: the only backend answering true is Windows AppContainer, whose BACKENDS_THAT_DISCLOSE row is DeclaredOn::WindowsOnly, so grading only through it would leave this criterion ungraded on every host the release is actually built on. The real backend IS graded too, on Windows: blocks_powershell was added to DISCLOSURE_METHODS and to the appcontainer row's `declares`, which forces `every_disclosure_override_is_registered_for_read_grading` (source scan, every target) and a new arm in `assert_disclosure_reaches_the_operator` -- an unrecognised method there panics by design rather than being skipped. AvailabilityStub was given a blocks_powershell delegate in the same change; without it the trait default would have made that row's grade vacuous, which is the decorator failure this very ticket records. WRONG-REFUSAL CONTROL in the same test body: a backend answering false must get blocks_powershell=false, powershell_downgrade=None, JSON null (never an empty string), and a human arm that does NOT say it refuses PowerShell -- a disclosure that fires for every backend discloses nothing and would send an operator hunting a downgrade that never happened. Verified on hetzner at this commit, clean tree: cargo check --workspace --all-targets = 0; cargo clippy -p wcore-config -p wcore-sandbox -p wcore-cli --all-targets -- -D warnings = 0; cargo nextest run -p wcore-config -p wcore-sandbox -p wcore-cli --retries 0 = 4970 tests run, 4970 passed, 0 failed."
  - id: c3
    text: "The disclosure names the consequence (the command is downgraded to another shell) and not only the fact"
    state: met
    evidence: "file:crates/wcore-cli/src/sandbox_cmd.rs:181:DOWNGRADES a `powershell` / `pwsh` command"
    owner: core
    note: "MET cfcf97d0. The disclosure names what happens to the operator's command, not only the capability: 'backend X cannot run PowerShell, so the agent's Bash tool DOWNGRADES a powershell / pwsh command (and bash / sh) to cmd /C and runs it there: your command text is preserved, the shell you asked for is not.' That is read off downgrade_unsupported_shell_for_sandbox (crates/wcore-tools/src/bash.rs:199-232), which is what the four production call sites at :715, :800, :1018 and :1180 pass the boolean to: it matches powershell/pwsh/bash/sh, replaces the argv prefix with wcore_config::shell::windows_cmd_payload_prefix(), and re-appends the command -- so 'command preserved, shell not' is the measured behaviour and bash/sh are named because they are downgraded too. It warns ONCE via tracing::warn!, which is precisely why this criterion exists: with RUST_LOG unset only ERROR reaches stderr, so that warning reaches nobody. Graded, not merely written: c2's test asserts the string contains both 'downgrad' and 'cmd' and that the human arm prints the identical string the --json arm carries. A SECOND DEFECT WAS FOUND AND FIXED HERE BY THAT GRADE'S OWN STANDARD -- the sentence was first assembled through a line continuation that collapsed into runs of spaces, so what an operator would have read was mangled; commit cfcf97d04 rewrites it and the test now refuses a consequence containing whitespace runs. Red arm for that guard: re-mangling the literal (MUTATION_SITES=1, CHECK_EXIT=0) gives TESTS_EXIT=100 quoting the mangled sentence; restore blob-verified (6dd392d6c8a4eb3f2c0bf142c17cc4c384b6ae8d). Verified on hetzner at this commit, clean tree: cargo check --workspace --all-targets = 0; cargo clippy -p wcore-config -p wcore-sandbox -p wcore-cli --all-targets -- -D warnings = 0; cargo nextest run -p wcore-config -p wcore-sandbox -p wcore-cli --retries 0 = 4970 tests run, 4970 passed, 0 failed."
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

## Graded by lane doc-truth, cfcf97d0

c1, c2 and c3 met on `lane/f13-s2-doc-truth`, cut from `integ/f13` at `ca15a48bf`.
NOT CLOSED -- that is a maintainer action.

### The residual this lane did NOT take, and why

`SessionSandboxBackend` (`crates/wcore-agent/src/orchestration/anvil/forge.rs:167`) still
does not delegate `known_limitations` or `unavailable_reason`, and still cannot delegate
`availability_probe_is_startup_safe` without a new `SandboxRegistry` accessor. The earlier
note here asks whoever takes c1 to fix it in the same change. This lane did not, for the
reason that note itself records: `forge.rs` is in the generated Desktop contract corpus'
`SOURCE_INPUTS`, so a six-line delegation forces a corpus regeneration, and churning that
at RC time to close a path nothing reaches is the worse trade. It remains latent -- nothing
projects `SandboxStatus` through that decorator -- and it remains open. Stated here rather
than silently dropped, because a deferral with no carrier decays into nothing.

Note also that `blocks_powershell` joining `DISCLOSURE_METHODS` did NOT widen the source
scanner's blindness: `declared_limitations_are_registered.rs` walks `wcore-sandbox/src`
only, so the `forge.rs` decorator escapes it exactly as before. That boundary is deliberate
(widening it to the workspace sweeps in `wcore-tools`' test doubles) and is why the residual
above needs a human, not a wider grep.
