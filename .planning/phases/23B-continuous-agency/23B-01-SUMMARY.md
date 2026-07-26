---
phase: 23B-continuous-agency
plan: "01"
subsystem: session-operator-lifecycle
status: complete-with-named-open-verbs
requirements:
  - F23-02
requirements_disposition:
  F23-02: incomplete
tags: [session, recovery, reconcile, cancel, export, checkpoint, operator-surface]
provides:
  - crates/wcore-agent/src/session_lifecycle.rs
  - crates/wcore-cli/src/session_cmd.rs
  - crates/wcore-cli/tests/session_operator_lifecycle.rs
  - scripts/f23-session-operator-drive.sh
key-files:
  created:
    - crates/wcore-agent/src/session_lifecycle.rs
    - crates/wcore-cli/src/session_cmd.rs
    - crates/wcore-cli/tests/session_operator_lifecycle.rs
    - scripts/f23-session-operator-drive.sh
    - .planning/SEAM-REQUESTS/23B.md
  modified:
    - crates/wcore-agent/src/session.rs
    - crates/wcore-agent/src/lib.rs        # FENCED — seam request SR-23B-01
    - crates/wcore-cli/src/lib.rs          # FENCED — seam request SR-23B-02
    - crates/wcore-cli/src/main.rs         # FENCED — seam request SR-23B-03
    - crates/wcore-cli/src/tui/checkpoint.rs
    - crates/wcore-cli/src/tui/mod.rs
    - crates/wcore-cli/Cargo.toml
commits:
  - c81eabd5
  - a875a8fc
  - 30153232
---

# Phase 23B Plan 01: Session Operator Lifecycle Summary

`wayland-core session` now performs every Success Criterion 2 verb from a real command
line, and the reconcile/cancel pair closes live Windows UAT defect D2 — proved end to
end against the shipped binary on Linux, with macOS, Windows and the TUI legs left
explicitly open.

**Termination state: 2 — complete with named open verbs.**

## What shipped

`crates/wcore-agent/src/session_lifecycle.rs` composes the existing `SessionManager`
and durable session journal into operator primitives: search, inspect, fork, retry,
export, retain, reconcile-list, reconcile-resolve and cancel. It reimplements neither
substrate. `crates/wcore-cli/src/session_cmd.rs` exposes them as
`wayland-core session <verb>`, each printing one greppable `F23_SESSION=` token plus
the identifier it acted on to STDOUT, with a documented exit-code map (0 ok, 3
not-found, 4 refused-by-authority, 5 outstanding-reconcile) asserted by the integration
suite.

## Decisions worth naming

**The export envelope carries no free text.** `wcore-protocol`'s `RecoveryTurnSnapshot`
already enforces "opaque identifiers and typed state, never transcript text, prompts,
tool arguments, output, paths or provider payloads". The export envelope honours the
same rule, representing each message by a SHA-256 digest and a byte length. This is
what makes "a run-time nonce planted in the session is absent from the export" true by
*construction*. The alternative the plan warned against — running the transcript
through `output_redaction.rs` — could not have satisfied it: that module wraps
`PIIScrubber`, a shape matcher, and no shape matcher can catch an arbitrary run-time
value. The cost is that an export is not a transcript backup; F26-03 gets provenance
and divergence detection, not content.

**Reconcile and cancel reuse states the product already defined.** The journal model
already carries `ToolResolutionSource::Operator` and `SessionEvent::TurnCancelled`, and
the reducer already accepts both. The product was *designed* for an operator to resolve
an unknown effect; no command ever surfaced it. No ninth `RecoveryReconcileReason` was
added — that enum is CI-checked against the Desktop contract corpus.

## The defect this plan existed to close, and the one inside the first fix

The first cut of the reconcile projection looked only at tool executions. A live crash
proved that wrong, and the failure reproduced D2 one level down:

```
session reconcile <id>  ->  outstanding=0
session cancel <id>     ->  invalid journal state transition:
                            turn ... has nonterminal provider attempt ...
```

