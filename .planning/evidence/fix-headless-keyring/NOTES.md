# NOTES — lane/fix-headless-keyring

Base integration commit: `bc90ee1c1f08b76e6682b4beab2386fc7216a52e`.
Started 2026-07-30.

## Minute 0-15: source located, single upstream point identified (pre-build, unproven)

Two UAT lanes reported the same user-visible failure from opposite directions:

- `uat-channels-live` #1: every inbound channel turn → `WARN inbound turn dispatch failed
  error=Session persistence authority unavailable: secure recovery storage is unavailable:
  no OS keyring was usable and no encrypted credentials vault is unlocked`.
  Workaround found: `[session] enabled = false`.
- `uat-tui-unix` F2: `wayland-core --no-tui "<prompt>"` → rc=1, same message.
  Workaround found: `WAYLAND_VAULT_PASSPHRASE`.

### The two workarounds are two ends of ONE call chain

```
AgentEngine::run / run_with_content            crates/wcore-agent/src/engine.rs:6258
  └─ if self.session_journal.is_some()          <- [session] enabled=false kills this arm
       self.recovery_request_protection.preflight(&self.config)?      engine.rs:6259
         └─ RecoveryRequestProtector::with_key   recovery_confidential.rs:192
              ├─ reject_backend_without_confidential_storage(config)  (static half)
              └─ config.open_confidential_credentials_store()         recovery_confidential.rs:207
                   └─ wcore_config::credentials::open_confidential_store  credentials.rs:1451
                        └─ select_confidential_backend                credentials.rs:470
                             Auto => keyring available? else vault_unlock_material_present()?
                                     else Err("no confidential credential backend is available")
                                                                       <- WAYLAND_VAULT_PASSPHRASE
                                                                          flips this to Ok
```

`[session] enabled = false` removes the *caller*. `WAYLAND_VAULT_PASSPHRASE` satisfies the
*callee*. One chain, two ends — which is why two lanes found two different fixes.

### The defect is TIMING, and half of it was already fixed

`init_session` (engine.rs:3608-3646) already refuses early — but only for the **statically
decidable** half:

```rust
// engine.rs:3620
crate::recovery_confidential::reject_backend_without_confidential_storage(&self.config)
```

and its doc comment (recovery_confidential.rs:107-113) states the intent exactly:

> "this is a pure function of config with no side effects, so a persisted session can refuse
> to open instead of accepting the session and failing every turn afterwards."

The **dynamically decidable** half — "no OS keyring and no unlocked vault", i.e. *the entire
headless-Linux case* — is NOT checked there. It is only discovered at the first turn.
`preflight(&self.config)` takes no turn state; it is a pure function of config + ambient env,
so it can run at exactly the place the static check already lives.

**Single upstream point: `AgentEngine::init_session`, `crates/wcore-agent/src/engine.rs:3614`
(the `if let Some(mgr) = &self.session_manager` arm).** Both entrances pass through it —
the channel path must, because `session_journal` (the thing gating the per-turn preflight) is
only ever set there.

## Chosen behaviour: DEGRADE, scoped to one cause (rationale, pre-proof)

`NoSecureBackendAvailable` (no keyring + no vault) → run this session **without durable
persistence**, announce once at startup. Every other confidential error keeps today's refusal.

Why degrade rather than refuse at startup:
1. It writes **no** confidential material anywhere, so it relaxes no security property. It is
   the same posture the operator reaches today by hand with `[session] enabled = false`.
2. A gateway that answers messages without crash-recovery beats one that answers none.
3. It makes `Healthy` truthful *structurally* — the product can answer — instead of by adding
   another health probe. That keeps me out of `fix-channel-health-truth`'s files.
4. An operator on a server should not have to know an env var exists.

Why scope it to ONE cause: a wrong vault passphrase (`SecureStoreUnreadable`) or an explicitly
configured plaintext backend (`PlaintextBackendRejected`) are configurations the operator
*chose*. Silently degrading those would hide a real misconfiguration. Only "this host has no
secure store at all" degrades.

## Still to establish

- [ ] Confirm `session_journal` is set ONLY by `init_session` (grep, with known-positive).
- [ ] Reproduce the bug on hetzner BEFORE the fix (quadrant 2).
- [ ] Implement.
- [ ] Quadrants 1/3/4 on hetzner.
- [ ] fmt / clippy / check --workspace --all-targets / cargo metadata --locked.
