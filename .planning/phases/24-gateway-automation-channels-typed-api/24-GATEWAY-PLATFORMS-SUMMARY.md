---
phase: 24-gateway-automation-channels-typed-api
criterion: "24-C5 (setup-to-recovery journeys) + GATEWAY-* platform-coverage cap"
lane: gateway-platforms
branch: lane/gateway-platforms
status: complete
merge-base: b2ddf113681647221dc9e5bbfc7de79b1da90b54
candidate-proved: 7c61079842346988f6d19d399b7d92b672dec680
driver-commit: 13361302e6ebc32e90f2c503dcd0630e498e6211
source-files-changed: 0
grade: "The GATEWAY-* platform-coverage cap is REMOVED as stated: all three OS families drive the 17-step journey against CI-built release binaries at ONE candidate commit, and `wayland-journey bind` — a gate never previously observed to pass — returns BOUND. One new Windows defect found: an intermittent duplicate delivery the headline counts cannot see."
---

# 24-GATEWAY-PLATFORMS — removing the cap

**One sentence: the ledger's stated reason for holding `GATEWAY-*` below REACHED
was false at `b2ddf113` before I started and is now false with three-platform
provenance behind it — `bind` reports `BOUND` across linux, macos and windows at
a single candidate — and the one thing that went wrong in the process was a
duplicate delivery on Windows that the receipt's own headline counts report as
`duplicates: 0`.**

Nothing here was merged, pushed to `main`, tagged, released, or used to close an
issue. No requirement is marked complete. **No source file was modified** — this
lane changed `.planning/` only (`git diff <merge-base> -- crates/ scripts/` is
empty), so it carries no shared-file fence exposure and no fmt/clippy surface.

---

## 1. My brief's premises — four were false before I began

LANE-BRIEF requires re-verifying brief measurements. Four of the load-bearing
claims in mine were true when the ledger row was written (2026-07-28) and had
been falsified by `lane/24-journey` and `lane/24-c5-finish`, both merged into
integration before `b2ddf113`.

| Brief / ledger claim | Verdict at `b2ddf113` |
|---|---|
| "Criterion 5's three-platform journey is **untouched**" | **FALSE** — `24-C5-FINISH-SUMMARY.md` grades it MET on all three |
| "24-04's four tasks were **never started**" | **FALSE by supersession** — true of `24-04-SUMMARY.md` itself, but the two C5 lanes executed them |
| "the macOS CI binary **provably does not carry this code**" | **FALSE** — measured below |
| "the Windows gateway path was **never exercised**" | **FALSE** — Task Scheduler registration and platform-restart already recorded |

**I did not manufacture work to match the brief.** The real open item was the one
`24-C5-FINISH-SUMMARY.md §6a` names itself: the three receipts sat at three
different commits, so `bind` refused them. That is what I closed.

### The ledger's own measurement, re-run

The row caps the family on `--help | grep -cE "^\s+gateway"` → **`0`** for macOS.
LANE-BRIEF §3b-i warns that this exact shape — a `grep -c` returning zero — is
what `rtk` was caught fabricating, so it was captured to a file and read with the
Read tool, with controls in the same invocation:

```
needle: gateway                     -> 1     (the ledger measured 0)
KNOWN-POSITIVE  needle: acp         -> 1     instrument alive
KNOWN-POSITIVE  needle: cron        -> 1     instrument alive
KNOWN-NEGATIVE  needle: zzznotareal -> 0     instrument can return zero
```

and the line itself:

```
  gateway   F24-B: the persistent gateway runtime — install / uninstall / start /
            stop / restart / status / drain, plus the `run` verb every generated
            launchd, systemd and scheduled-task unit invokes
```

`wayland-core gateway --help` then lists twelve working verbs. **The claim that
the macOS binary does not carry this code is false**, and the instrument that
would have reported a false zero is proved alive in both directions.

## 2. The inversion that made three platforms cheap

The previous lane pinned a candidate and then waited **45 minutes** on the darwin
runner pool for a binary, and stopped waiting. macOS is the only platform that
**cannot** be built on a permitted host (LANE-BRIEF §0 forbids a workspace build
on the Mac). So macOS must **choose** the candidate and the other two follow it,
not the reverse.

I enumerated CI artifacts and selected the newest commit that (a) is an ancestor
of my HEAD, (b) contains `978f49d7` — the Windows restart fix, without which the
Windows leg cannot recover — and (c) already has a complete five-target artifact
set. That is **`7c610798`**. All three legs then ran **CI-built release binaries**,
which is stronger provenance than locally-built ones, and no leg waited on CI.

Identity was established before any binary was trusted, three ways per platform:

| | macOS | Linux | Windows |
|---|---|---|---|
| artifact `head_sha` | `7c610798…` | `7c610798…` | `7c610798…` |
| binary's own `--build-info` | `7c610798…` | `7c610798…` | `7c610798…` |
| digest | `11e14834…` | `b3190b19…` | `0db1025c…` |

## 3. The three legs

All at candidate `7c610798`, driver `13361302`, each 17/17.

