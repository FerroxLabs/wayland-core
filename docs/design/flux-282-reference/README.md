# Flux #282 — implementation reference for the Core lane

The Flux side of the context-routing contract (`../2026-06-23-flux-context-routing-contract.md`) is **built, deployed, and ENABLED in production**. This folder is the frozen handshake surface so the Core lane can examine the exact format and behavior while building its side. It is reference material, not a runnable package.

- **`test_context_contract.py`** — the behavior spec *by example* (53 cases). Read this first: it shows exactly how each `x-wl-*` header parses (case-insensitive, defensive), the `REQUIRED` math, the `×1.15` floor, the `409 context_overflow` JSON shape, and the signal-back headers. If your client matches this file's expectations, you interoperate.
- **`flux-contract-surface.py`** — the actual Flux code (extracted verbatim from the live proxy): the four helper functions, the pre-call filter call site, the post-select backstop, and the signal-back header emission. The surrounding proxy routing is omitted (private + irrelevant to the handshake).

Canonical source of truth (Flux, private): `TradeCanyon/flux-router` @ `master` (`eb6a6b2`, PR #79) — `src/forge_hook.py` + `tests/test_context_contract.py`. Live image `capgw42-ctxcontract-eb6a6b2`, flag `FLUX_CONTEXT_CONTRACT_ENABLED=true`.

## The wire format (frozen)

### Core EMITS — request headers, tier-alias requests only (`flux-auto`/`flux-fast`/`flux-standard`/`flux-reasoning`; a concrete model id opts out)
| Header | Type | Meaning |
|---|---|---|
| `x-wl-context-tokens` | int | assembled prompt tokens you send this turn. **Required** to activate the contract. |
| `x-wl-expected-output` | int | your output/completion budget. |
| `x-wl-context-managed` | `true` | send it — gets you the signal-back + 409 path; Flux never silently truncates you. |
| `x-wl-conversation-id` | string | stable conversation id. |

### Flux SIGNALS BACK — response headers
| Header | Type | Meaning |
|---|---|---|
| `x-flux-routed-model` | string | model actually served *(pre-existing)*. |
| `x-flux-model-window` | int | real context window of the served model. |
| `x-flux-context-pressure` | float 0..1 | `REQUIRED / window`. |
| `x-flux-context-tokens-counted` | int | Flux's authoritative input count — calibrate your estimator. |

### Hard overflow (managed client) → HTTP **409**, match the `error` field (not status/substring)
```json
{ "error": "context_overflow", "required_tokens": N, "model_window": M,
  "routed_model": "…", "message": "request exceeds the window of every capable model; compact and retry" }
```

## Two implementation facts that make your side simpler
1. **Flux floors `REQUIRED` at `max(your x-wl-context-tokens + output, Flux's own count + reserve)`.** Your header is the *optimization* signal; Flux's own count is the *safety floor*. Under-counting can never route you onto a too-small model (Flux backstops it). See `_compute_context_required` + the call-site `max(...)` in `flux-contract-surface.py`, and `test_under_declared_*` in the test file.
2. **`x-flux-context-pressure` is clamped to `0..1`.** Overflow magnitude lives in the 409 body (`required_tokens`/`model_window`). Want an unbounded ratio instead? Say so on #282 and we amend §2 together.

Back-compat: all `x-wl-*` are additive; absent → Flux falls back to body token-counting (today's behavior), so you can ship one header at a time. Test against `https://api.fluxrouter.ai/v1/chat/completions`. Questions → comment on FerroxLabs/wayland #282.
