# Phase 25 — Criterion 1 repair — SUMMARY (lane `25-c1-cleanup`)

| | |
|---|---|
| **Lane** | `lane/25-c1-cleanup` |
| **Base** | `fd22dbf4` (`lane/grade-25`), then merged `gh/plan/f20-unified-audit-repair` @ `4a872413` |
| **Final commit** | `05a493a2` (see HEAD in the report; everything below re-proved at this commit) |
| **Mandate** | close the two named gaps in `25-PHASE-VERDICT.md` Criterion 1 |
| **Hosts** | controller `hetzner-dsm`; ssh far end = dedicated container `f25c1-sshd` (`ddc848056f15`); cloud = real Fly machines, app `wayland-f25-test` |

## Verdict

**Both gaps closed, and a third defect was found and closed on the way.**

| gap | before | after |
|---|---|---|
| **1b — ssh cleanup leaks the task root on failure** | 1 leaked root, `input.bin` readable on the far end | 0 roots, 0 leaked input, status still `Failure { code: "exit-7" }` |
| **1a — cloud cancellation never exercised** | never driven at any commit | driven; machine census 1 → 0 on two independent instruments |
| **NEW — cancelled cloud run wrote NO receipt** | receipt ABSENT, run exits 1 with a transport error | receipt WRITTEN, `terminal: Cancelled { reason: "operator cancelled" }`, integrity OK |
| **bonus (verdict G3) — four surfaces span two commits** | composition across `5e620ef0` + 25-01 | all four run at `05a493a2`, `NORMALIZED DIFF: EQUIVALENT` in ONE invocation |

I am **not** grading Criterion 1 — that is the verifier's call. What I can say is what the
verdict said would move it: cancellation is now demonstrated on the cloud surface with a
receipt, the cleanup defect is fixed and proved fixed, and the cross-commit qualification is
gone.

---

## Gap 1b — the ssh task-root leak

### The defect

`crates/wcore-exec-backend/src/backends/ssh.rs`, `REMOTE_RUNNER`. Under `set -e`,
`wait "$child"` **aborts the script** whenever the task exits non-zero, so `rm -rf "$root"`
never runs. Every failing task left `${TMPDIR:-/tmp}/wayland-f25-<nonce>/` on the far end
containing `input.bin` — the task's own input bytes. (`25-HOSTS-SUMMARY.md` FINDING 5,
MEDIUM, BACKLOG, still open at the graded base and still open on the merge train.)

### The fix

```diff
-wait "$child"
-status=$?
+status=0
+wait "$child" || status=$?
+cd /
 rm -rf "$root"
```

`|| status=$?` takes that one command out of `set -e`'s reach. The status is still the
child's and is still what the runner exits with.

**No `trap 'rm -rf "$root"' EXIT`, deliberately.** When the controller dies the runner is
signalled while the `setsid` child survives — that surviving child is Criterion 4's only
*unplanted* positive control, and `$root/.pid` is the primary signal `REMOTE_SCAN` reads to
find it. An EXIT trap would delete a live orphan's evidence and convert a real finding into a
clean zero. Cleanup therefore runs only where the child has already exited; cancellation
cleans up through `REMOTE_KILL`, which already ends in its own `rm -rf "$root"`. There is a
test asserting the trap stays absent.

### Live evidence — before and after, on a real ssh transport

`evidence/25-c1-cleanup/ssh-cleanup-{BASE,FIXED,FINAL,TRAIN}.txt`, all driven through the
shipped `wayland-core backend run --backend ssh`.

| measurement | BASE `20a76bd4` | FIXED `01ebc765` | TRAIN `05a493a2` |
|---|---|---|---|
| runner shape in the binary (`\|\| status=$?` / known-positive) | 0 / 2 | 1 / 2 | 1 / 2 |
| instrument: decoy root planted / removed | 1 / 0 | 1 / 0 | 1 / 0 |
| task actually ran on the far end (witness written by the task body) | 1 | 1 | 1 |
| **leaked roots after a failing task** | **1** | **0** | **0** |
| **leaked input bytes readable on the far end** | **1** | **0** | **0** |
| roots after a *succeeding* task | 0 | 0 | 0 |
| receipt terminal | `Failure { code: "exit-7" }` | same | same |