| | macOS | Linux | Windows |
|---|---|---|---|
| host | this Mac (arm64, macOS 26.3) | `hetzner-dsm` | `SeanD@seandesktop` |
| service manager | **launchd** | **systemd --user** | **Task Scheduler** |
| exit | `MACRC=0` | `LRC=0` | `WLRC=0` (status-file, read by a separate ssh call) |
| kill → recover | 50849 → **59271** | recorded | 48144 → **46392** |
| who restarted it | `launchctl`, `LastExitStatus = 9` | `systemctl` | `schtasks`, `Status: Running`, `Next Run Time` advancing |
| counts | 12/12/12, 0 dup, 0 loss | same | same |

**The journey issues no start command at step 12 on any platform** — the
platform's own supervisor brings it back, and each supervisor was queried for its
own account of the restart rather than the fact being inferred from the pid alone.

## 4. `bind` — a gate with no previously demonstrated pass state

LANE-BRIEF §3b-iii: a permanently-red gate proves as little as a permanently-green
one. **Every recorded invocation of `wayland-journey bind` in this phase returned
rc=1.** Its pass state had never been constructed, so it was not known to be
measuring anything.

It now passes:

```
BOUND commit=7c61079842346988f6d19d399b7d92b672dec680
      driver=13361302e6ebc32e90f2c503dcd0630e498e6211
      receipts=3 platforms=linux,macos,windows
      adapters=3/10 exercised=slack,sms,whatsapp
BIND_PASS_RC=0
```

**And it still fails when it should.** Swapping in a receipt from an earlier run
of mine:

```
wayland-journey: receipts disagree on the driver commit:
  ["13361302…", "24143f95…"]      BIND_FAILA_RC=1
```

`bind` earned its keep on the way: my first attempt failed **not** on the
candidate commit but on the **driver** commit, because I had committed evidence
between the three runs. Three journeys driven by three different drivers are not
one experiment, and nothing else I ran would have noticed. I re-drove all three
from worktrees pinned to one asserted driver SHA.

### `verify`, both directions, on all three receipts

Pass: rc=0 on all three, each with the digest the verifier computed **itself**
rather than the one the receipt asserts. Fail: rc=1 on wrong platform, wrong
commit, and a one-byte-appended binary (`binary digest mismatch`) — and, more
usefully, **rc=1 on real data**, see below.

**Running-pair count, per LANE-BRIEF (a skip is not a pass): 3 platforms × verify
= 3 pass cells + 4 refusal cells, plus bind 1 pass + 1 refusal. 9 cells, 0
skipped, 0 unrun.**

## 5. NEW FINDING — Windows delivers twice, and the headline says `duplicates: 0`

On my first complete Windows run the verifier **refused the receipt**:

```
wayland-journey: per-adapter arrived sums to 24 but the receipt's top-level
count is 12; the breakdown and the headline are describing different runs
```

Confirmed at the sink's own journal, independently of the receipt:

| | arrival lines | distinct texts | arrivals-per-text |
|---|---|---|---|
| Windows | 27 | 13 | **`{2: 12, 3: 1}`** |
| macOS | 13 | 13 | `{1: 13}` |

**Every one of the twelve deliveries arrived twice.** The time-ordering shows a
clean first pass at 02:17:55–02:18:02, then **all twelve again in one burst at
02:19:03** — the Task Scheduler `PT1M` repetition boundary (`Next Run Time:
9:19:00 AM`).

**It is not a lock failure.** I sampled `Get-Process wayland-core` every 5s
through a whole run: **`count` never exceeded 1** — the pid changed four times
(33692 → 53472 → 58324 → 47208) but two runtimes never coexisted. The gateway's
own ledger agrees: 27 **distinct** delivery ids, each settled exactly once,
`delivered: true`. So exactly-once *per delivery id* holds; what is doubled is
**delivery creation** — a restarted runtime re-fires cron jobs that already fired.

The severity is in the reporting, not only the behaviour: the receipt's headline
reads `arrived: 12, duplicates: 0, losses: 0`. **A criterion graded on the
headline would have called this run exactly-once.** Only the per-adapter
breakdown dissents, and only `verify` catches the disagreement. This is the
self-passing-count family, and it is why the tally cross-check earns its place.

**It is intermittent.** My second Windows run was clean (4/4/4, 13 arrivals, each
once) because it finished before crossing a repetition boundary. So the honest
statement is: *whenever a Windows run crosses the `PT1M` boundary with live cron
jobs, deliveries repeat.* The prior lane recorded the non-durable-`stop`
divergence as MEDIUM; **that this same mechanism causes re-delivery was not
recorded, and is the new part.**

I did **not** fix it. It is a product change in a Windows recovery mechanism
chosen deliberately by an earlier lane, my lane changed no source, and the fix
wants the owner of that decision. Filed below.

## 6. A defect in MY OWN instrument, repaired in-lane (§6b-ii)

My first Windows attempt launched the driver with `Start-Process -WindowStyle
Hidden` over ssh. Windows OpenSSH tears down the session's process tree on
disconnect, so **the node driver and the independent sink were both killed
seconds after launch**. The gateway survived — Task Scheduler owns it — kept
attempting deliveries, and wrote **52 records all `delivered:false`**.

