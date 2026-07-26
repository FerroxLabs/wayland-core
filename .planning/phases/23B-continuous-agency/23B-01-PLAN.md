---
phase: 23B-continuous-agency
plan: "01"
type: execute
wave: 1
depends_on: []
files_modified:
  - crates/wcore-agent/src/session_lifecycle.rs
  - crates/wcore-agent/src/lib.rs
  - crates/wcore-cli/src/session_cmd.rs
  - crates/wcore-cli/src/lib.rs
  - crates/wcore-cli/src/main.rs
  - crates/wcore-cli/src/tui/checkpoint.rs
  - crates/wcore-cli/src/tui/commands/mod.rs
  - crates/wcore-cli/tests/session_operator_lifecycle.rs
  - scripts/f23-macos-binary.sh
  - scripts/f23-session-operator-drive.sh
  - scripts/f23-session-operator-drive.ps1
  - .planning/phases/23B-continuous-agency/evidence/
  - .planning/phases/23B-continuous-agency/23B-01-LIVE-EVIDENCE.md
autonomous: true
requirements:
  - F23-02
domain: code
must_haves:
  truths:
    - "SUCCESS CRITERION 2 IS A LIST OF VERBS A HUMAN PERFORMS, NOT A LIST OF FUNCTIONS THAT EXIST. Search, inspect, checkpoint, retry, fork, rewind, export, retain and reconcile are things a user DOES to a session. A Rust function that implements each one, covered by a unit test, does not close this criterion. The criterion closes when the shipped `wayland-core` binary performs each verb from a real command line and the result is observable in captured output and on disk. Phase 20A drove Windows and macOS to green in CI and nobody ever launched the binary; that is the exact failure this plan exists to not repeat."
    - "THE PRODUCT ALREADY HAS PART OF THIS AND THE PLAN MUST BUILD ON IT, NOT AROUND IT. `crates/wcore-cli/src/tui/checkpoint.rs` is a real workspace checkpoint store (meta.json plus opaque numbered blobs, absent files restored back to absence) written specifically as the engine behind `/rewind`. `crates/wcore-agent/src/session.rs` owns SessionManager with create/save/load/list/merge_wal and a SessionMeta index. `crates/wcore-agent/src/session_journal/` owns the durable journal, its reducer and its snapshots. `wcore-protocol`'s `RecoveryReconcileReason` already enumerates the eight unknown-effect states (ApprovalExpired, ProviderOutcomeUnknown, ToolOutcomeUnknown, EffectRequiresOperator, BudgetExhausted, ContextUnrestorable, CancellationAmbiguous, UnknownCriticalState) and is CI-enforced against the Desktop contract corpus. Reconcile means SURFACING those existing states to an operator and letting them resolve one, not inventing a ninth."
    - "EXPORT IS A SECRET-DISCLOSURE SURFACE AND IS THE HIGHEST-SEVERITY THING IN THIS PLAN. A session transcript contains prompts, tool arguments, tool output, file contents and provider payloads. F23-02 says the export is REDACTED and F26-03 later consumes this exact envelope for portable migration. A run-time-generated nonce planted in the session must be provably absent from the exported artifact. Redaction that is asserted rather than measured is not redaction."
    - "REWIND AND FORK MUST NOT REACH OUTSIDE THE WORKSPACE THE SESSION WAS AUTHORIZED FOR. The checkpoint store records absolute destination paths in meta.json and writes those bytes back on restore. A meta.json whose recorded path escapes the session's workspace root turns rewind into an arbitrary-file-write primitive. The restore path must refuse a destination outside the authority the session actually held, and the refusal must be proved with a hostile fixture, not assumed from the fact that the store writes opaque numbered blobs."
    - "RETRY MUST NOT REPLAY AN EXPIRED APPROVAL. Re-running a turn that originally contained an approved tool call must re-derive approval under the CURRENT session authority, never inherit the recorded one. `ApprovalExpired` is already a reconcile reason precisely because approval is time-bound. A retry that silently reuses a stale approval is an authority-amplification defect at HIGH severity and blocks this plan."
    - "A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, add an ignore or allow attribute, raise a timeout, re-gate, or delete an inconvenient test to reach a gate. If a verb cannot be closed honestly inside this plan, record it as an OPEN clause with its evidence and its reason and stop. Phase 20A closed with four requirements explicitly open and that was the correct outcome."
    - "A GATE THAT CANNOT GO RED IS WORSE THAN NO GATE, AND THIS PLAN ALREADY SHIPPED THREE OF THEM. The previous revision closed every Windows leg with `ssh host '...' | grep -v CLIXML | grep -v '^<Objs'`. A pipeline's exit status is the LAST command's, so that reported grep's status, not ssh's: any surviving output line greened the gate even when the remote build failed, and grep's exit 1 on empty output meant it reddened on silent success. It could not detect failure. The same class has two further instances closed here — reading an exit code from a PowerShell block that also emits output, and letting `cargo clippy` pass on a host tree that does not contain the module this plan creates. For every command written into a `<verify>` block, answer 'what makes this go red?' before writing it. If the honest answer is 'nothing' or 'only if output is empty', it is not a verification."
    - "THE macOS LEG HAD NO BINARY AND NO EXECUTABLE STEP, AND THE ARTIFACT IT NAMED DOES NOT EXIST. The previous revision drove macOS against 'a PREBUILT wayland-core artifact obtained from the macOS CI job'. Measured against `.github/workflows/`: `ci.yml` uploads only `nextest-junit-${{ matrix.os }}` JUnit XML and no binary of any kind, and `release.yml` builds Darwin binaries only on a `v*-wayland-*` tag push or an explicit dispatch — both Sean-only, as is pushing. No such artifact is reachable from inside this phase. Worse, not one `<automated>` command executed anything on macOS at all: the macOS rows were closed by grepping an evidence file the executor itself wrote, which is a tautology and not proof. The macOS leg now builds its own binary on this Mac and runs the real driver locally, and every leg's binary must prove its own provenance through `--build-info`."
  artifacts:
    - path: crates/wcore-agent/src/session_lifecycle.rs
      provides: "The operator-verb primitives over the existing SessionManager and session journal: full-text session search, lineage and fork, retry-from-turn under re-derived authority, retention state, the redacted export envelope, and the unknown-effect reconcile projection built from RecoveryReconcileReason"
    - path: crates/wcore-cli/src/session_cmd.rs
      provides: "The `wayland-core session` subcommand — the actual operator surface for every verb, with stable stdout tokens and distinct exit codes so a script can observe the result"
    - path: crates/wcore-cli/tests/session_operator_lifecycle.rs
      provides: "Integration coverage driving the real binary's session surface and the real TUI over a PTY, including the hostile path-escape and stale-approval cases"
    - path: scripts/f23-macos-binary.sh
      provides: "The phase's shared macOS binary resolver: builds `wayland-core` on this Mac into `target/f23-macos` (or accepts an externally built binary through `WAYLAND_F23_MACOS_BIN`), asserts the binary's `--build-info` source SHA equals the commit under test, prints the absolute path on stdout, and exits non-zero with a named reason rather than ever skipping. Owned by this plan; 23B-02, 23B-03 and 23B-04 consume it unchanged."
    - path: scripts/f23-session-operator-drive.sh
      provides: "The three-platform live driver that exercises every verb against the shipped binary and writes one captured transcript per verb, ending in one nonce-bound terminal marker that only a fully passing run emits"
    - path: .planning/phases/23B-continuous-agency/23B-01-LIVE-EVIDENCE.md
      provides: "The recorded live outcome per verb per platform, with the exact invocation, the observed stdout token, the exit code and the on-disk consequence"
  key_links:
    - from: crates/wcore-cli/src/session_cmd.rs
      to: crates/wcore-agent/src/session_lifecycle.rs
      via: "the CLI subcommand dispatch — every verb the user types reaches exactly one primitive"
      pattern: "cli-to-engine"
    - from: crates/wcore-cli/src/tui/commands/mod.rs
      to: crates/wcore-cli/src/tui/checkpoint.rs
      via: "the /rewind and /checkpoint handlers driving the existing workspace checkpoint store"
      pattern: "tui-to-store"
    - from: scripts/f23-session-operator-drive.sh
      to: .planning/phases/23B-continuous-agency/23B-01-LIVE-EVIDENCE.md
      via: "captured per-verb transcripts promoted into the recorded live outcome"
      pattern: "live-evidence"
