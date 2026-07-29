---
lane: 24-media-live
criterion: "24-C3 (reference channels / the inbound matrix)"
grade-24-C3: "STILL NOT MET, and this lane does not claim it. Seven lanes have declined it; this is the eighth. What changed: the `media` clause is no longer degraded-direction-only. Its AUDIO half is now proven in the POSITIVE direction — a real voice attachment inbound on telegram (a reference adapter), fetched by the connector, transcribed by a REAL provider, with the transcript reaching the model — live, on `hetzner-dsm`, from the real binary through `gateway run`, reproduced twice, with a one-variable negative control proven to redden and an anti-echo control that defeats a canned-response pass. The IMAGE half is NOT met and is structurally unreachable in current code. Six of the eight clauses are untouched by this lane; macOS and Windows still have nothing on any clause."
new-finding: "F24-C3-H7 (MEDIUM) — inbound VISION enrichment is unreachable by CODE ABSENCE, not by capability absence. `build_vision_backend()` takes no `&Config` and reads only ANTHROPIC/OPENAI/GEMINI, while `build_transcription_backend()` got TWO additional arms (config-resolved + `FLUX_API_KEY`). Measured live: the same credential drives Flux vision correctly over the OpenAI chat wire (HTTP 200, ground truth recovered). So an operator whose only key is `FLUX_API_KEY` gets working transcripts and a permanent 'set ANTHROPIC_API_KEY…' notice for images, though their existing key would work."
fence-exposure: "14 files vs merge-base `15cda12d`, ALL ADDITIONS (`git diff --name-status`, no M and no D), and **0 Rust source files**. Two scripts (`scripts/f24-media-live.mjs`, `scripts/f24-secret-sweep.sh`) plus 12 under `.planning/phases/24-…/` (report, notes, 10 evidence files). `crates/wcore-cli/src/{lib,main}.rs` changed: **0**. No shared script edited — `scripts/f24-inbound.mjs` and `scripts/f24-tg-fixture.mjs` deliberately untouched. 0 untracked files left behind."
credential-disclosure: "FLUX_API_KEY, from `~/.wayland-secrets/flux.env` (mode 600, outside every repo). Ran on `hetzner-dsm` (Linux). Reached the process two ways, both stdin-only: (1) piped to `curl --config -` for the three direct probes; (2) `printf | ssh 'read -r K; export FLUX_API_KEY=$K'` so the gateway child inherited it via env. NEVER in argv, never written to persistent disk, never echoed, logged, committed, or placed in any evidence file. Sweep hit count: 0 on both machines, each with a live aliveness assertion (14 and 45 readable text files). Spend: 3 direct probes + 4 gateway transcriptions ≈ USD 0.12."
status: complete
---

# 24-MEDIA-LIVE — the media clause, driven in the direction that can actually fail

**Verdict up front: `24-C3` is NOT MET and I do not claim it.** Eight lanes have now declined it.

What this lane changed is narrow and real. The predecessor proved `media` in the **degraded
direction only** — an inbound image produced an honest "I cannot see this" notice. That is correct
behaviour, but as its own report said, *every* attachment producing a degraded notice would pass
such a gate. It is the green-by-universal-denial failure in a new costume.

The positive direction is now proven for **audio**: a real voice attachment arrives on a reference
adapter, the connector fetches the bytes, a **real** transcription provider returns **real text**,
and that text reaches the model. Two reproducible runs, four gates, a negative control that
reddens, and an anti-echo control.

The **image** direction is still not met, and I now know exactly why — and it is not the reason the
predecessor recorded.

---

## 1. The measurement

**Adapter: telegram** — a designated reference adapter, driven end-to-end from
`wayland-core gateway run` (real binary, `0.12.25`, debug, built on `hetzner-dsm` at the lane
commit) against a local fixture. Driver: `scripts/f24-media-live.mjs` (new, standalone).

### Why telegram and not discord — this is why the predecessor could not get here

`wcore-channel-discord/src/rest.rs:337` pins media fetches to a CDN host allowlist
(`cdn.discordapp.com`, `media.discordapp.net`), enforced at `:349` **before any network call**. So
discord **physically cannot fetch bytes from a localhost fixture**. The predecessor's adapter choice
capped it at the degraded direction by construction.

