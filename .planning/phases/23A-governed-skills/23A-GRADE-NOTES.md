# 23A GRADE NOTES — lane `grade-23a`

Running notes. Appended after every measurement, per LANE-BRIEF §6b-i. The verdict file is
`23A-PHASE-VERDICT.md`; this file is the working record and keeps the reasoning recoverable
if the lane dies.

## T+0 — established facts

- Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-grade-23a`,
  branch `lane/grade-23a`, HEAD/base `861d1b1a716240165209336b1fa38d36f9445716`.
  `git rev-parse --show-toplevel` resolves to the lane path (NOT `/Users/seandonahoe/dev/waylandcore`).
- Job: produce `23A-PHASE-VERDICT.md`. **No verdict file exists for 23A** — confirmed by
  `ls .planning/phases/23A-governed-skills/` (14 files, none named `*VERDICT*`).
- I am GRADING. Fences: no `crates/` edits, no `.github/workflows/*`, no merge/PR/tag/publish/
  issue-close, no `wcore-contract generate`.

## T+0 — the criterion under grade

`.planning/ROADMAP.md:102` (Phase 23, Success Criterion 1), which `ROADMAP.md:109` assigns to 23A:

> 1. Generated skills cannot execute before governed promotion and can be observed, revoked,
>    and rolled back.

`ROADMAP.md:109`: "Phase 23A closes governed-skill/M3 contracts first. Phase 23B Continuous
Agency (operator lifecycle, memory, index, cache economics, multi-day journey) begins only from
the admitted 23A contract." So Criteria 2–6 are 23B's, not 23A's. **To verify: whether 23A
also owns any part of Criterion 6 / the F23-0x requirements.** Requirements listed for Phase 23
are F23-01..F23-06.

The criterion is a **four-clause conjunction**; it is met only if all four hold:
- C1.a cannot execute before governed promotion
- C1.b can be observed
- C1.c can be revoked
- C1.d can be rolled back

## T+0 — the two `23A-C1` efforts (this is the crux, and the prior verdict is stale)

Two distinct bodies of work share the name. Measured with `/usr/bin/git`:

1. **MERGED, in my base.** `460fad3b` "merge(23a-c1): reversibility landed before capability,
   and a known-negative was found self-passing", 2026-07-29 08:50:48 +0700, parents
   `5a5da69d` + `e721526b`. `git merge-base --is-ancestor 460fad3b HEAD` → in base.
   Artifacts in-tree: `23A-C1-SUMMARY.md`, `evidence/23A-C1/{NOTES,CROSS-AUDIT,LIVE-EVIDENCE,
   harness-selftest.sh,panel-prompt.txt}`.
2. **PENDING, NOT in my base.** `3a2234d7` "docs(23A-C1): lane deliverable, live-proof harness
   and kill-distribution harness", 2026-07-29 14:42:08 +0700, on `lane/23a-c1-governed`
   (local + `remotes/gh/`). `git merge-base --is-ancestor 3a2234d7 HEAD` → **rc=1, NOT an
   ancestor.** Claimed content: real governed promotion/revocation/rollback on the shipped
   binary, 34 live assertions, kill distribution 0 partial writes in 35 kills, deliverable
   `.planning/23A-C1-GOVERNED.md`.

The standing ROADMAP row (`ROADMAP.md:221`) and `23A-04-SUMMARY.md` grade C1 **NOT MET** with
clauses c and d NOT MET ("nothing implements revocation" / "rollback"). Those predate BOTH
efforts above. **The prior grade is therefore stale and must be re-derived, not inherited.**

## T+0 — instrument discipline for this lane

Per brief §3b / §3b-i, and because two named instruments may have poisoned earlier evidence:
- All load-bearing commands via `/usr/bin/git`, `/usr/bin/grep`, absolute-path `cargo`.
- **Every absence claim gets a known-positive in the same invocation.** The prior NOT MET on
  clauses c/d rests on exactly the assertion shape (`grep for a revoke surface returns zero`)
  that §3b-i proves is self-passing on a dead instrument. I must re-run those greps with a
  live-instrument control before repeating or overturning them.
- `no-tests = "fail"` in `.config/nextest.toml` is silently ignored by the installed nextest →
  any "suite passed" evidence I lean on must carry an executed-count, not an exit status.
- `cargo nextest` "flakiness" in this repo was fd exhaustion, never a real failure — a past red
  showing `exec failed` is not a regression and must not be graded as one.

## STILL TO ESTABLISH

- [ ] Whether 23A owns criteria beyond C1 (check 23A plan set + F23-0x requirement mapping).
- [ ] Re-derive each of the four clauses against the merged tree at `861d1b1a`.
- [ ] Read `.planning/23A-C1-GOVERNED.md` from `lane/23a-c1-governed@3a2234d7` and grade the
      pending delta separately, marked pending-not-merged.
- [ ] Verify the 34-assertion / 35-kill claims are not self-passing (executed counts, known-positives).
- [ ] Gap list with lane-session costs and credential-vs-build classification.

## T+40 — MEASURED. The decisive fact is the SHIPPED SURFACE, not the code.

All via `/usr/bin/git`, each absence carrying a known-positive control in the same invocation.

### M1 — merged base `861d1b1a` DOES contain revoke + rollback (library)
`git cat-file -e 861d1b1a:crates/wcore-skills/src/govern.rs` → exists, **566 lines**.
Public surface read back: `revoke()` :221, `rollback()` :290, `live_revocations()` :330,
`is_revoked()` :370, `record_suppression()` :400, `journal()` :426, `governance_root()` :460.
So `23A-04-SUMMARY.md`'s "Clause 3 NOT MET — nothing implements revocation" and "Clause 4
NOT MET" are **STALE**: that summary is dated 2026-07-26, the merge `460fad3b` is 2026-07-29.

### M2 — but at base those verbs are NOT reachable on the binary customers install
`git show 861d1b1a:crates/wcore-cli/src/main.rs | grep -n skills_promote|skills_revoke|...`
- `skills_promote` present (:474) but `run_skills_promote` (:2537) is an **unconditional
  `bail!`** — "temporarily unavailable while governed promotion is being implemented".
- `skills_revoke`, `skills_rollback`, `skills_govern` → **NOT PRESENT AT ALL** at base.
Control: `skills_archive` IS present (:483, :1546) in the same grep — instrument alive.

### M3 — the merged capability ships to nobody (verified, with control)
```
git show 861d1b1a:.github/workflows/release.yml | /usr/bin/grep -c "wayland-core"   -> 31  [KNOWN-POSITIVE]
git show 861d1b1a:.github/workflows/release.yml | /usr/bin/grep -c "skill-govern"   ->  0  [the absence]
```
`git grep -n wcore-skill-govern 861d1b1a -- ':!*.rs'` → **9 hits, every one a `.planning/`
document**; zero workflow, zero packaging manifest, zero Cargo `[[bin]]` declaration outside
auto-discovery. So `23A-C1-SUMMARY.md:53`'s "The capability ships and is drivable today" is
**false as customers experience it** — it is buildable from source, not shipped.
This independently reproduces the pending lane's HIGH. I did not take it on trust.

### M4 — clause (a) enforcement call sites are real at base
`loader.rs:448` sets `disable_model_invocation = true`; `slash/skill.rs:115-117` refuses
("this skill is quarantined and cannot be run"); `refs.rs` filters at :129/:294/:313/:325/:335.
`slash/skill.rs:327` is a unit assert on the refusal string.

### M5 — pending `3a2234d7` adds exactly the missing surface
`promote.rs` **ABSENT at base** (rc≠0) / **PRESENT at 3a2234d7** — control `loader.rs` present
at both, so the negative is not a dead instrument. Diff vs merge-base `75babf32`: 20 files,
+2941/−188, incl. `promote.rs` (589), `govern_catalog_enforcement.rs` (299),
`skills_promote_advertised_and_works.rs` (319, replacing `skills_promote_not_advertised.rs`),
`ProcedureStatus::Revoked` in `wcore-memory/src/v2_types.rs`, and four flags wired in `main.rs`
(:479 promote, :496 revoke, :502 rollback, :507 govern; dispatch :1566-1583).
NOTE the doc says head `597c3275`; branch head is `3a2234d7` (the docs commit on top).

### M6 — 23A-02 and 23A-03 remain `status: not_started` in-tree; 23A-04 discharges only its Task 3.

## OPEN
- [ ] F23A-01-H2 (any errored tool call kills the session) — open or fixed at base?
- [ ] Audit pending lane's live-proof + kill harness for self-passing before crediting 34/35-kill claims.

## T+75 — the two OPEN items closed

### M7 — F23A-01-H2 is FIXED, and the fix IS in my base
`32a5fc90` (fix) and `81508b74` (RED-first test) are BOTH ancestors of `861d1b1a` (verified
`git merge-base --is-ancestor` → YES for each). `orchestration/mod.rs:78` declares
`mod d1_refusal_terminal_tests;` so the file is wired, not orphaned. Five test fns read back
from the file at base, including `approval_denial_control_leaves_turn_committable` — **a
control**, which is what makes the other four falsifiable.
→ Every "H2 open and red" caveat in `23A-01/02/03/04-SUMMARY.md` and in `ROADMAP.md:221` is
**STALE**. `lane/record-truth`'s `23A-STATUS-CORRECTION.md` says so, but that file is on
`lane/record-truth` and is **NOT in my base** (`11a6c044` → not an ancestor). So the correction
itself is unmerged and invisible to anyone reading the phase directory.

### M8 — the 16-route census WAS re-measured at HEAD, and its control fires
`2a315b83` + `6ae6e611` are in base. Evidence read directly from the logs, not the table:
- `run-E-baseline-final.log:11` → `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
- `run-F-selftest-final.log:25` → `test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 filtered out`
- run-F lines 5-8: all four route checks flip to `refused=false` under substitution
- run-F line 9: `F23A-SELFTEST-LEGACY: R7 /skill list old_matcher=true` — the pre-repair
  matcher reports "quarantined draft is hidden" while handed a **visible** control skill.
- `run-status-sentinels.txt`: `WLRC=0`/`WLDONE` and `WLRC=101`/`WLDONE`, read back separately.
**The control fires and discriminates on all 4 live-driven routes.** This is the strongest
instrument in the phase and it post-dates every summary that grades the phase.
Coverage limit stated by the census itself: **4 LIVE-DRIVEN, 10 STATIC@HEAD, 2 UNREACHABLE.**
R2 and R5 (the two model-visible-payload routes) are un-driven because
`OpenAiFixtureScript` retains only hashes (`fixtures/openai.rs:311-321`, intent comment :317).

### M9 — a HIGH that is REAL IN MY BASE and is NOT recorded anywhere in the phase
Base `rollback()` (`govern.rs:290`) restores with a bare `copy_tree(&payload, &record.source_dir)`
straight into the live skills directory. `copy_tree` (:536) is `create_dir_all` + per-file
`std::fs::copy` (:561). **No staging dir, no `rename(2)`.** The `sync_all` at :454 is inside
`atomic_write`, used for the JSON records only — NOT for the restored tree.
→ A kill mid-rollback leaves a **partially restored skill directory** in the user's skills root.
Independently corroborates the pending lane's measured `PARTIAL 11/35`. I did not re-run that
harness (see M11), but the code shape at base is exactly what produces it.

### M10 — pending lane's code does what it claims (source-verified at 3a2234d7)
- `loader.rs:185 apply_governance`, called at **:83 and :156** — both `load_all_skills` and
  `load_catalog`, i.e. one choke point per entry.
- `promote.rs`: `content_digest()` :527 → `sha256:{:x}` :542; grant field `content_digest` :108;
  `Refusal::Revoked` :122 with revocation checked FIRST at :281 and :327; `promotion_state()`
  :234 compares found vs granted digest → mismatch path :249.
- `skills_promote_advertised_and_works.rs`: **5 `#[test]`**, and the negative test at :151 carries
  an explicit positive control ("a command that always failed would pass the negative assertion
  for free"). The guard was inverted, not deleted.

### M11 — I CANNOT re-execute the pending lane's numbers, and I say so
`ssh hetzner-dsm`: `/root/wayland-23a-c1/target/debug/wayland-core` → **No such file**. The lane
removed its worktree as the brief requires. 21 other lane worktrees live; `df -h /root` → 709G
free. Rebuilding `wcore-cli` to re-verify is a non-targeted build on a host under heavy
multi-lane load, and I am a grading lane. So the pending 34-assertion / 70-kill figures are
**audited-but-not-re-executed**. What I DID verify: `kill-23a-c1-distribution.sh` is genuinely
falsifiable — it has a **vacuity guard** (`CO==0 || GR==0` → `INVALID MEASUREMENT`, exit 2),
self-calibrates its kill window, seeds delays deterministically, **retries every NOT-STARTED
trial and asserts recovery**, and its `PARTIAL=0` claim carries a reddening control (revert the
atomic restore → `PARTIAL=11/35`). That is a known-positive for a known-negative claim.

## GRADE SETTLED (see 23A-PHASE-VERDICT.md)
23A owns Phase 23 **Success Criterion 1 only** (ROADMAP:102 via :109); requirement **F23-01**
only — all four 23A plans declare exactly `F23-01`, no other.
At base `861d1b1a`: a = MET-WITH-STATED-EXCEPTIONS, b = PARTIAL, c = PARTIAL, d = PARTIAL.
**SC-1 = NOT MET at base — but for materially different reasons than the standing record gives.**
