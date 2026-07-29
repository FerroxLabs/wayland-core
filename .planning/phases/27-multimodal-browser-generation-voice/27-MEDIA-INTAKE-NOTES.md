# 27-MEDIA-INTAKE — working notes (append-only, committed continuously)

Lane `27-media-intake`. Branch `lane/27-media-intake`. Base `861d1b1a`.
Started 2026-07-29T10:06Z. **This file is committed before the work, per LANE-BRIEF §6b-i.**

Target: Phase 27 Criterion 1 — "Standalone and host messages use one bounded,
validated attachment/document intake path and degrade explicitly on unsupported
providers." Plus BACKLOG `BL-F24-C3-H7`.

---

## M1 — `BL-F24-C3-H7` reproduced independently at base `861d1b1a`

Not inherited. Re-measured with `/usr/bin/grep` (unproxied, per §3b), globs quoted.

**Query run, verbatim:**

```
/usr/bin/grep -rn "build_vision_backend" crates/ --include="*.rs"
/usr/bin/grep -rn "transcription_backend_from_config" crates/ --include="*.rs" | /usr/bin/wc -l
```

**Result — target:**

```
crates/wcore-agent/src/capability_advisory.rs:34      (doc comment)
crates/wcore-agent/src/tool_backends/mod.rs:321       DEFINITION
crates/wcore-agent/src/tool_backends/video_analyze.rs:22   (doc comment)
crates/wcore-agent/src/tool_backends/video_analyze.rs:46   use
crates/wcore-agent/src/tool_backends/video_analyze.rs:516  CALL
crates/wcore-agent/src/bootstrap.rs:1327                   CALL
crates/wcore-agent/src/bootstrap.rs:3250                   CALL
crates/wcore-agent/src/channel_inbound_host.rs:221         CALL
```

**Liveness control (§3b-i.1): `transcription_backend_from_config` → 7 refs, non-zero.**
The instrument discriminates; a zero from it would be a real zero. Signature at
`mod.rs:321` confirmed verbatim:

```rust
pub fn build_vision_backend() -> Option<Arc<dyn VisionBackend>> {
```

**No `&Config` parameter. Four call sites, none able to pass one.** Resolution order is
`ANTHROPIC_API_KEY` → `OPENAI_API_KEY` → `GEMINI_API_KEY`, all via `read_env_key`. There
is no Flux arm and no `base_url` arm.

## M2 — the misdirection hazard is real, and it is why no key substitution is allowed

`crates/wcore-agent/src/tool_backends/openai_vision.rs:50`:

```rust
.post("https://api.openai.com/v1/chat/completions")
```

Hardcoded host, taken with `api_key` supplied by the caller. So setting
`OPENAI_API_KEY=<flux key>` — the obvious workaround — **sends a FluxRouter credential
to OpenAI**, a third party, rather than failing closed. Confirmed by reading, not
assumed. This is the reason `BL-F24-C3-H7` says the leg must not be worked around with a
key. I will not run that experiment even once.

## M3 — the sibling seam I am to copy exists and is proven

`transcription_backend_from_config(config)` at `mod.rs:~412` resolves BOTH key and host
from `Config`, using two helpers already unit-tested in `shared.rs`:

- `shared::openai_wire_media_base(config) -> Option<String>` (`shared.rs:56`)
- `shared::join_openai_endpoint(base, path) -> String` (`shared.rs:29`)

