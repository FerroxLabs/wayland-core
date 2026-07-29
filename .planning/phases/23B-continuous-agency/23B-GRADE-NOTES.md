# 23B GRADE NOTES — running record (lane `grade-23b`)

Started 2026-07-29. Base `861d1b1a`. Worktree `.../lane-grade-23b`, branch `lane/grade-23b`.
Purpose: produce `23B-PHASE-VERDICT.md` — Phase 23's six Success Criteria graded against
evidence already in the tree. **Append after every measurement. Do not batch to the end.**

## Instrument discipline for this lane

- All load-bearing reads via `/usr/bin/git`, `/usr/bin/grep`, `/usr/bin/wc`, `/usr/bin/find`.
  `rtk` rewrites `git log`, `grep`, `cargo`, `wc -c` — measured, brief §3b.
- Any absence claim needs a known-positive in the same invocation (brief §3b-i).
- `cargo nextest` "flakiness" in this repo was fd exhaustion — 40 runs, 0 real failures.
  `.config/nextest.toml`'s `no-tests = "fail"` is silently ignored by the installed nextest,
  so a green suite may have run nothing. Downgrade confidence wherever evidence rests on either.
- Re-derive all arithmetic. Do not inherit a prior verdict's counts.

## The six criteria (from `.planning/ROADMAP.md` §Phase 23, lines 101-107)

1. Generated skills cannot execute before governed promotion; observable, revocable, rollback-able.
2. Users can search, inspect, checkpoint, retry, fork, rewind, export, retain, reconcile session effects.
3. Users can see/control memory/user-model activation, provenance, correction, forgetting, privacy,
   retention, nudges.
4. Cache and compaction expose quality, invalidation, token-pressure, cost truth.
5. Multi-day wait/resume/complete journey preserves cumulative authority, resource, evidence,
   memory, delivery state.
6. Persistent incremental hybrid repository index — bounded lexical/symbol/optional-semantic
   retrieval with provenance, staleness, privacy, performance truth.

Internal order (ROADMAP line 109): **23A owns criterion 1**; 23B owns 2-6. Criterion 1 is graded
here only from what is in-tree, and is flagged as a sibling lane's authority.

## Inventory taken (measured, not claimed)

`.planning/phases/23B-continuous-agency/` — 20 markdown files, 5036 lines; `evidence/` 39 files,
2467 lines. Four plans (23B-01..04), four SUMMARYs, three LIVE-EVIDENCE files, two phase
dispositions (rev1 + rev2), plus a five-file 23B-H1 record.

**No `23B-PHASE-VERDICT.md` exists.** Confirmed by directory listing above (the listing is the
known-positive: 20 other `.md` files were returned by the same `wc -l *.md` invocation).

## Prior claims to VERIFY, not inherit

- `23B-PHASE-DISPOSITION-v2.md` grades: C2 PARTIAL, C3 PARTIAL/NOT MET, C4 NOT MET,
  C5 NOT MET, C6 NOT MET. Its own header says the C6/C5 rows are **superseded** — 23B-03 and
  23B-04 have since executed. So rev2's body is stale on exactly the two criteria it calls
  unexecuted. Grade from the SUMMARYs + evidence logs, not from rev2.
- `F23-04` (criterion 4) reportedly NEVER STARTED — criterion 4 depends on it entirely.
- `23B-04` reportedly day-one-only; macOS nothing run; blocked until 2026-07-30T23:54:26Z.
- `BL-23B-H1` re-graded MEDIUM after 92 runs / 153 tool events / 0 mismatches. Its **earlier**
  evidence was worthless in both directions (dead port + placeholder key ⇒ no tool event ever
  dispatched; non-reaching runs counted as successes). Treat the re-grade's instrument with the
  same suspicion applied to the original.

## Open questions at this point

- Does 23B-03's "all three platform legs PASS" survive reading the actual drive logs, given the
  macOS-cargo-on-Mac prohibition that rev2 says blocked every macOS row?
- Is there ANY F23-04 artifact (cache/compaction cost truth) in `crates/`?
- Does the 23B-H1 re-grade harness have a known-positive proving it dispatches tool events?

_(Appended below as measurements land.)_

---

## MEASUREMENT LOG

### M1 — artifact existence (one invocation, 18 PRESENT / 4 ABSENT — instrument proven alive)

