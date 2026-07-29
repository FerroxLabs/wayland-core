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

---

# T+75 — M4 resolved, native actions fully defined, two more instrument defects

## M0-c. `head -30` truncation nearly produced a false absence

My production-`Attachment {`-construction list was cut by `head -30` and discord was **not in
it**. I was one step from reporting "the reference adapter never builds an inbound attachment".
It does — `crates/wcore-channel-discord/src/gateway.rs:381-391`. **A truncated list is not an
absence.** All absence claims in this lane are re-run without `head`.

## M0-d. A SUBSTRING false POSITIVE — the mirror of §3b-i

I reported `AckMode|ack_mode` = **56 hits**. That number was wrong. `ack_mode` matches inside
**fallb`ack_mode`ls**:

| search | count |
|---|---|
| `/usr/bin/grep -rn "AckMode\|ack_mode"` (substring) | 56 |
| …of which are `fallback_models` contaminant | **38** |
| `/usr/bin/grep -rnw "AckMode\|ack_mode"` (word-boundary) | **14** |

§3b-i is written about false *negatives*; this is the same defect class producing a false
*positive*, and an inflated "56 call sites" would have made a decorative surface look
well-used. **All counts in this lane use `-w` where the token can embed.**

## M4 RESOLVED — the declared bound is enforced NOWHERE; three different numbers are in play

`fetch_media_on` (`manager.rs:774-785`) does **not** consult `media_bounds()`; it delegates
straight to the adapter. Where a cap exists it is an unrelated hardcoded constant:

| adapter | **DECLARES** `media_bounds()` | **ACTUALLY** enforced | where | divergence |
|---|---|---|---|---|
| discord | 25 MiB / 10 attachments | **100 MiB** | `discord/src/rest.rs:370` `MAX_MEDIA_BYTES` | **4× larger than declared** |
| email | 10 MiB / 20 attachments | **2 MiB** | `email/src/imap.rs:619` `MAX_INLINE_ATTACHMENT_BYTES` | **5× smaller than declared** |
| other 8 | (trait default 25 MiB / 10) | nothing declared, nothing consulted | — | — |

The trait doc at `wcore-channels/src/lib.rs:165-166` states the bound is *"Enforced by
[`media::normalize`]"*. **`media::normalize` has no production caller.** The doc asserts an
enforcement that does not occur.

`max_attachments` is enforced nowhere at all. Control proving the instrument: the sibling
declared bound `max_message_len` has **9** non-definition production uses, so this search shape
DOES find consumers when they exist; `max_attachments` has **0** outside declarations,
`media.rs` itself, and one test.

Worse for discord specifically: its `MessageAttachment` (`gateway.rs:129-135`) deserializes only
`url` and `content_type` — **it never parses Discord's `size` field**, so a per-attachment size
bound is unenforceable there *by construction*, independent of the dead `normalize` path.

**Grading this honestly:** the OOM/SSRF defenses are real and correct (host allowlist +
`read_body_capped`). What is decorative is the **declared, per-adapter `MediaBounds` API** and
the `MediaDisposition::Degraded` "never drop silently" record, which no production path emits.
I file this as **F24-C3-H6 (MEDIUM)** — no memory-safety or SSRF exposure, but an advertised
enforcement surface that does nothing, on the exact clause I was sent to measure.

## M5. `native actions` — the production path, fully mapped

`channel_inbound.rs:503-556`, `run_turn`, a three-step best-effort state machine gated by
`AckMode`:

1. `ack.reactions()` → `react_on(ch, conv, msg_id, "👀")` **on receipt**
2. `ack.typing()` → `spawn_typing_keepalive(...)` wrapped in `AbortOnDrop` for the turn
3. after dispatch → `react_on(..., "✅")` on Ok / `"❌"` on Err

`AckMode` defaults to **`Off`** (`dispatch/access.rs:191`), so **no native action fires unless
`[inbound] ack` is configured**. That default is itself why six lanes could run inbound matrices
and never once exercise this clause — the surface is off unless asked for.

