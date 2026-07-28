# Phase 25 — status

## CURRENT STANDING (read this first)

| Criterion | Grade | Closed by |
|---|---|---|
| 1 — one task across local / container / ssh / cloud | **MET** | `lane/25-cloud`, 2026-07-28 |
| 2 — nodes pair, advertise, revoke, recover, mixed versions, attribution | **MET on every named property**, one limitation recorded | `lane/25-hosts`, 2026-07-28 |
| 3 — twelve-verb plugin lifecycle | **MET on Linux**, PARTIAL on Windows | `lane/25`, 2026-07-27 |
| 4 — fail closed, no orphaned execution | **MET** | `lane/25-hosts`, 2026-07-28 |

**Nothing in Phase 25 is now waiting on Sean.**

### Two corrections to what this file used to say

1. **This header used to claim "two of four MET" while the verbatim gradings below
   showed only Criterion 3.** The table above is the governing statement; the verbatim
   section is kept as a dated record of how each criterion was graded when it was
   graded, and its Criterion 2 and 4 entries are SUPERSEDED by
   `25-HOSTS-SUMMARY.md`.

2. **Criteria 2 and 4 were recorded as blocked on Sean establishing SSH trust between
   `hetzner-dsm` and `SeanD@seandesktop` (item 2 and 3 under "What is genuinely broken",
   below). That blocker did not exist.** The trust was already live and was measured on
   2026-07-28:

   ```
   $ ssh hetzner-dsm 'ssh -o BatchMode=yes -o ConnectTimeout=15 SeanD@seandesktop hostname; echo RC=$?'
   SeanDesktop
   RC=0
   ```

   Both criteria were then closed with no credential, no new machine and no Sean action.
   A wrongly-reported blocker costs a reserved round trip for nothing, which is why the
   correction sits at the head of this file rather than at the foot.

### What closing them actually found

Running the real thing on a second real host produced **three HIGH defects that a green
suite could not see**, all in the ssh backend, all fixed on `lane/25-hosts`:

| # | Defect | Would it have failed loudly? |
|---|---|---|
| 1 | `backend scan --task-id 'x;id>/tmp/w;echo y'` executed `id` **as root on the far end** — ssh does not carry an argv, and the far end's login shell re-parsed every value | No — it "worked" |
| 2 | An empty task input **vanished from the wire**, shifting task argv left, so the ssh backend could not run a task with empty input at all | Yes — exit 1 |
| 3 | The orphan sweep reported **`0 (MEASURED)` while an orphan ran** on a Windows far end, because msys `ps` rejects `-eo`, its stderr went to `/dev/null`, and the pipeline ended in `\|\| true` | **No — silent false zero** |

Detail, live transcripts and both control directions: **`25-HOSTS-SUMMARY.md`**.

---

## Original hand-back (2026-07-27, lane/25)

All four plans executed. Graded verbatim below.

Branch: `lane/25` (worktree of `waylandcore-ferrox`, based on
`plan/f20-unified-audit-repair` @ `de977949`, plus the coordinator's base
`Cargo.lock` fix `9a86b287` cherry-picked so `--locked` works).

**UPDATE 2026-07-28, `lane/25-cloud` @ `5e620ef0`.** The Fly credential arrived and the
cloud leg was run. **Criterion 1 is now MET** and **Criterion 4's cloud half is closed**,
leaving SSH as its only unmeasured surface. Criteria 2 and 3 are untouched by that lane and
their gradings below stand as written. Detail: `25-CLOUD-SUMMARY.md`,
`evidence/25-cloud-ledger.txt`.

---

## Plans

| plan | wave | state |
|---|---|---|
| 25-01 — execution-backend contract + four reference backends | 1 | **COMPLETE**, termination state 2 (bounded cloud gap) |
| 25-02 — twelve-verb plugin lifecycle | 1 | **COMPLETE**, termination state 1 |
| 25-03 — node/device contract | 2 | **COMPLETE**, termination state 2 (one named gap) |
| 25-04 — hostile fail-closed matrix + orphan scanner | 3 | **COMPLETE**, termination state 2 (three HIGH findings, all fixed) |

---

## Success Criteria, graded verbatim

> **1. The same task runs locally, in a container, over SSH, and on one hibernating cloud
> backend with equivalent policy, receipts, cancellation, and cleanup.**

**MET** — closed by `lane/25-cloud` on 2026-07-28 at `5e620ef0`, once Sean minted the Fly
credential. See `25-CLOUD-SUMMARY.md` and `evidence/25-cloud-ledger.txt`.

The cloud leg was **broken, not merely unexercised**: machine create sent no request body,
no nonce metadata was ever set, and the task never ran on the machine — stdout was the
submitted input echoed back by the controller. The last two would have produced a FALSE
GREEN rather than a failure. All three are fixed.

Cloud now reports `yes / vendor_api_call`, runs the reference task (exit 0, machine
`8ed9d7dc3e5618`, terminal Success), and diffs **EQUIVALENT** against local and container at
the same commit. Hibernation is observed as a genuine **suspend**, not a stop: a `/dev/shm`
witness survived the transition and the guest `boot_id` was unchanged, while the stop/start
control on the same machine lost the witness and changed the boot id. A separate provenance
gate — a task reading a file only the guest has — proves the work ran on the machine rather
than being echoed, which the equivalence diff alone cannot detect.

