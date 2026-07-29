# 27-CREDENTIALLED — what the Flux credential closed, and what it did not

Lane `lane/27-credentialled`, branched from `plan/f20-unified-audit-repair` @ `3cfc336f`.
Measurement host `hetzner-dsm` (Ubuntu 24.04, x86_64), binary built at lane HEAD, plus
API-level probes from the Mac. **No credential value appears in this file, in any evidence
artefact, or in any log this lane wrote.** Every capture was passed through a redactor and
then swept for tokens ≥24 characters; the only long strings that survive are a UUID and a
script filename.

---

## Headline

**Phase 27 Criterion 3 has no NOT MEASURED cells left.** It went from
`10 PASS / 0 FAIL / 7 NOT MEASURED` to **`13 PASS / 5 FAIL / 0 NOT MEASURED`**. Every one of
the seven cells the prior lane could not reach was reached and graded on evidence. Four of
them graded FAIL — which is the honest outcome, and is worth more than the NOT MEASURED it
replaced, because the reason for each failure is now located in a specific file and line.

**Criterion 4 did not move as far, and the reason is not the product.** The credential
reaches transcription — proven with a verbatim round-trip through the exact wire shape the
shipped backend sends — but the **product cannot route to it**, and the provider does not
serve TTS at all. Both are stated below as findings, not worked around.

---

## Per-clause grades, with counts

### 27-C3 — four generation shapes: discovery, credentials, accounting, failures

| Shape | discovery | credentials | failures | accounting |
|---|---|---|---|---|
| A built-in | PASS *(prior)* | PASS *(absent key, prior)* / **FAIL** *(present key — new)* | PASS *(prior)* | **FAIL** *(new)* |
| B MCP-only | PASS *(prior)* | PASS *(prior)* | **PASS** *(new, in-shape)* | **FAIL** *(new)* |
| C late-MCP | PASS *(prior)* | PASS *(prior)* | **PASS** *(new, in-shape)* | **FAIL** *(new)* |
| D combined | PASS *(prior)* | PASS *(prior)* | **PASS** *(new, in-shape)* | **FAIL** *(new)* |
| control absent-server | — | — | PASS *(prior)* | — |

`WLRC=0 PASS=6 FAIL=3 NOT_MEASURED=0` for the nine cells this lane's driver graded
(`evidence/27-credentialled/c3-bcd-credentialled-status.txt`); shape A was driven separately.

**Criterion 3 is still NOT MET.** All four shapes now complete a generation or a named
failure, but the accounting clause fails in every shape and the discovery clause is
inconsistent across shapes (the previously-unfiled MEDIUM, now filed). What changed is that
"we could not look" became "we looked, and here is what is wrong."

### 27-C4 — streaming voice

| Clause | Grade | Movement |
|---|---|---|
| capture | unchanged — one host, one run | not re-run by this lane |
| cancellation | PASS *(prior)* | unchanged |
| transcription | **NOT MET** | provider proven reachable; **product cannot route to it** — see F-27-CRED-01 |
| TTS / barge-in | **NOT MET, and now provably not credential-blocked-by-Flux** | `/v1/audio/speech` returns HTTP 500 for every allowed model — see F-27-CRED-02 |
| accounting | **FAIL, structural** | `TranscriptionOutcome::Ok` has no cost field and the backend never reads response headers — see F-27-CRED-03 |
| ordering / compatibility | untouched | this lane did not work on them |

---

## What the provider could and could not reach

Route existence established by POSTing `{}` and reading 404 vs 400 — free, and it is the
only discriminator, because `/v1/models` metadata carries no capability field at all.

| Route | Serves? | Evidence |
|---|---|---|
| `/v1/chat/completions` | **yes** | drives every model turn in shapes B/C/D; `cost_usd` in `usage` |
| `/v1/images/generations` | **yes** | 87,337-byte JPEG direct; 46,216-byte JPEG through the product |
| `/v1/audio/transcriptions` | **yes** | verbatim round-trip, `verbose_json`, segments, language |
| `/v1/audio/speech` | **no** — HTTP 500 on `flux-voice`, `flux-voice-fast`, `flux-auto` | `c4-flux-voice-probe.log` |
| `/v1/audio/translations` | **no** — HTTP 404 | route absent |

