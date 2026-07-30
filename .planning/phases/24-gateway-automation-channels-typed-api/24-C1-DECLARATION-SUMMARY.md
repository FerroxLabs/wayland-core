---
lane: lane/24c1-declaration
base: 0d48b551
phase: 24
criterion: 24-C1
status: DELIVERED
grade-24-C1: >-
  The residual is now WRITTEN DOWN and ENFORCED, not closed. docs/delivery-semantics.md states
  the per-adapter guarantee — exactly-once on 3, at-most-once on 7, at-least-once on 0 — with
  every cell traced to a source line or a measurement, and a test that fails the build if the
  table and the code ever disagree (proven in both directions on the real artifacts). The
  criterion's no-loss half is UNCHANGED and I do not upgrade it: this lane wrote the
  description, it did not change any adapter's behaviour. What it does change is that the
  remaining decision is now a one-sentence approval of a document that is already true, rather
  than a policy Sean has to invent.
---

# 24-C1 — the delivery-semantics declaration

## What was asked, and the one-line answer

Write the per-adapter delivery-semantics declaration **from measurement**, and make it
enforceable. Done: `docs/delivery-semantics.md` plus
`crates/wcore-channels-registry/tests/delivery_semantics_declaration.rs`.

**The brief's central judgement — "every adapter fixable in code has been fixed" — was tested
and HOLDS.** I fixed no adapter and say so plainly. Three of its measured figures were
re-verified and all three held.

## 1. Premise check (LANE-BRIEF "your brief's measurements are probably stale")

| Brief's claim | Verdict at base `0d48b551` |
|---|---|
| exactly-once is **3 of 10** | **HOLDS** |
| Matrix overrides at `lib.rs:294` | **HOLDS**, byte-exact |
| Discord overrides at `lib.rs:344` | **HOLDS**, byte-exact |
| "no-loss fails on 7 of 10, an earlier 9-of-10 was wrong" | **HOLDS** — 3 override, 7 inherit `false` |
| "every adapter fixable in code has been fixed" | **HOLDS** — tested, see §4 |

Unusually, the brief was accurate throughout. `CRITERIA-STATUS.md:27` is the stale artefact: it
still reads *"no-loss still fails on 9 of 10"*. **That row is wrong and should read 7 of 10.**
I did not edit it — the status file is contended and re-grading is the orchestrator's.

## 2. The table (full version in `docs/delivery-semantics.md` §2)

| | Adapters |
|---|---|
| **exactly-once** | Slack, Matrix, Discord |
| **at-most-once** — outcome-unknown delivery is *abandoned*, recorded, operator-queryable | Telegram, Twilio SMS, WhatsApp, Email, Signal, iMessage, MS Teams |
| **at-least-once** | none |

**Five of ten rows say NOT MEASURED and mean it** — Discord, Email, Signal, iMessage, MS Teams
have never had a replay driven at a real destination. Discord is the uncomfortable one: it is in
the exactly-once column on the strength of a mockito test, and its dedup *window* is unbounded
(`BL-24C1-DISCORD-WINDOW`). The row says so in the guarantee cell itself, not in a footnote.

The four live rows (Slack, Telegram, Twilio, WhatsApp) are the known-positive that makes the
rest interpretable: a replayed key genuinely produced **two** messages at three real
destinations, so the abandonment prevents a real duplicate, not a theorised one.

## 3. The two things a comfortable version of this document would not say

**(a) The guarantee is scoped to a delivery id, not to a message.**
`FireContext::delivery_id()` (`wcore-cron/src/runner.rs:324-338`) is
`cron:{job_id}:{scheduled_for_millis}[:{occurrence}]`. Exactly-once means *one job, one scheduled
instant, one message*. If anything mints a second delivery id for what a customer calls the same
message, no adapter's dedup can suppress it, because **a different key is not a replay**.

**(b) F24-GWP-H1 makes the exactly-once rows conditional on Windows — for all three of them.**
The finding is confronted rather than worked around. `lane/gateway-platforms` measured, at the
sink's own journal, Windows `{2:12, 3:1}` against macOS `{1:13}` — twelve deliveries each
arriving twice at the Task Scheduler `PT1M` boundary. Its ledger recorded **27 distinct delivery
ids, each settled exactly once**: the spine was perfect and the duplicate was created *above* it.
So this is a platform row that applies to **every adapter**, not a subset, and the document says
so. I did not write "exactly-once" for any adapter on Windows without that qualification.

**And I did not grade from the headline.** `F24-GWP-M1` — the same run's receipt read
`duplicates: 0`. The Windows section is sourced from the sink's journal for exactly that reason,
and the drift test asserts the histogram stays in the document, because a warning without its
evidence is just an assertion.

## 4. Was anything actually fixable? Tested — no

Cross-audit panel, all three legs live: **codex 5.6 Sol, gemini 3.1 Pro, kimi K3 — unanimous
7/7 NO**, no platform of the seven accepts a client-supplied token the destination honours.
Codex supplied primary sources (Twilio's own retry-safety page, RFC 5321 §6.1 / RFC 5322 §3.6.4,
the signal-cli jsonrpc man page, the Bot Framework activity spec). Internal adversarial pass
argued the seven are "effectively exactly-once since we never re-send"; rejected — the message
may not have arrived, which is the definition of at-most-once.

