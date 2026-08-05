# 5. Delivery semantics for platforms with no idempotency primitive

Date: 2026-07-31
Status: Accepted (Sean, 2026-07-31)

## Context

When the gateway sends an outbound message and the connection dies before the
platform answers, the outcome is genuinely unknown: the message may have arrived,
or it may not. There are exactly three responses available, and no fourth:

1. **at-least-once** — send again. If the first send did arrive, the destination
   now shows it twice.
2. **at-most-once** — do not send again. If the first send did not arrive, nobody
   ever receives it.
3. **exactly-once** — send again carrying a key the destination recognises, so the
   destination discards the duplicate itself. Only available when the platform
   actually honours such a key.

Option 3 is available on **1 of 10** adapters. Matrix honours
`PUT …/send/m.room.message/{txnId}`, where the transaction id *is* the idempotency
slot, and that has been driven end-to-end against matrix.org across a real
`kill -9`.

It is **not** available on the other nine, and two of those were believed to have
it until they were driven live on 2026-07-30:

* **Slack** — the adapter sends an `Idempotency-Key` header; `slack.com` ignores
  it. A replayed key produced **two** messages.
* **Discord** — the adapter sends `nonce` on message create; Discord does not
  dedupe on it. A replayed key produced **two** messages.

Both had held the exactly-once claim on mockito evidence. That is the origin of the
standing lesson that a mock proves what we send and nothing about what the
destination does.

The remaining seven — Telegram, Twilio SMS, WhatsApp (Meta Graph), Email (SMTP),
Signal, iMessage, MS Teams — expose no dedup slot at all. For them the choice
between 1 and 2 is a property of the platform, not of our implementation. No amount
of engineering makes option 3 reachable.

## Decision

**Keep at-most-once as the default on every platform that provides no idempotency
primitive. Do not add automatic retry.**

This ratifies what already ships rather than changing it: nine of ten adapters
already abandon an outcome-unknown delivery instead of retrying it, and the
abandonment is recorded, listed by `wayland-core gateway abandoned`, and re-sendable
by an operator (`docs/delivery-semantics.md` §; closed under #109).

Three reasons:

1. **Auto-retry on a platform that cannot dedupe manufactures the exact defect we
   just finished proving in Slack and Discord** — duplicates we can neither detect
   nor clean up afterwards.
2. **An agent sends action-shaped messages**: one-time codes, approvals,
   "I've transferred the funds". A silent duplicate there is worse than a re-send a
   human authorised.
3. **The recovery queue removes the usual objection to at-most-once.** The message
   is not lost, it is parked for a human. "Nothing is ever silently dropped" is the
   property that matters, and it is the one we can actually hold.

### Consequence for criterion 24-C1

`24-C1` reads *"no delivery lost **and** none duplicated"*. On seven platforms that
conjunction is unsatisfiable for reasons outside the codebase, so **as written the
row can never go green no matter what is built** — the permanently-red-gate failure
this project treats as equivalent to a gate that cannot fail.

The criterion is re-scoped to what is deliverable and provable:

> No delivery is lost **silently**; every outcome-unknown delivery is recorded and
> recoverable by an operator.

### Accepted follow-on, not part of this decision

A **per-channel operator opt-in to at-least-once**, default off. Some operators
would rather receive a duplicate outage alert than miss one. This is a small,
explicit, opt-in surface — it does not change any default.

## Consequences

* No behaviour change ships from this ADR. It ratifies the existing default and
  fixes a criterion that could not be satisfied.
* **The at-most-once claim itself remains unproven on seven of ten adapters.**
  `docs/delivery-semantics.md` marks Telegram, Twilio, WhatsApp, Email, Signal,
  iMessage and Teams as **NOT MEASURED at a real destination**. That is precisely
  the state Slack and Discord were in the morning before they were falsified, so it
  should be read as unproven rather than as working. Driving the recovery queue live
  on the channels we hold credentials for is the open work, and it is measurement,
  not design.
* Matrix's exactly-once holds only **below** `max_message_len`; above the cap the
  multi-chunk arm correctly drops the idempotency key, so a retry duplicates. That
  precondition is tracked separately (#153) and must be stated wherever the
  guarantee is published.
