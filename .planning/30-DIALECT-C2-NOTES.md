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

---

## T+1 — MEASUREMENT 1: the inherited "all nine legs are confounded" framing is WRONG in part

Read from `evidence/30-02/{legs.tsv,protocol.json,records/*.jsonl}` — the raw records, not the
summary. Distributions computed with python3 over all 9 record files (n read back per file).

### 1a. What the raw records actually say

| record | n | outcome | fixture_requests | token_units | violations |
|---|---|---|---|---|---|
| hermes-correctness | 30 | SUCCESS 30 | 2 | 20 | none |
| hermes-recovery | 30 | SUCCESS 30 | **3** | **30** | none |
| hermes-cost | 15 | SUCCESS 15 | 2 | 20 | none |
| wayland-correctness | 30 | FAILURE 30 | 2 | 20 | none |
| wayland-recovery | 30 | FAILURE 30 | **3** | **30** | none |
| wayland-cost | 15 | FAILURE 15 | 2 | 20 | none |
| openclaw-correctness | 30 | FAILURE 30 | 2 | 20 | none |
| openclaw-recovery | 30 | FAILURE 30 | **3** | **30** | none |
| openclaw-cost | 15 | FAILURE 15 | 2 | 20 | none |

### 1b. FINDING A — Wayland DID recover from the injected 503. The record proves it.

`dimension_specs.recovery` requires TWO things: the fault was actually served, AND the workspace
reaches the oracle state. The recovery script is `[http_error 503, tool_call, text]` and the meter
is FIFO-cursored, so **request count is a direct readout of how far down the script the harness
walked**. All three harnesses made **exactly 3 requests / 30 token units** — i.e. every one of them
took the 503, retried, consumed the tool-call turn, and came back for the final text turn.

So Wayland Core's published **0/30 on recovery is not a recovery failure at all.** The retry-after-
fault half of the definition succeeded 30/30; the leg scores zero solely on the artifact half, which
is downstream of a tool name Wayland does not expose. Same for OpenClaw.

This is favourable to Wayland and it is still only worth what it is worth: it demonstrates HTTP-level
fault retry, which is a proper subset of the dimension as pre-registered. **It does not license
"Wayland recovers 30/30."** The pre-registered observable is conjunctive and it was not met.

### 1c. FINDING B — the three cost legs are NOT dialect-confounded, and v2 does not repair them

`dimension_specs.cost.observable` = `synthetic_token_units_per_attempted_trial`, defined as the sum
over **every usage frame the fixture emitted**, explicitly *including trials that ultimately failed*.
The fixture emits a fixed script. Therefore:

- cost is **invariant for any harness that follows the script** — measured: 20/20/20 on correctness-
  shaped runs and 30/30/30 on recovery-shaped runs, across three harnesses, with zero variance;
- Wayland's FAILURE trials and Hermes' SUCCESS trials report the **identical** 2 requests / 20 units.
  The metric cannot distinguish a harness that did the work from one that did nothing;
- the only way a harness can register a different cost is by deviating from the script — and
  deviation is separately scored as an `unexpected_request` violation, not as higher cost.

**So the dimension has resolution only in the region the protocol treats as invalid.**

Now the consequence that matters for this lane: **protocol v2 does not touch this.** v2 changes the
tool *name* in the script (`write_file` → `Intent::WriteFile` → each harness's own tool). It does not
change the number of script steps. After a perfect dialect compilation, Wayland executes `Write`,
succeeds on correctness — and still reports 20 units. **The cost comparative comes back
`PRACTICALLY_INDISTINGUISHABLE` for exactly the same non-reason.**

### 1d. The correction to the inherited framing, stated as a table

Inherited (30-DIALECT.md §1, 30-DIALECT-PROTOCOL-V2.md §0, and MILESTONE-RC §6):
*"all nine RUN legs are confounded [because the script spoke one competitor's dialect]"*.

Measured — nine RUN legs, **two** dispositions, not one:

| legs | dimension | disposition | repaired by v2? |
|---|---|---|---|
| 01,02,06,07,11,12 (6) | correctness, recovery | **dialect-confounded** — inherited framing correct | **YES** |
| 04,09,14 (3) | cost | **degenerate by construction** — observable is defined on the fixture's own scripted emissions | **NO** |

### 1e. Why this is a defect in the v2 pre-registration, not just a wording nit

Absence claim, with its query stated and a known-positive in the same invocation
(LANE-BRIEF §3b-i), run with `/usr/bin/grep`:

- known-positive: `grep -c -i cost` → `30-DIALECT.md:6`, `30-DIALECT-PROTOCOL-V2.md:3` — instrument alive.
- known-positive control: `grep -n -E 'stays (UNPROVEN|open)|NOT_MEASURED|does NOT fix'` → 4 hits,
  correctly locating §3 "What v2 does NOT fix" and the LIM-19 / cognitive-tax entries.
- the query: every occurrence of `cost` in both documents, read individually (9 hits total).
  **Not one of them names cost as a dimension v2 fails to repair.** Six of the nine are the English
  word "costs"/"cost" in prose about transfer size or the G6 gate.

Meanwhile `30-DIALECT-PROTOCOL-V2.md` §2 inherits, byte-for-byte and by name, *"trial counts
30/30/30/15/0"* and *"the cost non-inferiority guard"* — i.e. **v2 re-registers cost as a live,
repairable dimension.** Its §3 "What v2 does NOT fix" lists security, FIFO, the vendor-authored
token tables, the compiler blind spots and cognitive tax. Cost is absent from that list.

**So a v2 execution, run exactly as registered, would publish a cost comparative that means nothing,
under a pre-registration that does not warn the reader.** That is the same class of defect as v1's
dialect bug: an instrument limitation the protocol does not price. Recording it now, before any leg
is run, is the only moment at which recording it is not an amendment-after-measurement.

### 1f. Peer availability — SR-30-6 may be closable after all

`/usr/bin/git cat-file -t` against Sean's Mac reference checkouts:

| peer | checkout | pin | present? |
|---|---|---|---|
| Hermes | `/Users/seandonahoe/dev/resources/hermes-agent` @ `d59b79fa` | `dbe734be` | **YES — `commit`** |
| OpenClaw | `/Users/seandonahoe/dev/resources/openclaw` @ `3659c85e` | `11a0ad10` | **YES — `commit`** |

Both working trees clean (0 porcelain lines). So the pins are obtainable without Sean and without a
network fetch from a vendor. The prior lane's blocker was that the *build host* copies were deleted,
not that the pins were lost.

## T+1 — status

Confound analysis done and it corrects the inherited framing on 3 of 9 legs. Next: (a) audit the
compiler's own test suite for a real negative control, (b) attempt peer provisioning at the pins.
