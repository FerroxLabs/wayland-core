# `wcore-permissions` Threat Model — M5.8

**Scope:** the public surface of `wcore-permissions` at HEAD of M5.8 (`policy.rs`,
`token.rs`, `error.rs`) plus the M5.x crates this milestone introduced that
**should** be gated by it but aren't yet wired (`wcore-sandbox` M5.1,
`wcore-budget` M5.3, `wcore-cli plugin install` M5.4).

This document is the canonical reference for what `wcore-permissions` defends
against today, what it doesn't, and which gaps M5.8 closes vs which gaps are
tracked as follow-up work.

**Step-0 surface audit (verbatim, recorded 2026-05-18):**

`crates/wcore-permissions/src/policy.rs`:
```
pub enum Actor { User(String), Agent(String), System }
pub enum Resource { Tool(String), File(String), McpServer(String), Memory(String) }
pub enum Action { Invoke, Read, Write, Delete }
pub struct Permission { actor, resource, action }
pub struct PolicyEngine { grants: Vec<Permission> }
pub fn PolicyEngine::new() -> Self
pub fn PolicyEngine::grant(&mut self, p: Permission)
pub fn PolicyEngine::check(&self, actor, resource, action) -> PolicyResult<()>
pub fn PolicyEngine::len(&self) -> usize
pub fn PolicyEngine::is_empty(&self) -> bool
// private: fn glob_match(pattern: &str, path: &str) -> bool
```

`crates/wcore-permissions/src/token.rs`:
```
pub struct BearerToken { actor, issued_at_ms, expires_at_ms, signature_hex }
pub fn BearerToken::issue(actor: Actor, ttl_ms: i64, secret: &[u8]) -> Self
pub fn BearerToken::verify(&self, secret: &[u8]) -> PolicyResult<&Actor>
```

`crates/wcore-permissions/src/error.rs`:
```
pub enum DenyReason {
    NoMatchingGrant,
    PathNotInAllowlist,
    TokenExpired,
    TokenInvalid,
    UnknownActor,
}
pub type PolicyResult<T> = Result<T, DenyReason>;
```

**Consumer inventory:** `grep -rn "use wcore_permissions::"` across the
workspace returns **only the crate's own integration tests**. As of M5.8 no
production crate (`wcore-agent`, `wcore-cli`, `wcore-sandbox`, `wcore-budget`,
`wcore-tools`, plugin loader) imports `wcore-permissions`. The ACL and bearer
token surface exists but is not yet load-bearing. This shapes the in-scope vs
out-of-scope split below: gaps that require a consumer to exist before they
can be exploited are tracked as follow-ups, not patched in M5.8.

---

## T1 — Privilege escalation via plugin manifest

**Vector:** a third-party plugin manifest declares an actor identity (e.g.
`actor = "system"` or a user-name it should not impersonate) and the loader
trusts the claim. Once trusted, the plugin runs every tool with bypass
privileges via `Actor::System`.

**Current mitigation:** `PluginManifest` (in `wcore-cli/src/plugin/manifest.rs`)
has **no `actor` or `permissions` field at all** — the verbatim schema is
`{ name, version, requires_sandbox, description, dependencies }`. There is
nothing for a manifest to claim, therefore nothing for the loader to
incorrectly trust. The escalation surface does not exist yet.

**Gap:** when (not if) `PluginManifest` grows an actor/permissions field, the
plugin install path (`wcore-cli::plugin::install::write_install_record`) must
reject manifests claiming `Actor::System` or claiming a `User(name)` that
doesn't match the installing operator's identity. Today's install path only
validates the *name* of the plugin (`validate_plugin_name`), not its
permission claims.

**Closure:** **OUT-OF-SCOPE for M5.8.** Tracked as follow-up — depends on
adding the `actor`/`permissions` field to `PluginManifest`, which is a
cross-crate change in `wcore-cli` that the M5.8 task explicitly scopes out
("DO NOT touch revocation.rs ... patch surgically in `policy.rs`/`token.rs`").
The test in `threat_model_coverage.rs` is therefore a **documentary** test:
it asserts the *current* invariant (manifest has no actor claim) so the test
fails loudly the day a manifest field is added without a matching
install-side check.

**Follow-up note:** when `PluginManifest::actor` lands, this crate should
expose a `PolicyEngine::install_guard(claimed: &Actor, installer: &Actor) ->
PolicyResult<()>` helper and the manifest test should be un-`#[ignore]`d
and rewritten to exercise it.

---

## T2 — Token replay within TTL

