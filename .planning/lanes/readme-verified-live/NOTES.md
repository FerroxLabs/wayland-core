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

## Decision — form

**A prose bullet placed immediately after the ten-platform sentence**, first in the existing
channels bullet list. Cross-audit panel unanimous (codex `PANEL_POSITION=C`, gemini `C`, kimi
`C`); internal adversarial pass argued for a 10-row table and lost on a specific piece of
evidence: `delivery-semantics.md`'s own evidence cells could **not** be reduced to Yes/No — every
one of them carries prose inside the cell ("**NOT MEASURED at a real destination** — see the
correction below"). A README column would force the binary the authority doc itself declined to
use, and the binary is what invites reading the seven as broken. Under-claiming is also a wrong
claim.

Rejected `:348` ("What we do not claim yet") as the location: that paragraph is about **feature
completeness** (duplex, attachments, staged `--slash` jobs), a different axis from **evidence
strength**, and it sits 35 lines below the claim it would be qualifying.

Not mentioned: the WhatsApp Node bridge. It adds no platform (same `whatsapp` platform string,
opt-in `backend` key), is not shipped, and has never sent a message — so the README's "ten" stays
correct and silence is the accurate posture. Mentioning it would over-claim capability.

## Enforcement — `crates/wcore-channels-registry/tests/readme_live_evidence_agreement.rs`

Parses the *"Replay measured at a real destination?"* column of §2 and requires the README to
state the same partition: both spelled counts, each platform on the correct side (exclusive —
neither set may leak across), and the link to the enforced doc.

Run on `hetzner-dsm`, worktree `/root/wayland-readme-live` at
`c76d7c108436889c1852e6216fea16c8c7e6f21b` (SHA asserted after checkout).

| run | result |
|---|---|
| at HEAD | `6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| README mutated on disk, `Three` → `Four` | `WLRC=101`, `3 passed; 3 failed; 0 ignored; 0 filtered out` |
| pre-existing `delivery_semantics_declaration` | `8 passed; 0 failed; 0 ignored; 0 filtered out` — unaffected |

**Can it fail** — the mutated run reported, from the real files:
`README says 4 platform(s) have been driven at the real platform; docs/delivery-semantics.md §2
shows 3`. The two collateral failures are the `replace_once` guard firing
(`control cannot be constructed: "Three of the ten" is not present in the input`), which is the
anti-vacuity assertion proving those controls read the real README and not a private copy.

**Can it pass** (§3b-iii) — `the_comparator_passes_when_a_seventh_row_becomes_measured`
constructs the world in which Telegram *has* been driven live (doc cell rewritten, README counts
and sides updated) and requires zero problems. It passes, so the gate has a reachable pass state
under a changed fact and is not stuck green or stuck red.

`0 ignored` / `0 filtered out` are present in every count above, so none of these runs came
through the `rtk` cargo rewrite that strips exactly those two fields. Logs were fetched by `scp`
and read with the Read tool, never through a proxied shell.

`cargo fmt --all -- --check` → rc=0 on the Mac.