### The transcription round-trip, in full

Positive input generated locally and free (macOS `say` → `afconvert -f WAVE -d LEI16@16000
-c 1`), 115,566 bytes, 1 ch / 16 kHz / 16-bit / 55,735 frames / 3.48 s — the same format
`f27_voice_capture` produces:

```
POSITIVE          http=200 bytes=458
  text='The quick brown fox jumps over the lazy dog near the riverbank.'
  verbatim_match=True   has_segments=True   language='English'   duration=3.483437568
NEGATIVE_CONTROL  http=200 bytes=292
  text='Thank you.'
  verbatim_match=False
```

The negative control (identical duration and format, all-zero samples) returns **different**
text, so the positive is not a driver echoing the expected string. `response_format=verbose_json`
is the exact form `openai_compat_whisper.rs:61` sends, so this is compatibility with the
shipped request shape, not merely with "OpenAI-ish".

Reproducible: `scripts/f27-flux-voice-probe.sh` (reads `FLUX_API_KEY` from the environment,
prints no key).

---

## Product defects found — every one of them needed a real credential to see

### F-27-CRED-01 — the transcription resolver cannot reach a configured provider (MEDIUM; this is the answer to the lane's question 2)

`build_transcription_backend()` (`crates/wcore-agent/src/tool_backends/mod.rs:344`) accepts
**`GROQ_API_KEY` then `OPENAI_API_KEY` and nothing else**, with no config route. The
inventory measured this and it holds. Observed live with the key present:

```
WARN transcription: no API key found (GROQ_API_KEY or OPENAI_API_KEY) — tool hidden
```

**But the shape is much better than "needs a new backend".** The backend it builds is
`OpenAiCompatWhisperBackend::new(key, endpoint, model, label)` — fully parameterised — and
flux-router serves that exact endpoint in that exact format, proven above. **No wire code
needs writing.** What it would take is a third resolver arm of roughly eight lines:

```rust
if let Some(key) = read_env_key("FLUX_API_KEY") {
    return Some(Arc::new(OpenAiCompatWhisperBackend::new(
        key,
        "https://api.fluxrouter.ai/v1/audio/transcriptions".to_string(),
        "flux-voice-fast".to_string(),
        "flux-router",
    )));
}
```

