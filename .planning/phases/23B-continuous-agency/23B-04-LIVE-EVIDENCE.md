# 23B-04 — live evidence, IN PROGRESS

**Status: the multi-day journey is RUNNING and has not finished.** This file
records what has actually happened so far and is deliberately incomplete. It
does not carry a verdict for Success Criterion 5, a disposition for F23-05, or
any phase-level statement — Task 3 is unstarted by design, and a successor
completes both once the span has elapsed and 23B-03 has landed.

Nothing below is a claim about a day that has not happened.

---

## The pinned journey SHA

```
0ed05322462e64cb44e2b80aa15b7357263b8187
```

Every journey invocation asserts the driving binary's own `--build-info` source
SHA equals this value before doing anything, so the span cannot silently switch
binaries mid-flight. Proved able to fail: a deliberately wrong `--sha` exits
`68` with `binary source SHA '0ed05322…' != commit under test 'deadbeef…'`.

## Authorized clock policy

`real-time-full` — see `23B-04-CLOCK-DECISION.md`. No leg may be accelerated.
All three platforms carry `*_required_real_span_seconds=259200`. No platform
records a zero span, so no `weaker_claim_<platform>=acknowledged` line exists.

---

## Per-platform status

| Platform | Host | Day 1 recorded | Days 2–3 | Verify | Status |
|---|---|---|---|---|---|
| **linux** | `hetzner-dsm` | **2026-07-27T14:21:19Z** | scheduled | not yet due | **RUNNING** |
| **windows** | `SeanDesktop` | not yet | — | — | build in flight at hand-off |
| **macos** | this Mac | **not started** | — | — | **OPEN — blocked, blocker named below** |

**The earliest moment any platform's journey may be closed is
`2026-07-30T14:21:19Z`** (Linux day one plus the authorized 259,200 seconds).
A `--verify` run before then exits `72` and prints
`F23_04_SPAN_MEETS_AUTHORIZED_POLICY=false`. That is not a bug to work around;
it is the gate doing its job.

---

## Linux — day one, verbatim from the append-only run log

`/root/.f23-journey-linux/runlog.txt` on `hetzner-dsm`:

```
# ---- invocation day=1 platform=linux ts=2026-07-27T14:21:19Z host=Ubuntu-2404-noble-amd64-base pid=3798295 sha=0ed05322462e64cb44e2b80aa15b7357263b8187 rc=0
F23_04_DAY=1 platform=linux ts=2026-07-27T14:21:19Z host=Ubuntu-2404-noble-amd64-base pid=3798417
F23_04_INVARIANT=loop-owner platform=linux day=1 status=PASS
F23_04_INVARIANT=cumulative-budget platform=linux day=1 status=PASS
F23_04_INVARIANT=authority-envelope platform=linux day=1 status=PASS
F23_04_INVARIANT=memory-recall platform=linux day=1 status=PASS
F23_04_INVARIANT=evidence-chain platform=linux day=1 status=PASS
F23_04_INVARIANT=delivery-once platform=linux day=1 status=PASS
F23_04_LOOP_OWNERS_OBSERVED=1
F23_04_GOAL_LIFECYCLE=Waiting { wait: Event { event: "f23-span-elapsed-259200s" } }
F23_04_JOURNAL_CURSOR=seq=7 checksum=7ae2ddc938274ee2a2307b1a175a41913c13de428cc61f69e783c3b36f1c094c
F23_04_DAY_INVARIANTS_ALL_PASS=true
F23_04_STEP=OK day=1 platform=linux nonce=4ab44763e42849fd
```

The process exited. `pgrep -f multi_day_journey_test` on the host returns
nothing, so between day one and day two no process of this journey exists —
which is the property the whole leg is for.

Journey nonce: `4ab44763e42849fd`, also at
`/root/.f23-journey-linux-nonce.txt`.

### The resume days are scheduled, not left to be remembered

```
NEXT                          LEFT    UNIT                     ACTIVATES
Tue 2026-07-28 14:25:00 UTC   24h     f23-journey-day2.timer   f23-journey-day2.service
Thu 2026-07-30 14:31:00 UTC   3 days  f23-journey-day3.timer   f23-journey-day3.service
```

Both call `/root/f23-journey-day.sh <n>`, which invokes the committed driver
with the pinned SHA, nonce and harness. Each day is idempotent: a second
invocation on a day already recorded prints `F23_04_DAY_ALREADY_RECORDED` and
exits zero without double-counting. If a timer misses, a successor may run the
day by hand with the same command and lose nothing.

