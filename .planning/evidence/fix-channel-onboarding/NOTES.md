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

## Open questions / next steps

1. Reproduce the ORIGINAL failure on hetzner with the release binary before changing docs.
2. Decide implement-vs-remove on `keychain:`; cross-audit.
3. Design the credential verb against the existing provider-credential idiom (`auth` /
   `provider_keys.rs`) — find it first.
4. Doc-honesty test: candidate precedent
   `wcore-channels-registry/tests/delivery_semantics_declaration.rs`.