Both `react_on` failures are **swallowed** (`tracing::debug!` and `let _ =`). Consequence for
instrument design: *Core's own logs cannot prove a native action occurred.* It must be measured
on the PLATFORM side. Fixture-side counting is therefore the only valid instrument, not a
convenience.

Adapter support (override present in `src/`; counts include in-file `#[cfg(test)]`):

| adapter | react | send_typing | fetch_media | builds inbound Attachment |
|---|---|---|---|---|
| discord | yes | yes | yes | **yes** (`gateway.rs:381`) |
| telegram | yes | yes | yes | yes (`longpoll.rs:220`) |
| matrix | yes | yes | yes | (lane/24-h6 owns this crate — not touched) |
| slack | yes | — (default `Ok(())`) | yes | yes |
| whatsapp | yes | — | yes | yes |
| msteams | — | yes | — | — |
| email / imessage / signal / sms | — | — | yes | yes (except sms metadata) |

Trait-default asymmetry, noted and NOT inflated: `send_typing` defaults to a silent `Ok(())`
(`lib.rs:256`) while `react` defaults to a named `Unsupported` (`lib.rs:279`). The `send_typing`
default is explicitly documented as deliberate ("platforms without a typing API simply do
nothing", best-effort, failure ignored). I judge it **defensible as documented**, not a finding.
Recording it because it means a `send_typing` `Ok` is not evidence of a typing indicator.

## M6. Reachability constraint that shapes the whole measurement

`discord/src/rest.rs:337` — `MEDIA_HOSTS = ["cdn.discordapp.com", "media.discordapp.net"]`, and
`download_bytes` refuses any other host. There is **no env/config override seam** (`api_base_url`
is configurable; the CDN allowlist is a hardcoded `const`). So a local fixture **cannot serve
discord attachment bytes** — correctly, this is deliberate fail-closed SSRF defense
(`rest.rs:615-622` tests it against `169.254.169.254`).

This does NOT block the media clause on discord, because `enrich()` short-circuits **before any
fetch** when no backend is configured (`channel_media.rs:164-172`): `Image` + `vision.is_none()`
→ writes `IMAGE_NO_VISION_NOTICE` and `continue`s. So the honest-degradation half of the media
clause IS measurable on the reference adapter with zero credentials and zero network.

## Measurement plan (what I will actually drive)

Target: **discord**, a designated reference adapter, via the existing `DiscordFixture`
(`scripts/f24-discord-fixture.mjs`) which already journals `typing[]`, `reactions[]` and
`report()` totals. I will NOT edit it, nor `scripts/f24-inbound.mjs`; my driver is a new
additive file that imports it.

| leg | instrument | positive expectation | negative control that must redden |
|---|---|---|---|
| native actions | fixture `reactions_total` / `typing_total` | `ack="both"` → 👀 + ✅ reactions ≥2, typing ≥1 | `ack="off"` → **0 / 0** |
| media (degraded) | LLM fixture captures turn prompt | image attachment → prompt contains `IMAGE_NO_VISION_NOTICE` | text-only message → notice absent |

Vision live leg: **UNREACHABLE** — no `ANTHROPIC_API_KEY`/`OPENAI_API_KEY`/`GEMINI_API_KEY`
available and `build_vision_backend` never consults `FLUX_API_KEY`. Reported as a determination,
not attempted.

---

# T+150 — MEASURED. Both clauses driven on discord; report written.

Runs 3 and 4 both `all_pass=true`. G1 `reactions=2 emojis=["👀","✅"] typing=1`; G2 `turn_ran=true
reactions=0 typing=0`; G3 `notice=true`; G4 `capture_alive=true notice=false`.

Instrument defects this lane: **7**, three of them in my own driver (sync sleep blocking the
in-process fixture; reading `report.reactions` which `report()` does not expose; plus the
matcher hazard the self-test guards). Both driver defects produced FAILs, never false passes.

Gates proven able to redden by real runs, not only mutation: run 1 failed 4/4 (NOT MEASURED),
run 2 failed G1 alone.

Secret sweep executed: value length 51, name-hits 3 (control alive), value-hits **0**. No
credential used; spend zero.

Final report: `24-MEDIA-ACTIONS.md`. Verdict: `24-C3` STILL NOT MET and not claimed.