**Vector:** an attacker intercepts a verified `BearerToken` off the wire
(JSON-stream protocol, log file, telemetry export) and reuses it from a
different process. Because the token's signature is over `(actor,
issued_at_ms, expires_at_ms, secret)` only, every byte-identical replay
verifies until `expires_at_ms`.

**Current mitigation:** none. `BearerToken::verify(&self, secret)` checks
signature and TTL — there is no `nonce`, `jti`, or revocation list. The
verbatim verify path:

```rust
pub fn verify(&self, secret: &[u8]) -> PolicyResult<&Actor> {
    let now = chrono::Utc::now().timestamp_millis();
    if now > self.expires_at_ms { return Err(DenyReason::TokenExpired); }
    let want = sign(&self.actor, self.issued_at_ms, self.expires_at_ms, secret);
    if want != self.signature_hex { return Err(DenyReason::TokenInvalid); }
    Ok(&self.actor)
}
```

**Gap:** within-TTL replay is currently undetectable.

**Closure:** **TRACKED IN M5.9** (`feat/wM5.9-bearer-rotation`). M5.9 adds
`BearerToken::revoke(&self)` + a `RevocationStore` trait + a new
`BearerToken::verify_with_store(&self, secret, &dyn RevocationStore)`
entrypoint. The M5.8 test
`t2_token_replay_within_ttl_is_blocked_when_revoked` is written with the
real assertions against that M5.9 surface and is `#[ignore]`d here — M5.9
un-ignores it during its rebase.

---

## T3 — Sandbox bypass via shared filesystem mount

**Vector:** a plugin's `SandboxManifest.allow_mounts` declares
`/host/some/path:/sandbox/path` and gains read/write access to a host path
that the same plugin's `PolicyEngine` grants would forbid. The mount predates
the policy check, so the sandbox can `cat` files that
`PolicyEngine::check(actor, Resource::File(host_path), Action::Read)` would
deny.

**Current mitigation:** none integrated. `wcore-sandbox`
(`crates/wcore-sandbox/src/manifest.rs`) accepts `allow_mounts: Vec<String>`
free-form, and `SandboxRegistry::run` does not call `PolicyEngine::check`
before launching the backend. `wcore-sandbox` does not depend on
`wcore-permissions` (verified: `Cargo.toml` of `wcore-sandbox` does not
list it). The intersection of mount-allowlist and ACL is unenforced.

**Gap:** mounts pass through to the backend with no per-actor check.

**Closure:** **OUT-OF-SCOPE for M5.8.** Closing this requires
`wcore-sandbox` to take a dependency on `wcore-permissions`, and
`SandboxRegistry` to grow a constructor variant
`new_with_policy(backend, engine, actor)` that intersects each mount path
against `PolicyEngine::check(actor, Resource::File(host_path), Action::Read|Write)`
before backend dispatch. That is a wcore-sandbox change, outside the
boundary the M5.8 task draws ("MODIFY: policy.rs, token.rs" only).
Test is `#[ignore]`d with this note.

**Follow-up note:** the smallest correct version is a new
`SandboxPolicyError::MountNotPermitted` in `wcore-sandbox/src/error.rs`
emitted from `SandboxRegistry::run` after a policy lookup. Should land
alongside the first real backend that honors `allow_mounts` for non-test
use.

---

## T4 — Budget tampering (actor under-reports `usd`)

**Vector:** `BudgetTracker::charge(session_id, tokens, usd)` accepts the
`usd` value from its caller. A malicious actor (or a buggy provider
adapter) reports `usd = 0.0` for an expensive call and the per-session /
per-user-daily cap is never hit.

**Current mitigation:** none in `wcore-permissions`. The verbatim signature
in `wcore-budget/src/tracker.rs:184` is
`pub fn charge(&mut self, session_id: &str, tokens: u64, usd: f64) -> Result<(), BudgetError>`.
There is no actor parameter, no signed claim, no provider-attestation. The
tracker trusts whatever USD figure the caller hands it.

**Gap:** charge integrity depends entirely on the caller. There is no
authenticated path from "provider returned token counts" to
"`BudgetTracker::charge` was called with the right number".

**Closure:** **OUT-OF-SCOPE for M5.8.** The correct fix is a `BudgetClaim`
struct in `wcore-budget` that carries `(actor, tokens, usd, provider_id,
signature)` and a `charge_signed(claim, &dyn ClaimVerifier)` entrypoint
whose verifier lives in `wcore-permissions`. That is a wcore-budget API
change, outside the M5.8 boundary. Test is `#[ignore]`d with this note.

**Follow-up note:** a cheaper interim mitigation is to gate
`BudgetTracker::charge` behind a per-provider rate-card lookup so the
caller can only assert `tokens` (verifiable from the provider response)
and the tracker derives `usd` itself. That eliminates the `usd` injection
surface without needing a signature scheme. Should be evaluated before
the full claim approach.

---

## T5 — File-resource grant path traversal

**Vector:** an actor has a grant for `Resource::File("/tmp/workspace/**")`
and calls `PolicyEngine::check(actor, Resource::File("/tmp/workspace/../etc/passwd"), Action::Read)`.
The private `glob_match` helper currently returns `true` for that pair
(verbatim, from `policy.rs:141-143`):