**Nearest miss, and still a no: SMTP.** `make_outbound_message_id`
(`wcore-channel-email/src/smtp.rs:287`) mints a fresh `Message-ID` per send and deriving it from
the delivery key would take minutes. But no RFC and no common MTA guarantees dedup on it. Setting
`supports_outbound_idempotency() -> true` on "some mailboxes probably will" is a reassuring
sentence over code that does not implement the guarantee — the exact defect class this lane
exists to prevent. Left unchanged, documented as the one candidate a product decision could
revisit.

**One nuance kept because "the platform" ≠ "the API we use":** Telegram's MTProto API has
`random_id`; the Bot API's `sendMessage` does not expose it, and we are a Bot API client.

## 5. What I did fix — two live drift defects, both found by doing this work

1. **`docs/channels.md:178` listed "Outbound idempotency nonces" under "Not yet built."** Three
   adapters have had it since Phase 24 and Discord's is literally a nonce. A contradicting
   sentence two files from the declaration is the drift the declaration exists to stop.
2. **`wcore-cli/tests/f24_c1_outbound_idempotency.rs` carried three stale facts and a test named
   `..._by_slack_alone_...` that checks four of ten adapters.** Header said Slack was the only
   override, cited slack `:234` (now `:249`), and said "the other nine" (now seven). Corrected
   and renamed.

## 6. The drift test, and both directions

`crates/wcore-channels-registry/tests/delivery_semantics_declaration.rs` — the registry is the
only crate depending on all ten adapters. It `include_str!`s the document (a wrong path is a
compile error, not a silent zero-row parse), builds **all ten adapters through the production
factory** `channel_factory_for` with hermetic fixture configs and no real credentials, and
compares. It also asserts the row set and the constructible-adapter set are the **same set**, so
a new adapter with no row fails the build — the drift a row-by-row check cannot see, because
there is no row to disagree with.

**Both directions, run on the REAL artifacts on hetzner, not reasoned about:**

| Run | Mutation | Result |
|---|---|---|
| known-positive | none | **`8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`**, rc=0 |
| A — doc over-promises | `telegram = at-most-once` → `exactly-once` in the document | rc=**101**, `2 passed; 6 failed` — *"the document says exactly-once … but the adapter returns false"* |
| B — code drifts | telegram adapter gains `-> true` | rc=**101**, `2 passed; 6 failed` — *"the document says at-most-once … but the adapter returns true"* |
| C — coverage proof | matrix `true` → `false` | old 4-adapter test **`6 passed`, rc=0 (BLIND)**; new census rc=**101**, naming `matrix` |

Mutation C is §6b-ii's third assertion — *the old instrument would have missed it* — executed,
not hypothesised. Every mutation reverted; `git status --porcelain | wc -l` = 0 after each.

Six of the eight tests are gates that can fail on a mutated input and were shown doing so. The
two that cannot fail on these mutations are the prose/machine-block agreement check and the
F24-GWP-H1 disclosure check; both were shown failing under mutation A.

## 7. Gate and instrument discipline

- **Every number in this document was written to a file and read with the Read tool**, never off
  a Bash stdout render. The brief's `rtk` warning is live: it fabricated counts for another lane
  the same day.
- Known-positive **and** known-negative in the same invocation for every absence claim. The
  "seven have no idempotency surface" grep returned 8 hits (all lifecycle `start`/`stop`
  idempotence) against a same-run known-positive of slack 12 / matrix 10 / discord 12.
- `N passed` read back on every run, with `0 ignored` and `0 filtered out` present — the ssh
  transport does not strip them the way the local `cargo` proxy does.
- SHA asserted after every checkout (`003661d8`, then `39f53536`).
- Hetzner `/tmp` files all prefixed `lane-24c1-`; 999G free at start, targeted per-crate builds
  only, `CARGO_BUILD_JOBS=10`.

## 8. What I did NOT do

- **Changed no adapter's behaviour.** No `supports_outbound_idempotency` flipped, no send path
  touched. The criterion's no-loss half is exactly where it was.
- **Did not fix F24-GWP-H1.** It is `lane/gateway-platforms`' open HIGH and a Windows recovery
  design decision; I documented its effect on the guarantee and left the fix to its owner.
- **Did not edit `CRITERIA-STATUS.md` or `CRITERIA-GAP-LEDGER.md`**, though `CRITERIA-STATUS.md:27`
  is measurably wrong ("9 of 10" should be "7 of 10"). Contended files; re-grading is the
  orchestrator's. Recommendation only.
- **No workspace build, no full-suite run** — targeted per-crate only, per §2 and because other
  lanes are live.
- **No real credentials anywhere.** Every fixture uses `*.invalid` hosts and `fixture.*` handles
  that resolve to nothing.
- Did not measure Discord's dedup window (`BL-24C1-DISCORD-WINDOW`, needs a credential on no
  build host), and did not drive Email, Signal, iMessage or MS Teams at a real destination.
  Those five appear in the table as **NOT MEASURED** rather than filled in.

## 9. The sentence for Sean

> **Approve that for the seven channels whose platforms provide no deduplication token, a
> delivery whose outcome is unknown is abandoned and surfaced to an operator rather than silently
> retried — every cell of the table cites the code or the measurement behind it, and a test fails
> the build if the code and the table ever disagree.**

He is approving a description, not inventing a policy. The only genuine choice inside it is
at-most-once-with-visible-abandonment versus at-least-once-with-duplicates for those seven, and
the code already implements the former.
