# NOTES — lane/whatsapp-bridge

Started 2026-07-30. Base: `4caaa31c891c0d606e5de1e91cdcd3e5a79ab767` (integration
`plan/f20-unified-audit-repair`). Append-and-recommit after every measurement (§6b-i).

## Goal (from brief)

Let Core *speak to* the existing Desktop WhatsApp bridge (Node subprocess, JSON-RPC 2.0
over stdio) so all three backends (`baileys`, `whatsapp-web`, `meta-business`) are
reachable. **OPT-IN. `meta-business` stays the default. Node must not become mandatory.**
Do NOT reimplement Baileys.

## Measurement log

### M1 — reference bridge read (READ-ONLY, `/Users/seandonahoe/dev/wayland/app/src/process/channels/whatsapp-bridge/`)

Confirmed against source, not the brief:

- Framing: one JSON value per line, `\n`-delimited. `bridge.js:67-78`. Confirmed.
- Parent owns ids; bridge emits notifications `inbound.message`, `connection.status`,
  `qr.update`, `error`. `bridge.js:26-30`, `emitNotification` at `:115`. Confirmed.
- Backend flag `--backend <name>`, **default `baileys`**, `bridge.js:42`. Unknown name
  throws `Unknown backend: <name>` at `:55`, surfaced as JSON-RPC `-32000` at `:157`
  — NOT `-32601`, and NOT at launch: the backend is lazily loaded on the first
  non-`health` RPC (`bridge.js:149-160`). **This matters for our selector design:
  a bad backend name is only detectable after a round trip.**
- Allowlist rejects with `-32601`. `bridge.js:135-138`, `allowlist.js:36-38`. Confirmed.
- Also takes `--session <dir>` (`bridge.js:43`) — the brief does not mention it, and
  Baileys auth state lives there.

**Brief premise partially incomplete (not false):** the brief lists the RPC surface as
`connect, disconnect, sendText, sendMedia, setPresence, react, subscribe`. The real
`ALLOWED_RPC_METHODS` (`allowlist.js:23-33`) has **nine** entries — those seven plus
`webhookDelivery` and `health`. `health` is special-cased *before* backend load
(`bridge.js:140-147`), which is the only RPC that answers without loading a backend.
That makes `health` the natural liveness probe and it is absent from the brief's list.

### M2 — PROVENANCE: the brief's premise is FALSE if we vendor the JS

Brief says: *"The bridge is Sean-owned Desktop code moving to Sean-owned Core code.
That raises no third-party question."*

That holds for Rust I write. It does **not** hold for the bridge JS itself:

- `bridge.js:2-5` and `allowlist.js:1-4` each carry a header the **Desktop project
  itself wrote**: *"Portions adapted from Hermes Agent … Copyright (c) 2025 Peter
  Steinberger / Hermes Agent contributors - MIT License"*.
- `README.md:68`: *"`backends/baileys.js` — session, auth-store, identity logic ported
  from OpenClaw … MIT"*.
- `README.md:69`: `backends/whatsapp-web.js` wraps `whatsapp-web.js`, Apache-2.0.

Per `.planning/PROVENANCE-COMPARISON.md` §4.1, the OpenClaw→Desktop hop is *"planned as
a derivation, executed as one, and attributed as one by the Desktop project itself"* —
i.e. this is the **real** kind, not the five false headers §6 says to strip.

**Consequence:** vendoring the bridge JS into the Core repo carries three third-party
obligations into a Core release (Hermes MIT, OpenClaw MIT, whatsapp-web.js Apache-2.0),
plus `@whiskeysockets/baileys` MIT at the dependency level. Not vendoring carries none.
This is now an input to the distribution decision, not an afterthought.

### M3 — Core already has the "don't advertise what you can't deliver" surface

`crates/wcore-channels/src/probe.rs` — `ProbeOutcome{Ok,Incomplete,Unauthenticated,
Unreachable,Unsupported}`, `is_ready()` true **only** for `Ok`; `Unsupported` is
explicitly not-ready (`probe.rs:69-73`, test `unsupported_is_not_ready` at `:191`).
Its module doc states the exact rule the brief restates. **Reuse this rather than
inventing a health surface.** `ProbeReport::incomplete(channel, platform, missing)`
already carries the "name exactly what is missing" contract.

## Open questions

1. Distribution: vendor / fetch / operator-path. (M2 pushes hard toward operator-path.)
2. Where the backend seam lives without colliding with `lane/twilio-whatsapp-identity`.
3. What can be live-proven vs. what must be reported unrun.

---

## Closing measurements (see WHATSAPP-BRIDGE-SUMMARY.md for the full account)

- **M4 — distribution:** cross-audit 3/3 for (D) operator-provided path. All three raised
  version skew; that produced the `health` handshake.
- **M5 — my own comment was false.** `bridge.js` does NOT fall back to baileys on an
  unrecognised `--backend`; it echoes the value verbatim and fails at load with `-32000`. The
  fallback applies to an ABSENT or valueless flag. Corrected everywhere.
- **M6 — my probe over-claimed.** The real bridge answers `health` with no `node_modules`;
  only `connect` fails. Added `bridge_dependencies` and `whatsapp_pairing` gates.
- **M7 — final verification at `0d844959`:** unit 69 passed / 0 ignored / 0 filtered;
  live 6 passed / 0 ignored / 0 filtered against the real hash-verified bridge.js under real
  Node; clippy `-D warnings` rc=0; `cargo check --workspace --all-targets` rc=0.
