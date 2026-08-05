# WhatsApp through the Desktop bridge (opt-in)

**What wayland-core can and cannot do with WhatsApp, what you must install yourself, and what
happens when you have not.**

This document is a description of what the code does today, not an aspiration. Every claim
below traces to a source line or to a measurement on this page.

---

## 1. The short version

| | |
|---|---|
| **Default** | `meta-business` — the official Meta WhatsApp Business Cloud API, over HTTPS. **No Node, no bridge, nothing to install.** A config with no `backend` key is this. |
| **Opt-in** | `baileys` and `whatsapp-web` — two **unofficial** WhatsApp Web clients, driven through a Node subprocess that wayland-core **does not ship**. |
| **If you do nothing** | Nothing changes. The bridge code is inert until a config names a bridged backend. |
| **If you opt in without installing the bridge** | The channel refuses to start and names exactly what is missing. It never reports healthy. |

---

## 2. Terms of service — read this before enabling a bridged backend

`baileys` and `whatsapp-web` drive the WhatsApp **Web** protocol from a personal WhatsApp
account. Both are reverse-engineered, unofficial clients. Meta does not support them, has not
published permission for them, and **bans accounts for automated use** — Wayland Desktop's own
bridge README states the risk plainly as *"Risk of Meta bans for high-volume bot use."*

"Widely used in practice" is an observation about other people's accounts, not a guarantee about
yours. The account at risk is the phone number you pair, and a ban is applied to that number,
not to this software. `meta-business` is the supported, always-works alternative: it is a
commercial API, it is paid per message, and it does not put a personal number at risk.

This is the same posture [providers.md](providers.md) takes on the Codex `client_id` path, and
for the same reason: the honest statement is that the path works today and is not sanctioned.

Additionally, `npm install` for the bridge currently reports a **published security advisory
against the pinned Baileys version** (`GHSA-qvv5-jq5g-4cgg`, measured 2026-07-30 against
`@whiskeysockets/baileys@7.0.0-rc.9`). That dependency tree is the operator's, not
wayland-core's — which is one of the reasons wayland-core does not ship it.

---

## 3. Why wayland-core does not ship the bridge

The bridge is a Node program with a **139 MB** dependency tree (measured 2026-07-30). Four
distribution options were considered; the operator-supplied path was chosen, unanimously
endorsed by a three-model cross-audit:

| Option | Why not |
|---|---|
| Vendor bridge + `node_modules` into this repo | Puts 139 MB and someone else's CVE surface into every clone and every release of a single-binary CLI. |
| Vendor the JS, `npm install` on first use | Makes a Node toolchain a de facto dependency of installs that will never send a WhatsApp message, and turns the CLI into a package installer at runtime. |
| Download the bridge at install or runtime | Fetching and executing code over the network is a supply-chain surface this project does not otherwise have. It is the worst option on that axis. |
| **Operator-supplied path** ✅ | Costs the operator real setup work — and nothing else. Zero bytes in the release, no Node dependency, no network fetch, no redistribution. |

**On licensing.** Executing a copy of the bridge that you already have is not redistribution, so
wayland-core carries no third-party notice for it. If that ever changes — if a release ships or
fetches the bridge — then `@whiskeysockets/baileys` (MIT), `whatsapp-web.js` (Apache-2.0), and
the upstream attributions the bridge's own sources carry would all belong in
`THIRD-PARTY-NOTICES.md`. They are deliberately absent today because nothing is being
redistributed, and an attribution for code we do not ship would be its own defect.

The honest cost of this choice: **you must obtain and install the bridge yourself, and keep its
version compatible.** §5 is the handshake that stops a version mismatch from being silent.

---

## 4. Setup

### 4.1 What you need

1. **Node 18+** on `PATH`, or an explicit `node_path`.
2. **A copy of the bridge** — the `whatsapp-bridge` directory from a Wayland Desktop install
   (`<resources>/whatsapp-bridge/`) or from a checkout of it.
3. **Its dependencies installed** — run `npm install` (or `bun install`) *in the bridge's own
   directory*. This is the step people miss; see §5.

### 4.2 Config

`~/.wayland/channels/wa-personal.toml`:

```toml
platform = "whatsapp"

[options]
backend      = "baileys"
bridge_path  = "/opt/wayland/whatsapp-bridge/bridge.js"
session_dir  = "/var/lib/wayland/whatsapp"      # optional
node_path    = "/usr/local/bin/node"            # optional; PATH is used otherwise
```

Leaving `backend` out, or setting it to `meta-business`, selects the Cloud API adapter and the
rest of this document does not apply — see the `WhatsappChannelConfig` schema instead.

### 4.3 Pairing

Both bridged backends authenticate by QR. On first `connect` the bridge emits a `qr.update`
notification and prints the code; wayland-core surfaces this as a platform warning telling you
to scan it under **WhatsApp → Linked devices**. The resulting session material is written under
`session_dir` (`<session>/baileys/creds.json`, or `<session>/whatsapp-web/session-wayland`).

