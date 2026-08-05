# 24-C1 — Outbound idempotency: verification, measurement, and an honest grade for Criterion 1

**Lane:** `lane/24-idempotency` · **Base:** `ef1d97be` · **Build host:** hetzner `hz/24-idempotency`
**Subject:** OUTBOUND delivery. The inbound path and `scripts/f24-inbound.mjs` belong to lanes
`24-c3-tg-email` / `24-c3-discord` and were not touched.
**Status:** investigation complete, measured, **no product fix landed** (reasons in §6).

---

## 1. The finding, verified before acting on it

All three claims are **TRUE**.

| Claim | Verdict | Evidence |
|---|---|---|
| The trait method defaults to `false` | TRUE | `crates/wcore-channels/src/lib.rs:139` — `fn supports_outbound_idempotency(&self) -> bool { false }` |
| Slack is the **sole** override in the workspace | TRUE | Workspace grep, 9 `*.rs` hits, `/target/` excluded. One adapter override: `crates/wcore-channel-slack/src/lib.rs:234`. The rest are the trait default, a forwarding method (`wcore-channels/src/manager.rs:665`), the spine's consumer (`wcore-agent/src/cron.rs:152`), and doc/test references |
| The 12-of-12 tally ran entirely through Slack | TRUE | `scripts/f24-journey.mjs:380` is `'platform = "slack"',` and is the **only** `platform = "…"` line in the driver — the journey has exactly one channel config |

Adapter count is **10** (`crates/wcore-channel-{discord,email,imessage,matrix,msteams,signal,slack,sms,telegram,whatsapp}`), each with exactly one `impl Channel for`. The 1-of-10 / 9-of-10 arithmetic is correct.

### One correction to the brief: this was **not** unfiled

The brief described it as "unfiled, found by an independent inventory an hour ago". It is filed, in near-identical language, in **four** committed documents at base — `.planning/CRITERIA-GAP-LEDGER.md:244`, `24-C-SUMMARY.md:241`, `.planning/REQUIREMENTS.md:233`, `.planning/ROADMAP.md:223` — all of which already assert the *answer* ("*now correctly abandoned rather than duplicated — safe and honest, and not the same thing as delivered*").

What was **never** filed is a measurement of it on any adapter other than Slack. That is what this lane closes.

---

## 2. What actually happens on the other nine — measured, not reasoned

### 2a. The mechanism is narrower than "every retry"

`wcore-gateway/src/automation.rs:129-209`. Abandonment fires only at the intersection of three conditions: the delivery id is a `Duplicate`, **and** its state is `Attempted` not `Settled` (the process died between `begin_attempt` and `settle`, so the outcome is genuinely UNKNOWN), **and** the destination cannot recognise a replay. A *known* failure never reaches it — both outcome arms call `settle`, so a failed send becomes `Settled` and the next fire short-circuits at `return Ok(())`.

So the honest claim is: **on nine of ten adapters, a delivery that was in flight when the process died is abandoned.** Not "all sends are abandoned".

### 2b. The decisive question the trait cannot answer

A `false` could be a truthful capability statement **or** an unimplemented stub on a platform that would in fact have deduplicated. Those call for opposite fixes. So I replayed one delivery key twice through **real adapters over real HTTP**, built by the **production factory** (`wcore_channels_registry::auto_register_from_dir`, the same path a deploy uses), and counted arrivals at the destination.

Harness: `crates/wcore-cli/tests/f24_c1_outbound_idempotency.rs` (new; **no manifest and no `Cargo.lock` change** — `wcore-cli` already carries `wcore-agent`, `wcore-channels-registry`, `wcore-cron`, `wcore-gateway` and `mockito`). Three different transports plus the known-positive.

**Result — the `false` is TRUTHFUL. A replay really does duplicate:**

```
POST /bot111:AAAA-f24c1-bot-token/sendMessage    idempotency-key: (missing)  ...but received 2
POST /2010-04-01/Accounts/AC…/Messages.json      idempotency-key: (missing)  ...but received 2
POST /v18.0/10987654321/messages                 idempotency-key: (missing)  ...but received 2
POST /api/chat.postMessage    idempotency-key: cron:f24c1-job:1785121776528  ...but received 2
```

Telegram, Twilio SMS and WhatsApp each put **two messages** at the destination with **no dedupe token on the wire**. Slack carried the key on **both** attempts — that known-positive is what makes the other three interpretable rather than merely absent.

