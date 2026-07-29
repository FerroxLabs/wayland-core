# Phase 23B — Criterion 4, the three live gaps — LANE SUMMARY

**Lane:** `c4-live-cache` · **Branch:** `lane/c4-live-cache`
**Base:** `plan/f20-unified-audit-repair` — session 1 branched at `19c10666`; session 2
merged integration tip `0d4d1842` in (which subsumes the `eaff921d` the brief named).
Merged, **not** rebased — LANE-BRIEF §0 forbids `git rebase`.
**Date:** 2026-07-29
**Evidence:** `evidence/23B-C4-LIVE/23B-C4-LIVE-EVIDENCE.md` (§1–§5 session 1, §6–§13
session 2), `…-NOTES.md`, raw captures in `evidence/23B-C4-LIVE/live/` (session 1) and
`evidence/23B-C4-LIVE/live-s2/` (session 2).

> ### INCIDENT — two agents ran on this lane at once, and the final deliverable nearly died
>
> Session 2 was dispatched on the premise that session 1 "was killed and its context is
> gone", leaving five commits. **Session 1 was still alive.** Its commits `61005508`
> (15:14:13Z) and `9fa515fd` (15:15:57Z) bracket session 2's own `b591ce26` (15:14:21Z)
> — `9fa515fd`'s parent *is* session 2's commit. Session 1 then finished, wrote this
> SUMMARY and deleted its hetzner worktree and binary, which vanished mid-investigation
> from under session 2.
>
> `9fa515fd` — carrying EVIDENCE §4/§5, this SUMMARY and the retained captures — **was
> never pushed**, existing only as the hetzner branch `hz/c4-live-cache`. Session 2
> deleted that branch while recreating a worktree and recovered the commit only because
> git prints the SHA it deletes. Re-pushed as **`lane/c4-live-cache-rescue`** and merged.
>
> **"The previous agent is dead" is an absence claim and must be measured** — commit
> timestamps and running processes — before it is acted on. It is the same failure class
> as every other self-passing negative in LANE-BRIEF §3b-i.

Lane `23b-c4-cache` graded Criterion 4 **MET on all four sub-clauses** and stated three
live gaps it could not close because no permitted host had a working prompt-caching
credential. Sean supplied one. **I am not re-grading C4 — the grade stands, and nothing
I found contradicts it.** This lane converts the three gaps into observations.

---

## VERDICT: all three observed live — and the third one found a HIGH defect

| # | Gap stated by `23b-c4-cache` | Status |
|---|---|---|
| 1 | No live cache **HIT** | **CLOSED** — `cache_read=10474`, `warm_hit_ratio=0.9744` |
| 2 | No live **compaction** | **CLOSED** — and it failed first (C4L-F1), then succeeded after the fix: `tokens_reclaimed=16096` |
| 3 | No live **`history_rewritten`** | **CLOSED** — observed twice: once as a false positive (C4L-F2), once true |

Every observable comes from `wayland-core cache report` / `cache show` / `cache verify`
— the operator-reachable path the criterion demands — not from an internal probe.

**Provider read back from the product's own output (LANE-BRIEF §3b-ii):** every
round-trip of every session records `provider=anthropic model=claude-haiku-4-5` in the
ledger the engine wrote. Corroborated independently: `cache_read`/`cache_write` are
non-zero, and Ollama — the arm the prior lane ran on, and the arm this box's injected
env could have silently supplied — implements no prompt cache and cannot produce
either. The two agree, so this is not a hit claim that ran on the wrong arm.

---

## The finding — C4L-F1 (HIGH, fixed, live re-proved)

**Autocompact sent Anthropic a message list whose trailing `tool_use` blocks had no
`tool_result`, so every autocompact attempt in a tool-using session failed with a 400.**

```
kind=auto_failed trigger=watermark watermark=16997 threshold=8000 tokens_freed=0
error=API error 400 … "tool_use ids were found without tool_result blocks
immediately after: toolu_01CXdh78hXfCf8ZBShzdPbKr"
```

