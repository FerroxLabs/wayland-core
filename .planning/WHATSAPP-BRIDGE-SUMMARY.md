# SUMMARY — lane/whatsapp-bridge

**Verdict: goal ACHIEVED, with one capability honestly unrun.** Core can now reach all three
WhatsApp backends. It cannot yet be said to have *delivered a WhatsApp message* through the two
new ones, and that cell is reported unrun rather than filled in.

- Branch `lane/whatsapp-bridge`, HEAD **`0d8449590e6a92a21e467deff222b6a1bcf53751`**
- Merge-base **`4caaa31c891c0d606e5de1e91cdcd3e5a79ab767`**
- Diff: 12 files, +2755 / −2. `Cargo.lock` untouched. Fenced `wcore-cli` files untouched.

---

## 1. The distribution decision, and why

**Chosen: (D) operator-provided path.** `bridge_path` in the channel config names an existing
`bridge.js`; Core spawns `node <path> --backend <name>` and never ships, fetches or installs
anything.

Cross-audited three ways per LANE-BRIEF §4 — **codex 5.6 Sol, gemini 3.1 Pro, Kimi K3 all
returned `PANEL_POSITION=D`** (3/3; instrument controls: known-positive grep = 1, known-negative
= 0). All three named the same counter-argument — operator friction and **version skew**, since a
path says nothing about which dialect that bridge speaks. That objection is what produced the
`health` handshake in §3; the panel earned its keep.

The reasoning I'd defend independently of the vote:

1. **A single-binary install must keep working.** The bridge's dependency tree is 122 MB as
   Desktop ships it and 139 MB from a fresh `npm install` (both measured 2026-07-30). Vendoring
   it, or running a package install on first use, makes Node a de facto dependency of every Core
   install including the ones that will never send a WhatsApp message. Sean's constraint was
   explicit and (A)/(B) both violate it.
2. **Fetching executable code at runtime** is a supply-chain surface Core does not otherwise
   have. (C) is the worst option on that axis and I rejected it outright.
3. **Not redistributing carries no licence obligation** — see §2.

**Its honest cost:** the operator must obtain the bridge, install its dependencies, and keep the
version compatible. That is real friction and I have not disguised it; `docs/whatsapp-bridge.md`
§3 states it as the price of the choice.

---

## 2. Provenance — the brief's premise is FALSE if we vendor, TRUE if we do not

The brief says *"The bridge is Sean-owned Desktop code moving to Sean-owned Core code. That
raises no third-party question."* Measured against the source, that holds **only** because we
chose (D):

- `bridge.js:2-5` and `allowlist.js:1-4` carry a header **the Desktop project itself wrote**:
  *"Portions adapted from Hermes Agent … Peter Steinberger / Hermes Agent contributors — MIT"*.
- Its `README.md:68`: `backends/baileys.js` — *"ported from OpenClaw … MIT"*.
- Its `README.md:69`: `backends/whatsapp-web.js` wraps `whatsapp-web.js`, Apache-2.0.

Per `PROVENANCE-COMPARISON.md` §4.1 the OpenClaw→Desktop hop is the **real** kind — *"planned as
a derivation, executed as one, and attributed as one by the Desktop project itself"* — not one of
the five false headers §6 says to strip. So **vendoring the JS would have imported three
third-party obligations into a Core release.** Executing an operator's own copy imports none.

**Action taken: I added nothing to `THIRD-PARTY-NOTICES.md`, deliberately.** Nothing is
redistributed, and an attribution for code we do not ship would be the same over-attribution
defect the audit flagged. The reasoning is recorded in `docs/whatsapp-bridge.md` §3 with the
trigger condition spelled out: if a release ever ships or fetches the bridge, Baileys (MIT),
whatsapp-web.js (Apache-2.0) and the upstream headers all belong in that file. **I did not write
the word "ported" anywhere, and translated no code.**

---

## 3. What Core can now do that it could not

`crates/wcore-channel-whatsapp/src/bridge/` — a JSON-RPC-2.0-over-stdio client for the Desktop
bridge. `backend = "baileys" | "whatsapp-web"` in a channel TOML routes to it; absent or
`"meta-business"` routes to the existing Cloud API adapter, unchanged.

