# 24-MEDIA-ACTIONS — running NOTES (append-only)

Lane: `lane/24-media-actions`. Base: `e77b44b0` (`plan/f20-unified-audit-repair` at fetch time).
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-media-actions`.

Committed at T+~12 min per LANE-BRIEF §6b-i, before any investigation, and re-committed after
every measurement.

---

## T+12 — criterion text located at source (not a paraphrase)

`.planning/ROADMAP.md:119`, Phase 24 Success Criterion 3:

> Reference channels prove setup/auth, access, routing, **media**, **native actions**,
> idempotency, reconnect/reload, and health.

Eight clauses. Per the lane assignment and `24-C3-FINISH.md:99`, **`media` and `native actions`
have never been measured on ANY adapter** across six consecutive lanes. Every other clause has at
least one adapter measurement.

`.planning/CRITERIA-GAP-LEDGER.md:311-325` grades `24-C3` **PARTIAL (Linux), NOT MET (macOS,
Windows)** and calls it partially release-blocking.

## T+12 — what I must establish BEFORE building anything

1. What does the criterion's `media` clause actually promise? `README.md:348` reportedly discloses
   image->description and voice->transcript are inert without a vision/transcription key. A key
   exists at `~/.wayland-secrets/flux.env` (LANE-BRIEF §0 sanctioned: stdin only, sweep after,
   disclose). So the "no key" excuse may no longer hold — CHECK, do not assume.
2. What is a `native action` on an INBOUND channel? Which adapters expose one, and what does
   exercising it look like? If few/none do, that is a finding about the clause.

## T+12 — traps I am holding (from LANE-BRIEF)

- §3.2 a green from universal denial: the access leg once passed on 3 adapters because everything
  was DENIED. Prove positives with counts.
- §3b-i a known-negative is self-passing on a dead instrument. If I report "no media events", I
  must first prove the instrument can SEE one (known-positive in the same invocation).
- §3b unproxied tools for any number that reaches the report: `/usr/bin/grep`, `/usr/bin/git`.
- §6b-ii repair my own instrument in-lane; self-test with 3 assertions incl. "old matcher misses".
- Byte-count every capture; `${PIPESTATUS[0]}` returns empty here.
- `wcore_types::process_liveness` exists — use it, do not hand-roll a liveness check.
- Do NOT edit `scripts/f24-inbound.mjs` (shared, in active use by other live lanes).
  Off-limits: `.github/workflows/ci.yml`, `crates/wcore-cli/src/{lib,main}.rs`,
  `.planning/BACKLOG.md`, `crates/wcore-channel-matrix/` (lane/24-h6 owns it).

## Status

NOTHING MEASURED YET. Investigation starts after this commit.

---

# T+40 — source investigation complete. Both clauses now DEFINED.

## M0. TWO instrument defects caught before they produced a false number

**M0-a. zsh ate an unquoted `--include=*.rs`.** My first search for `native_action` returned
`0` — and my known-positive control (`ChannelManager`) ALSO returned 0, with
`(eval):1: no matches found: --include=*.rs`. Had I not run the control in the same
invocation I would have reported "native actions do not exist in source" off a dead
instrument. **This is LANE-BRIEF §3b-i happening live.** Fix: quote the glob (`"--include=*.rs"`).
Re-run gave known-positive **23 files**, so the instrument is alive; `native_action` is then a
TRUE zero.

**M0-b. NEW — bare `wc -c` reports 0 bytes for a 72-byte file.** Measured:

| command | result |
|---|---|
| `wc -c < ~/.wayland-secrets/flux.env` | **0**  |
| `/usr/bin/wc -c < ~/.wayland-secrets/flux.env` | **72** |
| `/usr/bin/stat -f%z ~/.wayland-secrets/flux.env` | **72** |

This is a new instance of the LANE-BRIEF §3b rewriting class, and it lands exactly on my
assignment's "byte-count every capture" instruction. A capture byte-counted with bare `wc`
reads **0**, which is indistinguishable from "the capture failed" / "nothing was produced" —
i.e. it manufactures the self-passing negative §3b-i warns about. **All byte counts in this
lane use `/usr/bin/wc` or `/usr/bin/stat`.**

## M1. `native actions` — the term does not exist in source; the CONCEPT does

Searched (instrument proven alive, 23-file known-positive in the same invocation):
- `/usr/bin/grep -rn "native_action\|NativeAction" crates "--include=*.rs"` → **0**
- `/usr/bin/grep -rni "native action" . "--include=*.md"` → only `.planning/` criterion text
  and lane summaries. **Zero hits in `README.md`, `docs/`, or any crate.**

So `native actions` is **criterion vocabulary with no implementation counterpart by that name.**
Per §3b-i rule 3 I searched the concept instead. The concept is the **ack surface** — the
platform-native affordance an adapter performs *on the platform* in response to an inbound
message:

- `README.md`: `ack = "both"   # off | reactions | typing | both`
- `AckMode` / `ack_mode`: **56 hits** across `crates`
- `crates/wcore-channels/src/dispatch/access.rs:89` — `pub fn typing(self) -> bool`
- per-adapter REST verbs: slack `add_reaction`, whatsapp `send_reaction`, matrix
  `send_reaction`, discord `add_reaction`

