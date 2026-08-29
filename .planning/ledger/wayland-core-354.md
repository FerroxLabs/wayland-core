---
issue: 354
repo: FerroxLabs/wayland-core
kind: defect
title: "MCP malware gate: make the OSV fail-open an explicit operator choice (strict/permissive)"
status: open
last_verified_commit: 4e24bda6
criteria:
  - id: c1
    text: "A config key selects the malware-gate mode, permissive or strict, defaulting to today's permissive behaviour"
    state: met
    owner: core
    evidence: "test:crates/wcore-config/src/config.rs::malware_gate_defaults_to_permissive_and_parses_both_modes"
    note: "`[mcp] malware_gate = \"permissive\" | \"strict\"` on `McpConfig`, through the config layer -- no hardcoded conditional. `#[default] Permissive`, so omitting the key leaves every existing install byte-identical; the test asserts the omitted case, both spellings, and that a typo (`\"strcit\"`) is a LOAD ERROR rather than a silent fall back to permissive. Cascade is asymmetric on purpose (`McpMalwareGateMode::stricter_of`): a project config may tighten to strict but can never loosen an operator's strict, the same rule `trust_project_hooks` already applies -- graded by `a_project_config_cannot_loosen_the_malware_gate`. Installed process-wide from `AgentBootstrap::build` via `wcore_mcp::malware_gate::install_mode` (one-shot, like `wcore_egress::install_global_policy`), because `StdioTransport::spawn` has no config handle."
  - id: c2
    text: "Under strict, a backend error from check_package_for_malware refuses with McpError::MalwareBlocked instead of returning Allowed"
    state: met
    owner: core
    evidence: "test:crates/wcore-mcp/tests/mcp_launch_malware_gate.rs::strict_refuses_an_unchecked_package_before_exec"
    note: "`MalwareCheckOutcome` gained `Unavailable(String)`; a backend error and the SSRF short-circuit now return it instead of collapsing into `Allowed`, so `Allowed` finally means \"queried and cleared\". `malware_gate::decide` (symbol:crates/wcore-mcp/src/malware_gate.rs::decide) maps `Unavailable` through the mode: permissive -> Ok, strict -> `McpError::MalwareBlocked` naming the key and the way out; the message content is graded by `the_strict_refusal_names_the_key_and_the_way_out`. RED ARM RUN, quoted verbatim in the note below."
  - id: c3
    text: "The SSRF short-circuit, where is_safe_url fails, follows the same mode as the backend-error path"
    state: met
    owner: core
    evidence: "test:crates/wcore-mcp/src/malware_gate.rs::the_ssrf_short_circuit_follows_the_same_mode_as_a_backend_error"
    note: "True by construction, not by two matching copies: both paths return `MalwareCheckOutcome::Unavailable`, and `decide` has exactly ONE arm for it. The test drives both for real -- the SSRF arm against 169.254.169.254 (never queried, asserted by an empty backend call log) and the error arm against a public-IP endpoint with a Network error -- and asserts the two verdicts are equal in BOTH modes, and that they are errors exactly when the mode is strict."
  - id: c4
    text: "A test per mode drives StdioTransport::spawn against an unreachable backend: permissive launches and logs at ERROR, strict refuses and never reaches exec"
    state: met
    owner: core
    evidence: "test:crates/wcore-mcp/tests/mcp_launch_malware_gate.rs::permissive_launches_an_unchecked_package_and_logs_at_error"
    note: "Both arms go through the real `StdioTransport::spawn` with the same `CapturingOsvBackend::with_error(Network)`. Permissive: marker file present (child ran) AND a captured `tracing::Level::ERROR` -- the fail-open stays audible, since ERROR is the only level a user with RUST_LOG unset sees. Strict: `Err(McpError::MalwareBlocked)` whose message contains `malware_gate`, marker ABSENT after the helper's 500ms wait, and the backend call count is still 1 so strict refuses AFTER trying rather than instead of trying. The strict arm is cited on c2 (`strict_refuses_an_unchecked_package_before_exec`) because the gate takes one token per criterion; it is the same test. A `ModeGuard` restores the process mode on drop so a mode cannot leak into the next test in the `osv_gate` serial group."
  - id: c5
    text: "A negative control shows a clean package launches in BOTH modes"
    state: met
    owner: core
    evidence: "test:crates/wcore-mcp/tests/mcp_launch_malware_gate.rs::a_clean_package_launches_in_both_modes"
    note: "Two controls in each mode: `uvx clean-pkg==1.0` IS queried (call count asserted 1, so the pass is not vacuous) and launches, and `my-mcp-server --port 0` is never queried and launches -- strict must not start gating commands that fetch nothing. The polarity control in the other direction is `test:crates/wcore-mcp/tests/mcp_launch_malware_gate.rs::permissive_still_blocks_known_malware`: a definite malware hit is refused in permissive too, so \"strict blocks malware\" cannot be satisfied by a build that only blocks in strict."
  - id: c6
    text: "The mode is documented in docs/mcp.md and surfaced in /doctor"
    state: met
    owner: core
    evidence: "symbol:crates/wcore-cli/src/doctor/mod.rs::malware_gate_line"
    note: "`docs/mcp.md` gains `## Supply-chain malware gate -- [mcp] malware_gate`: the four-answer table, which one the key governs, the honest cost of strict (no network means no npx/uvx MCP servers), the stricter-layer-wins cascade, and the /doctor line (`file:docs/mcp.md`). `print_mcp_section` prints `malware_gate_line(cfg.mcp.malware_gate)` whether or not any server is declared -- a fresh config with no servers is exactly when an operator wants to see the posture. The line is a pure function so the CONTENT is graded by `test:crates/wcore-cli/src/doctor/mod.rs::doctor_names_the_malware_gate_mode_and_what_it_does`: each mode must name the key, its value, and the consequence, and the two modes must differ."
  - id: c7
    text: "The already-shipping non-session MCP launch path reads the operator's chosen mode, not the uninstalled permissive default"
    state: met
    owner: core
    evidence: "test:crates/wcore-cli/tests/doctor_probe_malware_gate.rs::doctor_probe_launches_under_the_operator_malware_gate"
    note: "CLOSED on lane/f13-n-mcp-gate. `print_mcp_section` now calls `install_and_report_malware_gate` (symbol:crates/wcore-cli/src/doctor/mod.rs::install_and_report_malware_gate), which installs `cfg.mcp.malware_gate` and then builds the printed line from `malware_gate::mode()` — the READ-BACK is the point: the posture doctor prints and the posture doctor enforces are now one value, so they cannot disagree again. Enumeration behind `not the instance`: `grep -rn 'McpManager::connect' crates/ --include=*.rs` outside tests reaches four call sites — bootstrap.rs:1843, plugins/mcp_delivery.rs:167, main.rs:5452 and main.rs:5742 (all post-`AgentBootstrap::build`), engine_bridge.rs:3038 (in-session `AddMcpServer`) — plus doctor/mod.rs:654. Doctor was the only pre-bootstrap one, which is what c7 says. REPRODUCED FIRST on origin/integ/f13 at 362ba8a60 with the real binary: under `unshare -n` (OSV unreachable) and a config carrying `malware_gate = \"strict\"`, doctor printed `[mcp] malware_gate = \"strict\" — ... REFUSES the launch` and the stand-in `npx` still ran (marker file present). Same command on the fix: `✕ probe failed: MCP server launch refused: 'npx' fetches and executes a package ... malware_gate = \"strict\" refuses a launch whose malware check could not be performed`, marker ABSENT. Negative control on the same binary with `malware_gate = \"permissive\"`: marker PRESENT, so the fix did not turn doctor into a refuse-everything path. Red arm: `git checkout origin/integ/f13 -- crates/wcore-cli/src/doctor/mod.rs` (diff against the branch then empty, so the test ran against byte-identical pre-fix source) failed with `--doctor --probe-mcp executed the package runner although [mcp] malware_gate = \"strict\"`; restored, touched, green."