---

<objective>
Make Success Criterion 2 true through the shipped product: a user can search, inspect, checkpoint, retry, fork, rewind, export, retain and reconcile session effects by running the real `wayland-core` binary, and each verb's result is observable in captured output and on disk on Linux, macOS and Windows.

Purpose: F23-02 is the operator-recovery half of governed continuous agency. Everything downstream depends on it — F23-05's multi-day journey resumes through these verbs, and F26-03 consumes this plan's redacted export envelope for portable migration. The engine already owns the durable pieces (SessionManager, the session journal and its reducer, the workspace checkpoint store behind `/rewind`, and the eight `RecoveryReconcileReason` states). What is missing is the operator surface over them and the proof that a human can actually drive it.
Output: One `wayland-core session` subcommand covering every verb; `/checkpoint`, `/fork` and `/export` wired into the real TUI alongside the existing `/rewind`; integration coverage including the hostile path-escape and stale-approval cases; and one captured live transcript per verb per platform recorded in `23B-01-LIVE-EVIDENCE.md`.
</objective>

<execution_context>
@$HOME/.codex/gsd-core/workflows/execute-plan.md
@$HOME/.codex/gsd-core/templates/summary.md
</execution_context>

<context>
@AGENTS.md
@.planning/HANDOFF-2026-07-26-phase20-20A-complete.md
@crates/wcore-agent/src/session.rs
@crates/wcore-cli/src/tui/checkpoint.rs
@crates/wcore-cli/tests/support/pty.rs
@crates/wcore-cli/tests/support/mock_llm.rs
@crates/wcore-cli/tests/harness_cli_surface.rs
</context>

<execution_rules>

**THE TWO AMENDED PHASE RULES — verbatim, and they bound this plan.**

- Findings at CRITICAL or HIGH must be fixed or disproved. MEDIUM and below are logged to BACKLOG and DO NOT BLOCK execution.
- Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean.

**TERMINATION CRITERION FOR THIS PLAN (hard — the plan STOPS rather than spawning more work).** This plan implements nine verbs once and proves them once per platform. It terminates in exactly one of three states, and in all three it writes its SUMMARY and stops:
1. **Complete** — every verb runs against the shipped binary on Linux, macOS and Windows with its observable outcome captured, and both hostile cases (path escape, stale approval) are proved closed.
2. **Complete with named open verbs** — one or more verbs could not be closed honestly. Record each as OPEN in `23B-01-LIVE-EVIDENCE.md` with the exact blocking evidence and the reason, mark F23-02 incomplete, and stop. This is a successful outcome, not a failure.
3. **Escalated** — a CRITICAL or HIGH finding requires a change outside this plan's declared files. Record it with severity and stop.
Under no circumstances does this plan create additional plans or extend its own task list.

**SCOPE BOUNDARY (hard).** Memory and user-model control belong to 23B-02. The repository index belongs to 23B-03. The multi-day journey and the phase's terminal acceptance belong to 23B-04. Success Criterion 1 (governed skill promotion) belongs to Phase 23A and its contract is an ADMITTED INPUT here — do not re-derive, re-verify or modify it. If work leads toward any of those surfaces, record the connection and stop.

**THIS PLAN OWNS THE `main.rs` DISPATCH SEAM FOR THE PHASE.** `crates/wcore-cli/src/main.rs` is a single 320 KB file that every CLI-surface plan in this phase would otherwise edit concurrently; the same is true of `crates/wcore-cli/src/tui/commands/mod.rs`. That is why 23B-01, 23B-02 and 23B-03 are consecutive waves rather than one parallel wave — the wave numbers express a real file seam, not an invented dependency. Do not "optimize" them into one wave.

