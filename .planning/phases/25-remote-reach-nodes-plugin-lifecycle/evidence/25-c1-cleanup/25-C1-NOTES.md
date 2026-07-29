# Phase 25 Criterion 1 repair — NOTES (running log, lane `25-c1-cleanup`)

Started 2026-07-29. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-25-c1-cleanup`, branch
`lane/25-c1-cleanup`, base `fd22dbf4196b3114ec679b66d8c96efcb3707e1a` (= `lane/grade-25` HEAD,
verified with `/usr/bin/git rev-parse`).

**Mandate:** close the two named gaps in `25-PHASE-VERDICT.md` Criterion 1.

- **Gap 1b (priority, a LIVE DEFECT):** the ssh remote runner leaks its task root, including
  `input.bin` (the task's input bytes), on every failing task. Fix it and prove the leak gone
  with a before/after count on a real ssh target.
- **Gap 1a (never exercised):** cloud cancellation was never driven at any commit. Drive it.

Not mine: Criterion 4 (egress / secret denial) — lane `25-c4-egress` owns that.

---

## t0 — the defect located in source, before any build

`crates/wcore-exec-backend/src/backends/ssh.rs:186-205`, `const REMOTE_RUNNER`:

```sh
set -eu
...
setsid "$@" &
child=$!
echo "$child" > "$root/.pid"
wait "$child"          # <-- under `set -e`, a NON-ZERO status aborts HERE
status=$?
rm -rf "$root"         # <-- never reached when the task exits non-zero
exit "$status"
```

So: **every task that exits non-zero leaves `${TMPDIR:-/tmp}/wayland-f25-<nonce>/` on the far
end, containing `input.bin` (the task's input bytes) and `.pid`.** The success path cleans up;
only the failure path leaks. That matches `25-HOSTS-SUMMARY.md` FINDING 5 ("six such roots were
found on the node and purged by hand"), which is still `BACKLOG`/unfixed at this base.

### The fix I will NOT make, and why (recorded before I write the fix)

The obvious "robust" fix is `trap 'rm -rf "$root"' EXIT`. **That would be a regression, not a
fix.** When the controller is killed `-9` the runner `sh` dies on SIGHUP while the `setsid`
child deliberately survives — that is exactly the *unplanted* positive control Criterion 4's
no-orphan half rests on (`evidence/25h-ssh-orphan-ledger.txt`, pid 1170). An EXIT trap would
`rm -rf` the root out from under that surviving orphan, deleting `$root/.pid` — the **primary
signal** `REMOTE_SCAN` uses (`ssh.rs:471-476`). Criterion 4 is MET on that half; I am not
trading it for Criterion 1.

So the repair is scoped to the path where the child has **already exited**: take `wait` out of
`set -e`'s reach, keep the status, then clean up.

Cancellation is already covered: `REMOTE_KILL` (`ssh.rs:549`) ends with
`rm -rf "$root" 2>/dev/null || true`, and the wall-clock timeout path routes through
`self.cancel()` (`ssh.rs:332`).

## t1 — the leak REPRODUCED live at BASE

`evidence/25-c1-cleanup/ssh-cleanup-BASE.txt`, driven by `f25c1-ssh-cleanup.sh` on
`hetzner-dsm` against a dedicated containerised sshd far end (`f25c1-sshd`, port 2226,
far-end hostname `ddc848056f15` — a different host name and a different `/tmp` from the
controller `Ubuntu-2404-noble-amd64-base`).

Binary provenance read out of the binary itself, not assumed from the directory:

```
tree HEAD:  20a76bd473787bcdd1dbc51ba513118256086e88   (lane base + NOTES, pre-fix)
binary sha: aeb00fd682e0cfd3cb7aef52ce3b7ddb2fc4c54dbec54cacab3ab41fb4ed3996
fixed-shape  '|| status=$?'  : 0      <- the fix is NOT in this binary
known-positive 'rm -rf $root': 2      <- but the reader can find the script
```

| measurement | BASE |
|---|---|
| `F25C1-INSTRUMENT` (decoy planted / removed) | `positive=1 negative=0` — the counter answers in both directions |
| `F25C1-TASK-RAN-ON-FAR-END` | `1` — the task's own body wrote `f25c1fail ran on ddc848056f15` |
| **`F25C1-LEAKED-ROOTS-AFTER-FAILING-TASK`** | **`1`** — `/tmp/wayland-f25-f25c1fail` survived |
| **`F25C1-LEAKED-INPUT-BYTES`** | **`1`** — `input.bin` is readable and contains the task's input verbatim |
| `F25C1-ROOTS-AFTER-SUCCEEDING-TASK` | `0` — the success path already cleaned up |
| receipt terminal | `Failure { code: "exit-7" }`, integrity OK |

So the defect is exactly as FINDING 5 describes, it is still live at the graded base, and
the leaked artefact is the task's input bytes — not merely an empty directory.

## t2 — Gap 1b CLOSED live

`ssh-cleanup-FIXED.txt`, same script, same far end, binary rebuilt at `01ebc765`
(`binary sha 9bc18141…`, `fixed-shape '|| status=$?': 1`).

| measurement | BASE `20a76bd4` | FIXED `01ebc765` |
|---|---|---|
| instrument (decoy planted / removed) | 1 / 0 | 1 / 0 |
| task ran on the far end (witness) | 1 | 1 |
| **leaked roots after a failing task** | **1** | **0** |
| **leaked input bytes** | **1** | **0** |
| roots after a succeeding task | 0 | 0 |
| receipt terminal | `Failure { code: "exit-7" }` | `Failure { code: "exit-7" }` |

The status still propagates, so cleanup was not bought by swallowing the outcome.

`gate-can-fail.txt`: the fix was reverted IN PLACE (restored from a file copy, never with
git) and the new test went **RED** — `1 passed; 2 failed; 0 ignored; 0 filtered out`, the
failure message being *"input.bin — the task's own input bytes — was left on the far end after
a failing task"*. Restored, tree clean (`git status --porcelain` empty), test **GREEN** at
`3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.

