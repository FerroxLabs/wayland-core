# 24-GRADE-NOTES — working notes for the Phase 24 verdict

Lane `grade-24`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-grade-24`,
branch `lane/grade-24`, base `861d1b1a`.

Append-and-recommit after every measurement (LANE-BRIEF §6b-i). This file is the resume point.

---

## T+0 — established facts

**Confirmed: Phase 24 has NO verdict file.** Search over `.planning/phases` for `*VERDICT*`
returns verdicts for 21, 22, 27, 28, 29, 30 — and nothing for 23A, 23B, 24, 25, 26. That is the
five-of-eleven gap named in the brief; 24 is one of them.

Instrument note: that is an ABSENCE claim, so per §3b-i it needs a live-instrument proof. The
same `find` in the same invocation returned 10 positive hits (`27-PHASE-VERDICT.md` etc.), so the
instrument was alive. Query recorded verbatim below in the measurement log.

**The five Success Criteria, verbatim from `.planning/ROADMAP.md` (Phase 24 section, line 112ff):**

> **Goal**: Operators can install, run, automate, connect, inspect, recover, and support one
> persistent Core runtime on every OS family.
>
> 1. Native service lifecycle, profile isolation, active-turn visibility, drain, restart, upgrade,
>    and rollback work without lost or duplicate delivery.
> 2. Scheduled, event-driven, webhook, polling, and commitment work has bounded history, retry,
>    continuation, and delivery.
> 3. Reference channels prove setup/auth, access, routing, media, native actions, idempotency,
>    reconnect/reload, and health.
> 4. Typed authenticated clients recover event gaps and produce useful redacted health/log/support
>    evidence.
> 5. Setup-to-recovery journeys pass on macOS, Linux, and Windows.

**Grade vocabulary:** MET / MET-WITH-STATED-EXCEPTIONS / PARTIAL / NOT MET.

---

## Method I am committing to before I look at the evidence

Stating this first so the grade cannot be reverse-engineered from what I find.

1. **Re-derive, never inherit.** `RC-READINESS.md` and `MILESTONE-RC.md` are declared partly stale
   and one holds a superseded section. I read them for pointers to primary evidence only; every
   number in the verdict comes from the SUMMARY/evidence file that produced it, or from a
   measurement I take.
2. **C3 is graded clause-by-clause.** The criterion names eight clauses (setup/auth, access,
   routing, media, native actions, idempotency, reconnect/reload, health). A criterion made of
   eight conjuncts cannot be graded as one blob. Each clause gets its own status, its own adapter
   coverage, and its own platform coverage. The criterion grade is then the floor of the clauses,
   not their average.
3. **Two instruments are known bad and both bear on this phase:**
   - nextest "flakiness" here was **fd/inotify exhaustion**, not real failure — 40 runs, zero real
     failures. So a red attributed to flakiness is not automatically a defect, AND a green taken
     under contention is not automatically a pass.
   - `no-tests = "fail"` is **silently ignored** by the installed nextest. A green suite may have
     executed nothing. Any criterion resting on "the suite is green" is downgraded explicitly and
     the `N passed` count must be read back.
   - Corollary already burned into §3b-i: a **known-negative assertion is self-passing on a dead
     instrument**. C3's strongest new evidence includes "zero advertised-but-dead" and
     "DIVERGENT=0" and "lost=0 duplicated=0" — every one of those is a negative. I must check each
     had a live-instrument / positive-control proof, or downgrade it.
4. **Merged vs pending is a first-class distinction.** Two lanes' evidence (`native-actions`,
   `e2e-product-smoke`) is on unmerged branches. Unmerged work is real evidence of capability but
   it is NOT in the release candidate. I will grade twice where it matters: as-merged, and
   as-if-pending-lands.
5. **Absences get their query recorded** (§3b-i.4).

## Grading stance

An inflated grade costs a customer who trusts a false claim; a deflated one costs weeks rebuilding
what works. Neither is safe. So the tiebreak is not "be conservative" — it is **be specific**: name
the clause, the adapter, and the platform, so a reader can see exactly how much is true rather than
reading a single word that is wrong in both directions.

---

## Measurement log

### M0 — verdict-file absence (T+0)

```
/usr/bin/find .planning/phases -name "*PHASE-VERDICT*" -o -name "*VERDICT*"
```
10 hits, none under `24-*`. Instrument alive (positive hits present in same run).

### M1 — evidence inventory for Phase 24 (T+0)

`.planning/phases/24-gateway-automation-channels-typed-api/` holds 18 evidence directories and
~40 markdown artifacts including 4 numbered PLAN/SUMMARY pairs (24-01..24-04), plus lane
artifacts for B (gateway-surface), C (arrival), C1, C2, C3 (+DISCORD/FINISH/H2/H4/TG-EMAIL), C5
(+FINISH), CHANNEL-LEASE, CHANNEL-STARVATION, EMAIL-MSTEAMS, H5, H6, MATRIX-SIGNAL,
MEDIA-ACTIONS, MEDIA-BOUNDS, MEDIA-LIVE, MSTEAMS-ATTACH, RECONNECT, and a `24-PHASE-REPORT.md`.

Still to establish (in order):
- [ ] read `24-PHASE-REPORT.md` — what it already claims and whether it is a verdict in disguise
- [ ] C1 from primary evidence (24-01, CHANNEL-LEASE, H5/H6)
- [ ] C2 from primary evidence (24-02, CHANNEL-STARVATION)
- [ ] C3 clause matrix across adapters × platforms
- [ ] C4 from primary evidence (24-03, 24-04, OPENAPI-CONSUMER)
- [ ] C5 re-derive the prior MET-on-three-platforms receipt
- [ ] the two pending branches read from their branch tips
- [ ] fence exposure vs `861d1b1a`
- [ ] release-blocking answer + costed gap list

### M2 — the superseded section in RC-READINESS.md, identified (T+25)

`RC-READINESS.md` contains **two** Item-3 sections that contradict each other on `24-C5`:

- lines 18-38 "Item 3 CLOSED 2026-07-28" → MET on all three platforms at `5ed01866`
- lines 39-58 "Item 3, measured 2026-07-28 evening — the honest state" → Windows RED at step
  12, macOS NOT RUN, "graded NOT MET on one of three platforms"

**The LOWER section is the stale one**, despite appearing later in the file. Git order settles it:

```
/usr/bin/git log --format='%h %ad %s' --date=short -1 -- <file>
  fd64bd5c 2026-07-28 docs(24-C5): the Windows leg reached step 12, and named the recovery gap
  e535c1a4 2026-07-28 docs(24-C5): the Linux receipt at the candidate, and the final macOS grade
  5ed01866 2026-07-28 Merge lane/24-c5-finish: 24-C5 MET on all three platforms