**NON-NEGOTIABLE.** A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. The specific temptation in this plan is to make export "redacted" by filtering a known-shaped token rather than by proving a run-time nonce is absent, and to make reconcile "work" by printing a state rather than by letting an operator resolve one. Both are engineered greens and both are forbidden.

**ENVIRONMENT.**
- Linux (authoritative Cargo proof): `ssh -o BatchMode=yes hetzner-dsm`, `/root/wayland`. Full workspace aggregate runs ~194s on this host.
- Windows (native live): `ssh -o BatchMode=yes SeanD@seandesktop` over Tailscale, checkout `C:\ferrox-win`, cargo at `C:\Users\seand\.cargo\bin\cargo.exe`. The remote default shell is PowerShell, so an `ssh` command string is PowerShell source and must end with an explicit `exit $LASTEXITCODE` for the status to propagate. `cargo fmt --all` FAILS there with os error 206; `justfile:96-98` already skips it on Windows. Windows clippy runs with warnings denied BEFORE tests, so any lint failure means tests never execute.
- macOS (native live): THIS Mac. See the macOS binary decision below. `cargo fmt --all -- --check` is the local formatting gate.
- ALWAYS `/usr/bin/grep` on the Mac (the ambient grep is proxied and silently drops lines — measured 32 returned versus 674 on one file) and `-F` for literals. Same caution for `ls`.

**GATE DISCIPLINE — every command in a `<verify>` block must be able to go RED. Three hard rules, each closing a defect this plan actually shipped.**

1. **A gate is NEVER a pipeline into a filter.** `ssh host 'cmd' | grep -v CLIXML | grep -v "^<Objs"` reports GREP's exit status, not ssh's. Any surviving output line greens it even when the remote build failed, and grep's exit 1 on EMPTY output means it reddens on silent success. Every Windows gate in the previous revision had exactly that shape and could not detect failure. The correct form redirects, captures the status on the NEXT line, asserts on it, and only then reads the log:
   `ssh -o BatchMode=yes HOST "…; exit \$LASTEXITCODE" > LOG 2>&1; rc=$?; test "$rc" -eq 0 && /usr/bin/grep -qF "MARKER" LOG`
   Filtering CLIXML noise while READING a log for a human is fine; it is fatal only when the pipeline IS the gate.
2. **Never read an exit code from a block that also emits output.** In PowerShell, `$x = & { cargo … | Tee-Object …; $LASTEXITCODE }` returns an ARRAY of every output line plus the code, so `if ($x -ne 0)` is an always-truthy array filter. That exact bug made an all-PASS 12/12 + 6/6 Windows soak report failure; the fix and its post-mortem are in `scripts/wayland-e2e-windows-soak.ps1:177-185` and `:247-251`. Read `$LASTEXITCODE` on the line AFTER the pipeline. The PowerShell driver this plan writes obeys that rule.
3. **Never let a gate pass on a tree that does not contain the work.** `cargo clippy -p wcore-agent -- -D warnings` on a host synced to the last PUSHED tip is clean and proves nothing about a module that does not exist there yet — a false green of the same family. Every remote gate therefore pins the exact commit under test and asserts a file THIS PLAN CREATES is present, in the same `&&` chain, before the compiler runs: take `SHA=$(/usr/bin/git rev-parse HEAD)` locally, `git checkout -q --detach $SHA` on the host, then `test -f <file this plan creates> && cargo …`. If the host cannot resolve that SHA the gate fails loudly with git's own error, which is the correct outcome. **Commit this task's declared files and get the working branch onto `gh` BEFORE running a remote gate.** Do not respond to a missing SHA by dropping the assertion.

**macOS BINARY SOURCE — DECIDED HERE, WITH ITS BASIS AND ITS MEASUREMENTS.** The previous revision drove the macOS leg against "a PREBUILT `wayland-core` artifact obtained from the macOS CI job". That artifact does not exist and cannot be produced from inside this phase. Measured, not assumed: `.github/workflows/ci.yml:204-208` uploads only `nextest-junit-${{ matrix.os }}` — JUnit XML, no binary of any kind, on any branch; `.github/workflows/release.yml:1-24` fires only on a `v*-wayland-*` tag push, a `workflow_call`, or an explicit `workflow_dispatch`, and its Darwin targets at `:70-74` therefore never build for `plan/f20-unified-audit-repair`. Tagging, releasing, dispatching and pushing are all Sean-only, so no CI run producing a macOS binary can be triggered from inside plan execution. **Decision: the macOS leg builds its own binary on this Mac, through `scripts/f23-macos-binary.sh`.** Basis: HANDOFF §3 item 7 — "This Mac CAN compile the workspace. The old 'never compiles on Mac' note is a workflow convention, not a fact" — plus the pinned toolchain `1.95.0-aarch64-apple-darwin` present under `~/.rustup/toolchains` and matching `rust-toolchain.toml`. **The convention's real purpose is preserved exactly: `hetzner-dsm` stays the sole authority for clippy, nextest and the aggregate proof. The Mac build produces a DRIVE TARGET, never a proof verdict, and is isolated in `--target-dir target/f23-macos`, which the existing `/target/` ignore rule already covers, so it disturbs no other state.** `WAYLAND_F23_MACOS_BIN` overrides with a binary built elsewhere; either way the resolver asserts the binary's own `--build-info` source SHA equals the commit under test, so a stale artifact reddens instead of silently proving the wrong code. If the Mac build fails, that is a RED to record: the macOS rows go OPEN with the compiler's exact error under this plan's termination state 2. It is never a silent skip.
- Both hosts' fetch refspecs are pinned to an unrelated branch: always `git fetch origin plan/f20-unified-audit-repair` explicitly, never `git fetch --all`. In the Mac repo `origin` is a STALE LOCAL WORKTREE; the real remote is `gh`.
- In `cmd`, the unquoted `set` form appends a trailing space to the value and Rust silently ignores it. Use the quoted form or the PowerShell environment form and PROVE the value took effect before trusting any run that depends on it.
- NO push to main, merge, PR, tag, release, deployment or issue closure. Those are Sean-only.