`24-C3-FINISH.md:351` independently reaches the same reading ("Discord's fixture already
implements typing + reactions REST"), which is corroboration from a prior lane, not my
invention.

## M2. `media` — the enricher IS wired into the persistent gateway inbound path

Not dead, contrary to what "inert without a key" might suggest at a glance:

- `crates/wcore-agent/src/channel_inbound_host.rs:220-231` builds `ChannelMediaEnricher` with
  `build_vision_backend()` + `build_transcription_backend(config)`;
- passes it into `ChannelTurnDispatcher::new(...)` (`:233-240`);
- `crates/wcore-agent/src/channel_dispatch.rs:138` calls `media.enrich(&mut
  enriched.attachments, channel_name)` — **the single production `.enrich(` call site**; the
  other 11 are unit tests in `channel_media.rs`.
- `bootstrap.rs:3255` wires the same enricher for the non-gateway path.

`enrich()` (`channel_media.rs:157-242`) writes an honest degraded notice into
`Attachment::transcribed` even when **inert**, so there is observable behaviour with NO key.

## M3. The key question answered: the "no key" excuse HOLDS FOR VISION, NOT FOR TRANSCRIPTION

`~/.wayland-secrets/flux.env` contains exactly one variable — **`FLUX_API_KEY`** (name only;
value never read, printed, or transmitted). 72 bytes, 1 assignment.

| leg | resolver | consults `FLUX_API_KEY`? | reachable with the available credential |
|---|---|---|---|
| image → description | `build_vision_backend()` `tool_backends/mod.rs:321-338` | **NO** — only `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` | **NO** |
| voice → transcript | `build_transcription_backend()` `tool_backends/mod.rs:370-403` | **YES — arm 4**, `read_env_key("FLUX_API_KEY")` → FluxRouter `flux-voice-fast` | **YES** |

So the lane assignment's hypothesis is **half right, and the half matters**: a prior lane proved
the *transcription* resolver reaches `flux.env`, and arm 4 is why. Vision has no such arm —
`FLUX_API_KEY` is never consulted by `build_vision_backend`. Cost note for the transcription
arm, from the source comment at `:365`: Flux transcription bills `$0.016670` with a
**10-second floor**.

## M4. NEW FINDING (candidate HIGH) — the media bounds subsystem has ZERO production callers

`crates/wcore-channels/src/media.rs` is a 353-line module whose documented purpose is to enforce
a declared per-adapter size/count bound and to guarantee "never drop silently". Measured call
sites, whole workspace, instrument alive (`MediaKind` known-positive = 17 files):

```
/usr/bin/grep -rn "media_bounds" crates "--include=*.rs"
  wcore-channel-email/src/lib.rs:535      fn media_bounds(...)   <- DECLARES
  wcore-channel-discord/src/lib.rs:405    fn media_bounds(...)   <- DECLARES
  wcore-channels/src/lib.rs:168           fn media_bounds(...)   <- trait default
  wcore-channels/tests/framework_matrix.rs:156  fn media_bounds(...) <- test impl
  wcore-channels/tests/framework_matrix.rs:373  let bounds = ch.media_bounds();  <- ONLY READER
```

**The only site that ever READS `media_bounds()` is a test.** Likewise `media::normalize` /
`normalize_all` have no production caller — the sole non-test references are that same
`framework_matrix.rs` (:379, :391) and a doc comment at `wcore-channels/src/lib.rs:166`.

Meanwhile the adapters build `Attachment` **directly**, bypassing normalisation entirely:
`slack/src/inbound.rs:133`, `telegram/src/longpoll.rs:220`, `sms/src/inbound.rs:174`,
`whatsapp/src/inbound.rs:218/228/238/251`, `email/src/imap.rs:858`, `imessage/src/channel.rs:67`.

Consequence to be verified before grading: the declared `max_bytes` / `max_attachments` bounds
that discord and email advertise appear to be **enforced nowhere on the inbound path**, and the
`MediaDisposition::Degraded` "never drop silently" record is never produced in production.
Only 2 of 10 adapters even declare bounds.

**Still to establish:** whether `fetch_media_on` enforces a size cap by another route (the
enricher does apply `VISION_MAX_BYTES` / `TRANSCRIPTION_MAX_BYTES` *after* download, which
bounds what reaches the model but NOT what is downloaded). Do not grade this until checked.

## Next

1. Check whether any size bound applies at fetch time (M4 open question).
2. Establish which adapters actually perform a native action inbound, and whether the ack
   surface is reachable from the gateway inbound path.
3. Then measure what is genuinely reachable, with counts, and prove the gate can redden.
