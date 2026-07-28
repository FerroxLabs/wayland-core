# C4 — voice: what is actually blocked, established rather than asserted

Phase 27's verdict read: "**NOTHING WAS EXERCISED.** No audio flowed on any
machine. No interruption occurred... `seandesktop` has audio, a toolchain, and
answered a reachability probe. The path existed and was not taken. This is an
execution shortfall, not an environmental impossibility."

The brief for this lane asked, specifically: **prove the impossibility rather
than asserting it.** So the first job here was to find out which parts are
genuinely blocked and which were merely not attempted. The answer splits.

## 1. Capture hardware exists on `seandesktop` — this is NOT an environmental block

Probed, not assumed (`Get-PnpDevice -Class AudioEndpoint`, `Win32_SoundDevice`,
`Get-Service Audiosrv`):

```
Shure MV7                       (Unknown / OK — USB microphone)
BRIO 4K Stream Edition          (OK — webcam with capture)
UNA USB audio                   (OK)
Jabra Elite 7 Active            (OK, plus Hands-Free endpoint)
Sean's Buds2 Pro #3             (OK, plus Hands-Free endpoint)
NVIDIA Broadcast                (OK)
Realtek High Definition Audio   (OK)
Audiosrv                        Running / Automatic
```

The verdict's read is confirmed: the hardware is there. C4 is not blocked by
the absence of a microphone.

One caveat that has to be measured rather than assumed: `SESSIONNAME` is empty
over ssh, so this is not an interactive Windows session. Whether WASAPI hands
cpal a default *input* device from that context is the open question, and it
is answered by the engine itself — `build_voice_mode_backend` returns `None`
and logs "cpal could not bind a default input device — tool hidden (CI /
container / SSH host?)" when it cannot. That is a real, self-reporting probe,
which is why the Windows build was worth doing rather than guessing.

## 2. The credential blocker is real, and it is precisely locatable

`crates/wcore-agent/src/tool_backends/mod.rs:344`, `build_transcription_backend`:

```rust
if let Some(key) = read_env_key("GROQ_API_KEY")  { ...Groq whisper-large-v3-turbo... }
if let Some(key) = read_env_key("OPENAI_API_KEY") { ...OpenAI whisper-1... }
tracing::warn!("transcription: no API key found (GROQ_API_KEY or OPENAI_API_KEY) — tool hidden");
None
```

**There is no local speech-to-text option.** Not whisper.cpp, not a bundled
model, not an offline path. Measured live on `hetzner-dsm` at lane HEAD with
the credential variables stripped:

```
WARN transcription: no API key found (GROQ_API_KEY or OPENAI_API_KEY) — tool hidden
WARN tts: no TTS backend configured — set OPENAI_API_KEY or ELEVENLABS_API_KEY
     (or download Piper voices via piper_download). Tool hidden.
```

So the criterion splits cleanly in two:

| C4 clause | Needs a credential? | Named blocker |
|---|---|---|
| audio capture flowing | **No** | — |
| cancellation of a live recording | **No** | — |
| transcription of captured audio | **Yes** | `GROQ_API_KEY` **or** `OPENAI_API_KEY` |
| spoken reply (TTS) | **Yes** | `OPENAI_API_KEY` or `ELEVENLABS_API_KEY`, **or** locally-downloaded Piper voices |
| barge-in interruption of a spoken reply | **Yes**, transitively | needs TTS playing to interrupt |
| accounting for a voice turn | **Yes**, transitively | needs a completed turn |
| ordered voice protocol events | **Yes**, transitively | needs a turn to order events around |

`build_voice_mode_backend` is explicit that the two halves are separable:

```
voice_mode: no STT backend configured — capture works, transcribe will error
            (set GROQ_API_KEY or OPENAI_API_KEY)
```

**"Capture works."** So real audio CAN flow with no credential at all. What
cannot happen without one is everything downstream of the microphone.

The TTS row has a credential-free escape the others do not: Piper voices are
downloadable and run locally. A successor who wants barge-in without a paid
key should pull that thread — it is the only route to a real interruption that
does not go through Sean.

## 3. Named blockers, exactly as required

- `GROQ_API_KEY` — Groq Whisper, described in-source as the free tier. This is
  the cheapest single unblock for the transcription half of C4.
- `OPENAI_API_KEY` — alternative for both transcription and TTS.
- `ELEVENLABS_API_KEY` — TTS alternative.

None was embedded, copied from the Mac, or printed. No value of any secret
appears anywhere in this lane's output.

## 4. Honest grade

**C4 remains NOT MET.** This lane did not make audio flow and did not perform
an interruption, so the verdict's headline stands unchanged and is not softened
here.

What changed is that "not attempted" has become "attempted, and here is exactly
where it stops and what would unblock it":

- the impossibility claim is **disproved for capture** — the hardware exists,
  it is enumerated above, and the engine's own probe is the arbiter;
- the impossibility claim is **substantiated for the full loop** — there is no
  local STT path in the tree, so transcription cannot happen without one of two
  named environment variables that are Sean-reserved.

That distinction is the whole point. A successor should not spend another night
looking for a microphone.
