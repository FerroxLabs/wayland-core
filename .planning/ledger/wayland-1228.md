---
issue: 1228
repo: FerroxLabs/wayland
kind: task
title: "[Destination needed] Telegram cap 4096 may be UTF-16 code units: the astral arm is committed and needs a chat id (#934 c8)"
status: open
last_verified_commit: 9d716bcb
criteria:
  - id: c1
    text: "A chat id the bot may post to is RECORDED in the live-cap credentials home, not merely used once"
    state: blocked
    owner: maintainer
    note: "This is the whole blocker and it is a human step, which is why this entry is kind: task. The bot token works - the token at /root/wl-live-cap/credentials.toml answers getMe as WaylandTestBot. The 2026-08-29 boundary run that produced wayland#1186 c1 HAD a destination and did not record it; getUpdates now returns zero rows because that run confirmed its offset and Telegram drops confirmed updates. Both cheaper routes were MEASURED shut on this branch: sendMessage to a nonexistent chat_id answers Bad Request: chat not found at BOTH 4,096 and 4,097 characters, and to a public channel the bot has not joined it answers Forbidden: bot is not a member of the channel chat at 10, 4,096 and 4,097 - so Telegram resolves chat and membership BEFORE it validates length, and no destination means no verdict. A bot cannot obtain a chat by itself."
  - id: c2
    text: "The astral run is executed and both arms of its LIVE_CAP_UNIT block are pasted onto the issue"
    state: not-met
    owner: core
    note: "Core work the moment c1 exists; the harness is already committed. drive_boundary() fills the body with U+1F600 instead of x when WL_LIVE_CAP_TELEGRAM_ASTRAL=1 and prints LIVE_CAP_UNIT with both arms. Both arms are required so a one-sided result cannot be read as a verdict."
  - id: c3
    text: "The verdict is APPLIED: a refusal at 4,096 astral scalars drops the declared cap to 2,048, an accept leaves 4,096 standing, and the telegram CapUnit cell stops being UnsettledAsciiOnly"
    state: not-met
    owner: core
    note: "A refusal means a shipped HIGH-6, not a documentation gap: max_message_len is Some(4096) at crates/wcore-channel-telegram/src/lib.rs:431, and if Telegram counts UTF-16 code units then 4,096 non-BMP scalars cost 8,192 units, the platform refuses, and send_to_keyed does not re-send. The checker is already built and already has a red arm without a live run - unit_safety_faults() refuses a scalar cap above limit/2 once a UTF-16 verdict is recorded, and the_unit_rule_refuses_a_cap_a_utf16_verdict_makes_unsafe constructs exactly the verdict an astral run would produce and requires it to refuse today's 4,096."
  - id: c4
    text: "docs/delivery-semantics.md records the unit, the date, and which arm settled it"
    state: not-met
    owner: core
    note: "So the next reader does not re-derive the unit from a note on a criterion marked met, which is how this residual went untracked in the first place."
---

Filed 2026-08-29 to carry the remainder of `wayland#934` c8, which was
`blocked owner=maintainer` with no ticket naming what now owns it.

It is NOT a duplicate of `wayland#1186`. That ticket's c1 is recorded MET: on
2026-08-29 a live boundary run measured Telegram at 4,096 accepted / 4,097
refused with `400 Bad Request: message is too long`. That run was driven in
ASCII, and its own note records the residual verbatim - whether the platform
counts characters or UTF-16 code units is still open and must not be read as
settled. A residual recorded in the note of a criterion marked `met` is not
tracked; it is a remainder nobody can find. #1186's telegram row is stale in
the other direction too: it says no harness is written, and one now exists and
is committed.

`kind: task` because the binding constraint is a destination a human must
obtain. Everything downstream of it is authored and green, and c2 through c4
are core's the moment c1 exists.