**So the answer to the brief's question (2) is: ABANDON** — and, critically, abandoning is preventing a *genuine* duplicate, not a hypothetical one. The design's choice is the right one on the facts.

### 2c. Gate results, with the counts read back

Isolated run on hetzner at `5d71b7e3` (`cargo test -p wcore-cli --test f24_c1_outbound_idempotency`, by target file, never by filter):

```
running 6 tests
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Executed count read back as **6**, not zero (§3.2 zero-test trap). Status via sentinel file `WLRC=0` + `WLDONE`.

**The gate can fail — proven twice, not asserted:**

- **M1** (`.expect(2)` → `.expect(1)`, all 4 sites): 4 failed / 2 passed, and mockito printed the true hit counts quoted above. This is what makes "two messages arrived" a **positive measurement** rather than a vacuous no-duplicate green.
- **M2** (trait default `false` → `true` at `wcore-channels/src/lib.rs:139`): reddened **exactly** the two capability tests and nothing else — proving they read the real adapter → `ChannelManager` → `EngineJobHandler` chain rather than restating source. Reverted; clean re-run returned to 6 passed / 0 failed.

Anti-self-passing built in: every replay asserts a real parsed receipt from the fixture **before** any count, labelled `INSTRUMENT_FAULT` so a suspect run grades **INCOMPLETE, never LOSS**; and the no-token discriminator is structural (`Matcher::Missing`), so an adapter that started sending a token would fall through to mockito's 501 and redden the file rather than silently keep passing.

---

## 3. A second finding, which I rate above the one I was sent for

The abandon path justifies itself in-source as: *"recorded, terminal, and nameable by an operator … a delivery whose fate is genuinely unknown must be surfaced"*.

**The code does not implement that.**

- `ledger.rs:214 pending()` filters to `Accepted | Attempted` — **`Abandoned` is excluded**.
- `ledger.rs:223 pending_count()` — same filter, and its doc says *"the number drain publishes"*.
- `ledger.rs:253 compact()` classes `Abandoned` as terminal history under the `retain_settled` bound — an abandoned delivery is **eligible to be compacted out of the journal**.
- `DeliveryState::Abandoned` has **no consumer anywhere outside `ledger.rs`** (workspace grep; other `Abandoned` hits are unrelated types — `CronFireOutcome`, `HookPhaseState`).

The only signal is a single `tracing::warn!`. No gateway verb lists an abandoned delivery, nothing re-sends it, and the record can be deleted. **This is the part that is entirely within our own code**, and it is what converts a deliberate, defensible recorded non-delivery into an effectively unrecoverable one.

---

## 4. Honest grade for Criterion 1

Criterion 1 is a **conjunction** — the gateway's own header states *"no delivery is lost and none is duplicated"*.

- **No-duplicate half: HOLDS** on all ten, and now measured on four rather than one.
- **No-loss half: FAILS on nine of ten**, by construction, in the crash-during-send window.

> **Criterion 1 is NOT MET.** Its 12-of-12 tally is graded on the one adapter of ten that implements the guarantee being tested. A defensible restatement is: *"no duplicate delivery on 10/10 adapters (measured on 4); exactly-once delivery on 1/10 (Slack, Linux only). On the other nine an outcome-unknown delivery is abandoned, and that abandonment is currently unrecoverable and unsurfaced."*

That is what I would put in the ledger in place of the current claim.

---

## 5. Severity — and where I depart from the brief's prior

The brief's prior was "at least HIGH, same family as the inbound silent-loss HIGH". Measurement changed two premises: it is **not silent** (there is a `warn!`, and it is a deliberate announced choice between two bad outcomes), and for most adapters it is **not fixable** (see §6).

Cross-audit panel (§4), asked with the measured facts: **codex 5.6 Sol = HIGH, gemini 3.1 Pro = MEDIUM, kimi K3 = MEDIUM**. Internal adversarial pass argued HIGH and produced the §3 evidence, which is what splits the grade.

I take the **majority on the policy** and the **minority on the sub-part where the evidence is strongest**:

| Item | Severity | Reasoning |
|---|---|---|
| The abandonment policy itself (7 platforms with no primitive) | **MEDIUM** | It is the correct and *only available* trade. Bounded to the crash-during-send window. The measured alternative is a real duplicate, which the criterion forbids. A HIGH here would demand a fix that cannot exist — the platforms provide nothing to fix it with. Backlog, non-blocking |
| **No operator surface or recovery path for an abandoned delivery** (§3) | **HIGH** | The source claims a mitigation the code does not deliver. `Abandoned` is excluded from `pending()`/`pending_count()`, compactable, and consumed nowhere. This *is* fixable in our code, is independent of every platform limit, and is what makes the loss unrecoverable rather than merely recorded |
| Matrix / Discord abandoned despite already having a wire token (§6) | **MEDIUM** | Real lost deliveries that were cheaply preventable, but bounded to the same narrow window |

I did **not** grade the Matrix txnId-reuse hypothesis (§6) at any severity: it is unmeasured.

---

## 6. Cost of closing — it is **not** uniform across the nine, and that reframes the finding

This is not "nine adapters are missing a feature we should go build".

- **7 of 10 platforms provide no idempotency primitive at all** — Telegram Bot API, Twilio Messages, Meta Graph, SMTP, signal-cli, AppleScript iMessage, MS Teams. For these, `false` is permanent and truthful. **Not closable by code.** Closing them requires a *product decision* — an explicit per-channel at-most-once vs at-least-once policy exposed as configuration — not an implementation.
- **2 of the 9 already put a token on the wire** and are cheap:
  - **Matrix** — `rest.rs:47` PUTs `…/send/m.room.message/{txn_id}`, which *is* the Matrix protocol's native idempotency slot. But `txn_id` comes from `static TXN_COUNTER: AtomicU64 = AtomicU64::new(1)` (`rest.rs:13`), a process-local counter that **resets to 1 on every restart** — it does not survive the exact event the ledger exists for. Threading the ledger's stable delivery key into it is a small change. **≈0.5 session.**
  - **Discord** — already sends a dedup `nonce`, reused across the in-adapter retry loop (that closed HIGH-7). But `next_nonce()` is `{wall-clock-ms:x}-{counter:x}`, documented as deliberately *"distinct across restarts"*, so a post-restart replay is not deduped. Deriving it from the delivery key (hashed to Discord's 25-char cap) is small; the residual risk is Discord's dedup window, which must be measured, not assumed. **≈0.5 session + a measurement.**
- **The §3 operator-surface defect** — list abandoned deliveries in the gateway's operator surface, exempt them from compaction until acknowledged, and give them a re-send path. **≈0.5–1 session**, no platform dependency. **This is the one I would do first.**

**Unmeasured hypothesis, deliberately not graded:** because Matrix's counter restarts at 1, a fresh process reuses txnIds 1,2,3… for *different* messages; a homeserver still holding the old txn in its per-access-token cache could return the original event and silently drop the new one. That is a distinct potential loss path. **I did not measure it and it must not be treated as fact.**

---

## 7. What I did NOT do

- **No product fix landed.** The cheap fixes (§6) are scoped but unbuilt — this lane's budget went to verification and measurement, and per the brief a measured refusal with the call sites is the deliverable.
- **No real `kill -9` end-to-end** through the full gateway service on a non-Slack adapter. My replay drives the real adapters over real HTTP but reaches the outcome-unknown state at the manager layer, not via a process kill; the spine's branch on that state is already unit-proven (`automation.rs:516`, `:559`) and 24-C did the real systemd kill for Slack.
- **Six adapters unmeasured for replay behaviour**: email, signal, iMessage have no HTTP seam at all (SMTP / subprocess / AppleScript); Matrix, Discord and MS Teams have one but were read rather than driven.
- Nothing touched in `scripts/f24-inbound.mjs` or the inbound path (other lanes), and no shared-fence file (`wcore-cli/src/lib.rs`, `src/main.rs`) was edited — the harness is a new `tests/` file.
- No merge, no PR, no tag, no issue closed.

---

## 8. Verdict

The finding is **real and correctly stated**, though it was already filed. The measurement upgrades it from an inference to a fact: on Telegram, Twilio SMS and WhatsApp a replay genuinely produces two messages, so abandoning is preventing a real duplicate and the design's choice is right.

**Criterion 1 is NOT MET** — graded on the single adapter of ten that implements the property under test. The abandonment is **MEDIUM** and mostly unfixable; the **unsurfaced, unrecoverable, compactable abandoned-delivery record is HIGH** and is fixable in our own code.