```

`24-C5-JOURNEY-SUMMARY.md` (fd64bd5c) is the Windows-RED/macOS-NOT-RUN state; `24-C5-FINISH-SUMMARY.md`
(e535c1a4) supersedes it and is what `5ed01866` merged. So the lower section describes the
JOURNEY lane and was never deleted when FINISH landed. **Anyone reading RC-READINESS top-to-bottom
gets the stale answer last, which is the worst possible ordering.** Flagging it for repair.

### M3 — `24-PHASE-REPORT.md` is not a verdict and is badly stale (T+25)

Dated 2026-07-26, grades all five criteria NOT MET, and records 24-02/24-03/24-04 as NOT STARTED.
That was true on 2026-07-26. It has been overtaken by ~20 lanes, all dated 2026-07-28/29. **It must
not be mistaken for the phase verdict** — it predates essentially all of the phase's evidence.
I am treating it as a historical execution report for wave 1 only.

### M4 — every lane declines 24-C3 (T+30)

Extracted `grade-24-C3:` frontmatter from every lane artifact. Twelve distinct lane artifacts carry
a C3 grade; **every one says NOT MET and explicitly declines to claim it** (the lanes number
themselves "the sixth… seventh… eighth… lane to decline it"). No lane has ever claimed C3.

The consistent reasons given across lanes, which I must now test independently rather than inherit:
1. `media` and `native actions` had zero evidence (this has CHANGED — see M5)
2. every figure is Linux; macOS and Windows have nothing
3. the two designated REFERENCE adapters (discord, email) lacked inbound fixture seams
   (discord CHANGED by `24-c3-discord`; email still unmeasurable by configuration alone)

### M5 — the two pending branches, read from their tips (T+45)

Both confirmed NOT merged: `/usr/bin/git merge-base --is-ancestor <branch> HEAD` → false for both.
Merge-base for both is `75babf32`.

**`lane/24-native-actions`** — `git diff --name-status 75babf32 lane/24-native-actions` = **10 files,
all `A` (additions), ZERO under `crates/`.** One new driver script `scripts/f24-native-actions.mjs`,
one report, one notes file, 7 evidence JSON/logs.

**This is the single most important structural fact about the pending work.** The lane changed no
product code. It *measured* product code that is already merged. So "pending merge" here means the
EVIDENCE is unmerged — **not** that the capability is unmerged. The `native actions` capability it
proves is in the RC today; only the proof of it is on a branch. That distinction changes the grade
materially and I nearly missed it.

Its matrix (6 adapters × 3 affordances, `gateway run`, Linux, platform-side counting, per-adapter
negative control): 5/5 adapters declaring `react` fire both reactions; 4/4 declaring `send_typing`
fire typing; msteams correctly reports `not supported` for react and telegram/matrix/discord fire
all three. **Zero advertised-but-dead.**

That last is a NEGATIVE claim, so per §3b-i I checked its instrument: the lane proved the census
grep alive on a known-positive in the same shape (`max_message_len` → 9 files) before reporting the
zeros. It also distinguishes `not-supported` from `not-fired` as separate verdicts — which is the
control that stops the silent `send_typing` no-op default from scoring a free pass. **Accepted.**

**`lane/e2e-product-smoke`** — also all additions, 0 `.rs` files. **The brief's "12/12" is not what
the report says.** Its own frontmatter: `steps-total: 14, steps-passed: 12, steps-failed: 0,
steps-not-reached: 2`. So **12 of 14**, and the 2 not reached are **TUI on a real terminal** and
**Windows/macOS cold start**. 12/12 would imply full coverage; 12/14-with-2-unreached does not.
Correcting the brief's arithmetic — this is exactly the "never inherit arithmetic" case.

Also: this lane is a general product cold-start smoke (turn, 5 tools, sandbox, skill, memory, MCP,
resume, crash). **It maps to the phase GOAL's "install, run" but to none of the five criteria
directly** — it does not touch gateway lifecycle, automation, channels or the typed API. I will not
credit it to any criterion. It is real evidence that the product is not hollow; it is not C1-C5.

### M6 — a stale claim tested and FALSIFIED: active-turn visibility IS rendered (T+55)

`24-PHASE-REPORT.md` says *"active-turn visibility — MET as a projection field; never rendered to an
operator, because no verb exists to render it."* **That is no longer true.**

```
/usr/bin/grep -rn "active_turn|turns_in_flight|in_flight" crates/wcore-gateway/src crates/wcore-cli/src --include=*.rs
```
(concept search, not one keyword, per §3b-i.3 — `active_turn` alone returns 0 in the gateway and
would have "confirmed" the stale claim for free; the concept lives under `turns_in_flight`.)

- `wcore-gateway/src/lifecycle.rs:182` — `StatusProjection.turns_in_flight: usize`, a `Serialize` field
- `wcore-cli/src/gateway.rs:479` — `--json` prints the whole projection
- `wcore-cli/src/gateway.rs:494` — **`println!("  turns in flight:    {}", proj.turns_in_flight)`**
  in the human-readable path

So `gateway status` renders active turns in **both** forms. Verb exists, field reaches the operator.
**Clause MET.** Instrument control: 58 `println!` and 23 `profile` hits in the same file, so the
file and the grep were both alive.

### M7 — C2 trigger state in the current tree (T+55)

`crates/wcore-cli/src/cron.rs:44-57` now documents `once/every/cron/event/commit` and states
verbatim that *"`webhook:` and `poll:` are NOT accepted: nothing in this build can fire them."*
`cron.rs:350` prints `WILL NEVER FIRE` for persisted legacy jobs. So the **false advertising is
retired** and `event` fires via `cron publish`. **The plane was not built**: webhook and poll have
no producers. C2 cannot be MET.

### M8 — NEW FINDING on 24-C4: the support bundle has no operator verb (T+75)

The gap ledger grades `24-C4` **MET on Linux / HTTP+SSE only**. Re-deriving rather than
inheriting it, I checked the criterion's SECOND half — *"produce useful redacted health/log/support
evidence"* — against the shipped binary.

```
/usr/bin/grep -rn "support_bundle" crates/ --include=*.rs
  crates/wcore-gateway/src/lib.rs:19:            pub mod support_bundle;
  crates/wcore-gateway/tests/support_bundle_redaction.rs:20  (use ...)
  crates/wcore-gateway/tests/support_bundle_redaction.rs:269 (doc comment)
