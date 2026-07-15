# Wayland Core F12 Crash-Complete Journal Receipt

**Status:** Linux committed-source implementation seal passed; native macOS,
Windows, release-artifact, and merge authority remain pending

**Implementation source:** `661b0ed337aae9480fe70da708f3050a1272d4ec`
on `feat/691`

**Implementation source tree:** `15d109049ae968dccca813b6fd02934e05d9c251`

**Version identity:** workspace `0.12.25`, described as
`v0.12.24-293-g661b0ed`; this is not a release tag

**F12 base:** `58e64fc6099f5a1360733f84389101e6f3c256e5`

## 1. Delivered authority contract

F12 replaces transcript-only recovery authority with a versioned append-only
execution journal. It now records the state required to recover turns, ordered
provider streams and physical attempts, tool effects, approvals, budget
reservations and settlements, checkpoints, children, and host delivery.

The implementation provides:

- binary frames containing schema version, session identity, monotonic
  sequence, previous checksum, event, and SHA-256 integrity;
- validation before append, durable `sync_all` before visibility, and explicit
  writer faulting when publication authority becomes uncertain;
- torn or incomplete final-frame recovery while rejecting complete-frame
  corruption, checksum or sequence breaks, session mismatch, and unsupported
  schemas;
- a reducer that rejects invalid ordering, duplicate terminal transitions,
  digest mismatches, invalid budget settlement, foreign approvals, and missing
  parents before those events receive durable authority;
- snapshots and compaction bound to session, cursor, cursor checksum, reduced
  state digest, and state, with fail-closed recovery for missing or mismatched
  compacted authority;
- a persistent pathname lease plus data-inode locking, symlink and multi-link
  rejection, owner-only Unix modes, and locked atomic replacement;
- provider attempt and structured stream evidence persisted before it is
  exposed to the engine or host; and
- explicit legacy-session import carrying its source schema and content digest.

Started external effects without a durable terminal record recover as
`Unknown`. F12 never fabricates success and never declares an uncertain effect
safe to repeat.

## 2. Verification evidence

All Cargo compilation and tests ran through the remote Hetzner Linux harness.
No Cargo build or test was run on the Mac.

### Final affected-package gate

```bash
/Users/seandonahoe/dev/ratchet/harness/remote-cargo.sh \
  wcore-f12-gate \
  /Users/seandonahoe/dev/waylandcore-worktrees/wt-691 \
  nextest run \
  -p wcore-agent -p wcore-providers -p wcore-egress -p wcore-cli \
  --no-fail-fast
```

At exact committed source `661b0ed337aae9480fe70da708f3050a1272d4ec`:

- nextest run `faf8624b-192e-45ed-a188-2fe6d7d10c9e`;
- 5,340 passed, 0 failed, 19 skipped;
- four tests exceeded nextest's 30-second slow threshold but passed; and
- the command exited zero.

### Strict lint gate

```bash
/Users/seandonahoe/dev/ratchet/harness/remote-cargo.sh \
  wcore-f12-clippy \
  /Users/seandonahoe/dev/waylandcore-worktrees/wt-691 \
  clippy \
  -p wcore-agent -p wcore-providers -p wcore-egress -p wcore-cli \
  --all-targets --all-features -- -D warnings
```

At the same exact commit, strict clippy passed and exited zero. The dependency
`imap-proto 0.10.2` emitted Cargo's future-incompatibility notice; it did not
produce a current clippy warning or failure.

### Focused proof retained during construction

| Surface | Result | Run identity |
|---|---:|---|
| Journal contract, crash, and compaction cohort | 44/44 passed | `9960edc7-34c2-493d-b0fc-0149a1eea862` |
| Engine physical-attempt persistence fixtures | 2/2 passed | `67bd722d-c785-41f7-9429-df92b8ed0f84` |
| Persisted compaction/provider fixture | 1/1 passed twice | `cd221ae1-e1c4-4ee2-a2e1-48e97ff3a3c4`, `d25b4cbd-d97f-42c9-b8f7-ad529a359879` |
| Persisted message accumulation repair | 1/1 passed | `c338b30a-2abe-4850-a01a-47d77cbba2f4` |
| Secret-redaction persistence repair | 1/1 passed | `d671b0d0-ed47-4c02-b0ce-0cb42981d69b` |
| Lifecycle guard scope | 1/1 passed | `69adda45-56cd-4e0b-b3b8-89e1f8d8e880` |

The two repair-specific runs exercised uncommitted candidate bytes before their
combined commit `2daacc41a1ee2f252a3e79b6add448f9aa4e424a`. The final
5,340-test run above is the authority binding those repairs to the exact
committed source.

Local non-compiling checks also passed:

```bash
cargo fmt --all -- --check
git diff --check
git status --short
```

After the docs-only receipt commit, the lane re-ran `git status --short`; its
output was empty. That is post-publication worktree evidence, not part of the
implementation tree identity above.

## 3. Fault and recovery coverage

The committed tests exercise crash injection before and after every declared
event family, torn final writes, complete-frame corruption, checksum, sequence,
session and schema tampering, invalid lifecycle transitions, duplicate
terminals, corrupt or missing compaction state, writer contention, process exit
while holding a lease, symlink and hard-link aliases, uncertain storage
publication, provider stream truncation, denied egress, and dropped or hung
provider streams.

Replay accepts only committed frames and produces the same reduced committed
state. Invalid candidate events do not advance durable sequence authority.

## 4. Independent review

Independent security and test reviews reported no HIGH or BLOCKER findings on
the journal implementation. A final review of the three fixture repairs found
no production weakening, helper leakage, retry misuse, cross-platform path
assumption, or coverage gap. The fixtures opt into a loopback physical-send
boundary; non-persisted mocks retain their prior behavior.

## 5. Honest boundary

This is an F12 Linux committed-source seal, not a cross-platform release seal.

- F12 does not make external side effects exactly-once. F13 owns idempotency,
  reconciliation, and operator resolution for unknown irreversible effects.
- F12 does not implement True Continue, reconnect cursors, Desktop/TUI host
  resynchronization, or restored cancellation state. F14 owns that work.
- Native macOS and Windows filesystem lock, link, rename, permission, and crash
  behavior remain unproven by this receipt.
- The crash matrix proves deterministic cut and injected-publication behavior
  at the OS `sync_all` contract. It does not claim survival under every hardware
  controller or filesystem cache mode.
- No release artifact, supply-chain signature, push, merge, or release tag is
  claimed here. Coordination issue #691 remains open for release authority.

These limitations do not block F13 implementation or the additive Desktop
producer-contract work, provided both pin this exact committed baseline or a
later fully re-proven descendant.
