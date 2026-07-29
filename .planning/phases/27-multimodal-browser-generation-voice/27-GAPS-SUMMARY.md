# 27-GAPS — closing pass on Phase 27

Lane `lane/27-gaps`, merge-base `0b16f86791a707c614c14a1e1ee9f1a0c17d27d9`.
Evidence: `evidence/27-gaps/`.

**The phase goal is still NOT ACHIEVED.** Two criteria moved materially, one
moved a little, one is unchanged, and one is reframed by a finding that makes
it unanswerable in its current form. Details below, per criterion, with the
grade first.

---

## What I chose to work on, and why

The brief's read was that C3 and C5 were the most tractable and C4 the most
likely to be genuinely blocked. I largely agree, and worked in that order —
but with one change I want to flag, because it drove everything else.

**I spent the first hour on a keystone that was not on anyone's list.** While
establishing what a packaged binary can do without credentials, I followed the
engine's own error message. On a missing API key it prints, verbatim: *"To use
a LOCAL model with Ollama, select a model id prefixed with `ollama:` — no API
key is needed."* Doing exactly that reproduced the identical error.

That is the same defect shape as the `[browser]` / `[browser.policy]` HIGH this
phase already carried: a surface whose stated remedy sends the user in a
circle. But it also mattered strategically, because **three of C3's four
generation shapes need the agent engine to boot, and the engine would not boot
without a credential — including by the credential-free route it advertises.**
Fixing it was the difference between exercising those shapes and not.

So the order was: fix the keystone → C5 (packaged smokes) → C3 (four shapes) →
C4 (voice). C2's remaining half is behind a fenced protocol seam and I did not
touch it; I did take one C2 measurement that was free.

---

## Per-criterion grades

### C1 — one bounded, validated intake path; explicit degradation
**PARTIAL — unchanged from the phase verdict. I did not work on this.**

One thing was added incidentally: the host protocol's honest init-failure path
is now measured on three platforms (`host_protocol_honest_init_failure`,
exactly one structured `error`/`init_failed` frame and a non-zero exit). The
"one intake path" gap and the unexercised terminal half are untouched.

### C2 — browser/CUA/web publish live readiness; policy preserved
**NOT MET, but closer than the verdict recorded, and one HIGH is already closed.**

Three things established:

1. **The `[browser]` vs `[browser.policy]` HIGH is already fixed on the
   integration branch** by another lane — `wcore-browser/src/tool.rs:499` now
   routes through `config_hint::disabled_by_default_hint()` with a round-trip
   test through the real loader. Successor item 2 from the phase verdict is
   closed. I did not redo it.
2. **Readiness narrows on a probe that fails, and I watched it fail.** On
   headless `hetzner-dsm`, live at lane HEAD:
   ```
   WARN not advertising browser_suite: ... reason=no browser backend can start:
        `camofox-browser` does not resolve on PATH and no sidecar answered
        http://localhost:9377/health
        remedy=install @askjo/camofox-browser, or set WAYLAND_CAMOUFOX_BIN ...
   WARN not advertising computer_use: ... reason=neither DISPLAY nor
        WAYLAND_DISPLAY is set ...
   ```
   Both narrowings carry a reason and a remedy.
3. **A new HIGH, found and fixed on this lane** — the false `ollama:`
   remediation described above. Same class as the one this criterion already
   carried.

Still not met: the activation ladder still has no identity for browser, CUA or
web (that is SR-27-1..3, fenced), and three of four policy guarantees still
have no baseline. **I did not run `wcore-contract generate` and wrote no new
seam request** — `.planning/SEAM-REQUESTS/27.md` stands as filed.

### C3 — four generation shapes: discovery, credentials, accounting, failures
**NOT MET, but no longer unexercised. All four shapes are now driven.**

`10 PASS / 0 FAIL / 7 NOT MEASURED` — `evidence/27-gaps/c3-generation/`.

| Shape | discovery | credentials | failures | accounting |
|---|---|---|---|---|
| A built-in | PASS | PASS | PASS | NOT MEASURED |
| B MCP-only | PASS | PASS | NOT MEASURED | NOT MEASURED |
| C late-MCP | PASS | PASS | NOT MEASURED | NOT MEASURED |
| D combined | PASS | PASS | NOT MEASURED | NOT MEASURED |
| control (absent server) | — | — | PASS | — |

Two findings:

- **MEDIUM — discovery is not consistent across shapes.** The same fixture's
  tools are announced as `media_generate_image` when its server is alone, and
  as `mcp__f27media__media_generate_image` when a late-added server collides
  with it. A host's view of a tool's name depends on what else is in the
  session. Worse, it is the *config-declared* server that gets renamed, though
  `RemoveMcpServer`'s own doc says configured servers "remain authoritative".
  MEDIUM → BACKLOG per the standing policy; nothing is dropped and names stay
  unique within a session. But C3 says "consistent", and it is not.