```

**Three hits. One is the module declaration, two are its own test file. ZERO production call sites,
and no CLI verb.**

Absence discipline (§3b-i), because this is a negative claim:
- **Instrument proven alive in the same shape:** `/usr/bin/grep -rln "acp" crates/wcore-cli/src`
  → **6 files**. So the search finds a sibling subsystem's name in the CLI when it is there.
- **Concept search, not one keyword:** also searched `supportbundle`, `support bundle`,
  `diagnostic`, `bundle`, `doctor`, `redact` case-insensitively across `crates/wcore-cli/src`.
  Hits are a TUI `/doctor` diagnostics panel and a `/config` "resolved config (redacted)" view —
  **neither is a support bundle an operator can hand to support.** `ReleaseVerifier::bundled()` is
  an unrelated homonym.
- **Query recorded above so it can be re-run.**

This is the **same advertised-but-dead class** as `F24-C3-H6`/`F24-MB-1` (`media_bounds()` read at
exactly one site, and that site a test). Filing as **`F24-C4-H1`, severity HIGH**: the criterion
names support evidence as one of its two halves, and the half is unreachable from the product.

Consequence for the grade: **`24-C4` cannot be MET.** The gap ledger's MET was derived from
`24-04-SUMMARY.md`'s test evidence for the *recovery* half and did not test the *support-evidence*
half against the shipped surface. Re-derivation caught it. This is the clearest vindication of the
"never inherit" rule in this grading.

### M9 — an instrument defect of my own, repaired in-lane (§6b-ii) (T+80)

Measuring whether the support-bundle suite is anti-vacuous, I first ran `/usr/bin/grep -c
'#\[ignore\]'` → **0**. **That zero was false.** The file's attribute is `#[ignore = "live: …"]`,
so the literal `]` in my pattern could never match. I had just written a note about known-negative
assertions being self-passing and then produced one.

