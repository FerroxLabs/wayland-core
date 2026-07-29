# 30-DIALECT-C2 — running notes (append-only, committed continuously)

**Lane** `lane/30-dialect-c2`. **Base** `75babf329235484684ecee3a65973b0c197840c1`.
**Worktree** `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-30-dialect-c2`
(verified via `/usr/bin/git rev-parse --show-toplevel`; NOT the dirty `dev/waylandcore` checkout).

Per LANE-BRIEF §6b-i: this file is committed inside the first 15 minutes and re-committed after
every measurement. There is no partial credit for uncommitted reasoning.

---

## T+0 — orientation, established by reading, not yet by measuring

### What already exists at base (verified by `ls`, not by document claim)

| Artifact | Path | State |
|---|---|---|
| Dialect compiler | `crates/wcore-eval-scenarios/src/dialect.rs` | present, 63074 bytes |
| Discovery meter | `crates/wcore-eval-scenarios/src/dialect_discovery.rs` | present, 19346 bytes |
| Protocol v2 pre-registration | `.planning/phases/30-*/30-DIALECT-PROTOCOL-V2.md` | present |
| Prior lane report | `.planning/phases/30-*/30-DIALECT.md` | present |
| Evidence dirs | `.planning/phases/30-*/evidence/30-{01,02,03,04,dialect}` | present |

**So SR-30-3 is NOT greenfield for this lane.** A prior lane (`lane/30-dialect`, branched
@ `8bcb052b`) already built the compiler, registered protocol v2, and ran a 4/4 panel. My task
brief describes building it; the tree says it is built. **I must not re-build it and must not
assume the prior lane's framing is correct** — the brief explicitly warns that the last lane which
inherited a framing propagated an error into two planning documents.

### The claim I have to independently verify FIRST

`30-DIALECT.md §1` and `30-DIALECT-PROTOCOL-V2.md §0` both assert:

> the frozen F30-03 script emits a tool call named `write_file`; Hermes 30/30, Wayland Core 0/30,
> OpenClaw 0/30 on correctness and recovery; therefore all nine RUN legs are confounded.

**This is inherited framing. Not yet verified by me.** Open questions I must answer from the
30-02 evidence directory and the frozen protocol, not from the summary:

1. Is the tool name in the *frozen* v1 protocol actually `write_file`? (read
   `evidence/30-02/protocol.json`, digest `d18407e0b9…`)
2. Are there exactly nine RUN legs, and is each one actually confounded *by this mechanism*?
   Some legs may be confounded for a different reason, or not confounded at all — e.g. a cost leg
   measuring token spend might be confounded differently from a correctness leg, and the security
   legs are separately `UNPROVEN` for a meter reason (SR-30-1), which is NOT the dialect confound.
3. Does `0/30` for BOTH Wayland and OpenClaw actually follow from dialect, or is OpenClaw's 0/30 a
   different failure that happens to co-occur? Two products failing identically is consistent with
   dialect but does not prove it — a provisioning fault would look the same.

**Q3 is the one most likely to be wrong**, because "both failed, therefore same cause" is exactly
the inference this program keeps getting burned by.

### The blocker the prior lane named, which is what C2 is really for

Protocol v2 status is `REGISTERED, NOT EXECUTED`. Four preconditions, prior lane's status:
corpus for all 3 harnesses (**1 of 3**); `cohort_eligibility` = ELIGIBLE (**NO — COHORT_TOO_SMALL:1**);
translations compiled+verified (**done for Wayland only**); **peers re-provisioned at pins (NOT DONE)**.

Peers are gone from the build host. Pins: Hermes 0.17.0 `dbe734be…`, OpenClaw 2026.6.2 `11a0ad10…`.
Prior lane says Sean's reference checkouts live at `/Users/seandonahoe/dev/resources/` — **unverified
by me; next measurement.**

So the honest shape of this lane is likely: **the instrument is built; the cohort is not.** If I
cannot provision two peers, the position stays unstatable and my deliverable is the precise reason
plus whatever legs a 2-member cohort permits.

### The negative control the brief demands

A deliberately mis-compiled dialect must redden. Must check whether the existing 28 dialect tests
already contain one, or whether the suite only proves the happy path. Per LANE-BRIEF §3.2, I must
read back the executed count and NOT trust exit status; per §3b, `cargo` under `rtk` strips
`0 ignored` / `0 filtered out`, so every cargo invocation goes through `/usr/bin/env cargo`.

### Instrument discipline for this lane

- `/usr/bin/git`, `/usr/bin/grep` for anything load-bearing. (`/usr/bin/cat` does NOT exist on this
  Mac — measured, exit 127. Use the Read tool.)
- Any absence claim needs a known-positive in the same invocation + the query stated (§3b-i).
- Fence: `crates/wcore-cli/src/{lib,main}.rs` — diff against captured `BASE`, never branch name.

## T+0 — status

Nothing measured yet. Nothing built yet. Next: verify the confound claim against 30-02 evidence.