- **Not a defect:** `AddMcpServer` without `--assistant` is refused with
  "active assistant identity is required for a runtime MCP declaration". That
  is deliberate per-assistant scoping (#111). Recorded because the first run
  looks like a failure and is not one.

**Accounting is NOT MEASURED in all four and I want to be explicit that this is
not a pass.** Shape A needs a cleared `FLUX_API_KEY`. Shapes B/C/D register
the media tools but invoking one needs a model turn, and no inference server
runs on the measurement host. The phase verdict's "accounting is SOURCE-ONLY"
is unimproved.

### C4 — streaming voice: interruption, cancellation, compatibility, accounting, ordering
**NOT MET. Two of five clauses now have live evidence; the criterion itself is
undercut by a finding.**

**Audio flowed.** On `seandesktop`, native Windows, over a non-interactive ssh
session:

```
F27_VOICE=BACKEND_BOUND cpal bound a default input device
F27_VOICE=WAV_BYTES 96044
F27_VOICE=CANCELLED cleanly, recorder idle
F27_VOICE=CANCEL_IDEMPOTENT second cancel on an idle recorder is a no-op
```

96,044 bytes minus the 44-byte header is 48,000 `i16` samples — exactly 3.0 s
at 16 kHz. The verdict's "no audio ever flowed on any machine" is no longer
true, and cancellation, one of the five named clauses, is exercised.

**The finding that reframes the criterion:** `voice_mode` is behind
`#[cfg(feature = "voice")]`; `wcore-cli`'s default features are
`["remote-registry", "workflow", "monitor", "review_artifact"]`; and
`release.yml` builds `cargo build --release -p wcore-cli` with no
`--features voice`. **Every shipped release artifact contains no voice tool at
all.** Confirmed live — the default build exits 2 reporting `FEATURE_OFF`.
There is no dishonesty attached: `docs/tools.md` does not advertise voice
either. But C4 asks about a capability that is not in the product a user
installs, and no amount of exercising would have found that, because the
exercise would simply have found no tool.

Interruption, accounting and event ordering remain NOT MET. They need
transcription or TTS, and there is **no local speech-to-text path in the tree**
— `build_transcription_backend` accepts only `GROQ_API_KEY` or
`OPENAI_API_KEY`. That is the blocker, named. See "Blockers" below.

### C5 — deterministic corpora and packaged smokes on native macOS, Linux, Windows
**MET for the shipped release on three platforms. NOT MET for the candidate.**

The verdict read "zero packaged smokes on zero platforms". There are now three,
each a published release archive extracted and executed on the real OS:

| Platform | Artifact | Result |
|---|---|---|
| macOS aarch64 | `v0.12.25` tar.gz | 8 PASS / 1 RED |
| Linux x86_64 | `v0.12.25` tar.gz (digest-verified) | 8 PASS / 1 RED |
| Windows x86_64 | `v0.12.25` zip (digest-verified) | 8 PASS / 1 RED |

Byte-identical grades on all three. Nine probes, run under a throwaway
`WAYLAND_HOME` with 18 credential variables stripped.

**Why it is not fully met:** these are the *shipped release*, not the phase
candidate. A packaged artifact at HEAD needs a release build per platform;
only Linux is producible on hardware this lane reaches, and I did not produce
it. The two aarch64 targets are **NOT MEASURED** — no aarch64 Linux or
Windows-on-ARM host was available. They are not recorded as 0 and not recorded
as passing.

For reference, the existing `post-tag-smoke` job in `release.yml` asserts that
`--version` matches a SemVer and nothing else, and for the two aarch64 targets
does not execute the binary at all. That is why a smoke job existed while C5
read NOT MET.

---

## The gate can fail, and I proved it in both directions

The corpus retains one probe, `ollama_hint_is_honest`, that follows the
engine's own printed remediation verbatim and grades whether it works:

| Binary | Result |
|---|---|
| v0.12.25 macOS aarch64 | **RED** |
| v0.12.25 Linux x86_64 | **RED** |
| v0.12.25 Windows x86_64 | **RED** |
| lane HEAD, Linux x86_64 | **GREEN** |
| lane HEAD, Windows x86_64 | **GREEN** |

Same probe, five real binaries on real hardware, red on the three that predate
the fix and green on the two that carry it, with `build_provenance` confirming
which SHA each binary was built from. Eight instruments on this program have
carried the defect they hunt; this one does not.

The voice probe likewise returns three distinct codes for three real conditions
— `2` feature-off, `3` no input device, `0` captured — rather than one bit.

## The one red run, grounded rather than hidden

`cargo test -p wcore-config` ran **RED** once: `550 passed; 1 failed`
(`config::tests::migrate_legacy_yaml_reads_from_wayland_home_when_set`). Two
controls settle it as contention, not regression:

- the same test alone at the same commit: `1 passed; 0 failed; 550 filtered out`
- the whole lib suite with `--test-threads=1`: `551 passed; 0 failed`

Those tests mutate process-global environment variables and collide under the
default thread-per-test runner. Reported red first, then grounded.

Other gates, executed counts read back rather than exit status trusted:

- `cargo test -p wcore-config --test local_model_no_credential_test` — **3 passed**, 0 ignored, 0 filtered out
- `cargo test -p wcore-types` — **137 passed** + **5 passed**
- `cargo clippy -p wcore-types -p wcore-config --all-targets` — rc 0, zero warnings
- `cargo fmt --all -- --check` — rc 0, 0 bytes of diff

The four pre-existing clippy errors in `journey.rs` belong to another lane. I
neither fixed, silenced, nor inherited them.

---

## Blockers, named exactly

Each needs a credential that is Sean's alone. **No key was embedded, copied
from the Mac, or printed, and no secret value appears anywhere in this lane's
output.**

| Blocks | Exact variable |
|---|---|
| C3 accounting, built-in shape — a completed image generation | `FLUX_API_KEY` |
| C4 transcription — the whole voice loop downstream of the mic | `GROQ_API_KEY` (in-source: the free tier) **or** `OPENAI_API_KEY` |
| C4 spoken reply, and therefore barge-in interruption | `OPENAI_API_KEY` or `ELEVENLABS_API_KEY` |

~~One of these has a credential-free escape worth a successor's time: **Piper
voices are downloadable and run locally**, which is the only route to a real
barge-in that does not go through Sean.~~

> **CORRECTION (lane `27-credentialled`, 2026-07-29) — the struck sentence is
> FALSE and was derived from the warning string, not the implementation.**
> Piper is dead four independent ways in this tree: `piper_download` is
> registered as a tool nowhere (`build_piper_download_backend()`,
> `piper.rs:295`, has zero production callers); `build_piper_tts_backend()`
> returns `None` unconditionally (`piper.rs:340-345`); `synthesize` is a hard
> stub (`piper.rs:374`); and `piper_tts` is in no `default` feature list, so
> the branch is not compiled into any shipped binary. A successor who follows
> this recommendation burns a session and finds a stub. **There is no
> credential-free route to barge-in today.** See `INV-26-27.md` BLOCKER-27-H1
> and `evidence/27-credentialled/`.

I did **not** verify the reported Anthropic 401 either way — nothing on this
lane needed an Anthropic credential once the local-model route worked, so I
had no honest occasion to test it. Recording that as not-done rather than
implying it was checked.

---

## What I did NOT do

- Did not run `wcore-contract generate`; wrote no new seam request.
- Did not touch `crates/wcore-cli/src/lib.rs` or `main.rs` — **zero edits to
  either fenced file.**
- Did not produce a packaged artifact at the candidate on any platform.
- Did not exercise C1's terminal/PTY half or its macOS leg.
- Did not baseline the three unmeasured C2 policy guarantees (downloads-root
  confinement, the CUA approval gate, process count and reaper interval).
- Did not perform a voice interruption, and did not invoke a registered MCP
  media tool.
- Did not fix the MEDIUM MCP naming inconsistency — it goes to BACKLOG.

## Source changes

Three files, none fenced:

- `crates/wcore-types/src/model_aliases.rs` — `LOCAL_MODEL_PREFIX` + `is_local_model`
- `crates/wcore-config/src/config.rs` — a local model needs no remote credential
- `crates/wcore-agent/src/bootstrap.rs` — refuse loudly if nothing claims the
  local route, rather than falling through to a remote provider with an empty key

Plus tests and probes: `crates/wcore-config/tests/local_model_no_credential_test.rs`
(with its negative control), `crates/wcore-agent/examples/f27_voice_capture.rs`,
`scripts/f27-packaged-smoke.py`, `scripts/f27-mcp-media-fixture.mjs`,
`scripts/f27-generation-shapes.py`.

---

## Honest bottom line

The phase goal — "Attachments, documents, browser/CUA/web, generation, and
voice work consistently across providers and hosts with honest readiness and
bounded authority" — **is not achieved.** C3 is exercised but not consistent
and not accounted. C4 has audio and cancellation and nothing else, and its
subject matter is not in the shipped binary. C5 is proved for the release and
not for the candidate. C2's central gap is still behind a fenced seam. C1 is
where it was.

What changed is that the phase now has measurements where it had assumptions,
and the two things it said were never done — the four generation shapes, and
audio flowing — have both been done.
