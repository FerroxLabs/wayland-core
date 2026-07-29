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
| ~~4~~ | ~~session journal unreadable~~ — **DOWNGRADED HIGH→MEDIUM 2026-07-29.** Does not reproduce: **92/92 runs reached the code, 153 tool events, 0 mismatches**, across three binaries including the original sighting's own base commit, under fsync saturation. **The prior evidence was worthless in both directions** — the reproduction harness pointed at a dead port with a placeholder key, so no run ever dispatched a tool event, and non-reaching runs were silently counted as successes. A **non-reproduction, not a disproof**; root cause still unidentified and the residual named | `23B-H1-MEASURED.md` | **MEDIUM, backlog** |
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

**ADDED 2026-07-29, and this one is worth more than any other credential on the list —
6. A REAL PROVIDER CREDENTIAL for the durability harness.** `23B-H1` — a clean exit writing a
session journal the product cannot read back — was independently re-verified and **reproduces as
unresolved at HIGH**. The lane could neither reproduce nor disprove it, because every attempt failed
before reaching a tool event: the harness cannot dispatch without a real provider key. **So a HIGH
data-loss defect is currently unmeasurable by anyone.**

That is a different class from the Phase 27 keys. Those buy grades on a feature we do not ship;
this one restores the ability to measure a defect that is in the product now. **It is the single
highest-value credential you can supply.**

---

## 6. What we are deliberately NOT doing

The 11 can-ship-open criteria stay off the blocking list. Treating every open criterion as blocking
is what turned Phase 20 into a 74-plan loop lasting two weeks.

**Parity is not the ship gate.** Phase 30's verdict is that the frontier position **cannot yet be
stated** — nine trial legs are confounded because the script spoke one competitor's dialect. The
cheapest route to a statable position needs no credential and is in Wave 4. Full parity against two
mature products is a multi-month goal; a defensible release is not the same target.

---

## 7. Decision taken 2026-07-29 — outbound delivery semantics

**Question:** on 7 of 10 platforms there is no idempotency primitive at all (Telegram, Twilio,
Meta, SMTP, signal-cli, AppleScript, Teams). After a crash during send, should the product abandon
the delivery (at-most-once, current behaviour) or retry it (at-least-once, risking a duplicate)?

**Measured, not assumed:** replaying one delivery key through real Telegram / Twilio SMS / WhatsApp
adapters over real HTTP put **two messages at the destination**. The `false` those adapters report
is truthful, and the abandon is preventing a genuine duplicate — not covering for a stub.

**DECIDED: keep at-most-once as the default.** Reasons, in order:

1. A duplicate on a **metered** channel (Twilio SMS) costs the operator money and cannot be undone.
2. The alternative was measured to produce real duplicates, not hypothetical ones.
3. 7 of 10 platforms cannot support the alternative safely at all, so at-least-once would be a
   per-channel patchwork with no uniform guarantee.

**Conditional on one thing, already dispatched:** at-most-once is only defensible if an abandoned
delivery is **visible**. Today it has no consumer outside `ledger.rs` and only a `tracing::warn!` —
a silent loss carrying a comment that claims it is "nameable by an operator". That is graded HIGH
and is being fixed. **If that surface does not land, this decision is wrong and must be revisited.**

**Also taken:** Matrix and Discord already put a dedup token on the wire and then defeat it (a
counter reseeded at 1 on restart; a nonce deliberately made distinct across restarts). Both get
restart-stable tokens and may then report `supports_outbound_idempotency` truthfully. That is 2 of
10 moved by fixing our own code, with no platform feature required.

**Not decided here, and not urgent:** whether to offer opt-in at-least-once per channel. It is
cheap once the two adapters above are honest, and pointless before then.

---

## 8. New risk introduced 2026-07-29 — the public REST surface changed version

`GET /openapi.json` now emits **OpenAPI 3.1.0**, not 3.0.3. This is a **side effect of a security
fix** (the utoipa bump that removed RUSTSEC-2024-0370 from the lock), not a deliberate API change,
and it was found by the lane driving the real binary rather than by review.

**Why it needs a decision rather than a note.** The shape changed, not just the version string —
measured live: **9 fields in 3.1's `type: [..., "null"]` form and 0 in 3.0's `nullable` form.** A
strict 3.0 client will not read this document. **No fixture covers `/openapi.json`**, so nothing in
CI would have caught it and nothing will catch the next one.

**It must ride the same train as the Desktop digest re-pin.** Desktop is the primary consumer and
`observation.rs:329` already makes a contract mismatch a hard error at `ready`; a REST surface that
silently changes shape is the same failure with no gate in front of it.

**Open question, not yet answered:** does any consumer actually parse this document strictly? If
Desktop does not consume `/openapi.json` at all, this is free and should be recorded as free. If it
does, we either pin the emitted version or co-release. **Nobody has checked** — and the pattern this
week is that "no consumer" claims are wrong about as often as they are right.