---

## 5. What happens when something is missing

Nothing is assumed and nothing is defaulted. `probe()` answers with a named finding, and
`ProbeOutcome::is_ready()` is true for exactly one of these — the last row.

| Finding | Meaning | Fix |
|---|---|---|
| `node_runtime` | No `node` on `PATH`, or `node_path` is not a file | Install Node 18+, or correct `node_path` |
| `bridge_path` | The path is not a file. **wayland-core does not ship the bridge** | Point it at a real `bridge.js` |
| `bridge_dependencies` | The script exists but its backend package does not resolve | `npm install` in the bridge's directory |
| `whatsapp_pairing` | Everything is installed and reachable; the number has never been paired | Start the channel and scan the QR |
| *(no findings, `Ok`)* | Reachable, correct backend confirmed, pairing material present | — |

Every one of these is verified live, against the real unmodified `bridge.js` under a real Node,
in `crates/wcore-channel-whatsapp/tests/live_bridge.rs`.

**`bridge_dependencies` exists because of a measurement, and it is the interesting one.** The
real bridge answers its `health` RPC perfectly happily with **no `node_modules` at all** —
`health` is special-cased before any backend is loaded. Only the first `connect` then fails with
`Failed to load backend baileys: Cannot find module …`. A readiness check built on the handshake
alone would therefore have reported a green for a bridge that could not send a single message.

**Two limits of the `Ok` verdict, stated rather than glossed:**

- **Pairing is inferred from files on disk.** Session material that exists but has been revoked
  server-side reads as paired until the bridge reports `logged_out`. A probe sends nothing, so
  it cannot ask WhatsApp.
- **`Ok` has never been reached with a genuine WhatsApp pairing in this project's testing.** It
  is proven *reachable* by a harness that places the session material, which demonstrates the
  gate opens — not that WhatsApp accepted anything.

### 5.1 The backend the bridge reports is the one we use, or there is no session

wayland-core spawns `node <bridge.js> --backend <name>` and then calls `health`, which returns
the backend the bridge actually loaded. **If that does not match what was requested, the session
is refused** and the probe reports `Unauthenticated{backend_mismatch}`.

This is not defensive decoration. Measured against the real bridge on 2026-07-30:

- `--backend baileys` → `health` reports `baileys` ✅
- `--backend whatsapp-web` → `health` reports `whatsapp-web` ✅
- **`--backend` absent, or present with no value → `health` reports `baileys`.** A bridge that
  does not understand the flag therefore looks exactly like this, and would silently drive an
  unofficial client against a personal number.
- `--backend baileyz` → `health` echoes `baileyz` verbatim; the failure surfaces only later at
  backend load, as `-32000 Unknown backend: baileyz`.

wayland-core never sends an unrecognised name — `backend` is a closed enum and an unknown value
is rejected when the config is parsed, naming the valid options. The handshake covers the case
the enum cannot: a `bridge_path` pointing at a bridge that ignores or predates `--backend`.

---

## 6. What works, and what does not

| Operation | Bridged backends | Note |
|---|---|---|
| Send text | ✅ | `sendText` |
| Receive text | ✅ | `inbound.message`; own echoes (`fromMe`) are dropped |
| React | ✅ | `react` |
| Typing indicator | ✅ | `setPresence: composing` |
| Group messages | ✅ inbound | `isGroup` maps to `ChatType::Group` |
| **Send attachments** | ❌ **refused, loudly** | Both backends' `sendMedia` require a **local `filePath`** and reject anything else; `OutgoingMessage` carries URLs. There is no honest mapping, so the send returns `Unsupported` rather than delivering the text and dropping the file. The `mediaUrl` form the bridge README mentions is `meta-business` only. |
| **Receive attachments** | ❌ not claimed | The bridge writes media to a local `mediaPath` rather than exposing a URL; `Channel::fetch_media` has no local-path contract here. |
| Edit / delete | ❌ | The bridge's `ALLOWED_RPC_METHODS` has no such method — there is no RPC to call. |
| Idempotent send | ❌ | No key is transmitted. `supports_outbound_idempotency()` is left `false`. See [delivery-semantics.md](delivery-semantics.md). |

---

## 7. Security notes

- The subprocess is spawned in **argv mode** (`wcore_config::shell::shell_command_argv`), so the
  operator-supplied `bridge_path` and `session_dir` are separate argv entries and no shell ever
  interprets them.
- The bridge's own `allowlist.js` rejects any RPC outside its allowlist with `-32601`; verified
  live (`evilMethod` → `-32601 Method not allowed`).
- The QR pairing payload is **not** copied into the event stream. `qr.update` surfaces an
  instruction, not the code, so the pairing material does not end up in logs.
- `kill_on_drop` is set on the child, and `stop()` reaps it explicitly, so a dropped channel does
  not leave a Node process holding a WhatsApp socket open.
