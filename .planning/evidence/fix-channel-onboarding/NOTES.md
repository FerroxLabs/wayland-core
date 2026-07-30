# NOTES — lane/fix-channel-onboarding

Base integration commit: `bc90ee1c1f08b76e6682b4beab2386fc7216a52e`
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-fix-channel-onboarding`
Started 2026-07-30.

Brief: fix UAT findings #3 (documented config does not load; inert `[secrets]`/`keychain:`)
and #4 (no CLI verb writes a channel credential).

---

## Brief-premise re-verification at base (LANE-BRIEF: "your brief's measurements are probably stale")

Every claim measured with `/usr/bin/grep` (unproxied), from the lane worktree root.

### Premise A — "`[secrets]` / `keychain:` syntax in `config.rs` is inert"

**HOLDS.** Search: `/usr/bin/grep -rn 'keychain:' crates --include='*.rs'`

10 hits. Breakdown:
- `wcore-acp/src/auth.rs` x5 + `wcore-cli/src/acp.rs` x1 — these are `wcore_config::keychain::`
  **module path** hits (`keychain::get_secret`), i.e. a REAL working API, unrelated to the
  channel `[secrets]` string syntax.
- `wcore-channels/src/config.rs:6,34` — the two doc comments that document the inert syntax.
- `wcore-channels/src/config.rs:142` — a test fixture using it.
- `wcore-channels-registry/src/lib.rs:532` — a test fixture using it.

So: **zero production code parses the `keychain:<service>:<account>` string form.**

Consumer search: `/usr/bin/grep -rn '\.secrets' crates --include='*.rs'`, filtered to
`ChannelConfig::secrets`. The sole consumer is
`wcore-channels-registry/src/lib.rs:459`:

```rust
let mut secret_keys: Vec<String> = cfg.secrets.keys().cloned().collect();
```

— **key names only, for a `ChannelSummary` report. Values are never read.** Confirms UAT #3.

Control (instrument alive): `/usr/bin/grep -rl 'ChannelConfig' crates --include='*.rs' | wc -l`
→ **21** files. The grep works.

Note the important asymmetry: `wcore_config::keychain::get_secret(service, account)` **exists
and works** (wcore-acp uses it in production). So implementing the syntax is cheap; that makes
"remove it" vs "implement it" a genuine decision, not a foregone one.

### Premise B — "exactly one `.put(` in wcore-cli, a provider OAuth token"

**HOLDS.** `/usr/bin/grep -rn '\.put(' crates/wcore-cli/src --include='*.rs'`
→ 1 hit: `crates/wcore-cli/src/tui/engine_bridge.rs:2390`.

### Premise C — "none of the four required fields is documented"

`docs/channels.md` is 200 lines. It documents `[inbound]` (access policy, tool posture, ack),
inbound media, `[inbound_webhook]`, and a "Recommended deployment baseline" that is an
`[inbound]` fragment ONLY — no `name`, no `platform`. Verification of the four fields pending
a live load.

Real schema, from source:
- `ChannelConfig` (`wcore-channels/src/config.rs`): `name` (required, must equal file stem),
  `platform` (required), `enabled` (default true), `options` (Table), `secrets` (Table),
  `inbound`. `#[serde(deny_unknown_fields)]`.
- `SlackConfig` (`wcore-channel-slack/src/config.rs`), parsed from `[options]`:
  `workspace_name` (required), `credential_handle_bot_token` (required),
  `credential_handle_signing_secret` (required), `default_channel_id`, `api_base_url`,
  `max_retry_attempts`. Also `deny_unknown_fields`.

---

---

## Measurement 2 — the credential idioms that ALREADY work (three, not one)

The brief says "design it consistently with the existing provider-credential path rather
than inventing a second idiom — find that path first." There is not one path. There are
three, and only two work:

| # | idiom | where | resolved by | status |
|---|---|---|---|---|
| 1 | `credential_handle*` key in `[options]`, value = a `CredentialsStore` key | all 10 channel adapters | adapter `start()` via `creds.get(handle)` | **WORKS** |
| 2 | `${cred:KEY}` embedded in an MCP header string | `config.toml` `[mcp.servers.*]` | `wcore-config/src/mcp_cred_refs.rs` at the connect boundary | **WORKS** |
| 3 | `[secrets] k = "keychain:<svc>:<acct>"` | channel config | **nothing** | **INERT** |

And separately, `auth add <provider> <key>` writes `config.toml`
`[providers.<slug>].api_key` — **plaintext in config.toml, not the credentials store at
all.** So `auth` is NOT the precedent for a channel credential; idiom 1 is, and the
`CredentialsStore::put` mechanics come from idiom 2 (`store_and_persist_forge`,
`engine_bridge.rs:2390` — the single `.put(` in the CLI).

### Consequence for the keychain: decision

Implementing idiom 3 would mean a **fourth** reference syntax reaching the **same** OS
keychain that `KeyringCredentialsStore` (the backend behind idiom 1) already reaches, and
would require editing all 10 adapters to read `cfg.secrets`. That is duplicate machinery for
an existing capability. **Decision: remove idiom 3.** Detail + cross-audit below.

## Measurement 3 — handle discovery must match a SUBSTRING and must RECURSE

Counted with `/usr/bin/grep -rhno '[a-z_]*credential_handle[a-z_]*' crates/wcore-channel-*/src/*.rs`:

```
 48 credential_handle                  20 user_credential_handle
 20 password_credential_handle         19 credential_handle_access_token
 10 credential_handle_app_id            9 credential_handle_auth_token
  9 credential_handle_app_secret        9 credential_handle_account_sid
  8 credential_handle_bot_token         8 credential_handle_app_password
  7 credential_handle_signing_secret
```

Two traps for anything that enumerates handles:

1. **Email breaks the prefix convention** — `user_credential_handle` /
   `password_credential_handle` are *suffixed*. A `starts_with("credential_handle")` scan
   silently misses email entirely. Must match *contains*.
2. **Email nests** — `EmailConfig` has `[options.smtp]` and `[options.imap]` sub-tables
   (`wcore-channel-email/src/config.rs:33,64`), each with its own pair. A flat scan of
   `[options]` misses all four. Must recurse.

This is the LANE-BRIEF §3b-i "search for the CONCEPT, not one keyword" trap, hit for real.

## Measurement 4 — `[secrets]` removal blast radius

`secrets` / `secret_keys` consumers, full list:
- `wcore-channels/src/config.rs` — the field + doc comments + 1 test fixture
- `wcore-channels-registry/src/lib.rs:459-467` — `ChannelSummary.secret_keys` (key names only)
- `wcore-cli/src/channel.rs:457` — `channel list --json` emits `"secret_keys"`
- `wcore-cli/src/tui/surfaces/diagnostics.rs:1758-1761` — TUI display

**No wire-contract fixture exposure**: `/usr/bin/grep -rn 'secret_keys' --include='*.json'`
over the tree returns 0. So no `wcore-contract generate` is implicated (LANE-BRIEF §0).

## Measurement 5 — the doc-honesty test has a usable seam

`channel_factory_for(platform)` (`registry/src/lib.rs:46`) returns a factory that runs
`parse_options::<PlatformConfig>()` and constructs the adapter **offline** (no network until
`start()`). So a test can take a TOML block straight out of `docs/channels.md`, parse it as
`ChannelConfig`, then hand `[options]` to the real factory with an in-memory
`CredentialsStore` — and a doc block missing `workspace_name` reddens. That is exactly the
drift the UAT hit.

Precedent to mirror: `tests/readme_live_evidence_agreement.rs`, which already structures
itself as a pure comparator plus doctored-input tests proving it fails AND a changed-world
test proving it can still pass.

---

---

## Decision 1 — `[secrets]` / `keychain:` — REMOVE. Cross-audit 4/4.

Question put to the panel is `/tmp/chanonb-audit-q.txt` (reproduced in the SUMMARY).
Raw replies: `/tmp/chanonb-audit-{codex,gemini,kimi}.txt`.

| panelist | vote | on the error message |
|---|---|---|
| codex `gpt-5.6-sol` | **B — remove** | NAMED migration error |
| gemini `3.1-pro-preview` | **B — remove** | NAMED migration error ("MUST") |
| kimi K3 | **B — remove** | NAMED migration error |
| internal adversarial (argued FOR keeping) | **B — remove** | NAMED migration error |

Unanimous. Extraction note (LANE-BRIEF §4): codex repeats its final block — took the LAST
`PANEL_POSITION=`; kimi bullet-prefixes and indents — matched unanchored, its vote is on an
indented `• PANEL_POSITION=B` line and an anchored `^PANEL_POSITION=` regex would have
dropped it, exactly as the brief warns.

### The internal adversarial pass, which I ran arguing AGAINST removal

Best case for keeping/implementing, and why it loses:

1. *"`[secrets]` is the only inline escape hatch — on a headless host with no keyring a user
   has nowhere else to put a secret."* — Loses. `CredentialsStore` already has
   `PlaintextCredentialsStore`, `EncryptedFileCredentialsStore` and `FallbackCredentialsStore`
   selected by config, so the headless case is a *store-backend* problem handled one layer down —
   and it is precisely what sibling lane `fix-headless-keyring` is repairing. Adding an
   inline-secret path here would undercut that lane and put plaintext secrets back into a
   per-channel file users paste into bug reports.
2. *"Removal breaks existing configs under `deny_unknown_fields`."* — Loses. Those configs
   are already broken, silently. Removal converts a silent auth failure into a named parse
   error. Strictly better for the same user.
3. **The decisive point, which the panel only half-reached: `[secrets]` HAS NO CONSUMER
   CONTRACT.** Even if the `keychain:<svc>:<acct>` string were resolved, there is nowhere to
   put the resulting value. Adapters read `creds.get(<handle from [options]>)`. No adapter
   field is bound to a `[secrets]` key; the resemblance between the fixture's `bot_token` and
   `SlackConfig::credential_handle_bot_token` is naming coincidence, not a binding. To
   "implement" it you would have to inject the resolved values into the `CredentialsStore`
   under synthesized keys — i.e. reimplement handles, with a second syntax. **The feature was
   never finished, not merely unimplemented**, so there is no design to restore.

So: remove the field, remove the doc comments, and reject a legacy `[secrets]` table with a
named error that points at the working path.

---

## Open questions / next steps

1. Reproduce the ORIGINAL failure on hetzner with the release binary before changing docs.
2. ~~Cross-audit the remove-vs-implement decision~~ — done, 4/4 remove.
3. Build `channel credential set|list|remove`, stdin-only for the value.
4. Doc-honesty test per Measurement 5, with a can-fail proof.