plus the base URL taken from `[providers.flux-router].base_url` rather than hardcoded (the
pattern already exists — `image_gen.rs` derives its endpoint from the provider config via
`join_openai_endpoint`, per #310).

**Per the lane brief I did not write it.** The brief's instruction was to establish whether
the product can reach the provider and, if it cannot, to report what it would take rather
than hack around it in-lane. It cannot; that is stated above; the change is a product
decision about which providers the STT resolver recognises, and it belongs in a plan.

### F-27-CRED-02 — Flux does not serve TTS, so C4's barge-in has no route here either

`/v1/audio/speech` returns HTTP 500 `internal_server_error` for `flux-voice`,
`flux-voice-fast` and `flux-auto`. That the failure is provider-side and not an entitlement
problem is established by contrast: requesting `tts-1` on the same route returns a *named*
entitlement error listing the key's allowed models — and the `flux-voice*` models **are** on
that list. So they are permitted and the route still fails.

Combined with the Piper correction below, **there is no route to barge-in today**: not
Piper, not Flux. It needs `OPENAI_API_KEY` / `ELEVENLABS_API_KEY`, or a real local synthesis
runtime (2–3 sessions).

### F-27-CRED-03 — the transcription path has no accounting surface at all (MEDIUM, structural)

Two independent facts, either of which alone makes C4 accounting impossible:

1. **The provider reports transcription cost only in HTTP response headers** —
   `x-flux-cost-usd: 0.016670`, `x-flux-billed-seconds: 10` — never in the JSON body.
2. **The product cannot see it and could not hold it.** `openai_compat_whisper.rs:85-86`
   takes `resp.status()` and immediately `resp.text()`; **response headers are never read**.
   And `TranscriptionOutcome::Ok { transcript, language, segments }` has no cost or usage
   field, so there is nowhere to put one regardless of provider.

### F-27-CRED-04 — `wayland-core image` default arm returns 401 on a cleared paid key (HIGH, with an explicit argument for MEDIUM)

Filed to `BACKLOG.md` as `BL-F27-FLUX-IMAGE-DEFAULT-ARM-401`. Same key, same run:

```
image --prompt "..." --out a-default.png            -> rc=1  API error 401 {"message":"unauthorized"}  no artifact
image --model flux-image --prompt "..." --out ...   -> rc=0  wrote a-flux.png (46216 bytes)  JPEG 1024x1024
```

`flux_image.rs:31` defaults to `flux-image-together-flux`; the key is entitled to
`flux-image`. The user is told their **credential** is unauthorized when the credential is
fine. The subcommand's own help promises a distinct `premium_locked` message for exactly
this case and it never fires. **Residual uncertainty, stated rather than buried:** whether a
different Flux plan entitles `flux-image-together-flux` cannot be determined from this side.
If it normally is entitled, this is a MEDIUM (bad message on an unusual plan) rather than a
HIGH. Only Sean can settle that.

### F-27-CRED-05 — `-m flux-router:flux-fast` boots into anthropic and blames the wrong key (MEDIUM)

Filed as `BL-F27-FLUX-PROVIDER-PREFIX-UNSUPPORTED`. With `FLUX_API_KEY` in the environment
*and* `[providers.flux-router]` populated, the prefix form dies at init naming only
`API_KEY / ANTHROPIC_API_KEY / OPENAI_API_KEY` and Ollama. The prefix form works for
`ollama:` and the error advertises it, so it looks general. Working form:
`-p flux-router -m flux-fast` plus a `[default]` block.

### F-27-CRED-06 — a reasoning model's empty completion is reported as an endpoint incompatibility (MEDIUM, not filed — see below)

After a successful tool call, a third provider attempt produced:

```
engine_error: Provider returned an empty response — no content and no tool calls.
The endpoint or model may be incompatible (verify it speaks the OpenAI chat-completions
streaming format and that the model name is valid).
```

The endpoint speaks the format correctly — it had just driven two successful turns and a
tool call in the same session. `flux-fast` is a reasoning model that can legitimately return
HTTP 200 with empty content and every token spent as `reasoning_tokens` (measured directly:
`max_tokens=2000` → `completion_tokens=29`, `reasoning_tokens=26`). The remediation text
sends the reader to check the wire format, which is the wrong place. Recorded here rather
than filed because I did not isolate it far enough to be sure the engine has no other cause
in play — treat it as a lead, not a finding.

### What I checked and did NOT find — recorded because I expected to

I predicted `session_cost` would report a false `$0.00` for a live billed session. **It does
not, and the product is more honest than the prediction.** It emits
`per_turn[].priced: false` and an explicit user-facing line:

```
Pricing unavailable for flux-router/flux-fast; the call remains bounded by the token
envelope and cost is unpriced, not $0.
```

That distinction — unpriced versus zero — is exactly what this program grades for, and it is
already implemented. The only improvement available is that the provider **does** return
`cost_usd` in `usage` on every chat completion, so a ProviderCompat cost row for flux-router
would make these turns priced. That is an enhancement, not a defect, and I am not filing it
as one.

---

## Corrections made to the phase record

- **`27-GAPS-SUMMARY.md`** and **`evidence/27-gaps/c4-voice/README.md`** both recommended
  Piper to a successor as *"the only route to a real interruption that does not go through
  Sean."* Both sentences are struck with an inline correction naming the four independent
  ways Piper is dead in this tree (`piper.rs:295`, `:340-345`, `:374`, and the absent
  `piper_tts` default feature). A successor following that advice loses a session.
- **`BACKLOG.md` had zero Phase 27 rows.** The MCP discovery-naming MEDIUM that two
  documents recorded as "→ BACKLOG per the standing policy" had never been filed — under
  that policy an unfiled MEDIUM is dropped, not dispositioned. Filed as
  `BL-F27-MCP-DISCOVERY-NAMING`, together with the two new credential-only findings.

---

## Instrument defect found in this lane's own harness, and repaired in this lane

The first accounting sweep read:

```bash
grep -in 'cost\|usd\|usage' FILES | head -20
echo "COST_GREP_RC=$?"          # reports head(1)'s status, not grep's
```

It printed `COST_GREP_RC=0` against files containing **zero** matches. That is the
pipe-steals-exit-status class the brief names, occurring inside the instrument built to hunt
accounting gaps — the twelfth recorded instance in this program of an instrument carrying
the defect class it hunts.

Repaired rather than written up (§6b-ii), with a three-assertion self-test at
`evidence/27-credentialled/cost-record-matcher-selftest.sh`:

```
ASSERT_1_KNOWN_POSITIVE=PASS (repaired matcher reports FOUND)
ASSERT_2_KNOWN_NEGATIVE=PASS (repaired matcher reports ABSENT)
ASSERT_3_OLD_MATCHER_MISSED_IT=PASS (old rc=0 found-shaped on a clean file; repaired rc=1)
SELFTEST_RESULT=PASS
```

Assertion 3 is the one that proves the repair does anything; without it the self-test passes
on the broken instrument too.

**A second driver defect, same lane, same discipline.** Queueing both messages on one stdin
got the first turn executed and the second **silently dropped**, so all three shapes reported
`media_generate_locked called=False` and `failures = NOT MEASURED`. That would have been a
NOT MEASURED caused by the driver and attributed to the product. Each turn now runs in its
own session, and all three shapes observe their own failure. This is why the same driver was
run three times: `0 PASS / 3 FAIL / 6 NM` (turn never ran — headless vault), then
`3 PASS / 3 FAIL / 3 NM` (second turn dropped), then `6 PASS / 3 FAIL / 0 NM`.

**A third, worth recording because it looks like a product outage and is not.** On the
headless host every turn died with `engine_error: Session persistence authority unavailable
... no OS keyring was usable` while `mcp_ready` still fired and the fixture still connected.
Every *discovery* observable looked healthy and no turn could ever execute. `WAYLAND_VAULT_PASSPHRASE`
(or `[session] enabled = false`) is required on a headless measurement host, and a driver
that grades only discovery will not notice.

---

## Approximate spend

Metered from the provider's own `x-flux-available` counter across the whole lane:
**819,664 units ≈ US$0.82.**

Unit prices measured before running anything in bulk, per the brief:

| Call | Price |
|---|---|
| trivial chat completion (`flux-fast`) | `$0.000126` |
| 3.48 s transcription | `$0.016670` — billed at a **10-second floor** |
| one image generation | ≈ `$0.08`, and **the API reports no cost for it at all** — no `usage`, no `x-flux-*` header |

Image and voice are two to three orders of magnitude more expensive than a text turn, which
is why exactly two images were generated in this lane and four transcriptions.

---

## What remains blocked, and on what

| Item | Blocked on |
|---|---|
| C4 transcription **as shipped** | a product decision to add a Flux arm to `build_transcription_backend` — *not* a credential any more. The provider is proven. |
| C4 transcription **accounting** | a `cost` field on `TranscriptionOutcome` **and** reading response headers. Structural; ~0.5 sessions. |
| C4 TTS / barge-in | `OPENAI_API_KEY` or `ELEVENLABS_API_KEY`, or a real local synthesis runtime. **Flux does not serve TTS** and **Piper is a stub** — both credential-free routes previously believed open are closed. |
| C4 capture, honest claim | one host, one run, dither-indistinguishable. Needs a re-run with a tone played into the mic asserting RMS rises. No credential. |
| C3 accounting, all shapes | a design decision: media calls produce no cost record because there is no per-tool cost channel. The provider does not price images either, so shape A cannot be fixed by the product alone. |
| C3 discovery consistency | `BL-F27-MCP-DISCOVERY-NAMING`, 0.5 sessions, no credential. |
| `BL-F27-FLUX-IMAGE-DEFAULT-ARM-401` severity | Sean — whether a normal paid plan entitles `flux-image-together-flux`. |

## What I did NOT do

- Did not touch C1, C2 or C5.
- Did not re-run the C4 audio capture leg, and do **not** repeat the stronger claim about it:
  the honest statement remains *"the capture pipeline bound a device and produced 3.0 s of
  non-zero i16 at 16 kHz"*. A muted or stuck device producing dither reads identically.
- Did not add the Flux arm to the transcription resolver (see F-27-CRED-01 for why).
- Did not run `cargo` on the Mac, did not run a full-workspace build, did not touch
  `.github/workflows/ci.yml`, did not merge, tag, PR or close anything.
- Did not write, print or transmit-to-disk any credential value.