**AGENTS.md discipline.** Surgical diffs; every changed line traces to a verb in F23-02. No drive-by refactor of `session.rs`, the journal reducer, or the checkpoint store's on-disk layout. Public API errors use thiserror; internal propagation uses anyhow. All process spawning goes through `wcore_config::shell`, argv mode for anything carrying session-derived data. Clippy-clean with warnings denied. Keep new modules under 1000 lines.

**Git hygiene.** Use `/usr/bin/git` on the Mac. Stage the exact paths in `files_modified`, never `-A`, never `.`. Never stage `AGENTS.md` or `.ijfw` churn. No `Co-Authored-By` trailers.
</execution_rules>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: Build the operator-verb primitives over the existing session substrate</name>
  <files>crates/wcore-agent/src/session_lifecycle.rs, crates/wcore-agent/src/lib.rs</files>
  <read_first>crates/wcore-agent/src/session.rs (SessionManager: create, create_for_run, persist_first_message, save, save_and_clear_wal, append_wal, merge_wal, load, load_for_run, list, update_index_for; and the SessionMeta index shape — id, created_at, updated_at, model, summary, message_count), crates/wcore-agent/src/session_journal/model.rs (the journal envelope and BudgetWallClockAuthority), crates/wcore-agent/src/session_journal/reducer.rs (how recorded events fold into state — read the entry points only, it is 164 KB), crates/wcore-protocol/src/events.rs around the RecoveryReconcileReason enum and RecoveryTurnSnapshot (note the deliberate rule that the snapshot carries opaque identifiers and typed state, never transcript text, prompts, tool arguments or output, paths, approval secrets or provider payloads — the export envelope must honour the same rule), crates/wcore-agent/src/output_redaction.rs (the redaction primitives that already exist — reuse them, do not write a second redactor), crates/wcore-agent/src/recovery.rs (how reconcile reasons are currently produced and what resolving one means today)</read_first>
  <behavior>
    - Search over persisted sessions returns the ids of sessions whose recorded content matches a query term, returns an empty result and a success status for a term that matches nothing, and never returns a session the caller's profile does not own.
    - Inspect returns, for one session id, its metadata, its turn count, its lineage parent if it was forked, its retention state, and its outstanding reconcile items.
    - Fork produces a new session id whose lineage parent is the source, whose transcript up to the fork point is present, and whose creation leaves the source session byte-identical on disk.
    - Retry re-runs one identified turn producing a new turn, retains the original turn rather than overwriting it, and re-derives tool approval under the current session authority. A turn whose original approval has expired is refused with the ApprovalExpired disposition rather than replayed.
    - The export envelope round-trips through serde, carries provenance (source session id, exporting build identity, export timestamp), and contains no value planted into the session as a run-time nonce.
    - Retention records an explicit retain-until state per session, and a session past its retain-until is reported as expired rather than silently deleted.
    - Reconcile projects the outstanding unknown-effect items for a session from the existing RecoveryReconcileReason states, and resolving one records the operator's disposition durably so the same item is not presented twice.
    - Every primitive is total over a corrupt or truncated on-disk session: it returns a structured error naming the file, never panics, and never fabricates an empty session.
  </behavior>
  <action>Create `crates/wcore-agent/src/session_lifecycle.rs` as a new module and declare it in `crates/wcore-agent/src/lib.rs`. Do not grow `session.rs` (75 KB) or the reducer (164 KB); this module composes them.

Write the tests first, one per bullet in the behavior block, against a temporary session directory built with the existing SessionManager so the fixtures are real persisted sessions rather than hand-authored structs. Confirm they fail for the right reason before implementing.

Implement the verbs as primitives that take a SessionManager and a session id and return structured results. Search reads the persisted transcript through SessionManager rather than maintaining a second index, so it cannot drift from what is actually stored; if that proves too slow on a large session directory, record the measurement and the chosen remedy in the SUMMARY rather than silently adding a cache. Fork copies the source session's persisted state to a new id and records the parent in the session metadata; the source must be re-read after the fork and compared byte for byte to prove it was untouched. Retry re-derives approval from the live session authority and refuses when the recorded approval is no longer valid — the refusal carries the existing ApprovalExpired disposition rather than a new error kind, per F23-02's requirement that unknown-effect reconciliation reuses the established states.

For export, reuse the redaction primitives already in `output_redaction.rs`. Do not author a second redactor and do not implement redaction as a pattern match against secret-shaped tokens: the acceptance is that a nonce generated at run time and planted into the session is absent from the exported bytes, which a shape-matching filter cannot satisfy. Carry provenance in the envelope because F26-03 will consume it.

For reconcile, project the outstanding items from the states `wcore-protocol` already defines. Resolving an item writes the operator's disposition through the existing journal so a restart does not re-present it. Do not add a ninth reason variant — `wcore-protocol`'s enum is part of the Desktop contract corpus that CI checks byte for byte, and widening it here would drift that corpus and is out of this plan's scope.