QUALIFICATION: the **ssh** leg was not re-run at this commit (`WAYLAND_EXEC_SSH_TARGET`
unset, 25-01's `f25-ssh-target` host entry gone). The four-surface claim is a composition
across two commits: local + container + cloud proven together here, ssh proven at 25-01.

> **2. Nodes pair, advertise capability, revoke, recover offline, and handle mixed versions
> without losing authority attribution.**

**NOT MET.** All six properties were exercised through the shipped binary and attribution
held after all five disruptions — but against a genuinely separate *machine identity*
(own hostname, own filesystem, own minted node key, own process table and netns, reached
over real ssh, genuinely stoppable), **not a second physical host**. Neither `hetzner-dsm`
nor `SeanD@seandesktop` holds an SSH key authorizing the other, and creating one is an
authorization grant on Sean's machines. Exact closing commands: `25-03-NODE-EVIDENCE.md` §7.

> **3. Plugins can be scaffolded, tested, signed, installed, approved, inspected, updated,
> rolled back, removed, published, and recovered.**

**MET on Linux.** All twelve verbs driven through the shipped release binary on
`hetzner-dsm` with an independently observed state change after each, and all four negative
cases held. On Windows eleven of twelve ran (`new` unrun — `cargo-generate` absent) with
all four negative cases holding and one divergence recorded PARTIAL. Detail:
`25-02-LIFECYCLE-TRANSCRIPT.md`.

> **4. Compromised keys/plugins/backends and denied secret/egress paths fail closed with no
> orphaned execution.**

**NOT MET** — but the cloud half is now closed, leaving SSH as the only unmeasured surface.

The fail-closed half holds: all five hostile cases refuse on both hosts with named verdicts,
nonzero exits and no fallback. The no-orphan half holds for **local**, **container** and now
**cloud**. **SSH alone still reports `NOT MEASURED`.**

Cloud closed by `lane/25-cloud` on 2026-07-28 at `5e620ef0`. It previously could not have
measured anything at all: machines were created with no metadata while the scan filtered on
`metadata.wayland_task_nonce`, so the filter matched a key nothing carried and the scan would
have returned an empty list unconditionally — a structural false zero. With the tag set, the
scan is checked in both directions: a **real leaked machine** (`82d1d97b062338`, leaked by a
`tail -1` parse defect in the lane's own script, not planted) was found as `count 1
(MEASURED)` with its raw row and a nonzero exit; an unused nonce measures `0 (MEASURED)`;
after the destroy the same nonce measures `0 (MEASURED)`; and the app is verified empty.

SSH remains correct-but-unmeasured, and one unmeasured surface is still not "across every
reference backend".

---

## What is genuinely broken or unresolved

1. ~~**The cloud credential** still blocks Criterion 1. Reserved to Sean.~~
   **RESOLVED 2026-07-28** — Sean minted the credential; `lane/25-cloud` ran the leg and found
   the backend was **broken, not merely unexercised** (three HIGH defects, two of which would
   have produced a false green). All fixed. (`25-CLOUD-SUMMARY.md`)
2. ~~**No SSH trust between the two physical hosts** blocks the cross-machine half of
   Criterion 2. Reserved to Sean. (`25-03-NODE-EVIDENCE.md` §7)~~
   **WITHDRAWN 2026-07-28 — this blocker did not exist.** The trust was already live;
   `lane/25-hosts` measured it and ran the full node corpus with `hetzner-dsm` as
   controller and `SeanD@seandesktop` as the node. See the head of this file.
3. ~~**The SSH orphan surface is unmeasurable on the proof hosts**, which blocks Criterion 4.
   It reports `NOT MEASURED`, never zero.~~
   **RESOLVED 2026-07-28 by `lane/25-hosts`.** The surface is now MEASURED on two far
   ends — a containerised sshd and the real Windows host — each checked in both
   directions. Closing it found the sweep was a **structural false zero** on Windows.
   The **cloud** orphan surface was closed on 2026-07-28 by `lane/25-cloud`: it is now
   MEASURED, checked by a real leaked machine the scan found and by an unused nonce it
   correctly measured as zero. (`evidence/25-cloud-orphan-control.txt`)
4. **The Windows Job Object reaping mechanism is not proven** and is not claimed by this
   phase. The orphan claim is an observation and is INDEPENDENT of the escalated
   `live_future_drop_reaps_descendant_job_tree`.

## Findings fixed in this phase (all HIGH, all false answers)

| # | Plan | Finding |
|---|---|---|
| 1 | 25-02 | `plugin sign` wrote the signature where the verifier never looks |
| 2 | 25-02 | `plugin install` had **no path at all** for a Wayland-native plugin |
| 3 | 25-02 | `plugin remove` could not remove a marketplace install |
| 4 | 25-02 | Both shipped templates were unusable; their smoke tests skipped, so it looked green |
| 5 | 25-03 | `node probe` reported a healthy node OFFLINE (hardcoded far-end binary) |
| 6 | 25-03 | `node probe` refreshed the advertisement from the **controller's** backends |
| 7 | 25-04 | The local orphan scan could not see an orphan at all |
| 8 | 25-04 | The orphan scanner counted **itself** |
| 9 | 25-04 | The Windows scanner reported a **MEASURED ZERO** while an orphan ran |

Plus two self-passing gates found and closed inside this phase's own evidence: the
scanner-vs-manual comparison taken only where both sides were zero, and two ledger verdicts
that overstated what a run proved.
