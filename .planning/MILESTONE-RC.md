# MILESTONE — what is actually left, measured 2026-07-29

Built from five parallel inventories that read **the tree**, not the planning documents. Every
prior plan on this program was built from the documents, and the documents have been wrong in both
directions — a ledger review found 13 falsified claims, a phase was graded before its own work
landed, and three separate "only Sean can unblock this" claims turned out to be false.

**Supersedes `CRITERIA-GAP-LEDGER.md`** (stale by 15+ merges) for planning purposes.

---

## 1. The one-paragraph truth

**Nothing is close to shipping, and the reason is not the feature gaps.** The feature work is
further along than the record says. What is not in place is the ability to *know* whether anything
works: **CI has produced zero successful runs since 2026-07-25**, every scheduled workflow reports
on a tree 16 days old, **61 of 86 gate scripts have never been shown to fail**, and 20 findings —
including two HIGHs — were routed to files nobody read. We have been building carefully on top of
instruments we never checked.

---

## 2. What breaks if we ship tomorrow — ranked, evidence-backed

| # | Breaks | Evidence | State |
|---|---|---|---|
| 1 | **Inbound messages vanish silently.** Three production sites each start a channel manager with no cross-process exclusion; polling is a destructive read, so whichever process wins deletes the message for the other. A user running the installed service and then opening a session loses mail. | `bootstrap.rs:3090`, `cron.rs:403/432`, `gateway.rs:725`. In-process version measured 8/8 lost at startup, 5/6 in steady state, no error | **lane running** |
| 2 | **We cannot detect a regression at all.** 201 CI runs since 07-25: 153 cancelled, 46 failed, **0 succeeded**. All 46 die at clippy over 4 lines; clippy precedes tests, so `nextest --workspace` has **never executed in CI on this tree** | `journey.rs:{683,695,707,717}` | **lane running** |
| 3 | **Skills are written into the user's global directory with no promotion, revocation or rollback.** A write-only learn loop touching a directory the user owns | `main.rs:2516` unconditional bail; zero revoke/rollback surface in `wcore-skills/src/` | **open — 3–4 sessions** |
| 4 | **A clean exit can write a session journal the product cannot read back**, with no repair path. Reproduced 8/8 and 9/10 | `23B-H1`, HIGH, **filed nowhere** until tonight | **lane running** |
| 5 | **Outbound "no duplicate" is graded on 1 of 10 adapters.** The trait defaults false; Slack is the sole override. On the other nine, no-duplicate may mean abandoning delivery | `wcore-channels/src/lib.rs:139`, `slack/src/lib.rs:234` | **lane running** |
| 6 | **The native certification certifies code that no longer exists** — bound candidates are 194 and 147 commits behind HEAD, including the commits that repaired the finding whose closure made the gate pass | `28-04` receipt vs HEAD | **lane running** |

---

## 3. The structural problems — these caused the list above

1. **CI is decorative.** Zero successes in four days; scheduled workflows all target `main`, 1,374
   commits and 16 days behind; `supply-chain.yml` exists only on this branch and 404s off the
   default branch, so it has **never run**. Real coverage exists only in lanes and evaporates when
   lanes stop.
2. **Instruments are unverified.** 61 of 86 gate scripts carry no falsification control — including
   `lint-plan-gates.py`, the meta-instrument whose job is finding self-passing gates. Twelve
   recorded instances of an instrument carrying the defect class it hunts; one recurred *because
   the earlier sighting was documented rather than fixed*.
3. **Findings leak.** 20 dropped findings recovered tonight, 2 HIGH. A checker now exists and
   catches both shapes — including a new instance it found unprompted.
4. **My own standing instructions caused harm.** I told ~8 lanes the `journey.rs` clippy errors were
   pre-existing and not theirs. That instruction is what kept CI dark for four days.

---

## 4. Waves

**Wave 1 — restore the ability to know anything.** *(running now)*
CI unblock + full workspace test triage · cross-process channel lease · outbound idempotency ·
28-drift · 29 dependency policy · remediation-string gate · record reconciliation.

**Wave 2 — close the last release blocker.** *(running now)*
`24-C3`: telegram leg (0.5) · email IMAP (2) · Discord seam + WS fixture (3). **No credential
needed for any of it.**

**Wave 3 — the customer-visible gaps.**
`23A-C1` governed promotion, revocation, rollback (3–4) · `22-C1` Goals on protocol + TUI (Phase 23
exit gate) · `23B` 16-route census re-run · Phase 27 clauses closable without keys.

**Wave 4 — honest position.**
Phase 30 C2 needs per-tool dialect compilation + protocol v2 (`SR-30-3`) — **no credential**, and
without it no peer comparative can be re-taken at all.

---

## 5. Sean's list — five items, verified, nothing padded

1. **Merge to main** (1,374 commits ahead). Everything scheduled runs against `main`, so until this
   lands, all nightly coverage reports on a 16-day-old tree.
2. **Mint two trust roots in one sitting** — the release root *and* `INDEX_PUBKEY_HEX`, a second
   all-zeros placeholder that Phase 29 fences to Phase 25 and Phase 25 does not carry. It will
   surface as a release-day surprise otherwise.
3. **Tag / publish**, with the Desktop digest re-pin on the same train.
4. **The core#254 reply** — drafted, precondition cleared, ready to paste unchanged.
5. **Close #142** (quick-xml tracking; fixed at source).

**Killed with evidence, do not re-add:** Discord/Telegram vendor credentials (the fixture *is* the
API — Telegram's seam landed; Discord's is the same two-field change), the RC platform envelope
(met on three platforms), the voice build decision (already decided), the receipt supersession
(scope text excludes it).

**Genuine but near-worthless:** Phase 27 vendor keys. They buy grades on voice, which is compiled
into no shipped artifact.

---

## 6. What we are deliberately NOT doing

The 11 can-ship-open criteria stay off the blocking list. Treating every open criterion as blocking
is what turned Phase 20 into a 74-plan loop lasting two weeks.

**Parity is not the ship gate.** Phase 30's verdict is that the frontier position **cannot yet be
stated** — nine trial legs are confounded because the script spoke one competitor's dialect. The
cheapest route to a statable position needs no credential and is in Wave 4. Full parity against two
mature products is a multi-month goal; a defensible release is not the same target.