Errors are thiserror-structured and name the offending path. Addresses F23-02; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch -q origin plan/f20-unified-audit-repair &amp;&amp; git checkout -q --detach $SHA &amp;&amp; test -f crates/wcore-agent/src/session_lifecycle.rs &amp;&amp; cargo clippy -p wcore-agent --all-targets -- -D warnings"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo nextest run -p wcore-agent --profile ci -E 'test(session_lifecycle)' --no-tests=fail --no-fail-fast"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -v '^//' crates/wcore-agent/src/session_lifecycle.rs | /usr/bin/grep -cF 'panic!')" -eq 0</automated>
  </verify>
  <done>`session_lifecycle.rs` exists, is declared in `lib.rs`, is clippy-clean with warnings denied on Linux, and its tests pass on `hetzner-dsm`. Every behavior bullet has at least one test that failed before the implementation landed. `wcore-protocol`'s RecoveryReconcileReason enum is unmodified. `session.rs` and the journal reducer are unmodified except where a verb genuinely required it, and any such change is named in the SUMMARY.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 2: Ship the operator surface — the `session` subcommand and the TUI verbs</name>
  <files>crates/wcore-cli/src/session_cmd.rs, crates/wcore-cli/src/lib.rs, crates/wcore-cli/src/main.rs, crates/wcore-cli/src/tui/checkpoint.rs, crates/wcore-cli/src/tui/commands/mod.rs, crates/wcore-cli/tests/session_operator_lifecycle.rs</files>
  <read_first>crates/wcore-cli/src/main.rs (the TopCmd subcommand enum and its dispatch arm — note the established pattern that each subcommand is one variant plus one module such as swarm, cron, profile, auth, migrate; and note that the existing session flags resume, continue, session-id and list-sessions already exist on the root command and that list-sessions currently prints to STDERR not stdout), crates/wcore-cli/src/swarm.rs (the shape of an existing subcommand module — clap Args struct plus a run entry point), crates/wcore-cli/src/tui/checkpoint.rs (the CheckpointStore capture and restore API, the meta.json layout, and the stated contract that an absent file is restored back to absence), crates/wcore-cli/src/tui/commands/mod.rs (the command registry, its category enum, and the existing /rewind and /compact entries), crates/wcore-cli/tests/harness_cli_surface.rs (the Layer 1 pattern that drives the compiled binary as a subprocess and asserts on its output), crates/wcore-cli/tests/support/pty.rs (Pty spawn, wait_for, send, screen_text, quit — the Layer 2 pattern that drives the real TUI), crates/wcore-cli/tests/support/mock_llm.rs (MockLlm builder plus RecordedRequest and received_requests, which let a test read the actual outbound request body)</read_first>
  <behavior>
    - Every verb is reachable as a subcommand of the shipped binary and prints a stable, greppable token plus the identifier it acted on to STDOUT, so a script can observe the outcome without parsing prose.
    - Exit codes are distinct and documented: success, not-found, refused-by-authority, and outstanding-reconcile-items each map to a different non-overlapping code.
    - Listing and searching sessions work without a provider API key, matching the existing behavior that a first-run user can see empty session history.
    - Taking a checkpoint through the TUI and taking one through the subcommand produce entries in the same store, and either can be restored by the other.
    - Restoring a checkpoint whose recorded destination path escapes the session's workspace root is REFUSED with a structured error and writes nothing, proved by a hostile fixture whose meta.json points outside the root.
    - Driving /rewind through the real TUI over a PTY restores the file bytes and the rendered screen names the restored file.
    - The TUI gains /checkpoint, /fork and /export entries in the command registry, each with the same one-line help idiom as the existing entries, and each reaching the same primitive the subcommand reaches.
    - A corrupt session file makes the subcommand exit non-zero with a message naming the file, and never leaves the store partially mutated.
  </behavior>
  <action>Add one `Session` variant to the TopCmd enum in `main.rs` with one dispatch arm delegating to a new `crates/wcore-cli/src/session_cmd.rs`, re-exported from `crates/wcore-cli/src/lib.rs`. Follow the existing subcommand module pattern exactly. This is the ONLY edit this phase makes to `main.rs`; 23B-02 and 23B-03 own different surfaces.

Give the subcommand one operation per verb in F23-02: list, search, show, checkpoint, retry, fork, rewind, export, retain and reconcile. Each prints a machine-observable token and the identifier it acted on to STDOUT. Note that the existing root list-sessions flag prints to stderr; the new subcommand prints to stdout so a driver script can capture it with a plain redirect, and the SUMMARY records that deliberate divergence. Define the exit-code map in the module's head documentation and assert it in tests.

Wire the store: the subcommand's checkpoint and rewind operations use the SAME CheckpointStore that `/rewind` already uses, so a checkpoint taken in the TUI is restorable from the shell and the reverse. Add the workspace-root containment check to the store's restore path — a recorded destination outside the root is refused with a structured error and nothing is written. Prove it with a hostile fixture whose metadata names a path outside the root; the fixture is authored by hand because no legitimate capture produces one.

Add /checkpoint, /fork and /export to the TUI command registry beside the existing /rewind entry, matching its category and help idiom.

Write `crates/wcore-cli/tests/session_operator_lifecycle.rs` in two layers, mirroring the existing harness split. Layer 1 spawns the compiled binary as a subprocess and asserts each verb's stdout token and exit code, and runs on every platform. Layer 2 drives the real TUI over a PTY using the existing support helper and asserts on the vt100 screen text; it carries the same unix-only gate the existing TUI harness carries, and Task 3 measures whether that gate can be lifted on real Windows hardware. Use the mock provider server so no network call or API key is needed; assert the outbound request body through the recorded-request helper wherever a test needs to prove what actually reached the provider.

Addresses F23-02; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch -q origin plan/f20-unified-audit-repair &amp;&amp; git checkout -q --detach $SHA &amp;&amp; test -f crates/wcore-cli/src/session_cmd.rs &amp;&amp; test -f crates/wcore-cli/tests/session_operator_lifecycle.rs &amp;&amp; cargo clippy --workspace --all-targets -- -D warnings"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo nextest run -p wcore-cli --profile ci --test session_operator_lifecycle --no-tests=fail --no-fail-fast"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo nextest run -p wcore-cli --profile ci --test harness_cli_surface --test harness_tui_flow --no-tests=fail --no-fail-fast"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo build --release -p wcore-cli --bin wayland-core &amp;&amp; ./target/release/wayland-core --build-info &amp;&amp; ./target/release/wayland-core session --help"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -v '^ *//' crates/wcore-cli/src/session_cmd.rs | /usr/bin/grep -cEi 'todo!|unimplemented!')" -eq 0</automated>
  </verify>
  <done>The `session` subcommand exists on the shipped binary with one operation per F23-02 verb, a documented and asserted exit-code map, and stdout tokens a script can grep. The TUI registry carries /checkpoint, /fork and /export beside /rewind. The workspace-root containment refusal is proved by a hostile fixture. The pre-existing `harness_cli_surface` and `harness_tui_flow` suites still pass unchanged — no existing test was modified to accommodate this work. Clippy is clean with warnings denied across the workspace.</done>
</task>

