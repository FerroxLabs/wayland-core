# lane/readme-verified-live — NOTES

Base: `570056c160a7e497e67bbfe9798aaf3843ac639c` (integration `plan/f20-unified-audit-repair`).
Goal: make README state accurately which channel platforms have been driven at a real
destination, without over- or under-claiming.

## Premises from the brief — measured at HEAD

| # | Brief premise | Verdict at HEAD |
|---|---|---|
| 1 | `README.md:313` lists all ten channel platforms as implemented | **TRUE** — Slack, Discord, Telegram, WhatsApp, Signal, SMS, email, iMessage, Matrix, MS Teams |
| 2 | `docs/delivery-semantics.md` is build-enforced by `crates/wcore-channels-registry/tests/delivery_semantics_declaration.rs` | **TRUE** — file exists; doc §7 describes the four assertions; a machine-readable block at `:538-551` is read by the same test |
| 3 | Slack / Discord / Matrix driven at the real platform, 2026-07-30 | **TRUE in substance, imprecise in detail** — see finding F1 |
| 4 | 7 of 10 NOT MEASURED at a real destination | **TRUE** — Email, Signal, iMessage, MS Teams, Twilio SMS, WhatsApp, Telegram (delivery-semantics §2) |
| 5 | Exactly-once is 1 of 10, Matrix alone | **TRUE** — machine-readable block: `matrix = exactly-once`, all nine others `at-most-once` |
| 6 | Eleventh surface: opt-in WhatsApp Node bridge, no message ever sent through it | **TRUE** — delivery-semantics `:55-67`, `docs/whatsapp-bridge.md` |

## Findings

**F1 — the brief's "all five message actions" is exact for Slack only.**
`live_slack_actions.rs` has one test, `five_message_actions_against_the_real_slack_api`, with
legs `leg_send` / `leg_edit` / `leg_delete` / `leg_receive` / `leg_idempotency`. Discord's file
carries `live_edit_and_delete_against_real_discord` +
`live_edit_leaves_changed_content_for_an_external_observer`; Matrix's carries
`matrix_inbound_reaches_the_product_from_a_real_room` +
`matrix_edit_and_delete_against_a_real_room`. For those two the **idempotency** leg was driven by
a lane harness (delivery-semantics §8 and §9), not by a committed test file. All three were
genuinely driven at the real platform; only Slack has all five legs in one committed test.
Consequence for wording: say "driven at the real platform", not "all five actions under test".

**F2 — the README does not currently CONTRADICT delivery-semantics.md; it OMITS it.**
`/usr/bin/grep -n -i "exactly-once\|at-most-once\|idempoten\|delivery-semantics" README.md`
returns exactly one line, `:128`, about the worktree swarm — unrelated. The channels section
links only to `docs/channels.md`. So there is no false sentence to soften; there is a missing
distinction and a missing pointer. `docs/channels.md:183` does link delivery-semantics.

**F3 — `docs/delivery-semantics.md` has a duplicated heading block at `:117-125`.**
"### Correction, 2026-07-30 — the Slack row was wrong, and how" plus its following two lines
appear twice, back to back. Editing accident, not a correctness defect. Reported, not silently
fixed — the file is another lane's authority surface.

## Open

- Choose the form (column / table / limitations sentence) and justify.
- Add a machine-checkable agreement assertion between README and delivery-semantics, and show
  it can fail (§3.2) **and** that it can pass (§3b-iii).