PRESENT with line counts: `session_lifecycle.rs` 1297, `session_cmd.rs` 499,
`session_operator_lifecycle.rs` 534, `f23-session-operator-drive.sh` 356,
`provenance.rs` 989, `v6_recall_control.sql` 35, repomap `store.rs` 1005 / `scope.rs` 404 /
`search.rs` 543, `index_cmd.rs` 201, `incremental_index.rs` 675, `retrieval_quality.rs` 386,
`f23-index-drive.sh` 514, `f23-index-drive.ps1` 473, `multi_day_journey_test.rs` 1018,
`f23-clock-probe.sh` 296, `f23-multi-day-journey.sh` 236 / `.ps1` 204.

ABSENT: `scripts/f23-macos-binary.sh`, `scripts/f23-context-economics-drive.sh`,
`crates/wcore-cli/tests/memory_control_lifecycle.rs`,
`crates/wcore-agent/tests/context_economics_test.rs`.

**The ABSENT set matches exactly what the SUMMARYs said was not built.** No summary claimed a
file that is missing. That is a point in favour of the narratives' honesty — measured, not assumed.

### M2 — F23-04 (criterion 4) has no artifact. Absence measured with a live instrument.

- `/usr/bin/grep -rln "F23_04\|F23-04" crates/ scripts/` → 5 files, and **all five are plan
  23B-04's multi-day-journey files** (plan number collision: plan `23B-04` implements requirement
  **F23-05**, not F23-04). Zero cache/compaction artifacts.
- Concept search (quoted globs, per §3b-i): `invalidation_cause|InvalidationCause|token_pressure|
  TokenPressure|cost_reconcil|CostReconcil|compaction_quality|CompactionQuality` over
  `crates/ --include=*.rs` → **20 hits, all in `crates/wcore-providers/src/cache_observation.rs`
  (19) + `lib.rs` (1)**. Known-positive in the same flags: `CacheDiagnostics` → 0, so the type is
  named otherwise; the 20-hit result is itself the proof the instrument was alive.
  **First attempt returned 0 because zsh ate the unquoted `--include=*.rs` — the exact trap
  §3b-i names. The 0 was discarded, not reported.**