Repaired to `#\[ignore` and self-tested with **three** assertions, per §6b-ii:

| # | assertion | result |
|---|---|---|
| A1 | known-positive: repaired matcher finds the ignore in this file | **1** ✅ |
| A2 | known-negative: repaired matcher on `lifecycle.rs` (no ignores) | **0** ✅ |
| A3 | **the OLD broken matcher would have missed it** | **0** ✅ — repair proven to do something |

A3 is the one that proves the repair is not cosmetic; without it the self-test passes on the broken
matcher too.

**Corrected measurement:** the suite has **5 `#[test]`**, of which **1 is `#[ignore]`d** —
`live_bundle_canary`, which is the *only* live one and requires `F24_LIVE_BUNDLE` /
`F24_LIVE_CANARY_FILE` / `F24_LIVE_SEEDED_DIR`. It is well built (it FAILS rather than skips when
the vars are absent), but it runs only under `-- --ignored`. So a plain `cargo test -p wcore-gateway`
exercises the 4 offline redaction tests and **not** the live canary. `24-03-SUMMARY.md`'s
"canary-proved" therefore describes an opt-in gate, and I downgrade confidence in it accordingly —
independently of M8, which is the decisive fact.

### M10 — outbound idempotency census (T+82)

```
/usr/bin/grep -rn "fn supports_outbound_idempotency" crates/ --include=*.rs
```
Overrides in **slack, matrix, discord** (3). Trait default `false` at `wcore-channels/src/lib.rs:139`.
Adapter crates: **10** (`ls -d crates/wcore-channel-*`). So **3 of 10** adapters can suppress a
cross-restart outbound replay; the other **7 ABANDON** an outcome-unknown delivery. Abandonment is
honest and fails closed — and it is not delivery. Bears on C1's "without lost … delivery" clause.

### M11 — fence exposure (T+85)

```
BASE=861d1b1a716240165209336b1fa38d36f9445716
/usr/bin/git diff --stat "$BASE" HEAD -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs
```
**Empty — zero fence exposure.** Live control in the same measurement: `git diff --numstat` reports
`209 0` for `24-GRADE-NOTES.md`, so the diff instrument was alive and the empty fence result is a
real zero. Files changed by this lane vs base: **1**, `24-GRADE-NOTES.md` (+ this verdict).
`crates/` or `.github/` paths touched: **0**.