<task type="auto">
  <name>Task 3: LIVE — drive every verb through the shipped binary on Linux, macOS and Windows and capture the evidence</name>
  <files>scripts/f23-macos-binary.sh, scripts/f23-session-operator-drive.sh, scripts/f23-session-operator-drive.ps1, .planning/phases/23B-continuous-agency/evidence/, .planning/phases/23B-continuous-agency/23B-01-LIVE-EVIDENCE.md</files>
  <read_first>scripts/f20-native-macos-proof.sh and scripts/f20-native-windows-proof.ps1 (the established shape of a native proof driver in this repo — argument handling, marker emission, cleanup traps; note the recorded follow-up that the macOS script pulls a container image unconditionally and should inspect first, and do not repeat that mistake), scripts/wayland-e2e-windows-soak.ps1 lines 174-190 and 244-255 (the WORKED EXAMPLE of PowerShell exit-code capture: `cargo @args 2>&amp;1 | Tee-Object -FilePath $log` on one line and `$suiteExit = $LASTEXITCODE` on the NEXT, with the in-file post-mortem explaining why the `$x = &amp; { … ; $LASTEXITCODE }` form returned an always-truthy array and reported a fully passing run as a failure — the PowerShell driver written here copies this discipline exactly), scripts/smoke.sh (how the P0 smoke runner reports hard-gate versus reported checks and why it never silently skips), crates/wcore-cli/build.rs and crates/wcore-cli/tests/build_provenance.rs (the `WAYLAND_SOURCE_SHA` embedding and the exact `wayland-core X.Y.Z (source &lt;sha&gt;)` shape that `--build-info` prints and that `build_provenance.rs` parses by the `(source` token — this is the binary-provenance mechanism every leg of this task asserts against), crates/wcore-cli/tests/support/pty.rs write_config and harden_child_env (how a hermetic WAYLAND_HOME plus a minimal config lets the binary boot without a real provider key)</read_first>
  <behavior>
    - `scripts/f23-macos-binary.sh` resolves ONE macOS `wayland-core` binary and prints its absolute path on stdout with every diagnostic on stderr, so a caller can capture the path with a plain command substitution. It refuses to run off Darwin. It uses `WAYLAND_F23_MACOS_BIN` when that variable names an executable, and otherwise builds with `cargo build --release -p wcore-cli --bin wayland-core --target-dir target/f23-macos`. Whichever source it used, it then runs the binary's own `--build-info` and requires the printed source SHA to equal the commit under test; a mismatch exits non-zero naming both SHAs. A missing toolchain, a failed build or a failed provenance check each exit non-zero with a named reason, and none of them is ever reported as a skip.
    - Each drive script takes `--binary <path>`, `--sha <commit>` and `--nonce <hex>`; refuses to run if the binary path is missing or is not executable; and asserts the binary's `--build-info` source SHA equals `--sha` before exercising anything, so a stale binary reddens instead of silently proving old code.
    - Each drive script emits exactly one terminal marker, `F23_01_DRIVE=PASS platform=&lt;linux|macos|windows&gt; nonce=&lt;the nonce it was given&gt;`, and emits it ONLY after every verb passed. Any failure exits non-zero and emits no PASS marker. The nonce is generated by the caller at run time, so a stale log from an earlier run cannot satisfy the caller's check.
    - The driver creates a throw-away WAYLAND_HOME and a throw-away workspace, seeds one real session by running the binary against the local mock provider, and removes both on exit including on failure.
    - The driver plants a run-time-generated nonce into the seeded session and asserts that nonce is absent from the exported artifact.
    - Every verb is exercised with an exact invocation, and for each the driver records the invocation, the captured stdout, the exit code and the on-disk consequence into a per-verb transcript file.
    - Rewind is proved by byte comparison: the file's bytes after restore equal the bytes captured before the mutation, and a file created after the checkpoint is gone after the restore.
    - Fork is proved by re-reading the parent session after the fork and finding it byte-identical.
    - Reconcile is exercised against a session deliberately interrupted mid-tool-call, and the driver records which reconcile reason was reported and that resolving it once removes it from the outstanding list across a restart of the binary.
    - The driver exits non-zero if any verb's observable outcome is absent, and it never treats a missing outcome as a skip.
    - The same driver logic runs on Linux, macOS and Windows; the PowerShell variant is a port, not a different test, and both emit the same marker vocabulary.
  </behavior>
  <action>First write `scripts/f23-macos-binary.sh`, the phase's shared macOS binary resolver — 23B-02, 23B-03 and 23B-04 consume it unchanged, so get its contract right once. It refuses off Darwin. It prefers `WAYLAND_F23_MACOS_BIN` when that names an executable; otherwise it builds with `cargo build --release -p wcore-cli --bin wayland-core --target-dir target/f23-macos`, isolated there so nothing else in the checkout is disturbed and the existing `/target/` ignore rule already covers it. Read the macOS binary decision in the execution rules before writing it: the CI artifact the previous revision named does not exist, the decision to build here is taken with its basis recorded, and `hetzner-dsm` remains the sole authority for clippy, nextest and the aggregate proof. After resolving the binary the script runs `--build-info` and requires the printed source SHA to equal the commit under test — `crates/wcore-cli/build.rs` embeds `git rev-parse HEAD` into `WAYLAND_SOURCE_SHA` and `main.rs` prints `wayland-core X.Y.Z (source &lt;sha&gt;)`, which `crates/wcore-cli/tests/build_provenance.rs` already parses by the `(source` token. Print the absolute binary path on stdout and every diagnostic on stderr. Exit non-zero with a named reason for a missing toolchain, a failed build or a provenance mismatch; never report any of them as a skip.

Then write `scripts/f23-session-operator-drive.sh` and its PowerShell port. Both take `--binary`, `--sha` and `--nonce`, refuse to proceed without an executable binary, and assert the binary's `--build-info` source SHA equals `--sha` before touching anything. Both end by emitting exactly one terminal marker, `F23_01_DRIVE=PASS platform=&lt;linux|macos|windows&gt; nonce=&lt;the given nonce&gt;`, and only after every verb has passed; any failure exits non-zero and emits no PASS marker. The PowerShell port reads `$LASTEXITCODE` on the line AFTER any pipeline and never as the trailing value of a `&amp; { … }` block, and always ends with an explicit `exit` — copy the discipline and the post-mortem comment from `scripts/wayland-e2e-windows-soak.ps1:174-190`.