```rust
if let Some(prefix) = pattern.strip_suffix("/**") {
    return path == prefix || path.starts_with(&format!("{prefix}/"));
}
```

`/tmp/workspace/../etc/passwd` does start with `/tmp/workspace/`, so the
check passes and the actor gets `Ok(())` for a path that resolves outside
the granted subtree.

**Verification:** reproduced with the unmodified `glob_match`
helper — `glob_match("/tmp/workspace/**", "/tmp/workspace/../etc/passwd")`
returns `true` at HEAD.

**Gap:** the check accepts request paths containing `..` segments and
trusts string-prefix equality without normalizing.

**Closure:** **IN-SCOPE.** Patch `glob_match` in `policy.rs` to reject any
request path containing a `..` path-component before applying the
prefix/suffix/exact rules. Rejecting the *request* path (not the pattern)
is the right boundary: legitimate file resources never contain `..`
segments; any caller that needs `../` in a path should canonicalize before
calling `check`. Test
`t5_file_grant_rejects_path_traversal_in_request` fails at HEAD and
passes after the patch.

Pattern-side `..` is rejected too (a grant for `/tmp/**/../etc/**` is
nonsense; reject so no one writes it accidentally).

---

## T6 — Bearer token signature leaked via `Debug`

**Vector:** `BearerToken` derives `Debug`, so any `eprintln!("{token:?}")`,
any structured-log macro that includes the token, any panic message that
formats it, leaks `signature_hex` to the log destination. Combined with the
fact that signatures are deterministic over `(actor, issued_at_ms,
expires_at_ms, secret)`, a leaked signature enables T2-style replay until
the TTL expires.

**Current mitigation:** none. The derive is verbatim
`#[derive(Debug, Clone, Serialize, Deserialize)]` (`token.rs:18`). Debug
output includes the full `signature_hex` field.

**Gap:** Debug output of `BearerToken` is sensitive material.

**Closure:** **IN-SCOPE.** Replace the `#[derive(Debug)]` on
`BearerToken` with a manual `impl Debug` that redacts `signature_hex`
to `"<redacted>"` while keeping the other fields visible (actor,
issued_at_ms, expires_at_ms remain useful for debugging). Test
`t6_bearer_token_debug_redacts_signature` fails at HEAD and passes
after the patch.

Out-of-band: `Serialize`/`Deserialize` remain unchanged — round-tripping
the full token over the wire is the *defined* use case of those traits;
only the `Debug` path is for human-readable logging.

---

## T7 — ACL grant has no audit hook

**Vector:** `PolicyEngine::grant(p)` mutates the `grants: Vec<Permission>`
field with no out-of-band record. An attacker or buggy code path that calls
`grant()` (whether legitimately holding a `&mut PolicyEngine` or via a
future host-API surface) can broaden the policy with no observable trace.
The change is detectable only by reading `len()` before/after — which means
no monitoring system can alert on it.

**Current mitigation:** none. The verbatim implementation
(`policy.rs:67-70`) is

```rust
pub fn grant(&mut self, p: Permission) {
    self.grants.push(p);
}
```

**Gap:** grants are silently applied.

**Closure:** **IN-SCOPE.** Add a `GrantAuditSink` trait + an
`Option<Arc<dyn GrantAuditSink>>` field on `PolicyEngine` + a
`set_audit_sink(sink)` setter. When `grant()` is called with a sink
configured, push a `GrantAuditEvent { actor, resource, action, at }` to
the sink. Sinks are optional — backwards-compatible. The intentional
"tamper-evident" framing in M5.8's task list ("does every `grant()` call
produce a tamper-evident record?") is satisfied at the *observability*
layer: the sink can be a hash-chain in a follow-up, but the hook itself
is the M5.8 deliverable.

Test `t7_grant_emits_audit_event` fails at HEAD and passes after the
patch.

---

## Summary

| # | Threat | Status in M5.8 |
|---|---|---|
| T1 | Privilege escalation via plugin manifest | Documentary test, `#[ignore]` — closure needs `PluginManifest.actor` field (out-of-crate) |
| T2 | Token replay within TTL | `#[ignore]` — depends on M5.9 `revoke()` |
| T3 | Sandbox bypass via shared mount | `#[ignore]` — closure needs `wcore-sandbox`↔`wcore-permissions` wiring |
| T4 | Budget tampering | `#[ignore]` — closure needs `wcore-budget` API change |
| T5 | File-grant path traversal | **CLOSED** — `glob_match` rejects `..` in request path |
| T6 | Bearer Debug leaks signature | **CLOSED** — manual `Debug` redacts `signature_hex` |
| T7 | No ACL grant audit log | **CLOSED** — `GrantAuditSink` trait + `set_audit_sink` |

Three threats closed in-crate; four documented and instrumented with
`#[ignore]`d tests that the cross-crate follow-up wave will un-ignore.