The leaked file was read back at BASE and contained the input verbatim:
`wayland-f25-c1-INPUT-BYTES-THAT-MUST-NOT-BE-LEFT-BEHIND`.

Four things stop each of those zeros being free: a planted decoy proves the counter can say
**1**; removing it proves the same counter can say **0**; the far-end listing is fenced by
`LIST-BEGIN`/`LIST-END` markers so a failed ssh reads NOT MEASURED rather than clean; and the
failing task writes a witness on the far end **from inside its own body**, so a phase
reporting zero roots still has to show the task ran there. A "fix" that cleaned up by never
running would be caught by that line alone.

### The test, and proof it can fail

`crates/wcore-exec-backend/tests/ssh_remote_runner_cleanup.rs` drives the **shipped**
`REMOTE_RUNNER` constant under a real `sh` in a private `TMPDIR`, and carries the pre-fix
script inline as a negative control that must be seen to leak before any other assertion runs.

```
3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`evidence/25-c1-cleanup/gate-can-fail.txt` — the fix was reverted **in place** (restored from
a file copy, never with git) and the test went **RED**:

```
thread 'a_failing_task_leaves_nothing_on_the_far_end' panicked at …:126:
input.bin — the task's own input bytes — was left on the far end after a failing task
test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
TESTRC_ON_UNFIXED=101
… restored, `git status --porcelain` empty …
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The file is `#![cfg(target_os = "linux")]` rather than `cfg(unix)` on purpose: the runner needs
`setsid(1)`, which macOS does not ship, and a `cfg(unix)` gate would produce a binary that
exits 0 having run nothing there.

---

## Gap 1a — cloud cancellation, driven for the first time

### No credential was needed from Sean, and the first "blocked" reading was wrong