Seed one real session by running the binary against the local mock provider in a hermetic home, planting a nonce generated at run time. Then exercise, in order, with an exact invocation each: list, search for a term present in the transcript, search for a term present nowhere, show, checkpoint before a file mutation, mutate the file through a second turn, rewind to the checkpoint, fork, show the fork's lineage, retry a turn, export, retain, and reconcile. Capture stdout, stderr, exit code and the on-disk consequence of each into its own transcript file under a run directory. Prove rewind by byte comparison against the pre-mutation capture and prove that a file created after the checkpoint is absent after restore. Prove fork by re-reading the parent and comparing byte for byte. Prove export by searching the exported bytes for the planted nonce and requiring zero occurrences.

For reconcile, seed a second session that is killed mid-tool-call so the journal genuinely records an unknown effect, then record which reason the product reports, resolve it, restart the binary and confirm the item is not presented again.

Run the driver three times, each against the exact commit under test and each with a nonce the caller generates at run time. On Linux, on `hetzner-dsm`, against a release binary built there after `git checkout -q --detach $SHA`. On Windows, on `SeanDesktop`, through the PowerShell port, after the same detached checkout of `$SHA` on `C:\ferrox-win`; the remote default shell is PowerShell, so the ssh command string ends with an explicit `exit $LASTEXITCODE` and is NEVER piped into a filter. On macOS, on this Mac, against the binary `scripts/f23-macos-binary.sh` resolves — that is a real local invocation of the real product, not an evidence-file grep. Each leg's ssh or local exit status is the primary gate; the nonce-bound terminal marker in the captured log is the second, independent one, and a stale log cannot satisfy it because the nonce is fresh per run.

Then run the TUI leg. On Linux and macOS, drive the real full-screen TUI over a PTY: send the /checkpoint, /rewind, /fork and /export commands as keystrokes, wait for the rendered anchors, and write the captured vt100 screen text into the run directory as the observation record. On Windows, the existing TUI harness is gated unix-only because the container-terminal backend in the headless hosted runner never surfaced the spawned binary's output. SeanDesktop is real hardware with a real console, not that headless runner, so MEASURE whether the backend surfaces output there — single variable, one attempt, recorded either way. If it does, run the same TUI leg on Windows and record it. If it does not, record the measurement and the exact failure, and record the Windows TUI verbs as OBSERVED-VIA-CLI-ONLY rather than claiming a TUI observation that did not happen.

Write `23B-01-LIVE-EVIDENCE.md` as a table: one row per verb per platform, carrying the exact invocation, the observed stdout token, the exit code, the on-disk consequence and a PASS, RED or OPEN verdict. A verb that did not run is OPEN with its reason, never silently absent. Then state F23-02's disposition: complete only if every verb is PASS on all three platforms, otherwise incomplete with the open rows named. Marks F23-02 complete only under that condition; otherwise records the disposition and leaves it incomplete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -x scripts/f23-macos-binary.sh &amp;&amp; test -x scripts/f23-session-operator-drive.sh &amp;&amp; test -f scripts/f23-session-operator-drive.ps1 &amp;&amp; bash -n scripts/f23-macos-binary.sh &amp;&amp; bash -n scripts/f23-session-operator-drive.sh</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; NONCE=$(/usr/bin/openssl rand -hex 8) &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-01-linux-drive.log &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch -q origin plan/f20-unified-audit-repair &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo build --release -p wcore-cli --bin wayland-core &amp;&amp; bash scripts/f23-session-operator-drive.sh --binary target/release/wayland-core --sha $SHA --nonce $NONCE" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0 &amp;&amp; /usr/bin/grep -qF "F23_01_DRIVE=PASS platform=linux nonce=$NONCE" "$L"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; test "$(uname -s)" = Darwin &amp;&amp; NONCE=$(/usr/bin/openssl rand -hex 8) &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; BIN=$(bash scripts/f23-macos-binary.sh) &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-01-macos-drive.log &amp;&amp; bash scripts/f23-session-operator-drive.sh --binary "$BIN" --sha "$SHA" --nonce "$NONCE" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0 &amp;&amp; /usr/bin/grep -qF "F23_01_DRIVE=PASS platform=macos nonce=$NONCE" "$L"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; NONCE=$(/usr/bin/openssl rand -hex 8) &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-01-windows-drive.log &amp;&amp; ssh -o BatchMode=yes SeanD@seandesktop "Set-Location C:\ferrox-win; git fetch -q origin plan/f20-unified-audit-repair; git checkout -q --detach $SHA; if (\$LASTEXITCODE -ne 0) { exit 91 }; cargo build --release -p wcore-cli --bin wayland-core; if (\$LASTEXITCODE -ne 0) { exit 90 }; powershell -NoProfile -ExecutionPolicy Bypass -File scripts\f23-session-operator-drive.ps1 -Binary target\release\wayland-core.exe -Sha $SHA -Nonce $NONCE; exit \$LASTEXITCODE" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0 &amp;&amp; /usr/bin/grep -qF "F23_01_DRIVE=PASS platform=windows nonce=$NONCE" "$L"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; for P in linux macos windows; do N=$(/usr/bin/grep -oE "nonce=[0-9a-f]{16}" ".planning/phases/23B-continuous-agency/evidence/23B-01-$P-drive.log" | tail -1) &amp;&amp; test -n "$N" &amp;&amp; /usr/bin/grep -qF "$N" .planning/phases/23B-continuous-agency/23B-01-LIVE-EVIDENCE.md || exit 1; done</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cE '^\| *(list|search|show|checkpoint|retry|fork|rewind|export|retain|reconcile) ' .planning/phases/23B-continuous-agency/23B-01-LIVE-EVIDENCE.md)" -ge 30</automated>
  </verify>
  <done>All three drive legs ran against the exact commit under test and each exited zero with its own fresh nonce echoed in the terminal PASS marker: Linux over ssh to `hetzner-dsm`, Windows over ssh to `SeanDesktop` with the status carried by an explicit `exit $LASTEXITCODE` and never through a pipeline, and macOS by invoking the real binary locally through `scripts/f23-macos-binary.sh`. Each binary's `--build-info` source SHA equalled the commit under test. `23B-01-LIVE-EVIDENCE.md` carries one row per verb per platform — ten verbs across Linux, macOS and Windows — each with its exact invocation, observed stdout token, exit code, on-disk consequence and verdict, and it carries the three run nonces so the table is tied to the runs that produced it. The rewind row cites a byte comparison, the fork row cites a byte-identical parent, and the export row cites zero occurrences of the run-time nonce. The Windows TUI backend measurement is recorded with its outcome either way, and no verb is claimed as TUI-observed on a platform where the TUI was not actually driven. F23-02 is marked complete only if every row is PASS; otherwise its disposition and the open rows are recorded and the requirement is left incomplete.</done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| operator shell → `session` subcommand | An operator-supplied session id, query string, output path and checkpoint id cross into engine state |