Telegram has no such allowlist on `api::download_bytes` (`api.rs:898`), and
`TelegramConfig::api_base_url` feeds **both** the bot-method base and the file-download base
(`api.rs:658 file_download_url`) — so one fixture serves `getUpdates`, `getFile` **and** the media
bytes. `msg.voice` → `MediaKind::Audio` (`longpoll.rs:149-159`), and `resolve_attachments`
(`longpoll.rs:217`) stores the `file_id` in `Attachment.path` for lazy resolution by
`TelegramChannel::fetch_media` (`lib.rs:379-396`).

### The design decision that made this measurable — and that the predecessor's log had already hinted at

`build_transcription_backend()` resolves in order: `GROQ_API_KEY`, `OPENAI_API_KEY`, **the active
OpenAI-wire provider from `Config`**, then `FLUX_API_KEY`. And `openai_wire_media_base`
(`tool_backends/shared.rs:56-77`) returns `Some` **only** for `ProviderType::OpenAI` and
`ProviderType::FluxRouter` — `_ => return None`.

That third arm is a trap for this harness. A local chat fixture declared `provider = "openai"` (as
the predecessor's discord harness declared it) **captures transcription** and points it at the chat
fixture, which serves no `/audio/transcriptions`. That is exactly the line in the predecessor's own
log: `transcription: using whisper-1 at http://127.0.0.1:36197/... (active OpenAI-wire provider)`.

So this harness declares the chat fixture as **`together`** — a Tier-2 OpenAI-compatible type
(`config.rs:2415`). Chat still speaks the OpenAI wire to the local fixture (so the turn prompt is
captured verbatim), arm 3 returns `None`, and **arm 4 resolves transcription to the real
FluxRouter**. Confirmed at runtime, verbatim from `run1-A-gateway.log`:

```
transcription: using FluxRouter flux-voice-fast (FLUX_API_KEY found)
```

Net effect: **chat → local fixture, transcription → real provider.** The credential is then the
*only* difference between leg A and leg B.

`GROQ_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY` and `GEMINI_API_KEY` are deleted from the
child env in **every** leg, so arms 1 and 2 can never fire. That is what makes leg B a total
negative rather than a partial one.

### Ground truth

Two utterances synthesised with macOS `say -v Samantha`, converted with
`afconvert -f WAVE -d LEI16@16000 -c 1` (header verified `RIFF....WAVE` → `detect_audio_mime`
returns `audio/wav`, which is in `SUPPORTED_AUDIO_MIMES`):

| file | bytes | spoken |
|---|---|---|
| `a1.wav` | 133028 | "The quantum ferret audited nineteen crimson bicycles on Thursday morning." |
| `a2.wav` | 136820 | "Seventeen velvet lighthouses inspected the marmalade orchestra last winter." |

Both were transcribed by Flux **directly, before any gateway run**, so the provider's answer is
known independently of the product path:

| audio | HTTP | transcript |
|---|---|---|
| a1 | **200** | `" The Quantum Ferret audited 19 Crimson Bicycles on Thursday morning."` |
| a2 | **200** | `" 17 Velvet Lighthouses inspected the Marmalade Orchestra last winter."` |

Note the engine renders numbers as **digits** — "nineteen"→"19". Scoring is therefore on content
words, not string equality: a1 recovers **7/8**, a2 **6/7**, against a threshold of 5.
**Cross-hits are zero in both directions on the real transcripts**, which is what makes the
anti-echo gate mean anything.

### The three legs — each differing from A by exactly one variable

| leg | audio | `FLUX_API_KEY` | purpose |
|---|---|---|---|
| A | a1 | present | POSITIVE |
| B | a1 | **absent** | NEGATIVE CONTROL |
| C | a2 | present | ANTI-ECHO |

### Results — two consecutive full runs, all four gates PASS in both

| gate | clause | kind | result | measured (run 1) |
|---|---|---|---|---|
| **G1** | media | POSITIVE | **PASS** | `a1_hits=7/8 required=5 notice=false download_bytes=133028` |
| **G2** | media | NEGATIVE CONTROL | **PASS** | `turn_ran=true capture_alive=true notice=true a1_hits=0` |
| **G3** | media | ANTI-ECHO | **PASS** | `c_a2_hits=6 c_a1_crosshits=0 a_a2_crosshits=0 prompts_differ=true` |
| **G4** | media | CONNECTOR FETCH | **PASS** | `getFile=1 download=1 bytes=133028 enriched_log=true` |

Run 1 `d6811182…`, run 2 `665444c1…` (`sha256sum`; distinct `generated_at`, distinct `out_dir`).
Both `all_pass=true`, identical transcripts.

### The evidence, verbatim

**Leg A** (`run1-A-key-audio1-turn-prompt.txt`, 199 bytes):

```
f24ml probe A-key-audio1

[attachments received with this message:
  1. Audio (audio/wav) — transcript: The Quantum Ferret audited 19 Crimson Bicycles on Thursday morning.]
Current date: 2026-07-29
```

**Leg B**, the negative control — same audio, same config, credential removed (321 bytes):

```
f24ml probe B-nokey-audio1

[attachments received with this message:
  1. Audio (audio/wav) — transcript: [Inbound audio received but NOT transcribed: no transcription backend is configured, so the assistant cannot hear this audio. To enable transcription, set GROQ_API_KEY or OPENAI_API_KEY.]]
Current date: 2026-07-29
```

**Leg C**, anti-echo — different audio, same config (200 bytes):

```
f24ml probe C-key-audio2

[attachments received with this message:
  1. Audio (audio/wav) — transcript: 17 Velvet Lighthouses inspected the Marmalade Orchestra last winter.]
Current date: 2026-07-29
```

**What this proves.** Inbound audio on a reference adapter traverses gateway → long-poll → adapter
parse → inbound subscriber → dispatcher → enricher → `getFile` → connector byte fetch → real STT
provider → `Attachment::transcribed` → turn prompt → model. And leg A's transcript is **byte-identical
to the independent direct probe**, so the text is genuinely the provider's, not something the
harness manufactured.

**Why each control is load-bearing:**

- **G2 defeats the universal-denial trap.** It requires `turn_ran=true` **and**
  `capture_alive=true` — the turn must still be admitted, dispatched and captured, with only the
  transcript missing. A binary that had simply denied, or never connected, would fail G2 rather
  than pass it. G1 and G2 can only both pass if the credential is genuinely the difference.
  Leg B independently shows `getFile=0, download=0` — with no backend, the connector is never even
  asked for the bytes.
- **G3 defeats a canned response.** A backend returning a fixed string sails through any naive
  positive gate. G3 requires the derived text to *track the audio*: 6 a2-words present, **0**
  a1-words present, and zero cross-contamination in leg A either.
- **G4 separates "a transcript appeared" from "the model invented one"** — the fixture counts, in
  another process, one `getFile` and one download of exactly 133028 bytes.

---

## 2. NEW FINDING — F24-C3-H7 (MEDIUM): vision is blocked by code absence, not by capability

The lane assignment asked me to **re-measure** the predecessor's vision-unreachability finding
rather than inherit it. I did, and the conclusion is the same but **the reason is materially
different, and the difference is the useful part**.

The predecessor concluded vision was unreachable because the credential could not satisfy it. That
is true of the resolver, but it implies the provider is the limitation. **It is not.**

### Three source paths, all closed — with the instrument proved alive

Concept search, not keyword search (LANE-BRIEF §3b-i rule 3). Known-positive control in the same
shape: `transcription_backend_from_config` has **7** references, so this search finds
config-resolved arms when they exist.

1. **No Flux arm.** `/usr/bin/grep -rn "FLUX_API_KEY" crates --include="*.rs"` — every read site is
   transcription (`mod.rs:384`), the standalone CLI subcommands (`fetch.rs`, `image.rs`),
   `config.rs:2849`, or `fingerprint.rs`. **Zero vision sites.**
2. **No config-resolved vision arm exists at all.** `build_vision_backend()` (`mod.rs:321`) is the
   only vision builder and **takes no `&Config`** — so unlike transcription it has no seam through
   which a configured provider could ever be honoured.
3. **Key substitution cannot redirect it.** `OpenAiVisionBackend` posts to a **hardcoded**
   `https://api.openai.com/v1/chat/completions` (`openai_vision.rs:50`). Setting
   `OPENAI_API_KEY=<flux key>` would ship the Flux credential to OpenAI's host and 401 — so the
   obvious workaround is not merely unsupported, it is a credential-misdirection hazard.

### And the provider *does* serve vision — measured, not assumed

`GET /v1/models` returns **77** models including `flux-pinned-claude-sonnet`,
`flux-pinned-gpt-5`, `flux-pinned-claude-opus`. Rather than infer viability from a catalogue, I
drove a real round-trip: a 64×64 PNG, left half red, right half blue, sent as an `image_url` data
URL to `/v1/chat/completions`.

```
HTTP=200   content: 'Red, blue.'
```

Ground truth recovered exactly.

**So the shape of the gap is:** the credential works for vision, the provider serves vision on the
same OpenAI wire the code already speaks, and the only missing piece is an arm in
`build_vision_backend`. An operator whose sole key is `FLUX_API_KEY` today gets working transcripts
and a permanent *"set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY"* notice on every image —
advice that is accurate for the code but wrong for their situation, since the key they already hold
would work.

**Graded MEDIUM, deliberately not higher, and deliberately not fixed here.** Nothing is incorrect or
insecure; the degraded notice is honest and the fail-soft behaviour is right. It is a capability gap
and an asymmetry with transcription. Per LANE-BRIEF §5 that is **BACKLOG, non-blocking** — I am not
inventing a stricter rule, and adding a vision arm at the end of a lane, unproven on the inbound
path, is exactly the blind end-of-lane change prior lanes were right to refuse. The predecessor
costed this at ~0.5 session; that estimate is now **measured as viable** rather than hoped.

---

## 3. Observation (LOW, fixture-induced — stated with its caveat)

The telegram long-poll loop backs off **only on failure** (`longpoll.rs:111-117`, scaled by
`consecutive_failures`). On a *successful* empty response it re-polls immediately, relying entirely
on the server honouring `getUpdates?timeout=`. My fixture answers immediately and does **not**
honour `timeout`, which produced **~14,400 polls in ~10s**.

**I am not reporting this as a product defect.** Real Telegram honours `timeout`, so against a
conformant server the loop is correct, and the non-conformant party here is my fixture. The honest
statement is narrower: *against a server that returns 200-empty without holding the connection, the
adapter has no success-path backoff.* Worth a look, LOW, BACKLOG. Recording it because the number
appears in my evidence and an unexplained 14,400 would otherwise look like a finding I had hidden.

---

## 4. Instrument defects — four, and the worst was in the instrument built to catch this exact class

LANE-BRIEF §3b-i and §6b-ii warn that instruments carry the defect they hunt. Four instances, each
**repaired in-lane** rather than merely written up.

| # | defect | effect | how caught |
|---|---|---|---|
| 1 | zsh ate unquoted `--include=*.rs` | `MediaKind::Audio` search returned **0** — a free confirmation of an absence | re-ran quoted, with a known-positive control (24 files) |
| 2 | `/usr/bin/ls` does not exist on macOS (`/bin/ls`) | command exited 127 mid-pipeline | non-zero rc |
| 3 | `cd X && nohup Y &` backgrounds the **whole chain**, so following commands ran in the original cwd | a `grep` reported "No such file or directory" for a file that exists | the error was loud |
| 4 | **the secret sweep itself** — see below | reported **"0 hits, clean"** from a completely dead grep | **the known-positive control, and nothing else** |

### Defect 4, in detail, because it is the sharpest one

The sweep was written as:

```sh
PATHS=".../notes.md .../evidence scripts/f24-media-live.mjs"
printf '%s\n' "$FLUX_API_KEY" | /usr/bin/grep -rIl -F -f - $PATHS | wc -l     # -> 0
```

**zsh does not word-split an unquoted parameter.** `$PATHS` arrived as ONE path named
`"a b c"`, grep printed *No such file or directory*, and the sweep reported **0 — clean**. This is
LANE-BRIEF §3b-i happening live on the single most safety-critical negative claim in the lane: *"the
secret does not appear in my artefacts."* The known-positive control returned 0 in the same breath,
which is the **only** reason it was caught.

### The repair, and the repair's own defect

Per §6b-ii a written-up instrument defect is a defect you have agreed to keep, so I built
`scripts/f24-secret-sweep.sh`: paths as real positional arguments, needle on stdin, and a refusal to
report clean unless aliveness is established first.

**Its first version was itself self-passing.** It planted a control token in its *own* temp
directory and swept `"$CONTROL_TMP" "$@"` — so the control counted its own plant and returned ≥1
unconditionally. It reported **CLEAN for two nonexistent paths**. The instrument written to catch
self-passing gates was self-passing. Caught by explicitly testing the guard rather than trusting it.

The repair asserts aliveness **over the caller's actual paths**: every path must exist and be
readable, and an empty fixed-string pattern must find at least one readable text file under them
(zero ⇒ wrong tree / unreadable / all-binary ⇒ a clean result would be vacuous).

`--selftest` runs **five** assertions, and the last two are the ones that reddened before the repair:

```
PASS  known-positive: the planted needle is found (n=1)
PASS  known-negative: an absent needle is not found (n=0)
PASS  THIRD: the collapsed-path invocation MISSES a planted secret (n=0) — the defect is real
PASS  FOURTH: nonexistent paths are REFUSED (rc=4), not reported clean
PASS  FIFTH: a planted secret is caught (rc=1)
```

The third assertion **reproduces defect 4** and proves it still misses a planted secret — so the
repair demonstrably does something. The fifth proves the sweep can detect a real leak end-to-end,
which no amount of clean results would ever establish.

---

## 5. Credential handling (LANE-BRIEF §0 disclosure)

**Machine:** `hetzner-dsm` (Linux). **Source:** `~/.wayland-secrets/flux.env`, mode 600, outside
every repo. Loaded on the Mac with `set -a; . ~/.wayland-secrets/flux.env; set +a`.

**Two delivery methods, both stdin-only:**

1. **Direct probes** — piped into `curl --config -`, so the `Authorization` header was constructed
   inside curl from its stdin and **never entered argv**.
2. **Gateway runs** — `printf '%s\n' "$KEY" | ssh host 'read -r K; export FLUX_API_KEY="$K"; …'`.
   The ssh command string contains the literal `$K`, not the value; the gateway child inherited it
   through `env`. The driver reads it from `process.env` and never writes it anywhere.

**Never** echoed, printed, logged, committed, placed in argv, or written into any evidence file.
One transient exception, disclosed for completeness: an early hetzner-side sweep wrote the needle to
`/dev/shm/nk` (**tmpfs, RAM-backed, not persistent disk**), shredded it, and verified removal
(`needle-file-removed: YES`). The final sweep instrument does not do this — it reads stdin into a
`mktemp` file with `chmod 600` removed on `EXIT` trap.

### Sweep results — with aliveness, because a clean sweep from a dead tool is worthless

| scope | aliveness (readable text files) | **secret-value hits** |
|---|---|---|
| Mac: notes + evidence dir + both scripts + full diff vs merge-base | **14** | **0** |
| hetzner: both run dirs, both run logs, all three probe responses, models list, driver | **45** | **0** |

Command (exact):

```sh
printf '%s\n' "$FLUX_API_KEY" | sh scripts/f24-secret-sweep.sh <path> [path...]
```

Guard re-proved after the repair: nonexistent paths → **rc=4 (refused)**, not "clean".

**Expected hit count: 0. Measured hit count: 0, on both machines.**

**Spend:** 3 direct probes (2 STT, 1 vision) + 4 gateway transcriptions (2 runs × legs A and C).
Flux STT is billed `$0.016670` with a 10-second floor; total ≈ **USD 0.12**.

---

## 6. Grades — only what I measured

| clause of 24-C3 | grade | by whom |
|---|---|---|
| setup/auth | unchanged | prior lanes |
| access | unchanged | prior lanes |
| routing | unchanged | prior lanes |
| **media — audio** | **MET on telegram / Linux**, live, positive direction, 2 runs, controls redden | **this lane** |
| **media — image** | **NOT MET**, and structurally unreachable (F24-C3-H7) | this lane |
| native actions | unchanged (discord only) | 24-media-actions |
| idempotency | unchanged | prior lanes |
| reconnect/reload | PARTIAL, F24-C3-H5 still unfixed | prior lanes |
| health | unchanged | prior lanes |

**`24-C3` overall: NOT MET.** One clause half is newly proven; six clauses are untouched by this
lane; macOS and Windows have nothing on any clause.

### Remaining distance

| # | what is left | cost |
|---|---|---|
| 1 | **media, image half** — add a vision arm (F24-C3-H7). Now measured viable: provider serves it, credential works, wire already spoken | ~0.5 session |
| 2 | **media breadth** — audio proven on telegram only. discord cannot be driven positively against a fixture at all (CDN allowlist); slack/whatsapp/matrix/signal unmeasured | ~1 session |
| 3 | **native actions breadth** — discord only | ~1 session |
| 4 | **reconnect/reload** — F24-C3-H5 unfixed | ~1 session |
| 5 | **macOS / Windows** — nothing, any clause | ~2 sessions |
| 6 | **F24-C3-H6** (predecessor, MEDIUM) and **F24-C3-H7** (this lane, MEDIUM) | BACKLOG |

---

## 7. What I did NOT do

- **Did not mark `24-C3` MET.** Six clauses untouched, two platforms with nothing.
- **Did not claim the `media` clause outright** — only its **audio half**, on **one** adapter, on
  **one** platform. Claiming "media" whole from an audio-only result on Linux/telegram would be the
  same overstatement this program keeps catching.
- **Did not fix F24-C3-H7.** MEDIUM → BACKLOG per §5. Adding a vision arm and shipping it unproven
  on the inbound path at the end of a lane is the blind end-of-lane change to refuse.
- **Did not fix F24-C3-H6** (the predecessor's decorative `MediaBounds` finding). Still open.
- **Did not modify any Rust source.** The measurement needed none — the positive path was already
  reachable and had simply never been asked for. Worth recording: **twice now** a `media` lane has
  found the product already capable and the evidence merely absent.
- **Did not touch** `scripts/f24-inbound.mjs`, `scripts/f24-tg-fixture.mjs`,
  `crates/wcore-cli/src/{lib,main}.rs`, `.github/`, or `.planning/BACKLOG.md`. Both new scripts are
  standalone precisely to avoid the cross-lane collision a previous lane had to repair.
- **Did not run the full workspace suite.** No Rust changed, and a full run under other lanes' load
  is not a measurement (§6).
- **Did not use the Darwin-behaviour exception.** Nothing here is macOS-specific; the only Mac work
  was `say`/`afconvert` audio synthesis and shell orchestration.
- **Did not merge, open a PR, tag, publish, close an issue, or run `wcore-contract generate`.**

## 8. Evidence

`.planning/phases/24-gateway-automation-channels-typed-api/24-MEDIA-LIVE-evidence/`

| file | bytes | what |
|---|---|---|
| `24-MEDIA-LIVE-NOTES.md` (parent dir) | — | append-only record, first committed at T+11 before any investigation (§6b-i) |
| `run1-summary.json` | 3764 | run 1, four gates, `all_pass=true`, sha `d6811182…` |
| `run2-summary.json` | 3764 | run 2, reproducibility, `all_pass=true`, sha `665444c1…` |
| `run1-A-key-audio1-turn-prompt.txt` | 199 | **the positive result** — real transcript in the turn prompt |
| `run1-B-nokey-audio1-turn-prompt.txt` | 321 | negative control — degraded notice, same audio |
| `run1-C-key-audio2-turn-prompt.txt` | 200 | anti-echo — different audio, different transcript |
| `run1-A-gateway.log` | 13593 | `transcription: using FluxRouter flux-voice-fast (FLUX_API_KEY found)` |
| `run1-B-gateway.log` | 13760 | `transcription: no API key found … tool hidden` |
| `flux-probe-a1.json` / `flux-probe-a2.json` | 471 / 464 | independent STT ground truth, before any gateway run |
| `flux-vision-probe-resp.json` | 775 | **vision viability** — HTTP 200, `'Red, blue.'` |

Byte counts via `/usr/bin/stat -f%z` — not `wc`, which the predecessor measured returning 0 for a
72-byte file.

Re-run:

```sh
node scripts/f24-media-live.mjs --selftest                 # 6 assertions, mutation-proved
sh   scripts/f24-secret-sweep.sh --selftest                # 5 assertions, incl. the defect it repairs
printf '%s\n' "$KEY" | ssh host 'read -r K; export FLUX_API_KEY="$K"; \
  node scripts/f24-media-live.mjs --binary <wayland-core> \
    --audio1 a1.wav --audio2 a2.wav --out <dir>'
```