Autocompact's trigger is evaluated mid-tool-loop, so the conversation it is handed
routinely ends with calls still in flight; appending the summary prompt after that puts
a plain user message where the provider requires a `tool_result`. **Autocompaction was
therefore entirely non-functional in the commonest agent shape** — under real pressure
a session would ride the watermark up into the emergency hard stop with no relief ever
available. Pre-fix the watermark went 16997 → 17132 → 17377, climbing, three failures.

This is invisible to unit tests: nothing rejects the request until a real provider
validates it. The prior lane's compaction coverage fed synthetic `TokenUsage` and never
drove a compaction against a live provider — which is exactly the gap it honestly
declared, and exactly what the credential bought.

**Fixed** by `drop_unanswered_tool_calls` in `compact/auto.rs`, then re-proved live on
the same prompt, config, box and model:

| | pre-fix `57e6a9a5` | post-fix `bc65e989` |
|---|---|---|
| `Autocompact failed` | **3** | **0** |
| `failed` / `tokens_reclaimed` | 3 / **0** | 0 / **16096** |
| watermark after compaction | 17132 (climbing) | **4634** |
| pressure after compaction | 2.1415 | **0.5793** |

> **SUPERSEDED BY SESSION 2.** The paragraph that stood here argued the pre-fix run
> *was* the known-negative, so no billable run was spent reverting the change in place.
> That argument does not hold: B and C were different sessions on two binaries nobody
> verified, and **both binaries were deleted before anyone could check them.** Session 2
> ran the controlled version — one worktree, one line flipped, rebuilt in place, same
> fixture/prompt/model/box — with the fix's presence asserted from the binary's symbol
> table each time (6 → **0** → 6, against a control that never moves). See
> EVIDENCE §7. The verdict is unchanged; the evidence for it is now real.

| run | fix | `compactions` | `auto` | `failed` | `tokens_reclaimed` | 400s | peak watermark |
|---|---|---|---|---|---|---|---|
| `FIX2` | **yes** | 1 | 1 | **0** | **16181** | **0** | 17022 |
| `REVERT` | **no** | 2 | 0 | **2** | **0** | **2** | 17567, *rising* |
| `RESTORE` | **yes** | 1 | 1 | **0** | **16024** | **0** | 17016 |

---

## What else the live run produced that was not asked for

- **`cost_truth=priced`, and `cache verify` exit 0.** The prior lane only ever saw
  `estimated` (Ollama misses the pricing catalog) and only ever saw exit **7** and
  **8**. `claude-haiku-4-5` resolves to a real catalog row, so the C4-F1 cost-truth
  machinery is now observed on **both** sides: correctly flagging the untrustworthy
  number then, correctly passing the trustworthy one now. All three exit states of
  that gate have now been observed live, which is what makes it a gate.
- **A positive live saving**, `saving_usd=0.004083 saving_ratio=0.1247` — and
  **negative** per-turn savings on write-only round-trips (`-0.002621`), because a
  cache write costs 1.25× input. The ledger reports both signs rather than flooring.
- **A second live invalidation cause**, `expired`, alongside the prior lane's
  `no_marker`.

## Findings filed, not absorbed

| ID | Sev | Status |
|---|---|---|
| C4L-F1 | HIGH | **FIXED + live re-proved** |
| C4L-F2 | MEDIUM | Filed. `compacted_since_last_round_trip()` filters on position, not outcome, so a **failed** compaction attributes the next miss to `history_rewritten` although nothing was rewritten. Observed as a false positive. One-line predicate fix (`error.is_none()`), deliberately not bundled unproved into a lane whose HIGH is already live-proved. |
| C4L-F3 | LOW | Filed. A miss caused by the cache **breakpoint moving** is labelled `expired` when nothing expired — `attribute_cause` falls through to `TtlExpiry` whenever the hashes match. Cosmetic, on a diagnostic surface. |

MEDIUM and LOW route to BACKLOG per LANE-BRIEF §5. Both were observed live here.

## A prediction of mine that was wrong, in public

`…-NOTES.md` §M1b, committed **before** any billable run, predicted `history_rewritten`
would be **unreachable** on Anthropic because compaction leaves the system/tools cache
zones intact, keeping `cache_read_tokens > 0` and skipping the override. It fired
anyway — the whole prefix moved, for a reason I had not modelled. The prediction was
committed in advance precisely so it could be falsified rather than quietly revised.