`require_turn_descendants_terminal` gates `TurnCancelled` on five classes — approvals,
provider attempts, tool executions, hook phases, children — and a crash mid-dispatch
leaves a *provider attempt*. Telling the operator there is nothing to reconcile and
then refusing to cancel is the same dead end, with a friendlier error. The projection
now covers all five, each item carrying its kind, the reducer's own typed reason, and
whether this surface can resolve it. Items this surface cannot resolve are still
reported — silence is what made the original defect undiagnosable.

Provider attempts gained a real resolution. Which receipt applies is decided by the
state the crash left, not by the operator: `Prepared` → `ProviderAttemptNotStarted`,
`Unknown` → `ProviderAttemptFinished{Cancelled}`, each with a dispatch-correlated V2
form. An attempt that durably streamed bytes stays engine-only, because its terminal
receipt must carry a digest recomputed from those exact bytes.

## Security change

`CheckpointStore::restore` previously *skipped* a recorded destination outside the
workspace root and applied the remaining entries. A poisoned or older un-validated
`meta.json` therefore produced a silent PARTIAL rewind: an operator asked for a known
state and received a different one with no signal. Restore now refuses the whole
checkpoint with a structured `DestinationEscapesRoot` error before anything is written.
Proved by a hand-authored hostile fixture, since no legitimate capture produces such a
path (T-23B01-02).

## Deviations from plan

- **[Rule 1 — bug] Reconcile projected one descendant class of five.** Found during
  Task 3's live drive. Fixed in `30153232`.
- **[Rule 2 — missing critical behaviour] `cancel` surfaced an opaque reducer error.**
  Now refuses with `OutstandingReconcile` (exit 5) and the blocking count.
- **[Rule 1 — bug] Checkpoint restore silently skipped escaping paths.** Now refuses.
- **`scripts/f23-macos-binary.sh` was NOT written.** The plan decided the macOS leg
  builds its own binary on this Mac. The phase's controlling instruction forbids
  running Cargo on the Mac. I honoured the controlling instruction and escalated the
  conflict rather than resolving it unilaterally.
- **TUI `/checkpoint`, `/fork`, `/export` were NOT added**, and no PTY leg ran.

## Findings raised, not fixed

| ID | Severity | Finding |
|---|---|---|
| 23B-H1 | **HIGH** | A cleanly-exited run can write a journal the product cannot read back: `--resume` fails with `journal checksum mismatch at sequence 16` on a session `--list-sessions` still shows. 8/8 and 9/10 in two bursts under load, 0/3 when the host was quiet. Pre-existing; nothing in this phase touches the journal write path. See `23B-01-LIVE-EVIDENCE.md` §3. |
| 23B-M1 | MEDIUM | `--list-sessions` prints to STDERR; the new subcommand prints to STDOUT. Deliberate divergence, not converged. |
| 23B-M2 | MEDIUM | `retry` derives approval admissibility from durable tool-effect state, not a live re-evaluation of the policy gate. Strictly conservative but not the full re-derivation F23-02 describes. |
| 23B-M3 | MEDIUM | Provider-attempt receipts carry no `source` field, so the journal cannot record that a human rather than the engine asserted the outcome. |
| 23B-M4 | MEDIUM | `--session-id <existing id>` errors "already exists" rather than resuming. Cost a vacuous gate in this plan's own driver before it was caught. |

## Verification

- `cargo clippy -p wcore-agent --all-targets -- -D warnings` — clean on `hetzner-dsm`.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean on `hetzner-dsm`.
- `cargo fmt --all -- --check` — clean on the Mac.
- `cargo nextest run -p wcore-agent -E 'test(session_lifecycle)'` — 14/14.
- `cargo nextest run -p wcore-cli --test session_operator_lifecycle --test harness_cli_surface` — 19/19, pre-existing harness unchanged.
- `scripts/f23-session-operator-drive.sh` — exit 0 on Linux with a caller-generated
  nonce echoed in `F23_01_DRIVE=PASS platform=linux`.
- **Not run:** full `-p wcore-agent -p wcore-cli` aggregate. The build host ran out of
  disk (`No space left on device`) with six phases building concurrently. That is an
  infrastructure limit, not a code result, and is reported rather than worked around.

## Self-Check: PASSED

All four created files exist; all three commits resolve in `git log`.
