# durable-posture — running NOTES (append after every measurement)

Lane: `lane/durable-posture`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-durable-posture`.
Base integration: `c9ab048b952c5bc74c75ea8f76df06788408de59` (asserted via
`/usr/bin/git rev-parse HEAD`, unproxied).

## T+0 — premise verification (Mac, unproxied tools only)

| # | Brief premise | Verdict | Evidence |
|---|---|---|---|
| 1 | `c73ac417` = "a host with no keyring now runs, degraded and announced" | **HOLDS** | `git log -1 c73ac417df54ec9069a2c376d72feba218f8e85c`, dated Thu Jul 30 20:20:56 2026 |
| 2 | `906287e1` (2026-07-16) = "feat(recovery): seal interrupted turn state" | **HOLDS** | `git log -1 906287e1790ab2e0c8a6f1f71940e9acc2b55c75`, Thu Jul 16 12:54:28 2026 |
| 3 | `crates/wcore-cli/tests/f14_sigkill_recovery.rs:1106` asserts no provider + no turn without secure store | **HOLDS, with an addition the brief omits** | fn `isolated_profile_without_secure_store_fails_before_turn_or_provider_intent` starts at line 1106. It asserts `engine_error`, `retryable=false`, `fixture.observation().requests.is_empty()` — AND at line 1143 it does `fs::read(&journal).expect("read preflight-failed session journal")`, i.e. **it requires the journal FILE to exist** and merely be free of `turn_started` / the prompt. See §"Premise 3 addendum". |
| 4 | `WAYLAND_VAULT_PASSPHRASE` / `_FD` live in `crates/wcore-config/src/credentials.rs` | **HOLDS** | credentials.rs:883 (`_FD`), :893 (env var), :1104 `vault_unlock_material_present()` |
| 5 | Test passes at `b8311575`, fails at integration head at `f14_sigkill_recovery.rs:204` | **UNVERIFIED at T+0** — needs hetzner (test is `#[cfg(target_os = "linux")]`, cannot run on the Mac) | — |

Instrument control for the greps above: the same `/usr/bin/grep -rn WAYLAND_VAULT_PASSPHRASE crates/`
returned 40 hits across 14 files (known-positive alive); the same grep restricted to
`crates/wcore-config/src/` returned 31 hits with rc=0.

### Premise 3 addendum — the July-16 test is NOT purely "must not run"

The brief characterises `isolated_profile_without_secure_store_fails_before_turn_or_provider_intent`
as asserting the product "must NOT reach the provider and must NOT start a turn". True, but
incomplete, and the omission is load-bearing for this lane: the test **also requires the session
journal file to exist and be readable** (`fs::read(&journal).expect(...)`, line 1143). So the
July-16 posture was *not* "refuse and write nothing" — it was **refuse, but still journal the
refusal**. That is much closer to what this lane is being asked to build than the brief implies,
and it means the c73ac417 degrade (which creates no `sessions/` directory at all) reverses TWO
properties, not one.

## Architecture as measured (integration head c9ab048b)

The decision chain, single point, `wcore-config/src/config.rs:2478`:

```
Config::resolve_inner
  -> durable_sessions_must_be_disabled(session.enabled, backend, || confidential_recovery_storage_available())
       == session_enabled && backend.supports_confidential_material() && !measure_availability()
  -> resolved.session.enabled = false ; record_durable_sessions_disabled_by_host()
```

`confidential_recovery_storage_available()` (config.rs:2544) →
`credentials::confidential_backend_available` (credentials.rs:1502) →
`confidential_backend_plan` + `select_confidential_backend` with availability inputs
`(keyring_available, vault_unlock_material_present())`.

`vault_unlock_material_present()` (credentials.rs:1104) is **only**:

```rust
#[cfg(unix)] if env WAYLAND_VAULT_PASSPHRASE_FD is set { true }
env WAYLAND_VAULT_PASSPHRASE is set
```

**So the whole cliff is one predicate.** `CredentialsBackend::Auto` offers exactly two
confidential candidates — `Keyring{service}` and `EncryptedFile{cipher,kdf}` — and the
EncryptedFile candidate is judged unavailable purely because nobody typed a passphrase.
There is no third candidate and no auto-provisioned key. That is the false dichotomy the
lane exists to dissolve, and it is localised to one function.

Also relevant: a backend marker is pinned at
`.credentials.confidential-backend.json` on first successful open
(`resolve_confidential_backend_with_availability`, credentials.rs:602-611), and a pinned
backend that is later unavailable is a **hard error**, not a downgrade
(credentials.rs:506-510). That is the existing precedent for task 2 (refuse when a journal
exists but its key does not) — the mechanism already exists at the credentials layer; the
question is whether it covers the *journal* rather than the *store*.

## Open / next

- [ ] Verify premise 5 on hetzner (both SHAs, same test, read `N passed` from a file).
- [ ] Read `wcore-agent/src/recovery_confidential.rs` — what the journal actually needs the
      confidential store FOR (a key? sealed request blobs?). Determines whether a
      machine-local key is even coherent.
- [ ] Design the key-at-rest question, then 4-way cross-audit BEFORE implementing.
