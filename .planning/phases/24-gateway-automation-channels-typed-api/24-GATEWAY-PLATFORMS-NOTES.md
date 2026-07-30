# 24-GATEWAY-PLATFORMS — NOTES (live, appended per measurement)

Lane `gateway-platforms`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-gateway-platforms`,
branch `lane/gateway-platforms`, base `b2ddf113`.

Brief: remove the cap on `GATEWAY-*` (CONSTRUCTED, "widest measured gap") by
supplying the macOS and Windows gateway legs and Criterion 5's three-platform
journey.

---

## 1. Brief premise re-verification (LANE-BRIEF "your measurements are probably stale")

**Ran before doing any work. Four of the brief's load-bearing claims are FALSE at
`b2ddf113`.** They were true when the ledger row was written (2026-07-28) and were
falsified by `lane/24-journey` and `lane/24-c5-finish`, both since merged to
integration.

| Brief claim | Verdict at `b2ddf113` | Evidence |
|---|---|---|
| "Criterion 5's three-platform setup-to-recovery journey is **untouched**" | **FALSE** | `24-C5-JOURNEY-SUMMARY.md`, `24-C5-FINISH-SUMMARY.md` — C5 graded MET on all three platforms |
| "24-04's four tasks were **never started**" | **FALSE** (stale-by-supersession) | true of `24-04-SUMMARY.md` itself (§6, `plans-not-executed`), but the four tasks were then executed by the two C5 lanes |
| "the macOS CI binary **provably does not carry this code**" | **FALSE** | `24-C5-FINISH-SUMMARY.md §2` drove `gateway install/start/stop/uninstall` on a real Mac against artifact `eba6e9d7` |
| "the Windows gateway path was **never exercised**" | **FALSE** | `24-C5-FINISH-SUMMARY.md §1` — Task Scheduler registration, hard kill of pid 44096, platform-restart to pid 46028 |

Ancestry check (all four candidate commits are ancestors of my HEAD, so the
artifacts are in my tree, not in some unmerged lane):

```
978f49d7: ANCESTOR-of-HEAD    eba6e9d7: ANCESTOR-of-HEAD
d89b81b6: ANCESTOR-of-HEAD    c61cf808: ANCESTOR-of-HEAD
```

`scripts/f24-journey.mjs` (44.9K), `scripts/f24-sink.mjs` (8.4K) and
`crates/wcore-eval-scenarios/bin/wayland-journey.rs` are all present in the
worktree.

**Consequence: I am not building the journey. It exists. I am closing what it
left open.**

## 2. What is ACTUALLY open on GATEWAY-* at `b2ddf113`

From `24-C5-FINISH-SUMMARY.md §6`/`§7` "Open":

1. **`wayland-journey bind` is unsatisfied** — the three receipts sit at three
   different candidate commits (`978f49d7` Linux, `978f49d7` Windows,
   `eba6e9d7` macOS), so `bind` exits **rc=1**: *"receipts disagree on the
   candidate commit"*. This is the one named coverage-adjacent gap.
2. F24-J-M1 — Windows `gateway install` needs elevation; module docs claim it
   does not. → BACKLOG.
3. Windows `gateway stop` is not durable while the task is registered (the
   repetition trigger restarts it). macOS `KeepAlive` behaves the same way.
   → BACKLOG, MEDIUM.

### 2a. `bind` has NEVER been observed to PASS — this is a §3b-iii permanently-red gate

`bind` exists to make the same-candidate claim. Every recorded invocation of it
returns **rc=1**. Per LANE-BRIEF §3b-iii, a gate with no demonstrated pass state
proves as little as one with no fail state, and the fail direction here is
already measured. **So the target is: construct the world in which `bind`
passes, and confirm it goes green.** If it cannot be made to pass, `bind` is not
measuring the criterion and that is the finding.

**Both directions required:** fail is measured (disagreeing trio, rc=1). Pass is
not. That asymmetry is the work.

## 3. Plan

Pick ONE candidate commit for which a `aarch64-apple-darwin` artifact is
obtainable, then re-drive all three platforms at it. Linux (hetzner) and Windows
(seandesktop) can build from source; macOS cannot (LANE-BRIEF §0 forbids a
workspace build on the Mac), so **macOS constrains the candidate choice** and the
other two follow it — not the reverse. That is the inversion the previous lane
did not take: it pinned a candidate and then waited 45 minutes on the darwin
runner pool.

## 4. Instrument discipline for this lane

Every number reported is captured to a file under
`.planning/phases/24-*/evidence/24-gateway-platforms/` or `/tmp/lane-gateway-platforms-*`
and read back with the Read tool, never through Bash — `rtk` fabricated a
`git diff --numstat` count and a `grep -c` zero on 2026-07-30 even through
absolute paths. `/tmp` on hetzner is shared: every path I write there is
prefixed `lane-gateway-platforms-`.
