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
