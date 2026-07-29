# 27-FIXES — running notes (committed early per LANE-BRIEF §6b-i)

Lane `lane/27-fixes`, branched from `plan/f20-unified-audit-repair` @ `54203b25`.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-27-fixes` (verified
`git rev-parse --show-toplevel`, NOT the forbidden `/Users/seandonahoe/dev/waylandcore`).

**No credential value appears in this file or in any artefact this lane writes.**

## Mandate (three defects, all measured by `lane/27-credentialled` in the prior hour)

1. **Transcription resolver cannot reach a working provider.** `build_transcription_backend`
   (`crates/wcore-agent/src/tool_backends/mod.rs:344`) accepts `GROQ_API_KEY` or
   `OPENAI_API_KEY` and nothing else. Flux serves `/v1/audio/transcriptions` verbatim in the
   exact `verbose_json` shape `openai_compat_whisper.rs:61` already sends. Add the arm
   (~8 lines), then **re-measure C4's transcription clause through the product**, not curl.
2. **HIGH — `wayland-core image` with no `--model` returns 401 on a working key.**
   `BL-F27-FLUX-IMAGE-DEFAULT-ARM-401`. Must first establish WHICH defect it is:
   (a) the default model id is genuinely unentitled → upstream 401 truthful, our defect is
   that we surface it as "your key is bad"; or (b) something else. The two fixes differ.
3. **No cost record for any media call.** C3 accounting FAILS in all four shapes. Note:
   `session_cost` is **honest** — it reports `priced: false` and "cost is unpriced, not $0".
   The prior lane explicitly recorded that as NOT a defect. Job is to make media calls
   priced, not to fix a false zero. If materially more than a small change, report shape+cost
   rather than half-build.

## Starting position (inherited, from 27-CREDENTIALLED-SUMMARY.md)

- Flux routes that serve: `/v1/chat/completions`, `/v1/images/generations`,
  `/v1/audio/transcriptions`. Do NOT serve: `/v1/audio/speech` (500), `/v1/audio/translations` (404).
- Transcription cost arrives **only in HTTP response headers** (`x-flux-cost-usd`,
  `x-flux-billed-seconds`), never in the JSON body. `openai_compat_whisper.rs:85-86` takes
  `resp.status()` then `resp.text()` — **headers are never read**. `TranscriptionOutcome::Ok`
  has no cost field.
- Image generation: the API reports **no cost at all** — no `usage`, no `x-flux-*` header.
  So image pricing may be structurally unavailable from this provider.
- Unit prices: text ≈ $0.000126; transcription $0.016670 at a **10-second floor**; image ≈ $0.08.
- Prior lane spent ~US$0.82. My budget: keep well under $1, report actual.
- Headless measurement hosts need `WAYLAND_VAULT_PASSPHRASE` (or `[session] enabled = false`)
  or every turn dies with "Session persistence authority unavailable" while discovery still
  looks healthy.

## Known determination lever for defect 2 (cheap)

The prior lane established that requesting a disallowed model on `/v1/audio/speech` returns a
**named entitlement error listing the key's allowed models**. If that same listing behaviour
holds on `/v1/images/generations`, I can determine entitlement of `flux-image-together-flux`
for ~$0.0001 without paying for an image. That is the first thing to try.

## Traps I must not repeat (from the brief)

- Pipe steals exit status; `echo "EXIT=${PIPESTATUS[0]}"` after a pipeline returns empty here.
- Byte-count every capture.
- Run test targets **by file, never by filter** (a filter matching no test exits 0 having run 0).
- Assert the `N passed` count; never trust exit status.
- `wcore-agent --lib` fails 13-19 in parallel, passes 2145/0 serially — **run serially**.
- Two messages on one stdin silently drops the second turn — drive turns one at a time.
- Any defect I find in my OWN harness gets repaired in THIS lane, with a three-assertion
  self-test whose third assertion proves the old form would have missed it (§6b-ii).

## Log

- [t0] Worktree created, toplevel verified, NOTES committed. Nothing measured yet.

## MEASURED — defect 2 (image 401) is DETERMINED

Free probes only (`f27-image-entitlement-probe.sh`, no image generated, ~$0 spend):

| Probe | HTTP | Body (36 bytes, byte-identical across the three 401s) |
|---|---|---|
| `/v1/models` (real key) | 200 | 77 models; **`flux-image-together-flux` ABSENT**, `flux-image` present |
| POST images, `flux-image-together-flux` (the product default) | **401** | `{"error":{"message":"unauthorized"}}` |
| POST images, `definitely-not-a-real-model-xyz` (control) | **401** | `{"error":{"message":"unauthorized"}}` |
| POST images, **genuinely invalid key** (control) | **401** | `{"error":{"message":"unauthorized"}}` |
| POST images, `flux-image` + empty prompt | 400 | `{"error":{"message":"prompt required"}}` — key authenticates fine on this route |

`/v1/models` is key-scoped: a bogus key gets HTTP 500 with a *named* auth error, so the
77-model catalogue is what THIS key is served. The only image arm in it is `flux-image`.

**Determination.** Upstream returns a byte-identical 401 for THREE distinct causes: unknown
model, unavailable-for-this-key model, and bad credential. So the upstream 401 is *not*
truthful-about-cause — it is *uninformative*. It follows that:
 - the product's default arm is not merely unentitled, it is **absent from the catalogue**;
 - the product **cannot** correctly infer "your key is bad" from a 401 on this route, because
   the same 401 means "unknown model". Rendering it as a credential failure is the defect.
 - `flux_image.rs:246` only maps `402 premium_locked` to a typed entitlement error. Flux
   never sends 402 here, so that mapper is dead on this route and everything falls to
   `ProviderError::Api{401}`.

Open question for the fix (cross-audit next): change `DEFAULT_IMAGE_MODEL` to `flux-image`,
or fix the message, or both. Changing a default has cost implications (contract documents
together-flux as the ~$0.01 cheapest arm; measured image ≈ $0.08).

## DECIDED — defect 2 fix, after cross-audit (§4) + adversarial pass

Panel (all three returned non-empty; no vote dropped — byte counts 543 / 521 / 2117):

| Member | Q1 change the default arm? | Q2 message |
|---|---|---|
| codex gpt-5.6-sol | both, but via a **key-scoped runtime `/models` check**, not a static swap | name both causes, name the model |
| gemini 3.1-pro | **message only** — "8x cost increase on all users based on one key" | name both causes, name the model |
| kimi K3 | **both** — swap the default, flag the doc stale | name both causes + list-models action |

**Q2 is unanimous** and is what I implement: never assert the credential is bad.

**Q1: I take the minority (gemini), and here is the adversarial argument that carried it.**
The consensus-to-swap rests on "the default is absent from the catalogue, therefore it is
stale." But I *proved* the catalogue is **key-scoped** (bogus key → HTTP 500 named auth error,
so the 77 ids are what THIS key is served). "Absent for this key" therefore does NOT entail
"absent for all keys" — the generalization the swap requires is exactly the one my own
measurement forbids. Against that, the cost is measured and one-directional: documented
together-flux ≈ $0.01 vs measured flux-image ≈ $0.08, an **8x silent cost increase imposed on
every user** to fix a message defect. Codex's dynamic-catalogue variant avoids the cost
generalization but buys an extra round-trip and a new failure mode on every image call.

**Most important: the brief's framing is falsified by the measurement.** The brief posed the
residual as "either the default is unentitled and the upstream 401 is *truthful*, or ...".
It is **not truthful** — the same byte-identical 401 is returned for a gibberish model id and
for an invalid key. So the product cannot infer *any* cause from it. That settles the choice:
the fix is the message, and the message must not assert entitlement either.

**Decision:** fix the message (and type the 401 rather than letting it fall through a dead
402-only mapper). Do **not** silently change `DEFAULT_IMAGE_MODEL`. File the default-arm
question as a finding carrying the cost figure and the key-scoping caveat, for Sean.
