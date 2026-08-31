---
issue: 400
repo: FerroxLabs/wayland-core
kind: defect
title: "sandbox status never says the backend refuses PowerShell, while four production sites silently downgrade the shell"
status: closed
last_verified_commit: 6c87400b2
criteria:
  - id: c1
    text: "sandbox status states, in both the human and the --json arm, that the active backend refuses PowerShell whenever blocks_powershell() is true"
    state: met
    evidence: "test:crates/wcore-cli/src/sandbox_cmd.rs::a_powershell_refusal_reaches_both_operator_arms_through_the_registry"
    owner: core
    note: "`SandboxStatus` gains `blocks_powershell` and `shell_downgrade_notice`, both projected in `SandboxStatus::project` straight off the live `SandboxRegistry` (sandbox_cmd.rs:201-204), emitted in `to_json` and rendered in the human arm under a `POWERSHELL IS REFUSED` heading. `blocks_powershell` also joins `DISCLOSURE_METHODS` (crates/wcore-sandbox/src/backends/mod.rs), so the two totality checks added for #368 c6 now range over it: every override under `wcore-sandbox/src` must be registered in `BACKENDS_THAT_DISCLOSE` against the method it overrides, and every registered row must survive backend -> registry -> status -> BOTH arms. THE FOUR PRODUCTION SITES WERE RE-CONFIRMED BY GREP, not taken from the ticket: `grep -n downgrade_unsupported_shell_for_sandbox crates/wcore-tools/src/bash.rs` gives exactly :715, :800, :1018, :1180, and the helper at :199 rewrites `powershell`, `pwsh`, `bash` AND `sh`.\n\nTHE SECOND INSTANCE THE TICKET RECORDS IS ALSO CLOSED, in `642b8b23f`, because c1's own ticket asks whoever takes c1 to do it in the same change. `SessionSandboxBackend` (crates/wcore-agent/src/orchestration/anvil/forge.rs) forwarded 5 of `SandboxBackend`'s 17 items; it now forwards `known_limitations`, `unavailable_reason` AND `availability_probe_is_startup_safe` -- the three whose trait defaults are wrong in the non-conservative direction. The third could not be delegated before: `SandboxRegistry` had no such accessor, so one was added with its single caller named in its doc. The other nine defaults were read individually and are safe (two return `PolicyNotSupported`, one delegates to a third that does, `hard_containment_identity`'s `None` is documented as structurally incapable of minting hard containment, four booleans default `false` and UNDER-claim containment, `execute_streaming` drives `self.execute` which is forwarded). `forge.rs` is in `SOURCE_INPUTS`: the Desktop corpus was regenerated and key-diffed before committing -- only `fixture_digest` and `source_inputs_digest` moved, `schema_digest` is unchanged at `sha256:e31365bb...b07b`, the same two keys are the only movement in the six fixture files, and `cargo test -p wcore-protocol` is 32 binaries all ok."
  - id: c2
    text: "The statement is graded through backend -> SandboxRegistry -> SandboxStatus -> both arms, and a red arm making blocks_powershell return false reddens a test"
    state: met
    evidence: "test:crates/wcore-agent/src/orchestration/anvil/forge.rs::the_session_decorator_does_not_answer_for_the_backend_it_wraps"
    owner: core
    note: "GRADED ON THE PATH, NOT ON THE BOOLEAN -- which is what #368 c6 was first graded WITHOUT. A sentinel backend answering `true` is wrapped in a REAL `SandboxRegistry`, projected through `SandboxStatus::project`, and both arms are read. It runs on EVERY target, deliberately: the only backend in the workspace that answers `true` is `cfg(windows)`, so graded only through the `BACKENDS_THAT_DISCLOSE` table this property would be unmeasured on the Linux and macOS legs, which is exactly how a fact gets deleted with everything green.\n\nRED ARM, the one the criterion names, run on hetzner-dsm: replace `SandboxRegistry::blocks_powershell`'s delegate body with `false`. It COMPILES (`cargo check -p wcore-cli --tests` clean before the red was believed), and:\n\n  FAIL wcore-cli sandbox_cmd::disclosure_tests::a_powershell_refusal_reaches_both_operator_arms_through_the_registry\n  panicked at crates/wcore-cli/src/sandbox_cmd.rs:694:9:\n  the registry dropped the backend's PowerShell refusal before the status was even built\n  Summary 10 tests run: 9 passed, 1 failed\n\nNine of the ten disclosure tests stayed green under it, so the one that reddened is the one that grades this path and not a neighbour. `every_status_field_reaches_the_json_arm` (added on the earlier lane) separately asserts the `--json` key set EQUALS the field set scanned from `SandboxStatus`'s own source, so a future field that never reaches `to_json` reddens before review sees it.\n\nSECOND RED ARM for the decorator half of c1 (`642b8b23f`): each of the three newly forwarded methods was removed in turn from `SessionSandboxBackend`. All three compile, and all three redden `forge::tests::the_session_decorator_does_not_answer_for_the_backend_it_wraps` at a named assertion -- `the decorator inherited the trait default vec![]` / `... None` / `... true, which would put a backend's startup-UNSAFE probe back on the --json-stream readiness path`. That test drives the PRODUCTION decorator through a real registry, with the registry itself asserted first as a control so a failure cannot be the fixture."
  - id: c3
    text: "The disclosure names the consequence (the command is downgraded to another shell) and not only the fact"
    state: met
    evidence: "test:crates/wcore-cli/src/sandbox_cmd.rs::the_disclosure_names_the_downgrade_and_not_only_the_refusal"
    owner: core
    note: "The notice is DERIVED FROM THE REWRITE, not from the method name: `downgrade_unsupported_shell_for_sandbox` rewrites `powershell`, `pwsh`, `bash` and `sh` to the canonical `cmd` prefix, so a notice naming only PowerShell would leave an operator whose `bash -c` was rewritten with nothing to attribute it to. That the helper really does rewrite all four was checked at the source (bash.rs:206) rather than assumed from the ticket. The test pins PROPERTIES and not wording -- the backend name, `DOWNGRADED`, `cmd /C`, `NOT refused` (an operator told only that PowerShell is blocked would expect their command to FAIL; it does not, it runs somewhere else, which is the harder thing to attribute), and all four rewritten prefixes -- so an honest rewording does not redden and a disclosure that drops the rewrite does. It then asserts the notice reaches `--json` verbatim and every wrapped line of it reaches the human arm.\n\nRED ARM: reduce `shell_downgrade_notice` to the bare fact, `\"backend `{backend}` blocks powershell\"`. It compiles, and reddens two tests:\n\n  panicked at crates/wcore-cli/src/sandbox_cmd.rs:760:9:\n  the notice must say the command is downgraded and to WHAT, or an operator cannot attribute the shell they see: \"backend `sentinel_backend` blocks powershell\"\n  Summary 10 tests run: 8 passed, 2 failed\n\nThat is the accurate-but-unreadable row #368 c6 identified in the capability booleans, and it is now blocked. The `tracing::warn!` on the rewrite path is not a second chance and is not counted as one: with `RUST_LOG` unset only ERROR reaches stderr, so it reaches nobody by default."
---

All three criteria graded against the tree on 2026-08-31 by lane `sandbox`
(branch `lane/f13-sandbox`), every red arm run on hetzner-dsm and every mutation
compile-checked before its red was believed.

The disclosure itself landed in `b34b160c3` (c1/c2) and `ede0ceaca` (c3). This
lane additionally closed the SECOND INSTANCE the ticket records rather than
leaving it: `642b8b23f` makes `SessionSandboxBackend` total over the three trait
defaults that were wrong in the non-conservative direction, adds the registry
accessor the third one needed, and pays the Desktop corpus regeneration that
`forge.rs` being in `SOURCE_INPUTS` forces. It is still LATENT -- nothing
projects `SandboxStatus` through that decorator today -- and is described that
way here rather than overstated, because overstating it would be the failure this
ticket is about.

Nothing here reopens the Windows filesystem-sandbox or AppContainer decision.
Disclosure only.
