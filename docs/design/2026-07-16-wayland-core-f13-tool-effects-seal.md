# F13 tool-effect integration seal

## Scope and identity

This receipt seals F13 against the clean committed integration source at
`e50ed42364a90f348ac80e920922a31b2c4aca45` with tree
`a649e350a1e330c6757fe96217d8bfa122f49f9b`. The atomic F13 implementation is
`f263b0d53c14720eb49c6a8c5f4a3125a6f0efd8`, based on
`d0aa0abc75afe056cc5434fcd652efa6d474ab0c`, and is an ancestor of the sealed
integration source.

The invariant is narrow and safety-critical: an effect with an unknown
external outcome is never blindly dispatched again. Automatic continuation
requires durable proof that the effect did not start, a provider-enforced
stable idempotency key, or authoritative reconciliation.

## Exact integration-source Linux proof

Every command below ran through
`/Users/seandonahoe/dev/ratchet-worktrees/wt-remote-cargo-cache/harness/remote-cargo.sh`
against committed source `e50ed42`; no Cargo command ran on the Mac.

| Gate | Cargo command | Result |
|---|---|---:|
| Durable dispatch state and crash cuts | `cargo test -p wcore-agent f13_durability_tests -- --nocapture` | 20 passed |
| Journal crash/replay matrix | `cargo test -p wcore-agent --test session_journal_crash_matrix_test -- --nocapture` | 4 passed |
| Guarded rollback | `cargo test -p wcore-agent --test rollback_tool_test -- --nocapture` | 8 passed |
| File-effect opacity and fixture CAS | `cargo test -p wcore-tools --test prepared_file_effects --test vfs_compare_exchange -- --nocapture` | 6 passed |
| Subprocess identity and ambiguous crash handling | `cargo test -p wcore-plugin-subprocess --test subprocess_e2e -- --nocapture` | 8 passed |
| Shared compile surface | `cargo check -p wcore-agent -p wcore-protocol -p wcore-cli` | PASS |
| Strict affected surface | `cargo clippy -p wcore-types -p wcore-tools -p wcore-plugin-api -p wcore-plugin-subprocess -p wcore-mcp -p wcore-browser -p wcore-agent -p wcore-protocol -p wcore-cli --all-targets --all-features -- -D warnings` | PASS |

The earlier exact-F13 candidate receipt on issue #889 additionally records an
affected default-feature run of 4,098 passed, 14 skipped, strict Clippy, and no
unresolved HIGH/BLOCKER review finding. The integration-source runs above are
the durable regression seal after later F14-F17 changes touched shared journal
and orchestration paths.

An independent read-only review of exact integration tree `a649e350` found no
BLOCKER, HIGH, or MEDIUM issue in durable append ordering, reducer retry
constraints, dispatch boundaries, restart reconciliation, denial handling,
host-file CAS honesty, adapter identity propagation, or operator evidence.

## Proved behavior

- Policy, approval, budget, circuit, cancellation, and preparation refusal are
  durable `not_started` states and do not fabricate a physical dispatch.
- Timeout, panic, post-start cancellation, and MCP/plugin/script adapter loss
  become durable `unknown` outcomes and block automatic redispatch.
- Journal truncation, corrupt tails, snapshot publication cuts, and corrupt
  snapshots fail closed or replay only committed frames.
- Ordinary host Write/Edit remain functional but honestly opaque. Real host
  files do not advertise authoritative compare-and-swap; rollback refuses when
  object identity or guarded content changed.
- Subprocess plugins preserve versioned effect identity and never replay an
  ambiguous request after worker failure; legacy plugins retain an explicit
  fallback path.

## Honest boundary

This is a committed-source Linux integration seal, not a release or native
cross-platform seal. Native macOS and Windows crash/filesystem behavior remains
for F28. Production host-file exactly-once mutation is not claimed: where the
filesystem cannot prove authoritative compare-and-swap, F13 fails closed and
requires reconciliation. No push, PR, merge, release, or issue closure is
claimed here; coordination issue #889 remains open.