---

Split out of `#340`. That ticket asked for two things on the OSV fail-open: at
minimum make it VISIBLE, and better, make it an explicit operator choice. The
minimum shipped on `lane/swarm-mcp` — a network, HTTP, timeout or parse failure
now logs at ERROR, the only level that reaches a user with `RUST_LOG` unset. The
operator choice did not, and it is deliberately not a log-level change: it is a
config surface with a docs and `/doctor` face and its own review.

The default must stay `permissive`. Refusing every MCP launch when the machine is
offline is a real product regression for anyone working on a plane, so this knob
is only worth adding with the mode plumbed all the way through.

Note the related standing decision Q3 in `.planning/DECISIONS.md`: the visibility
question was answered "a typed protocol frame, not a log level", and that frame
lands with Q4 (`FerroxLabs/wayland#1099`), not here.

## Lane note — `lane/f13-mcp-gate-mode` (2026-08-29)

All six criteria closed. The shape of the fix:

`MalwareCheckOutcome::Allowed` was doing two jobs — "queried and cleared" and
"could not query, proceeding anyway" — and that conflation is *why* c3's two
paths diverged by accident and *why* c2 had no strict half to write. They did
not diverge because someone chose differently in two places; they diverged
because both places spelled themselves `Allowed` and there was only ever one
direction out of that word. Splitting `Unavailable(String)` off is what makes
the mode expressible at all, and it collapses c2 and c3 into a single arm in
`malware_gate::decide` rather than two arms that must be kept in step.

The default is `permissive` and stays there. Nothing changes for an existing
user who never writes the key.

### Red arm (c2), run verbatim

Mutated `decide` so `Strict` returned `Ok(())` on `Unavailable` — i.e. the
pre-change behaviour with the config key still present, which is the exact
regression this criterion is supposed to catch:

```
$ cargo test -p wcore-mcp --test mcp_launch_malware_gate strict_refuses
running 1 test
test strict_refuses_an_unchecked_package_before_exec ... FAILED

thread 'strict_refuses_an_unchecked_package_before_exec' panicked at
crates/wcore-mcp/tests/mcp_launch_malware_gate.rs:320:9:
strict must refuse a launch whose malware check could not be performed; got Ok("spawned")

test result: FAILED. 0 passed; 1 failed; 14 filtered out

$ cargo test -p wcore-mcp --lib malware_gate
test malware_gate::tests::the_ssrf_short_circuit_follows_the_same_mode_as_a_backend_error ... FAILED
test malware_gate::tests::the_strict_refusal_names_the_key_and_the_way_out ... FAILED

thread '...::the_ssrf_short_circuit_follows_the_same_mode_as_a_backend_error' panicked at
crates/wcore-mcp/src/malware_gate.rs:389:13:
assertion `left == right` failed: Strict: wrong verdict for an unperformable check
  left: false
 right: true

test result: FAILED. 2 passed; 2 failed; 139 filtered out
```

### Red arm (c3), run verbatim

Second, independent mutation: returned `Allowed` from the SSRF short-circuit
in `osv_check` while leaving the backend-error path on `Unavailable` — i.e.
exactly the divergence c3 says must not exist:

```
$ cargo test -p wcore-mcp --lib the_ssrf_short_circuit
test malware_gate::tests::the_ssrf_short_circuit_follows_the_same_mode_as_a_backend_error ... FAILED

thread '...' panicked at crates/wcore-mcp/src/malware_gate.rs:370:9:
the SSRF short-circuit still reports a clean check: Allowed

test result: FAILED. 0 passed; 1 failed; 142 filtered out
```

Both mutations restored with `git checkout --` and `touch`ed afterwards, so
cargo could not serve a stale mutated binary. Post-restore:
`cargo test -p wcore-mcp --lib malware_gate` → 4 passed; 0 failed, and
`cargo test -p wcore-mcp --test mcp_launch_malware_gate` → 15 passed; 0 failed.

### Scope boundary on c6, stated rather than glossed

`/doctor` here means `wayland --doctor` (`wcore-cli/src/doctor/mod.rs`), the
canonical surface, and that is where the line is graded. The TUI has a SECOND
posture table — `tui::surfaces::diagnostics::scan_config_health`, which already
renders "valid but permissive" states as `Warn` (plaintext credential store,
egress guard off) — and the malware gate belongs there too. It was left alone
deliberately: the row needs a new `ConfigView` field plumbed through
`tui/mod.rs`, `tui/app.rs`, `surfaces/mod.rs` and `surfaces/config.rs`, which
is a settings-surface change rather than a doctor change. Follow-up, not a gap
in this criterion.

### Known limitation, not hidden

The mode is installed from `AgentBootstrap::build`, the one seam every agent
session passes through before an MCP server is connected. A future direct
caller of `StdioTransport::spawn` that never builds an agent would read the
uninstalled default (`permissive`) — that is the status-quo behaviour, not a
new fail-open, but it is a real ceiling on `strict` and should be revisited if
a non-session MCP launch path ever appears.

### Gate, run on hetzner at HEAD

`cargo fmt --all && git diff --exit-code` clean; `clippy --workspace
--all-targets --all-features -- -D warnings` exit 0; `check --workspace
--all-targets --all-features --locked` exit 0; `nextest run --workspace
--profile ci --no-fail-fast` 17,099 tests with ONE persistent failure, the
known `wcore-exec-backend::conformance_matrix::every_reference_backend_passes_
the_same_harness_or_reports_why_it_did_not`, plus one retry-flake
(`wcore-cli::deterministic_openai_loop::packaged_f04_run_is_repeatable_and_
content_addressed`, TRY 1 FAIL / TRY 2 PASS, a content digest diverging under
load).

An EARLIER run of the same gate reported 33 further failures. Every one of them
carried the same text — `DispatchAdmission("dispatch requires 9126805504 bytes
... but only 0 bytes are available")` — and `df` showed the shared box at 100%
with 0 bytes available to the dispatch gate. They are recorded here rather than
dropped: they were re-run once ~55 GiB came free and all 33 passed, which is
what makes "environmental" a measurement instead of an assumption.

## Lane note — `lane/f13-n-mcp-gate` (2026-08-29)

c7 closed. The `Known limitation, not hidden` section above is now history: the
mode is installed from `AgentBootstrap::build` **and** from the `--doctor` MCP
section, and doctor reports the mode it read back from the gate rather than the
one it read out of the config. The regression test owns its own process
(`crates/wcore-cli/tests/doctor_probe_malware_gate.rs`, one test in one file)
because the posture is a process-wide `OnceLock` and the test sets
`WAYLAND_HOME`; it carries its own control — a permissive-default launch that
MUST reach exec — so a refusal cannot be confused with a broken fixture.

Every criterion on this entry is now met. Closing #354 is Sean's action, not
the lane's.
