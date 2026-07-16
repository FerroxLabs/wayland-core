# F14 recovery and resynchronization integration seal

## Scope and identity

This receipt seals F14 against the clean committed integration source at
`e50ed42364a90f348ac80e920922a31b2c4aca45` with tree
`a649e350a1e330c6757fe96217d8bfa122f49f9b`. The direct F14 implementation is
`906287e1790ab2e0c8a6f1f71940e9acc2b55c75`, based on the F13 commit
`f263b0d53c14720eb49c6a8c5f4a3125a6f0efd8`, and is an ancestor of the sealed
integration source.

F14 resumes from a durable correlated cursor. It does not treat a process
restart, UI reconnect, or display transcript as authority to repeat provider
or tool work.

## Exact integration-source Linux proof

Every command below ran through
`/Users/seandonahoe/dev/ratchet-worktrees/wt-remote-cargo-cache/harness/remote-cargo.sh`
against committed source `e50ed42`; no Cargo command ran on the Mac.

| Gate | Cargo command | Result |
|---|---|---:|
| Packaged-process SIGKILL recovery | `cargo test -p wcore-cli --test f14_sigkill_recovery -- --nocapture` | 10 passed, 1 fixture helper ignored |
| Recovery protocol authority | `cargo test -p wcore-protocol --test recovery_protocol -- --nocapture` | 14 passed |
| Shared compile surface | `cargo check -p wcore-agent -p wcore-protocol -p wcore-cli` | PASS |
| Strict affected surface | `cargo clippy -p wcore-types -p wcore-tools -p wcore-plugin-api -p wcore-plugin-subprocess -p wcore-mcp -p wcore-browser -p wcore-agent -p wcore-protocol -p wcore-cli --all-targets --all-features -- -D warnings` | PASS |

The original exact-F14 candidate receipt on issue #457 records 7,823 passed,
28 skipped, affected-package check/Clippy success, a real PTY recovery test,
and independent security, recovery-authority, crash-consistency, runtime, and
cross-platform reviews with no HIGH/BLOCKER findings. The integration-source
runs above specifically re-prove the surfaces later F15-F17 commits touched.

An independent read-only review of exact integration tree `a649e350` found no
BLOCKER, HIGH, or MEDIUM issue in cursor authority, provider/tool/approval
recovery, stale-command rejection, active recovery serialization, TUI/host
projection convergence, or the later changes to shared engine and protocol
paths.

## Proved behavior

- SIGKILL during model streaming resumes as provider reconciliation without
  redispatch.
- SIGKILL during tool execution requires reconciliation without re-execution.
- An interrupted approval restores the exact gate; a later correlated approval
  executes the effect and continuation once.
- Host reconnect, non-genesis reconnect, standalone TUI, and JSON host project
  the same committed recovery state.
- Recovery commands are versioned and cursor-correlated. Unknown critical
  fields, stale authority dimensions, malformed digests, and unversioned or
  uncorrelated actions are rejected.
- Recovery snapshots serialize sanitized typed state, and replay is ordered and
  content-free at the protocol boundary.

## Honest boundary

This is a committed-source Linux integration seal, not a release or native
cross-platform seal. Native macOS and Windows process/filesystem behavior
remains for F28. Broader context-degradation work tracked by issue #636 is
separate from F14's committed-cursor and host-resynchronization invariant. No
push, PR, merge, release, or issue closure is claimed here; issues #457 and
#636 remain open.