Its doc comment names the exact bug class I am closing: *"the key is never sent to the
wrong host (the #310 class of bug)."* That is M2 restated by an author who already fixed
it once, on the transcription arm, and did not fix it on vision.

**Design decision: copy this shape, do not improvise.** Vision gets
`vision_backend_from_config(&Config)` returning a concrete type so the resolved endpoint
is unit-assertable without a network round trip — same rationale the transcription
resolver states for itself.

## M4 — the capability is present on the wire; only our code is missing

Deterministic fixture built (`evidence/27-media-intake/make-vision-fixture.py`):

```
vision-fixture.png  bytes=2843
sha256=0115a686f37bfcb1eb2d1363562ce53bda4adbc80d8dbd7fae86f983a4ec17a3
ground truth: token=VORTHAK  number=7492  shape=red triangle
```

Ground truth is deliberately **unguessable**. "VORTHAK" is not a word; 7492 is not a
default. A model answering blind cannot emit either. That is what makes recovery proof of
sight rather than proof of plausible guessing.

**Raw-wire probe (curl, credential passed via `curl -K -` config on stdin so it never
enters `argv`):**

```
POST https://api.fluxrouter.ai/v1/chat/completions   model=flux-auto   → HTTP 200
CONTENT: The image shows the text "VORTHAK" and below it "7492" in black font on a
         white background. To the right, there is a solid red triangle pointing upward.
```

**All three ground truths recovered.** Flux serves vision on the OpenAI chat-completions
wire, in the exact `image_url` + base64 `data:` URL shape `OpenAiVisionBackend` already
builds. So the backend body is already correct; **the only defect is that no code path can
give it a Flux base URL and key.** `flux-auto` is the router alias the repo already uses
(`openai.rs` tests, `reasoning_budget_test.rs`) and is the correct model constant.

Also measured: `GET /v1/models` → HTTP 200, **77 models**. Instrument alive.

## M5 — the fix, and proof its gate can fail

Landed at `3159a9c5`. `build_vision_backend(&Config)` now carries arms 4 (active
OpenAI-wire provider, resolved from `Config`) and 5 (`FLUX_API_KEY`), appended not
prioritised — mirroring `build_transcription_backend` exactly, so **no
previously-resolving configuration changes backend.** `OpenAiVisionBackend` gained an
`endpoint` field; `new()` still pins `api.openai.com` so arms 1-3 are untouched.

**Build (hetzner-dsm, `/root/wayland-27mi`, `/usr/bin/env cargo` unproxied):**

```
cargo test -p wcore-agent --lib tool_backends::
test result: ok. 238 passed; 0 failed; 3 ignored; 0 measured; 1940 filtered out
```

**Six new tests, run by `--exact` name so the count cannot be a filter artifact
(§3.2 flavour (c)):**

```
running 6 tests
... all six ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 2175 filtered out
```

`0 ignored` and a non-zero `6 passed` — this is not a suite that exited 0 having run
nothing.

**Can the gate fail? Yes — proved by mutation, not asserted.** I reverted
`vision_backend_from_config` to the pre-fix behaviour (hardcoded `OPENAI_API_BASE` while
still accepting the caller's Flux key — literally `BL-F24-C3-H7`) and re-ran:

```
test tool_backends::tests::a_flux_credential_never_resolves_an_openai_host ... FAILED
panicked at mod.rs:795:
a FluxRouter credential resolved endpoint https://api.openai.com/v1/chat/completions
 — that would misdirect the key to a third party (BL-F24-C3-H7)
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2180 filtered out
```

Mutation reverted by file copy (not `git checkout` — other lanes share the object
store); post-restore grep confirms the correct form is back, count 1.

**One deviation found by the compiler, not by me:** `self.config` is moved into the
engine before the enricher is constructed, so the second bootstrap call site could not
borrow it. Fixed by resolving `media_vision` beside the existing `media_transcription`
binding, which exists for the identical reason and says so in its comment. This is
evidence the seam I copied was the right one.

**A finding I did not go looking for.** Four user-facing remediation strings named only
`ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY` — `capability_advisory.rs:45` and
`:60`, `channel_media.rs:70` (the one that reaches the model's prompt), and the TUI
provider list at `tui/surfaces/config.rs:1245`. After this change that advice is not
merely incomplete: **a FluxRouter user following it verbatim would set
`OPENAI_API_KEY=<flux key>`, which is precisely the misdirection this fix exists to
prevent.** Same defect class as the open browser HIGH (`[browser] allowed_origins` vs
`[browser.policy] allowed_origins`) — an unavailable whose stated fix is wrong. All four
corrected in the same commit, per §6b-ii: repair the instrument in the lane that finds it.

---

## Still to establish

- [ ] Config seam threaded to all 4 call sites; endpoint provable without network.
- [ ] LIVE: real image in → real description out, on `hetzner-dsm`, through the real binary.
- [ ] Negative control reddening on ONE variable (credential removed).
- [ ] Degradation observable at the model's prompt or a user-visible surface — NOT a
      swallowed log line. A sibling lane found both `react_on` failure paths swallowed.
- [ ] Intake path count: is it ONE bounded validated path? Prior verdict says NO
      (composer + channel enricher bypass the chokepoint). Verify before repeating.
- [ ] Secret sweep with liveness control; report hit count.
- [ ] Fence delta vs `861d1b1a` for `wcore-cli/src/{lib,main}.rs`.

## Traps I am carrying (from LANE-BRIEF, pre-committed so I cannot forget under pressure)

- Assert output EXISTS and is NON-EMPTY before asserting what is not in it (§3b-i).
- Read back `N passed`; `cargo` via rtk strips `0 filtered out`. Use `/usr/bin/env cargo`.
- Compile ONLY on hetzner-dsm. Targeted `-p` builds, never full workspace.
- Never echo the credential. stdin only.