| on-disk session store → engine | Persisted session files, checkpoint `meta.json` and journal records are read back and acted on; a same-UID actor or a corrupted write can shape them |
| session state → export artifact | Transcript, tool arguments and tool output leave the trust boundary as a file the user will move, share or migrate |
| recorded approval → retried turn | A previously granted tool approval is re-presented across time |

## STRIDE Threat Register

| Threat ID | Category | Component | Severity | Disposition | Mitigation Plan |
|-----------|----------|-----------|----------|-------------|-----------------|
| T-23B01-01 | Information Disclosure | `session export` envelope | critical | mitigate | Reuse `output_redaction.rs`; acceptance is the absence of a run-time-generated nonce from the exported bytes, not a shape-matching filter (Task 1 behavior, Task 3 driver) |
| T-23B01-02 | Elevation of Privilege | `CheckpointStore::restore` destination paths | high | mitigate | Workspace-root containment check on restore; hand-authored hostile `meta.json` fixture pointing outside the root proves the refusal writes nothing (Task 2) |
| T-23B01-03 | Elevation of Privilege | `session retry` approval derivation | high | mitigate | Approval is re-derived under current session authority; an expired recorded approval is refused with the existing `ApprovalExpired` disposition rather than replayed (Task 1) |
| T-23B01-04 | Tampering | corrupt or truncated on-disk session file | medium | mitigate | Every primitive is total over corrupt input: structured error naming the file, no panic, no fabricated empty session, no partial store mutation (Task 1, Task 2) |
| T-23B01-05 | Repudiation | reconcile resolution durability | medium | mitigate | Operator disposition is written through the existing journal so a restart does not re-present a resolved item; proved across a binary restart in the live driver (Task 3) |
| T-23B01-06 | Information Disclosure | `session search` across profiles | medium | mitigate | Search is scoped to the caller's session directory as resolved by the active profile; a session outside it is never returned (Task 1) |
| T-23B01-07 | Tampering | operator-supplied query and path arguments reaching a shell | low | mitigate | All process spawning goes through `wcore_config::shell` in argv mode; no session-derived data is format-interpolated into a shell string (AGENTS.md forbidden-patterns rule) |
| T-23B01-SC | Tampering | package-manager installs | low | accept | This plan adds NO new external crate. `rusqlite`, `sha2`, `portable-pty` and `vt100` are already workspace dependencies. If a new crate becomes necessary, that triggers the Package Legitimacy Gate and a blocking human checkpoint before install, and this plan STOPS rather than installing |
</threat_model>

<verification>
- Workspace clippy clean with warnings denied on Linux, and on Windows (where clippy runs before tests, so a lint failure means tests never execute).
- `cargo fmt --all -- --check` clean, run on the Mac (it fails on Windows with os error 206 and `justfile:96-98` already skips it there).
- `cargo nextest run -p wcore-agent -p wcore-cli --profile ci --no-fail-fast` green on `hetzner-dsm` for the new and pre-existing session suites, with `harness_cli_surface` and `harness_tui_flow` unchanged and still passing.
- `scripts/f23-session-operator-drive.sh` exits zero on Linux and macOS and its PowerShell port exits zero on Windows, with one transcript per verb written per run.
- Every remote gate pinned the exact commit under test with `git checkout -q --detach $SHA` and asserted a file this plan creates is present before the compiler ran, so no gate could pass on a tree lacking the work.
- No gate in this plan is a pipeline into a filter, and no exit code is read from a block that also emits output. Each of the three drive legs is closed by its own process exit status first and by a caller-generated nonce echoed in the log second.
- The macOS leg ran a real `wayland-core` binary on this Mac, resolved and provenance-checked by `scripts/f23-macos-binary.sh`; no macOS row is closed by grepping the evidence file alone.
- `23B-01-LIVE-EVIDENCE.md` carries thirty verb-by-platform rows with verdicts, carries the three run nonces, and no verb is claimed on a platform where it was not driven.
</verification>

<success_criteria>
- Success Criterion 2's ten verbs each run against the shipped `wayland-core` binary from a real command line on Linux, macOS and Windows, with a captured observable outcome per verb per platform.
- Export provably omits a nonce planted at run time.
- Rewind restores byte-identical file contents and removes a file created after the checkpoint.
- Fork leaves the parent session byte-identical.
- Retry refuses an expired recorded approval rather than replaying it.
- Reconcile surfaces an existing `RecoveryReconcileReason` for a genuinely interrupted session and a resolved item does not reappear after a restart.
- Checkpoint restore refuses a destination outside the workspace root and writes nothing.
- Every existing test still passes; nothing was weakened, ignored, re-gated, timed out differently, or deleted to reach a gate.
</success_criteria>

<output>
Create `.planning/phases/23B-continuous-agency/23B-01-SUMMARY.md` when done, recording the termination state (complete, complete-with-named-open-verbs, or escalated), the live evidence table's verdict distribution, the deliberate stdout-versus-stderr divergence from the existing `--list-sessions` flag, the Windows TUI backend measurement, and F23-02's disposition.
</output>