Implemented: send text, receive text (own echoes dropped), react, typing, group inbound, QR
pairing surfaced as a platform warning, `logged_out` distinguished from a retryable drop.
Refused loudly: attachments (both bridged backends' `sendMedia` require a **local filePath** and
`OutgoingMessage` carries URLs — the `mediaUrl` form is `meta-business` only), edit/delete (no
such RPC in the bridge's allowlist).

**`supports_outbound_idempotency()` is left at the trait's `false` default.** The bridge
transmits no key. I set no capability bit on mock evidence.

**Reuse:** `wcore-channel-signal` — not `wcore-mcp` — is the in-repo precedent, and I modelled
the reader/pending-map on it. It is already a JSON-RPC-over-stdio subprocess channel; MCP's
stdio transport is coupled to MCP protocol semantics. Spawning goes through
`wcore_config::shell::shell_command_argv` (argv mode, `kill_on_drop`), which is stricter than
signal's own `Command::new`.

---

## 4. What an operator must install, and what happens when they have not

Four named findings, none of which can be reached by accident, all verified live:

| Missing | Finding | Verdict |
|---|---|---|
| Node | `node_runtime` | `Incomplete`, `is_ready()==false` |
| `bridge.js` | `bridge_path` | `Incomplete` |
| the backend's npm package | `bridge_dependencies` | `Incomplete` |
| a QR pairing | `whatsapp_pairing` | `Incomplete` |
| nothing | — | `Ok` |

**`bridge_dependencies` exists because I found a false-advertising defect in my own first cut.**
The real bridge answers `health` **with no `node_modules` at all** — `health` is special-cased
before any backend loads — and only the next `connect` fails with `Cannot find module`. My probe
was therefore reporting `Ok`/`is_ready()==true` for a bridge that could not send a single
message. Closed by resolving the backend's package, walking ancestors the way Node does so a
hoisted install is not a false red. A second gate now requires pairing material on disk, because
`ProbeReport::ok` sets `authenticated: true` and claiming that without evidence is the same
defect in a different place.

**Two limits of `Ok`, stated not glossed:** pairing is inferred from files, so revoked-but-present
session material reads as paired until the bridge reports `logged_out`; and `Ok` has never been
reached with a genuine WhatsApp pairing (§6).

---

## 5. Both-directions evidence

All figures read back from unproxied tools over ssh, `N passed` / `ignored` / `filtered out` all
asserted. Captures in `.planning/evidence/whatsapp-bridge/`.

**Unit — `69 passed; 0 failed; 0 ignored; 0 filtered out`** (whatsapp) plus `13/8/3/1 passed`
(registry). **Clippy `-D warnings` rc=0. `cargo check --workspace --all-targets` rc=0, 0 errors**
(workspace, not per-crate). `cargo fmt --all -- --check` clean on the Mac.

**Live — `6 passed; 0 failed; 0 ignored; 0 filtered out`**, against the **real, unmodified**
`bridge.js` (sha256 `0e9c4d0b…7239`, verified identical on both hosts) under real Node v22.21.1
on hetzner, with real `@whiskeysockets/baileys` and `whatsapp-web.js` installed:

| Direction | Test | Result |
|---|---|---|
| PASS | preflight clears against the real installed bridge | ok |
| PASS | handshake reaches the real bridge; verdict stops at `whatsapp_pairing` | ok |
| PASS | `--backend whatsapp-web` also clears — the selector genuinely selects | ok |
| **PASS** | **the `Ok` verdict is reachable** (§3b-iii — a permanently-red gate proves nothing) | ok |
| FAIL-CLOSED | missing bridge, **with real Node present** → `bridge_path` | ok |
| FAIL-CLOSED | byte-identical script, no deps → `bridge_dependencies` | ok |

**Selector, both directions:** `WhatsappBackend::from_str` accepts exactly three names and
rejects `baileyz`/`""`/`META-BUSINESS`/`whatsapp_web`/`cloud`; the registry factory constructs
the Cloud adapter for absent and for `meta-business`, the bridge adapter for `baileys`, and
**rejects `baileyz` with an error naming the typo and the valid options** — with the
known-positive in the same test so a factory that rejected everything could not pass.

**Raw measurement of the real bridge** (`real-bridge-backend-matrix.txt`): `--backend baileys` →
`health: baileys`; `--backend whatsapp-web` → `health: whatsapp-web`; a disallowed method →
`-32601 Method not allowed`; `connect` with no deps → `-32000 Cannot find module`.

---

## 6. Unrun, and why

**No message has ever been sent through the bridge from Core.** Doing so requires QR-pairing a
real personal WhatsApp number, which is Sean's to do and carries the ban risk in §7. Everything
up to and including the handshake and every readiness gate is live-proven; **delivery is not, and
the cell is unrun rather than inferred.** `docs/delivery-semantics.md` now carries a bridge row
labelled *"NOT MEASURED, and no replay has been driven at all"*.

The `Ok` verdict's live proof is a **reachability** proof: the harness places the session
material, demonstrating the last gate opens. It is not evidence WhatsApp accepted anything, and
the test says so in its own doc comment.

`live_bridge.rs` is `#[ignore]`d because it needs an artifact this repo deliberately does not
ship — but there is **no silent-skip path**: an unset or wrong `WCORE_TEST_BRIDGE_PATH` panics.

---

## 7. ToS wording landed

`docs/whatsapp-bridge.md` §2, matching the `providers.md:374` Codex-`client_id` precedent:

> `baileys` and `whatsapp-web` drive the WhatsApp **Web** protocol from a personal WhatsApp
> account. Both are reverse-engineered, unofficial clients. Meta does not support them, has not
> published permission for them, and **bans accounts for automated use** … "Widely used in
> practice" is an observation about other people's accounts, not a guarantee about yours. The
> account at risk is the phone number you pair, and a ban is applied to that number, not to this
> software.

Also disclosed there: `npm install` reports a **published advisory against the pinned Baileys
version** (`GHSA-qvv5-jq5g-4cgg`, `@whiskeysockets/baileys@7.0.0-rc.9`, measured 2026-07-30) —
an independent argument for not carrying that tree in a Core release.

---

## 8. For the orchestrator — shared files touched

Serialise against `lane/twilio-whatsapp-identity`, which is in the same crate:

| File | My change |
|---|---|
| `crates/wcore-channel-whatsapp/src/config.rs` | **+1 field** (`backend`, `#[serde(default)]`) + 1 line in `new_for_test` + 2 tests. Nothing else touched. |
| `crates/wcore-channel-whatsapp/src/lib.rs` | **+2 lines**: `pub mod bridge;` and one `pub use`. |
| `crates/wcore-channel-whatsapp/Cargo.toml` | tokio `io-util`/`process` features; `tempfile` dev-dep. **`Cargo.lock` unchanged** (both already in the workspace). |
| `crates/wcore-channels-registry/src/lib.rs` | `make_whatsapp` gains a backend peek + 2 tests. |
| `crates/wcore-channel-whatsapp/schemas/whatsapp.json` | +1 `backend` property. |
| `docs/delivery-semantics.md` | +1 table row, +1 paragraph. |
| **Mine alone** | `src/bridge/{mod,preflight,rpc}.rs`, `tests/live_bridge.rs`, `schemas/whatsapp-bridge.json`, `docs/whatsapp-bridge.md` |

No `wcore-cli` edit, no protocol seam, no contract request, no `Cargo.lock` change.

**Merge status:** integration has moved `4caaa31c` → `12b0c18d` under me. `git merge-tree`
against it reports **0 conflict markers** over 3490 lines of output (instrument alive:
`wcore-channel-whatsapp` appears 17 times in the same capture). Two files are *changed in both*
and auto-merge — `wcore-channel-whatsapp/src/lib.rs` (twilio added ~100 lines at :149, my edit
is 2 lines at the top) and `docs/delivery-semantics.md`.

### 8a. Pre-existing defect the orchestrator should know about: `Cargo.lock` is stale on integration

**`cargo metadata --locked` exits 101 on a pristine, clean checkout of both `4caaa31c` and
current integration head `12b0c18d`** — measured in a throwaway detached worktree, tree verified
empty by `git status --porcelain` in the same breath, since removed. This is **not mine**:
`crates/wcore-channels-registry/Cargo.toml` declares `serde_json`, `wcore-egress`, `hmac`, `sha2`
and `hex`, none of which the committed lock lists, and `git diff 4caaa31c -- <that Cargo.toml>`
is empty. A `rustls` entry is missing from another crate for the same reason.

It matters because `--locked` gates real jobs: `supply-chain.yml:165,168,180`,
`release.yml:343`, `ci.yml:301,691`. **Those are red on integration today, before any lane
merges.**

I deliberately did **not** commit a regenerated lock. Doing so would have attributed five other
crates' entries to this lane and guaranteed a conflict with every other lane that regenerated
it. My own change needs exactly one line — `tempfile` under `wcore-channel-whatsapp` — which a
single `cargo update -w`-style refresh by whoever fixes the pre-existing drift will pick up. I
restored the file with `git checkout -- Cargo.lock` (permitted; moves no ref).

---

## 9. Premises I measured false

1. **My own, and the most important.** I wrote — in code comments and a commit message — that
   `bridge.js` falls back to `baileys` on an *unrecognised* `--backend`. **False.** It echoes an
   unknown value back verbatim (`health: "baileyz"`) and fails later at load with `-32000`. What
   it *does* default to `baileys` is an **absent or valueless** flag. Corrected in the module
   docs, the error type's docs and `docs/whatsapp-bridge.md`. The handshake is still load-bearing
   — for a bridge that does not understand the flag at all, which is exactly the absent case.
2. **My probe over-claimed.** `health` succeeds with no `node_modules`; `Ok` was reachable for a
   bridge that could not send. Two gates added (§4). Found by driving the real artifact, not by
   reading it.
3. **The brief's RPC list is incomplete.** It names seven methods; `ALLOWED_RPC_METHODS` has
   **nine** — plus `webhookDelivery` and `health`. `health` is the one answered before backend
   load, which is what made the handshake possible at all.
4. **The bridge README's `sendMedia` signature is `meta-business`-only.** It documents
   `filePath | mediaUrl | mediaId`; both bridged backends accept **only** `filePath`. This is why
   attachments are refused rather than mapped.
5. **`wcore-mcp` is not the closest precedent**, despite the brief pointing there.
   `wcore-channel-signal` is already a JSON-RPC-over-stdio subprocess channel.
6. **`${PIPESTATUS[0]}` produced an empty string** in this shell (LANE-BRIEF §2), silently. The
   push had in fact succeeded. Switched to `bash -s` heredocs for everything load-bearing after
   that.