## Money

**≈ $0.25 total**, from the product's own catalog-priced figures. Session 1: $0.028670
(A) + $0.048917 (B) + $0.037402 (C) = $0.115. Session 2: $0.016389 (`FIX1`) + $0.036843
(`FIX2`) + $0.046981 (`REVERT`) + $0.036956 (`RESTORE`) = $0.137. `claude-haiku-4-5` at
`max_tokens=300` — the cheapest model that exercises prompt caching at all. One further
session-2 run cost nothing (`Unknown provider`: `-p` is `--provider`, not the prompt
flag — it died before any request left the box).

## Secret handling

The key was **never supplied by me and never left the box.** `/root/.wayland/.env`
already held it (mode 600, 108 chars); the runner sourced that file inside the remote
shell, so the value reached only the child process's environment — never `argv`, never
a new file, never a capture, never a commit. Session 2 re-swept everything both sessions
produced, with the needle fed to `grep -f` through a pipe so it never touched disk (the
session-1 sweep planted it into a scratch file, which §0 forbids), and with a **three**-
state control — the third being the one session 1 lacked, because the real sweep is
`grep -R` over a tree and *that* command's zero can come from a bad path:

```
NEEDLE_LEN=108
CONTROL_KNOWN_POSITIVE=1                     (MUST be 1)
CONTROL_KNOWN_NEGATIVE=0                     (MUST be 0)
CONTROL_RECURSIVE_SWEEP_ON_PLANTED_TREE=1    (MUST be 1 — the -R FORM can match)
SWEEP_COMMITTED_LANE_ARTIFACTS=0    SWEEP_RAW_SESSION2_CAPTURES=0
SWEEP_RAW_SESSION1_CAPTURES=0       SWEEP_WORKTREE_SOURCE=0
SWEEP_TOTAL_HITS=0
```

Scope included **all commit messages and the full lane patch**, not only working files.

Recorded environment fact for the next lane: an isolated `WAYLAND_HOME` does **not**
pick up `/root/.wayland/.env` — the first attempt failed `No API key found` (rc=1),
reproducing the prior lane's "isolated profiles do not import auth.json".

## Gates

```
cargo test -p wcore-agent --lib compact::auto
  test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 2194 filtered out
cargo fmt --all -- --check                      clean, rc=0
```

Captured over ssh so no local `rtk` proxy could strip `0 ignored` / `filtered out`.
The new tests carry an independent instrument (`violates_anthropic_pairing`) with its
own both-answers test, plus an assertion that the **pre-fix** request shape violates
the rule — without which the test would pass on a no-op sanitizer.

## Shared-file fence

**Untouched — and the check has a liveness control, because the obvious form of it is
now self-passing.** Integration `0d4d1842` has *already absorbed* session 1 through
`9fa515fd`, so `git diff $(git merge-base HEAD 0d4d1842) -- <fence files>` returns empty
**for every file in the repo, including ones the lane definitely changed.** That zero
would have been free. Measured instead as the session-2 delta, with a control:

```
git diff 0d4d1842 HEAD -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs
  → 0 lines                                                    (the fence)
git diff 0d4d1842 HEAD -- …/live-s2/run.sh
  → 62 lines            (LIVENESS CONTROL — same command form, known-positive)
git diff --name-only 0d4d1842 HEAD -- crates/
  → empty               (session 2 touches no source file at all)
```

**Note for the orchestrator: the C4L-F1 fix is already merged into integration**
(`c0b0e18e` and `bc65e989` are both ancestors of `0d4d1842`). Session 2's known-negative
therefore validates code that is *already on the integration branch*, which is the
better time to have found out it works than the alternative. Session 2 adds
`.planning/` evidence only.

## Files

Modified: `crates/wcore-agent/src/compact/auto.rs` (the fix + 4 tests).
Created: `.planning/phases/23B-continuous-agency/evidence/23B-C4-LIVE/*` and this file.

Not done, deliberately: no fix for C4L-F2 or C4L-F3; no change to `cache_diagnostics`,
`cache_ledger` or the CLI; no re-grade of Criterion 4; no `wcore-contract generate`;
no merge, PR, tag, release or issue action.