`/root/.wayland-f25-cloud.env` is live. It needs the **app-scope override every earlier live
run uses** (`/root/f25-cloud/live-cloud.sh`, `25-CLOUD-SUMMARY.md` §"app-scoped, not
org-scoped"): the file's `WAYLAND_F25_CLOUD_ORG` holds `sean-donahoe`, a personal **org**,
while the backend wants the **app** `wayland-f25-test`. Sourcing the file alone yields
`HTTP 404 app not found`, which reads exactly like a dead credential and is not one. That
wrong first attempt is kept as `evidence/25-c1-cleanup/cloud-cancel-PROBE2.txt` rather than
deleted.

### What was driven

`evidence/25-c1-cleanup/cloud-cancel-{BEFORE,AFTER,TRAIN}.txt`. Task: `sleep 120` on the cloud
backend, cancelled from a **second process** via `wayland-core backend cancel --task-id … --backend cloud`.

- The product's own probe was read back — `available: true`, `probe basis: VendorApiCall`,
  `app wayland-f25-test` — rather than inferred from what I exported (§3b-ii).
- The cancel is issued only **after the machine has been observed suspended and then started
  again**. Vendor event types read straight from the API before the cancel destroys the
  record: `start, start, suspension, suspension, start`. So what was cancelled is a task on a
  genuinely hibernated-and-resumed machine.
- Machine census taken by **two independent instruments** — the product's `backend orphans`
  and a raw vendor API call that does not go through the product:

| | before | **while running** | after cancel |
|---|---|---|---|
| product `backend orphans` cloud row | 0 | **1** | 0 |
| raw vendor API count | 0 | **1** | 0 |

  The `1` in the middle column is what makes the two zeros mean something.
- `backend cancel` → exit 0, `residual: none — the cleanup was verified by re-enumeration`.
- `backend scan --task-id f25c1cancel` after: `count 0 (MEASURED)`.

**No leaked billable machine, on any of the three drives.**

### The defect the drive exposed, and its fix

On the first drive the run process died with

```
transport failed: machine exec returned HTTP 412: {"error":"failed_precondition: exec request failed: EOF"}
```

exit 1, **and wrote no receipt at all** (`F25C1-CLOUD-CANCEL-RECEIPT-BEFORE: ABSENT`), while
local, container and ssh each write `Cancelled { reason: "operator cancelled" }`
(`evidence/25-01/cancellation-transcript.txt`). Criterion 1 names *receipts* and
*cancellation* as equivalence properties, so a cancellation the fourth backend cannot attest
is in scope. Cause: `cancel` destroys the machine out from under the in-flight exec, so the
vendor answers the exec with an error and `execute` propagated it.

Fix in `cloud.rs::execute`: when the drive fails **and the cancel marker was taken**, emit the
cancellation receipt instead of the transport error. The marker is authoritative — only
`cancel` writes it — so an error with no marker still propagates unchanged. The receipt claims
**no** hibernation (`NotObserved` with a reason), because the observation died with the failed
drive and binding condition C1 forbids claiming one it cannot show.

After (`cloud-cancel-TRAIN.txt`):

```
RUN_AFTER_CANCEL_EXIT=0
terminal:    Cancelled { reason: "operator cancelled" }
hibernation: NotObserved { reason: "the run was cancelled before it could report a hibernation observation; …" }
receipt:     …/receipt-cloud-cancel-TRAIN.json      INTEGRITY: OK
```

A unit test pins that receipt shape and carries a negative control — without the marker the
identical outcome must **not** read as a cancellation, or the arm would relabel every genuine
cloud failure as an operator cancellation, which is worse than the defect it fixes.

---

## Bonus — verdict gap G3 closed: four surfaces, one commit, one diff

`evidence/25-c1-cleanup/four-surface-one-commit.txt`, at `05a493a2`:

```
F25C1-SC1-LOCAL: exit=0   F25C1-SC1-CONTAINER: exit=0
F25C1-SC1-SSH:   exit=0   F25C1-SC1-CLOUD: exit=0 app=wayland-f25-test
NORMALIZED DIFF: EQUIVALENT
TOTAL: 0 orphan(s) measured across 4 backend(s); 0 surface(s) NOT measured
```

The verdict's stated qualification was that the four-surface claim spanned two commits because
`WAYLAND_EXEC_SSH_TARGET` was unset and the old sshd target was gone. This lane had to stand a
far end up anyway, so it cost one script. Note also **0 surfaces NOT measured** — the phase's
earlier scans always carried the cloud row as unmeasurable.

The ssh far end is a container on the same physical host, exactly as 25-01 disclosed
(`F25-SC1-SSH-TARGET: CONTAINERIZED-SSHD`). It proves the transport and remote cleanup; it
does not prove a second machine.

---

## Merge-train re-check (orchestrator correction)

Merged `gh/plan/f20-unified-audit-repair` @ `4a872413` at `05a493a2`; merge-base `fd22dbf4`;
no conflicts; `cargo fmt --all -- --check` clean. **Neither gap was closed by the train**, read
off the tip itself: `ssh.rs:201-203` still carries `wait "$child"` / `status=$?` /
`rm -rf "$root"`, and the cloud arm is absent. The only train commits touching this crate are
`bf9fe2b8` and `de47947b`, both of which change `tests/node_contract.rs` only. Every live
number above was then re-taken on the merged tree.

## Regression

At `05a493a2`, on `hetzner-dsm`:

- `cargo nextest run -p wcore-exec-backend` → **129 tests run: 129 passed (1 leaky), 2 skipped**
- `cargo clippy --release -p wcore-exec-backend --all-targets` → rc 0
- `cargo fmt --all -- --check` → rc 0 (also clean on the Mac)

**One pre-existing flake, not mine and not the train's:**
`registry::tests::a_recorded_task_is_readable_by_another_caller_and_removable` fails
intermittently under plain `cargo test` (measured 1 failure in 3 runs) and passes 3/3 in
isolation and 129/129 under nextest. The helper's own comment names the cause — `with_temp_state`
mutates the process-global `WAYLAND_EXEC_BACKEND_STATE_DIR` and is only isolation-safe "under
nextest, which runs each test in its own process". `git log` shows the train never touched
`registry.rs`. Reported, not fixed: it is not this lane's file and the repo's stated runner is
nextest. **BACKLOG, LOW.**

## Open, and honestly stated

1. **`backend run` on cloud without a cancellation, when the task outlives the vendor's exec
   window, also ends as a bare transport error** (`HTTP 408 deadline_exceeded`,
   `cloud-cancel-PROBE2.txt`) rather than a `Timeout` receipt. Same family as the defect fixed
   here, different clause. Not fixed — it was outside the two named gaps and I did not want to
   widen the cloud change while other lanes are building. **BACKLOG, MEDIUM.**
2. **One unexplained container failure.** At `74e002b2` the four-surface run had
   `container → Failure { code: "exit-125" }` with 316 bytes of stderr, making that diff
   DIVERGENT on `artifact, events, terminal` while local, ssh and cloud agreed digest-for-digest.
   Exit 125 is a docker daemon-level refusal. It did **not** reproduce: container is `Success`
   4/4 at `05a493a2`, including inside the EQUIVALENT four-surface run. Cause unknown; five
   lanes share this docker daemon. **I also lost that capture** — the re-run overwrote
   `four-surface-one-commit.txt`, because I did not phase-name that script's output as I did
   the others. The DIVERGENT transcript exists only in my session log, so treat this item as an
   observation, not evidence.
3. The ssh far end is containerised, on the controller's own host (see above).
4. `F25-05`, Criterion 2's controller-side attribution (G7), `node probe` refresh (G8), and
   all of Criterion 4 are untouched — Criterion 4 belongs to `lane/25-c4-egress`.

## Secrets

The Fly token reached hetzner only by being sourced from the existing 0600
`/root/.wayland-f25-cloud.env` into the environment, and reached `curl` on **stdin** via
`--config -` so it never appeared in an argv or the host's process table. Swept afterwards
with the live value as the needle:

```
files: 32
TOKEN-VALUE-HITS: 0 (expect 0)
KNOWN-POSITIVE-HITS: 10 (the same sweep, on a string that IS present)
```

## Instrument defects found in my own harness, and repaired

Per §6b-ii these are repaired here, not merely noted.

1. **A cancel trigger that could not observe its own transition.** The first cloud script
   waited for `"state":"suspended"`; sampled every 2s the machine reads `suspending` on the way
   down and is `started` again by the next sample, so the trigger never fired and the run
   completed uncancelled. Now matches both spellings and settles 6s into the exec. The failed
   attempt is kept as evidence.
2. **A binary-provenance needle that matched nothing in either build.** The cloud script's
   "which arm is compiled into this binary" line searched for a phrase present in **no commit
   of this repository** — 0 for the pre-fix binary and 0 for the fixed one. It gated nothing,
   but a dud reader left in place is a dud reader the next lane trusts. Repaired with the
   three assertions §6b-ii asks for, measured against the retained pre-fix binary:
   `PRE-FIX → fixed-arm 0, known-positive 1, old dud needle 0`;
   `FIXED → fixed-arm 1, known-positive 1, old dud needle 0`.

## Shared-file fence

Zero exposure. `crates/wcore-cli/src/lib.rs` and `main.rs` untouched by this lane; the only
`crates/` files changed are `wcore-exec-backend/src/backends/{ssh,cloud}.rs` and the new test.

## Cleanup

Hetzner: `f25c1-sshd` container + image, `/root/f25c1*`, `/root/wayland-25c1` worktree and its
`target/`, and the `hz/25-c1` branch removed at the end of the lane; the small
`/root/f25c1-evidence` capture directory and the four scripts are retained so this is
re-runnable. No cloud machine survives any drive (0 on two instruments, three times).
