# NOTES — lane `fix-keyringless-inbound`

Branch `lane/fix-keyringless-inbound`, base integration `e7bc6d88`.

Goal: (1) drive a REAL inbound turn on a REAL platform from a keyring-less host,
with the platform's own API as the arbiter of the reply; (2) close the three
keyring residuals the merged lane (`c73ac417`) named openly.

---

## Phase 0 — premise verification (Mac, read-only)

Every instrument below is `/usr/bin/grep` with a quoted glob (zsh eats
`--include=*.rs` unquoted — hit that immediately, first invocation returned
`no matches found` with rc=1, which would have read as a false absence).
Each absence claim below is paired with a known-positive in the same capture.

### P1 — "`durable_sessions_disabled_by_host()` has NO consumer" — HOLDS

```
/usr/bin/grep -rn "durable_sessions_disabled_by_host" --include='*.rs' crates/
```
5 hits, ALL inside `crates/wcore-config/src/config.rs`:
- 2484 `record_durable_sessions_disabled_by_host();`  (producer, in `Config::resolve`)
- 2598 `pub fn durable_sessions_disabled_by_host()`   (the getter)
- 2604 `fn record_durable_sessions_disabled_by_host()` (the setter)
- 5232/5234 — inside the crate's own `#[cfg(test)]` module.

So: zero production consumers, and zero consumers outside the defining crate.
Known-positive control in the same capture: `fn main` under `crates/wcore-cli/src`
returned 5 hits, so the instrument was alive.

### P2 — "`switch_active_session`, journal writer #3, is still unguarded" — HOLDS

`crates/wcore-agent/src/engine.rs:3696`. Read the body: it validates the
incoming journal (session-id match, canonical baseline present), then
unconditionally `self.session_journal = Some(journal);`. There is no
`config.session.enabled` consultation anywhere in the function.

Contrast with writer #2, which `c73ac417` DID guard — `engine.rs` +3342:
```rust
let session_journal = if config.session.enabled { session_journal } else { None };
```

Production call site: `crates/wcore-cli/src/tui/engine_bridge.rs:3081`.
That path is FENCED to another lane, so the guard must live in `engine.rs`.
It belongs there regardless: the invariant is the engine's, not the caller's.

### P3 — "`--json-stream` / Desktop hosts are never told" — TO MEASURE

Decision still open. Contract regeneration is orchestrator-only, which
constrains the shape of any answer.

---

## Still to establish

- [ ] hetzner worktree at `e7bc6d88`, SHA asserted programmatically
- [ ] PROVE the keyring-less state rather than assume it (headless Linux may
      still carry a gnome-keyring socket — that is an absence claim and gets a
      known-positive like any other)
- [ ] Discord channel configured + `gateway run` with `channels registered>=1`
      (the prior lane's gateway evidence shows `registered=0`, so its gateway
      leg never had a channel at all)
- [ ] real inbound message -> turn -> reply, READ BACK FROM THE DISCORD API
- [ ] negative control on base `bc90ee1c` — must FAIL
- [ ] positive control with a working credential store — must PASS
- [ ] seeded-break control proving the harness can redden at all