Day 3 fires at `14:31 UTC`, ten minutes past the `14:21:19Z` threshold, so the
wait condition is genuinely met rather than marginally met.

---

## macOS — OPEN, and precisely why

The plan says: "If `scripts/f23-macos-binary.sh` is absent because 23B-01 did
not land it, STOP and record that as a blocking dependency rather than
improvising a second resolver." It is absent — `test -f` exits 1, and
`git ls-files` finds no macOS binary resolver on the branch. 23B-01's and
23B-02's summaries both record that they did not write it and escalated the
Cargo-on-the-Mac conflict rather than resolving it unilaterally. So this lane
stopped, as instructed.

**Two corrections to the plan's own reasoning, both measured on this tree.**

1. **The plan's claim that CI produces no binary is out of date.**
   `23B-04-PLAN.md` states `.github/workflows/ci.yml:204-208` "uploads only
   `nextest-junit-${{ matrix.os }}` — JUnit XML, no binary of any kind, on any
   branch". That is no longer true. `ci.yml:484-491` is an `Upload release
   binary` step publishing `wayland-core-${{ matrix.target }}` for six targets
   including `aarch64-apple-darwin` and `x86_64-apple-darwin`, added precisely
   so "the arm64 macOS leg, where the planning Mac is forbidden from running
   Cargo" is not structurally unreachable. `.planning/intel/MACOS-BINARY-IS-OBTAINABLE.md`
   documents the route. Measured further: the `build` job at `ci.yml:403` has no
   `needs:`, so the pre-existing `Check Desktop protocol contract corpus drift`
   failure in the `ci` job does **not** block the binary artifact.

2. **The macOS leg is nevertheless still blocked, on the HARNESS, not the
   binary.** This journey's Goal, budget-authority and delivery legs run through
   the compiled `multi_day_journey_test` binary, because — as measured in this
   plan's own probe and stated by the product's own
   `crates/wcore-agent/examples/p22_goal_live.rs` — the shipped `wayland-core`
   binary has **no Goal surface at all** and no way to construct an absolute
   deadline. CI uploads the product binary and does not upload test binaries.
   Building the harness on the Mac requires Cargo, which the phase's controlling
   instruction forbids. So the intel document solves a real problem that is not
   this leg's problem.

**The route a successor could take, stated so the choice is available rather
than assumed:** add a step to `ci.yml`'s `build` job uploading the output of
`cargo test -p wcore-agent --test multi_day_journey_test --no-run
--message-format=json`, and add the lane branch to `push.branches`. Both edits
are OUTSIDE this plan's declared `files_modified` and `ci.yml` is touched by
every lane, so this lane did not make them. It is a real, cheap route and it is
someone's call, not an improvisation this lane should have taken quietly.

**Consequence for the criterion, stated plainly:** because the authorized policy
is `real-time-full` with a 259,200-second threshold and macOS day one has not
started, **the macOS leg cannot meet its threshold before `2026-07-30`.** If
macOS is to be claimed at all, its day one must start at least three real days
before the phase closes. Recording it OPEN under termination state 2 is the
honest alternative, and it is what this file currently records.

---

## What has NOT been done, and is not claimed

- **No Success Criterion 5 verdict.** Two of three platforms have not finished a
  journey and the third has recorded one day of three.
- **No F23-05 disposition.**
- **No phase-level statement, no aggregate proof, no D2 record.** Task 3 is
  deliberately unstarted; it depends on 23B-03, which was still being built by a
  concurrent lane when this lane ran.
- **No claim that the committed regression test is the multi-day evidence.** It
  is not, and the file itself says so at the top. `multi_day_journey_invariants_accelerated`
  runs the whole cycle inside one process against real on-disk state at a
  compressed span. It never dies as a process and it never elapses days. The
  multi-day evidence is the run log's own first and last timestamps and nothing
  else.

## Substrate limitation worth naming before someone over-reads `memory-recall`

The `memory-recall` invariant is observed over the **durable session journal's**
reduced conversation state — a fact written on day one and re-read on every
later day from disk after the writing process stopped existing. It is **not**
observed over `wcore-memory`'s SQLite store, and 23B-02's `/memory` controls are
not exercised by this journey. The invariant proves durable recall across a real
restart; it does not prove 23B-02's memory subsystem specifically. Recorded as
finding 23B-04-M2.
