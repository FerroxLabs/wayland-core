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

## 4. The finding that reframes this criterion: voice is not in the shipped product

Found by reading the registration site, `crates/wcore-agent/src/bootstrap.rs:1317`:

```rust
#[cfg(feature = "voice")]
if let Some(vm) = crate::tool_backends::voice_mode::build_voice_mode_backend() {
    registry.register(Box::new(wcore_tools::voice_mode::VoiceModeTool::new(vm)));
}
```

And the two facts that make that decisive:

- `crates/wcore-cli/Cargo.toml`:
  `default = ["remote-registry", "workflow", "monitor", "review_artifact"]`
  — **`voice` is not in it.**
- `.github/workflows/release.yml:144`:
  `vx cargo build --release --target ${{ matrix.target }} -p wcore-cli`
  — **no `--features voice`.**

**Every shipped release artifact contains no `voice_mode` tool at all.**
Confirmed live rather than argued: the probe built with default features on
`hetzner-dsm` exits `2` and reports `F27_VOICE=FEATURE_OFF`.

The reason is documented in-source and is legitimate — the default binary must
not hard-link `libasound.so.2` on Linux for what is otherwise a TUI. Nothing
here is dishonest either: `docs/tools.md` does not mention `voice_mode`, so the
product does not advertise a voice capability it omits.

But it means Criterion 4 — "**streaming voice** supports interruption,
cancellation, compatibility, accounting, and ordered protocol events" — is
asking about a capability that is not in the product a user installs. That is
a bigger fact than any of the individual clauses, and no amount of exercising
would have surfaced it, because the exercise would simply have found no tool.

## 5. Audio flowed. On native Windows, from a real microphone.

`cargo run -p wcore-agent --features voice --example f27_voice_capture` on
`seandesktop`, at lane commit `a5cedb33`, over a **non-interactive ssh
session**:

```
F27_VOICE=BACKEND_BOUND cpal bound a default input device
F27_VOICE=RECORDING started, capturing for 3s
F27_VOICE=RMS_DURING 5
F27_VOICE=WAV_PATH C:\Users\seand\AppData\Local\Temp\wayland-voice-1785259888748656500.wav
F27_VOICE=WAV_BYTES 96044
F27_VOICE=CANCELLED cleanly, recorder idle
F27_VOICE=CANCEL_IDEMPOTENT second cancel on an idle recorder is a no-op
F27_VOICE_RC=0
```

96,044 bytes. Subtract the 44-byte WAV header and that is 96,000 bytes =
48,000 `i16` samples = **exactly 3.0 seconds at 16 kHz mono**, which is what
was asked for. `RMS_DURING 5` is a quiet room, but it is not zero: real
samples, not a zero-filled buffer. The probe refuses to call anything at or
below 44 bytes a capture, precisely so a header-only container cannot be
reported as "audio flowed".

**"No audio ever flowed on any machine" is no longer true.** It flowed here,
and the WASAPI question raised in §1 is answered: a non-interactive ssh session
on Windows *does* get a default input device.

`cancel()` also did what Criterion 4's cancellation clause asks — left the
recorder idle, and was a no-op on a second call. That clause needs no
credential and is now exercised.

## 6. The probe demonstrably distinguishes its outcomes

Three distinct exit codes from three real conditions, at the same commit:

| Host / build | Outcome | Code |
|---|---|---|
| `hetzner-dsm`, default features | `FEATURE_OFF` | 2 |
| `hetzner-dsm`, `--features voice`, headless | `NO_INPUT_DEVICE` | 3 |
| `seandesktop`, `--features voice`, real mic | captured 96,044 bytes | 0 |

A probe that returned 0 everywhere would prove nothing. This one returns three
different answers to three different machines, and the failing answers carry
their reason. Note also that codes 2 and 3 are *different*: over ssh to Windows
every non-zero collapses to 1, which is why the real code is written to a status
file and read back by a separate call.

## 7. Honest grade

**C4 remains NOT MET**, and that is not softened. Two of its five named
clauses now have live evidence and three do not:

| Clause | State |
|---|---|
| cancellation | **MET** — exercised live on native Windows |
| audio actually flowing | **MET** — 96,044 bytes captured (a precondition the criterion assumes rather than names) |
| interruption | **NOT MET** — needs TTS playing to barge in on; blocked on a credential |
| accounting | **NOT MET** — needs a completed voice turn |
| ordered protocol events | **NOT MET** — needs a turn to order events around |
| compatibility | **NOT MET** — only one platform's capture path was driven |

And the criterion as a whole is undercut by §4: the shipped product has no
voice at all, so nothing about it can be true of a release artifact today.

A successor should not spend another night looking for a microphone. The two
open questions are (a) whether Piper's locally-downloaded voices give a
credential-free TTS path, which is the only route to a real barge-in that does
not go through Sean, and (b) whether `voice` should be in the default feature
set at all — which is a product decision, not an engineering one.