- `cache_observation.rs` (278 lines) last touched by `38736654` (PR #186, cache-health
  **telemetry**) and `da5a18b5`; `cache_diagnostics.rs` last touched `58e64fc6` (2026-07-15,
  pre-23B); `compact/state.rs` last touched `2c70b7b8` (PR #65). Known-positive on the same
  `git log --` instrument: `provenance.rs` correctly shows the three 23B-02 commits.
- Only **2** references to `cache_observation`/`TokenPressure` across `wcore-agent` + `wcore-cli`.

**Conclusion (C4): no work exists. Criterion 4 is NOT MET, and the honest cause is that plan
23B-02 Task 2 was never started — which its own SUMMARY states.**

### M3 — index legs (C6) ran at TWO different SHAs. Re-derived, and it is benign.

- Linux `F23_03_PROVENANCE=ok sha=b33827d3`, macOS + Windows `sha=1eb2d7c2`.
- `git merge-base --is-ancestor b33827d3 1eb2d7c2` → YES.
- `git diff --stat b33827d3 1eb2d7c2 -- crates/wcore-repomap crates/wcore-cli/src/index_cmd.rs
  scripts/f23-index-drive.{sh,ps1}` → **product code IDENTICAL**; only `f23-index-drive.ps1`
  (+473, new) and `f23-index-drive.sh` (11 lines).
- The 11-line `.sh` delta is the **field-extractor repair** (anchored `sed` returned EMPTY for the
  FIRST field on a line). **So the Linux leg ran the broken extractor** — its log shows
  `F23_03_VERIFY=agrees= exit=6` (empty), while macOS/Windows show `agrees=false`.
- **Does that vacate a Linux gate? No — checked, not assumed.** `field verify agrees` is used at
  `f23-index-drive.sh:460` in an `echo` only; the verify gate at :461 reads `VERIFY_RC`. Every
  *asserted* extraction (`records`, `symbols`, `read`, `extracted`, mutation fields at :378-383,
  `search fallback` at :308) returned non-empty real values on Linux, so none was first-on-line.
  Cost of the defect: one informational field. **Confidence downgrade: minor, not material.**
- Platform identity is corroborated by paths that cannot be cross-faked: macOS
  `/private/var/folders/8h/…`, Windows `//?/C:/Users/seand/…`, Linux `/root/wayland-23B-03`.
- Secret-isolation is a known-negative (`STORE_NONCE_OCCURRENCES=0`) **and carries its own
  known-positive in the same run** (`STORE_CONTROL_OCCURRENCES=1`). This is the discipline §3b-i
  demands, and 23B-03 is the only plan in the phase that supplied it unprompted.

### M4 — **NEW LIVE FINDING (not in any artifact): the Linux journey leg is DEAD at day 2.**

Measured on `hetzner-dsm` 2026-07-29, by reading the live journey state the phase's own handoff
tells a successor to check. Nothing in the tree records this.

```
/root/.f23-journey-linux/scheduled.log
  scheduled day 1 exited 0 at 2026-07-27T14:40:03Z
  scheduled day 2 exited 1 at 2026-07-28T14:25:00Z     <-- FAILED
grep -c "F23_04_DAY=" /root/.f23-journey-linux/runlog.txt  ->  1
/root/.f23-journey-linux/scheduled-day2.log
  scripts/f23-multi-day-journey.sh: line 67: HOME: unbound variable
```

**Root cause, proved not asserted.** `scripts/f23-multi-day-journey.sh:28` sets `set -uo pipefail`;
line 67 is `[ -n "$ROOT" ] || ROOT="$HOME/.f23-journey-$PLATFORM"`. A systemd transient service
does not export `HOME`, so `set -u` aborts before any work. `/root/f23-journey-day.sh` does **not**
pass `--root`.

Reproduced three ways, with a known-positive in each pair:
- On the Mac, isolated: `env -i bash` → `line 6: HOME: unbound variable`, rc=1 (matches the
  observed `exited 1`); `env -i HOME=/root` → rc=0; `env -i ROOT=…` → rc=0. **Three assertions:
  known-negative fails, known-positive passes, and the proposed fix passes.**
- **On the actual host under the actual mechanism:** `systemd-run --wait --pipe` running line 67
  verbatim → `/bin/bash: line 1: HOME: unbound variable`; the identical code in an ssh shell →
  `ROOT=/root/.f23-journey-linux`. (The pipeline's `rc=0` there is ssh's, not the inner status —
  the error text is the evidence, not the rc.)

**This is the same defect class the lane DID guard on Windows and did not on Linux.** 23B-04's
TRAP 4 documents that a SYSTEM scheduled task's `USERPROFILE` differs, and the Windows resume
therefore passes `-Root` explicitly. The Linux day script passes no `--root` at all.

**It will recur on 2026-07-30 14:31 UTC.** `f23-journey-day3.timer` is still armed
(`systemctl list-timers` → `Thu 2026-07-30 14:31:00 UTC`) and calls the same unguarded script,
so day 3 will fail identically. The day-2 transient unit is gone (transient units do not persist);
day 3's `/run/systemd/transient/f23-journey-day3.service` still exists and still runs
`/root/f23-journey-day.sh 3`.

Nothing is lost: the pinned binaries survive (`target/release/wayland-core`,
`multi_day_journey_test-cd7922f357e39e40`), the worktree is still detached at the pinned SHA
`0ed05322`, and `/root/.f23-journey-linux/` retains `day-one.json` and `journey.journal`.
The leg is **recoverable**, not destroyed.

**I did NOT repair it.** My dispatch fences this lane to grading, not building, and injecting a
day-2 row under another lane's nonce would contaminate the evidence chain it belongs to. The
repair is one line (`--root /root/.f23-journey-linux` in `/root/f23-journey-day.sh`, or
`Environment=HOME=/root` in the units) and is in the gap list.

### M5 — **Windows day 2 LANDED.** The inherited "day one only" is wrong in Windows' favour.

```
C:\Users\seand\.f23-journey-windows\scheduled.log
  scheduled day 1 exited 0 at Tue 07/28/2026  6:55:13
  scheduled day 2 exited 0 at Wed 07/29/2026  6:58:18
runlog day rows = 2
  F23_04_DAY=1 platform=windows ts=2026-07-27T23:54:26Z host=SEANDESKTOP pid=39008
  F23_04_DAY=2 platform=windows ts=2026-07-28T23:58:17Z host=SEANDESKTOP pid=28584
schtasks f23win23B04day3 -> Status Ready, Next Run Time 7/31/2026 7:05:00 AM
```

So as of now: **Windows 2 of 3 and on track; Linux 1 of 3 and broken; macOS 0 of 3.**
The brief's "day one only" was correct when written and is now correct only for Linux and macOS.
This is exactly why the standing rule is to re-derive rather than inherit.

### M6 — **C2: the acceptance run 23B-01 cites is NOT in the tree.** Its retained log FAILED.

`23B-01-LIVE-EVIDENCE.md:18-22` tabulates fifteen PASS rows for run nonce `446156892e72cf2a`
(plus prior confirming runs `c7ac3ec01c882827`, `e7ee1a9bb0aaf5d8`) and says
"Full captured log: `evidence/23B-01-linux-drive.log`".

`evidence/23B-01-linux-drive.log` carries nonce **`f12f77104c3ca039`** and ends:

```
F23_01_DRIVE=FAIL platform=linux nonce=f12f77104c3ca039 failures=4
```

with `show`, `retry`, `export` and `reconcile` all dying on
`journal checksum mismatch at sequence 16` — i.e. 23B-H1.

Searched, with known-positives in the same invocation:

| nonce | files in `evidence/` | files in whole phase dir |
|---|---|---|
| `446156892e72cf2a` (cited) | **0** | 1 (the citing prose itself) |
| `c7ac3ec01c882827` (cited) | **0** | 1 |
| `e7ee1a9bb0aaf5d8` (cited) | **0** | 1 |
| `f12f77104c3ca039` *(control)* | **1** | — |
| `c3ebab28a4160e31` *(control)* | **1** | — |

**But criterion 2 is not thereby unevidenced** — the later re-drive IS retained and IS clean:
`evidence/23B-02-linux-operator-drive.log`, nonce `c3ebab28a4160e31`, sha `cd021a01`, all 15
verbs PASS, `F23_01_DRIVE=PASS`. And its provenance is *better* than 23B-01's own:

- `git ls-tree cd021a01 --` → `session_lifecycle.rs`, `session_cmd.rs`, `provenance.rs` all present.
- `git ls-tree 15971d1b --` (23B-01's binary sha) → **none of the three**, confirming 23B-01's
  own stated rsync caveat that its `--build-info` attests only to the base tree.
  Known-positive: `session.rs` IS at `15971d1b`, so the instrument discriminates.

**So C2's usable Linux evidence is the 23B-02 re-drive, and the verdict must cite that, not the
row-table 23B-01 published.** Grading conclusion unchanged in direction; provenance corrected.

### M7 — C3: the missing acceptance mechanism confirmed independently.

- `/memory why|correct|forget|privacy|retention` ARE wired: `slash/memory.rs:54-58` dispatch and
  `:97,160,188,204,253` implement.
- `received_requests` (the outbound-provider-body probe the plan is built around) in
  `crates/wcore-memory` + `crates/wcore-agent/src/slash` → **0 files**. Known-positive, same
  query in `crates/wcore-providers` → **7 files**. Instrument alive; the proof genuinely absent.
- `NudgeBudget` appears only in `wcore-memory/src/lib.rs` and `provenance.rs` — **no CLI, slash or
  TUI surface**, so the nudge bound is not a control a user can reach. Matches the SUMMARY.

### M8 — BL-23B-H1's re-grade arithmetic RE-DERIVED from the raw logs, and it holds.

Counted by me over `evidence/23B-H1-measure/*.log`, not read from the write-up:

```
F23_H1_RUN=            92          F23_H1_REACH=              92
sum(tool_events)      153          runs with tool_events=0     0
status=OK              92          status=CHECKSUM_MISMATCH    0
file_written=no         0          distinct binaries           3 (by sha256_16)
```

**The harness is genuinely reach-proven, and unusually well built.** Each `F23_H1_REACH` line
carries a real `tool_events=` count from `count_tool_events "$JOURNAL"` plus `file_written=yes`,
so a non-reaching run cannot be folded into a pass — the exact defect that made the *earlier*
evidence worthless. `scripts/f23-h1-repro-live.sh:112-117` carries a self-test with a
known-positive, a known-negative (`tool_events=0 file_written=no`), **and `_sum_old`, which proves
the old broken matcher would have missed it** — the three-assertion form §6b-ii demands, which
almost nothing else in this program supplies. Lines 279-285 record an instrument defect
(`grep -o`'s `.` not matching the `n` in `tool_events`) found AND repaired in the same lane.

**I endorse the MEDIUM re-grade.** It remains a non-reproduction, not a disproof — root cause
unidentified — and the residual the backlog names is real and I saw it happen: in
`23B-01-linux-drive.log`, ONE checksum mismatch took down **4 of 15** operator verbs at once,
because all twelve `session` verbs read the journal and no repair path exists.

### M9 — C1 is 23A's, and 23A itself grades it PARTIAL.

`.planning/phases/23A-governed-skills/23A-C1-SUMMARY.md:4` — "the criterion moves from NOT MET to
PARTIAL". Clause table (:20-23): observed MET, revoked MET, rolled back MET, cannot-execute-before-
promotion met and explicitly NOT vacuously — **`promoted` STILL NOT MET (`bail!`, untouched)**.
No VERDICT or DISPOSITION file exists in `23A-governed-skills/`. A sibling lane owns this; I
record it and defer.

### M10 — instrument-exposure check: this phase is NOT exposed to the two known-bad instruments

- **`no-tests = "fail"` silently ignored:** not load-bearing. `.config/nextest.toml:37` does carry
  it, but **every** 23B plan gate passes `--no-tests=fail` on the COMMAND LINE
  (`23B-01-PLAN.md:158,193,194`; `23B-02-PLAN.md:166,200,201,243`; `23B-03-PLAN.md:170,171`), and
  `23B-03-LIVE-EVIDENCE.md:183` red-proved the CLI flag works (exit **4** on a no-match filter).
  Every reported suite reads back a non-zero executed count.
- **nextest "flakiness" = fd/inotify exhaustion:** both red clusters in this phase were correctly
  diagnosed, not laundered. 23B-02's 14 raw-harness failures passed in isolation and at
  `--test-threads=1` (2101/0); 23B-03's four `child_authority_corpus` failures were re-run alone at
  the untouched base `32e2f57d` and fail identically there.

### M11 — C4 nuance that changes NOT MET from a blanket claim to a precise one

`/cost` and `/compact` DO exist as TUI commands (`tui/commands/mod.rs:197,227,596,604`), and
`/cost` renders session spend + per-turn breakdown (`tui/surfaces/diagnostics.rs:789,1890-1903`).
But: `CacheHealthWarn` at `engine.rs:11081` is, per the code's own comment, *"Warning-only
structured telemetry: greppable in the engine log, never alters the request"*; `TokenPressure` has
**0** refs in `wcore-agent`/`wcore-cli`; no cost-regression thresholds exist. So one of four
clauses has a partial surface and it **predates Phase 23 entirely**. Graded NOT MET rather than
PARTIAL so the phase is not credited with work it did not do.

### M12 — fence + supporting facts for the gap list

- `git diff --name-status 861d1b1a HEAD` → 1 file added (NOTES). `git status --porcelain` empty.
  `git diff --stat` over `crates/`, `.github/workflows`, and both fenced `wcore-cli` files → empty.
  **Zero fence exposure.**
- `ls scripts/f23*` → 15 scripts; **no `f23-session-operator-drive.ps1`**, so C2's Windows leg is
  unbuilt, not merely undriven.
- `ci.yml:7-31` already admits lane branches to `push.branches` (added 2026-07-27), so the macOS
  *binary* is obtainable; the macOS journey blocker is the compiled **test harness**, which CI does
  not upload → needs a `.github/workflows/ci.yml` edit, which lanes are fenced from.
- Clock: now `2026-07-29T10:03:04Z`; Linux `f23-journey-day3.timer` armed for
  `2026-07-30 14:31:00 UTC` → ~28 hours to recurrence of the G5a defect.

## VERDICT WRITTEN

`23B-PHASE-VERDICT.md` — **NOT ACHIEVED**. C1 PARTIAL (23A's, deferred), C2 PARTIAL,
C3 NOT MET, C4 NOT MET, C5 NOT MET, C6 MET WITH STATED EXCEPTIONS. Costed 16-item gap list;
G5a (Linux journey `HOME` abort) is urgent and recurs in ~28h.
