# F27-IMAGE-DEFAULT — running notes (LANE-BRIEF §6b-i)

Lane `f27-image-default`, branch `lane/f27-image-default`, base `plan/f20-unified-audit-repair`
@ `eaff921d710876e87372f01dcce3b185004426bc`.

Defect: **F-27C3-04**, HIGH, open. The built-in `image_generate` tool sends `gpt-image-1` in a
FluxRouter session; a Flux key is not entitled to it, so the tool fails by default and only
works with the undocumented `OPENAI_IMAGE_MODEL=flux-image`.

---

## Measured so far (source read, not yet compiled)

**Where the model is chosen today** —
`crates/wcore-agent/src/tool_backends/image_gen.rs`:

- `DEFAULT_OPENAI_IMAGE_MODEL: &str = "gpt-image-1"` (line 310) — a single global default.
- `openai_image_model_from_env()` (line 315) — `OPENAI_IMAGE_MODEL` env, else the global default.
- `DalleBackend::new(api_key, base_url)` (line 342) calls `openai_image_model_from_env()`. It
  has **no access to `Config`**, so it cannot know which provider it is serving. That is the
  whole defect: `dalle_backend_from_config` already resolves the Flux *endpoint* and the Flux
  *key* from config (#310), then throws the provider identity away before choosing the model.

**Where the fix must go** — `crates/wcore-config/src/compat.rs`:

- `ProviderCompat` (line 59) is the sanctioned layer (AGENTS.md: *"No Hardcoded Provider
  Quirks — this is the single most important rule for this codebase"*).
- Presets: `openai_defaults()` (509), `flux_router_defaults()` (705). Flux routes through
  `openai_compat_provider("flux-router")`.
- **`merge()` (844) is a ripple site with a comment saying so**: *"a new compat field MUST be
  threaded here or it is silently dropped when user config is merged over the provider preset."*
  A field added without a `merge()` arm compiles and silently loses every user override.

**Plan:**
1. `ProviderCompat::image_model: Option<String>`.
2. `openai_defaults()` → `Some("gpt-image-1")`; `flux_router_defaults()` → `Some("flux-image")`.
3. Thread through `merge()`.
4. `dalle_backend_from_config` reads `config.compat.image_model` and passes it to the backend;
   `OPENAI_IMAGE_MODEL` env keeps priority (documented #265 escape hatch — no regression), then
   compat, then the global `gpt-image-1` fallback.
5. NOT `if base_url.contains("flux")` — that is the exact anti-pattern AGENTS.md quotes as WRONG.

## Still to establish

- [ ] Compiles + unit gates on hetzner (`/root/.cargo/bin/cargo`, targeted `-p`, never bare).
- [ ] LIVE arm A: FluxRouter, **no `OPENAI_IMAGE_MODEL` set**, image generated, model read back
      from the product's own resolver log (LANE-BRIEF §3b-ii — this host injects
      `ANTHROPIC_API_KEY` regardless of what I unset).
- [ ] Known-negative: revert the compat default, watch the failure return, restore.
- [ ] OpenAI non-regression: preset still resolves `gpt-image-1` (unit; no OpenAI key expected).
- [ ] Secret sweep with a liveness control.

## Reusable prior art

`.planning/phases/27-multimodal-browser-generation-voice/evidence/27-c3-media/live-probe.sh` —
already handles the `export FLUX_API_KEY=` parse trap (a mangled key reads as a dead key), the
isolated `WAYLAND_HOME` with `[session] enabled = false`, and the "do NOT use `--json-stream`
with a positional prompt" trap. Reuse it; do not re-derive.