**That wreckage is indistinguishable from a product delivery defect** by
inspection of the product's own state, and I was one step from reporting it as
one. It is LANE-BRIEF §6a-i exactly: an actor that is not running is a dead
instrument, and a result measured against it is not a negative result — it is not
the experiment. What exposed it was `Get-NetTCPConnection` on the sink port
returning nothing while the sink's log still said `SINK_READY`.

Repaired rather than noted: the runner now executes under Task Scheduler, which is
not parented to the ssh session, and **the poller asserts participant liveness on
every iteration** instead of only reading the product's opinion of the outcome.
Three assertions, per §6b-ii:

1. **known-positive** — the repaired run holds `NODES=2` and `SINKLISTEN=1`
   throughout and reaches `WLDONE`;
2. **known-negative** — `NODES=0` before `WLDONE` aborts the poll as
   `PARTICIPANT-DEAD`, and the sink port goes `1 → 0` at run end;
3. **the old harness would have missed it** — not hypothetical: it *did*, and
   reported a product-shaped failure.

I also caught a **second, smaller instrument defect of my own** while doing this:
my first poller hardcoded sink port `65071` from the previous run, but the sink
binds an **ephemeral** port, so `SINKLISTEN` was structurally `0` — a
permanently-red indicator of precisely the §3b-iii kind. It did not mislead me
only because the abort keyed on `NODES`. The poller now parses the port out of
the sink's own `SINK_READY` line, and that column then reads `1` during the run
and `0` after it.

## 7. Instrument discipline

`rtk` bit once here and is worth recording: **`wc -l < file` returned `0` for a
file with 25+ lines.** Consistent with the `--numstat` fabrication recorded on
2026-07-30. Every number in this document was written to a file and read with the
Read tool, never off a Bash stdout render. Every `/tmp` path on the shared hetzner
box is prefixed `lane-gateway-platforms-`.

Two other stale environmental claims, minor: `C:` on SeanDesktop now has **671 GB**
free, not 167 GB. I worked on `D:` regardless and touched no `C:\actions-runner-*`.

## 8. Honest grade

**The `GATEWAY-*` cap, as the ledger words it, is removed.** Its two stated
reasons — "the macOS CI binary provably does not carry this code" and "the Windows
gateway path was never exercised" — are both measured false, and the coverage is
now bound rather than merely asserted: three OS families, three real service
managers, one candidate, one driver, `BOUND`.

**I am not claiming the family is REACHED, and this lane does not settle that.**
Two of the row's other reasons are untouched by me and remain true: nine channel
adapters still inherit `supports_outbound_idempotency() == false`, and the peer
half of the comparison is still at the pinned baseline. Those are the grader's
call, not mine. What I can say is that the *platform-coverage* argument the row
rests on no longer holds.

**Against that, I found a real Windows defect that makes the exactly-once claim
conditional on timing** — and the phase's own headline counts cannot see it. A
grade of REACHED that cited three-platform exactly-once delivery would now be
overclaiming on Windows.

### Open, filed, not fixed

1. **F24-GWP-H1 (HIGH, new)** — Windows re-delivers every live cron job when a run
   crosses the Task Scheduler `PT1M` repetition boundary. Not a lock failure
   (`count` never > 1); a restarted runtime re-fires already-fired jobs. Evidence:
   `windows-duplicate-arrival-timeline.txt`, `windows-gateway-process-sample.txt`.
2. **F24-GWP-M1 (MEDIUM, new)** — the journey receipt's top-level `counts` report
   `duplicates: 0` for a run in which twelve deliveries each arrived twice. The
   headline cannot express the duplicate its own `adapter_coverage` records.
   Only `verify`'s cross-check dissents. → BACKLOG.
3. Pre-existing and unchanged: F24-J-M1 (Windows install needs elevation — my ssh
   session was already elevated, so it did not bite), and Windows `gateway stop`
   not being durable while registered.

### What I did NOT do

- No fix for either new finding — no source file was touched.
- **No workspace suite, no clippy, no `cargo test`** beyond building the
  `wayland-journey` verifier binary. My lane changed no Rust; running them would
  have measured other lanes' work under contention.
- Criterion 5 was already MET before me; I did not re-open or re-grade it. I
  closed the provenance gap it left and added a platform finding it did not have.
- Did not edit `COMPETITIVE-LEDGER.md` — the row is contended and re-grading is
  the orchestrator's. Recommendation only, in §8.
- macOS was driven against a **CI-built** binary. I did not build on the Mac and
  did not use the §0 Darwin single-test exception.

## Self-check

Every number was copied from a captured file read with the Read tool. The gates
that refuse are shown refusing, including one refusal on real data rather than a
synthetic mutation. The instrument defect that nearly produced a false product
finding is recorded as having happened, and repaired. The duplicate-delivery
finding is reported as intermittent because one of my two Windows runs was clean,
and both runs are in the evidence directory.