## t3 — Gap 1a: cloud cancellation DRIVEN, and it exposed a second defect

The credential needed no Sean action. It did need the app-scope override every earlier live
run uses (`/root/f25-cloud/live-cloud.sh`): the env file's `WAYLAND_F25_CLOUD_ORG` is
`sean-donahoe`, a personal ORG, and the backend wants the APP `wayland-f25-test`. Sourcing the
file alone gives `HTTP 404 app not found`, which reads exactly like a dead credential and is
not one (`cloud-cancel-PROBE2.txt` records that first wrong attempt rather than hiding it).

`cloud-cancel-BEFORE.txt`, machine `8d967d9fe40308`:

- product read back its own probe basis: `VendorApiCall`, `available: true`, app
  `wayland-f25-test` — not inferred from what I exported;
- vendor state timeline from an INDEPENDENT raw API call: `created` → `suspending` → `started`,
  with the machine's own event types `start, start, suspension, suspension, start`. So the
  cancel lands on a machine that genuinely hibernated and resumed;
- census while running = **1** on BOTH instruments (the product's `backend orphans` and the raw
  vendor call). That is the known-positive that makes the later zeros mean something;
- `backend cancel --task-id f25c1cancel --backend cloud` → exit 0,
  `residual: none — the cleanup was verified by re-enumeration`;
- census after = **0** on both instruments. **No leaked billable machine.**
- **but: `F25C1-CLOUD-CANCEL-RECEIPT-BEFORE: ABSENT`.** The run process died with
  `transport failed: machine exec returned HTTP 412: failed_precondition: exec request failed:
  EOF` and exit 1, and wrote no receipt. Local/container/ssh all write
  `Cancelled { reason: "operator cancelled" }` (`evidence/25-01/cancellation-transcript.txt`).

**A second, previously unknown defect**, and exactly the kind only driving finds: the cloud
surface could not attest a cancellation at all. Criterion 1 names receipts AND cancellation as
equivalence properties, so this is in scope. Fixed at `74e002b2`; re-drive pending.

A separate observation, NOT fixed here: an uncancelled `sleep 120` with `wall_time_ms 60000`
also ends as a bare transport error (`HTTP 408 deadline_exceeded`, `cloud-cancel-PROBE2.txt`)
rather than a `Timeout` receipt. Same family, different clause; recorded for BACKLOG.

## t4 — merged onto the merge train (orchestrator correction), both gaps re-checked there

The lane was based on `lane/grade-25`, which predates the 24-branch merge train. Merged
`gh/plan/f20-unified-audit-repair` @ `4a872413` into this branch at `05a493a2`; merge-base
`fd22dbf4`, no conflicts, `cargo fmt --check` clean.

**Neither gap was closed by the train.** Read off the train tip itself, not inferred:

```
$ /usr/bin/git show gh/plan/f20-unified-audit-repair:crates/wcore-exec-backend/src/backends/ssh.rs
201: wait "$child"
202: status=$?
203: rm -rf "$root"          <- still unreachable under `set -e` for a failing task

$ /usr/bin/git log <merge-base>..gh/plan/f20-unified-audit-repair -- crates/wcore-exec-backend
bf9fe2b8  fix(node-contract): run the identity probe through the inner std command
de47947b  fix(ci): pin container source identity, derive node machine_id in a clean env, …
```

`bf9fe2b8` touches `tests/node_contract.rs` only (5 added lines). The train did not touch the
ssh runner or the cloud execute path, so both fixes remain necessary and both apply cleanly.
The cloud arm is likewise absent on the tip. Everything below is re-proved on the merged tree.

### Instrument repair (§6b-ii), found while re-checking

The cloud script's "which arm is compiled into this binary" line searched for the phrase
`the cancellation destroyed the machine`, **which appears in no commit of this repository**. It
printed `0` for the pre-fix binary and `0` for the fixed one — a reader that cannot
distinguish the thing it exists to distinguish. It gated nothing (the live receipt is the
proof), but it is repaired here rather than written up, with the three assertions §6b-ii asks
for, measured against the retained pre-fix binary:

```
repaired reader on /root/f25c1-bin-base (PRE-FIX):
  fixed-arm literal      : 0     <- known-negative fails
  known-positive literal : 1     <- reader is alive
  the OLD dud needle     : 0     <- the broken matcher would have missed it
```

## Still to establish

- [x] Leak reproduced live on a real ssh far end at BASE (count 1, `input.bin` present).
- [x] Instrument proven alive in both directions in the same phase.
- [x] Leak gone at FIXED commit (0 roots) with the witness still proving remote execution.
- [x] Exit status still propagates after the fix.
- [x] `cargo test -p wcore-exec-backend --test ssh_remote_runner_cleanup`: 3 passed, 0 ignored,
      0 filtered out — and proven able to go red.
- [x] Cloud cancellation driven live: cleanup lands, machine count 1 → 0 on two instruments.
- [ ] Cloud cancellation re-drive at `74e002b2`: a receipt carrying `Cancelled`.
- [ ] Regression pass on `wcore-exec-backend` at the final commit.
- [ ] Token-value sweep over every capture (done once at t3: 0 hits, known-positive 4).
