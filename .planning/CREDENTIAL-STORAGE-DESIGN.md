# Credential storage — verified design

**Owner's stance:** no plaintext fallback, ever. Windows has a working secret
store; use it correctly rather than degrade. Get a working solution.

Every claim below is either a primary source or measured on our own hosts.

---

## 1. What is actually wrong today

Three stores exist. `EncryptedFileCredentialsStore` (`credentials.rs:927`) is a
real, already-shipping secure tier — the **confidential** path chains
keyring → encrypted vault and never touches plaintext.

The **non-confidential Auto** path does not use it:

```rust
// FallbackCredentialsStore::put
match self.keyring.put(key, value) {
    Ok(()) => Ok(()),
    // "Keyring became unavailable mid-session -- persist to plaintext so
    //  the write is not silently lost."
    Err(_) => self.plaintext.put(key, value),
}
```

It skips the secure tier that already exists and silently writes cleartext.
That is indefensible, and it is not a hard problem — the tier is right there.

**Reachability is proven, not theoretical.** CI hit `CredWrite` failing with
Windows error 8 (`ERROR_NOT_ENOUGH_MEMORY` — the credential store was FULL,
because we leak one credential per profile and never delete). On such a host
this branch fires and an API key lands in `credentials.toml`.

---

## 2. Verified prior art

| tool | Windows behaviour | silent plaintext? |
|---|---|---|
| **git-credential-manager** | Credential Manager (wincred). Plaintext exists but must be SELECTED via `credential.credentialStore=plaintext` / `GCM_CREDENTIAL_STORE`. With no store configured it **fails**: `fatal: No credential backing store has been selected`. Docs: *"Never use plaintext storage except in environments where no other secure option is available."* | **No — fails closed** |
| **Docker CLI** | `docker-credential-wincred`; without a helper writes base64 (not encrypted) to `config.json` and prints `WARNING! Your password will be stored unencrypted` | No — loud |
| **VS Code / Electron `safeStorage`** | DPAPI **user** scope; Linux libsecret, degrading to `basic_text` obfuscation *and telling the user* | No — announced |
| **GitHub CLI** | plaintext `hosts.yml` by default; keyring is opt-in (`--secure-storage`) | Plaintext is the DEFAULT — widely criticised |
| **AWS CLI / npm / kubectl** | plaintext `~/.aws/credentials`, `.npmrc`, kubeconfig | Yes — and an entire third-party ecosystem (`aws-vault`, `Granted`) exists to fix it |
| **cargo** | plaintext `credentials.toml` default; `cargo-credential-wincred` / `-macos-keychain` providers must be configured | Plaintext default |

**Peer agents in `resources/` (same class of tool as us):**
- `gemini-cli` — depends on `@github/keytar`, and writes OAuth creds with
  `fs.writeFile(..., { mode: 0o600 })` plus an explicit `fs.chmod(0o600)`.
- `kimi-code` — models storage as an explicit **choice**:
  `readonly storage: 'file' | 'keyring'`. Its file store is deliberate, not a
  silent fallback: dir `0o700`, atomic temp `openSync(tmp,'w',0o600)`, then
  `chmod` again *"in case umask stripped bits during open"*.

**The norm among tools that are NOT criticised: never downgrade silently.**
Either fail closed (GCM) or make the tier an explicit choice (kimi-code).
The tools that silently ship plaintext are precisely the ones with a cottage
industry of third-party fixes.

---

## 3. DPAPI machine scope is disqualified

Considered and rejected as an automatic tier for the profile-less case.

Microsoft's own `CryptProtectData` documentation:

> If the `CRYPTPROTECT_LOCAL_MACHINE` flag is set when the data is encrypted,
> **any user on the computer** where the encryption was done can decrypt the data.

The machine master key lives under `%WINDIR%\System32\Microsoft\Protect\S-1-5-18`
and is reachable through the LSA by any local process. So machine-scope DPAPI
protects against *offline disk theft* and nothing else. It is obfuscation, not
confidentiality, and must never be presented as the latter. (Chrome's 2024
app-bound encryption exists precisely because "any local process can decrypt"
was being harvested by infostealers.)

DPAPI **user** scope is fine — but it needs a loaded profile, which is exactly
what the failing case lacks, so it does not solve our problem either.

---

## 4. The design

Ordered policy, identical shape on every platform:

1. **OS keyring** — Credential Manager / Keychain / Secret Service.
   Selected only when a **write probe** succeeds (landed: `set_password` →
   `delete_credential`). A read probe says nothing about writability.
2. **Encrypted file vault** — the existing `EncryptedFileCredentialsStore`.
   This is the headless/CI/container/no-D-Bus answer and it is a *secure* tier,
   not a downgrade. Directory `0700`, file `0600`, atomic write, re-`chmod`
   after open (kimi-code's umask lesson). Refuse to load a world-readable vault.
3. **Fail closed** — with an actionable error naming the two ways forward
   (supply the vault passphrase, or opt in explicitly). This is GCM's behaviour
   and it is the industry norm.

**Plaintext is never automatic.** It requires explicit opt-in
(`CredentialsBackend::Plaintext`, already expressible) plus a loud, repeated
warning. It is never reached by a fallback edge.

**No DPAPI machine scope** anywhere in the chain.

### Required supporting fixes

- **P3 — stop leaking credentials.** One stable target name per logical key;
  overwrite in place; delete on profile removal. This is what filled the store
  and produced error 8. Without it the keyring tier degrades again over time.
- **Re-migration.** When the keyring becomes usable again, move the secret back
  up and delete the lower-tier copy. A downgrade must not be permanent.
- **Mid-session revocation.** A write failure after the probe passed must be
  handled by the same ladder, not by a special plaintext branch. Treat the
  actual write as authoritative; the probe is a hint, and probe→write is a
  TOCTOU window by construction.
- **No plaintext temp files.** Never write cleartext and delete later; a crash
  leaves the secret on disk.

---

## 5. Rejected alternatives, and why

| option | why not |
|---|---|
| Keep the plaintext fallback, warn loudly | Docker's model. Still writes the secret in cleartext; a warning is not a control. Owner's stance rules it out and the evidence supports him. |
| Fail the write, no vault tier | Loses a secure tier we already ship. Strands every legitimate headless/CI/container user for no security gain. |
| DPAPI `CRYPTPROTECT_LOCAL_MACHINE` | Any user on the machine can decrypt (MS docs). Obfuscation sold as encryption. |
| Run the CI runner as a real user so Credential Manager works | Hides the defect instead of fixing it. A profile-less service account is a legitimate deployment target; if CI stops reproducing it we ship broken to those hosts. Keep the hostile account. |

---

## 6. Sources

- CryptProtectData — https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata
- GCM credential stores — https://github.com/git-ecosystem/git-credential-manager/blob/main/docs/credstores.md
- GCM configuration — https://github.com/git-ecosystem/git-credential-manager/blob/main/docs/configuration.md
- `resources/kimi-code/packages/oauth/src/storage.ts` (0700/0600, atomic, re-chmod)
- `resources/gemini-cli/packages/core/src/code_assist/oauth2.ts:765-767` (0600 + chmod)

---

## 7. Scope — every credential sink, with a disposition

§1–§5 govern exactly ONE edge: `FallbackCredentialsStore::put`. That is not the
whole attack surface, and a design that fixes one sink while leaving the paths a
typical user actually takes untouched is worse than no design — it converts an
open problem into a closed-looking one.

This section is the inventory. **An omission here is indistinguishable from
coverage, so every sink is listed even when the disposition is "not fixed".**
Dispositions are exactly three:

- **governed** — writes go through the credential ladder; cleartext is
  unreachable without an explicit `CredentialsBackend::Plaintext`.
- **accepted-plaintext** — writes cleartext by design, because nothing can read
  it from anywhere else. Requires an explicit user action and a loud statement
  at the point of the write.
- **deferred** — a real cleartext sink, not fixed here, with the reason and the
  follow-up named.

| # | sink | who writes it | disposition | where |
|---|------|---------------|-------------|-------|
| 1 | `credentials.toml` `[secrets]` via `Auto` | `open_store` → ladder | **governed** | `credentials.rs` `LadderCredentialsStore` |
| 2 | `credentials.toml` `[secrets]` via `backend = "plaintext"` | operator opt-in | **accepted-plaintext** | `credentials.rs` `warn_explicit_plaintext_backend` |
| 3 | `config.toml` `[providers.<slug>].api_key` | `auth add` | **governed** | `auth.rs` `add_cmd` |
| 4 | `~/.wayland/.env` — provider keys | TUI credentials modal | **governed** | `tui/surfaces/config.rs` `save` |
| 5 | `~/.wayland/.env` — tool keys | TUI credentials modal | **accepted-plaintext** | same |
| 6 | `~/.wayland/oauth/*.json` | OAuth login | **deferred** | `wcore-agent/src/oauth/storage.rs` |
| 7 | `config.toml` `[providers].api_key` | `migrate --include-credentials` | **deferred** | `wcore-cli/src/migrate/hermes.rs:252` |
| 8 | OS keychain, ACP auth | `wcore-config::keychain` | **deferred** (already fail-closed, but no vault tier and it leaks) | `keychain.rs`, `wcore-acp/src/auth.rs` |
| 9 | `[bedrock]` / `[vertex]` secrets in `config.toml` | hand-edited | **deferred** | `config.rs:93`, `config.rs:123` |

### Per-sink detail

**1–2 — the credentials store.** The subject of §1–§5. `Auto` is now
keyring → encrypted vault → refuse. The legacy cleartext file is mounted
READ-ONLY so an existing install keeps resolving its keys; there is no edge on
which a `put` reaches it. `backend = "plaintext"` still works and warns.

**3 — `auth add`, and it was the main path.** `add_cmd` wrote
`[providers.<slug>].api_key` into `config.toml` in cleartext. Worse, that key
**outranks the credentials store** in `resolve_api_key` (cli → config → store →
env, `config.rs`), so even a correct store write was shadowed by it. Now: the key
goes to the ladder, the write is read back before success is reported, and a
pre-existing cleartext copy is STRIPPED (otherwise it would shadow the new one).
`auth list` reports WHERE each key lives and names the cleartext ones; `auth
remove` clears both locations.

> **PRECEDENCE IS NOT CHANGED, and that is a reported risk, not an oversight.**
> `[providers.<slug>].api_key` still outranks the store for a key nobody
> re-adds. Inverting the order would silently change WHICH key an existing user
> authenticates with — a config value they can see losing to a store value they
> cannot — and that is not a change to make in the same commit as the storage
> rework. `auth add` migrating the key off cleartext is the safe half; the
> precedence flip needs its own change and its own migration note.

**4–5 — `~/.wayland/.env`.** The TUI credentials modal was the primary
INTERACTIVE way a user hands us a key, and it wrote cleartext to `.env`.
`config.rs` also documents that `resolve_api_key` never reads `.env`, so the key
was invisible until a restart. Provider keys (those with a
`credentials_store_key` slot) now go to the ladder and apply on the next rebind.
Tool keys (`TAVILY_API_KEY`, `BRAVE_SEARCH_API_KEY`, `ELEVENLABS_API_KEY`, …)
still go to `.env` because **nothing reads a tool key from the credentials
store** — routing them there would make them unreadable, which is a worse
outcome than the cleartext they have today. That is accepted-plaintext, and the
status line now says `Saved UNENCRYPTED to ~/.wayland/.env (0600)` rather than
implying a secure store. The file's own hygiene is good (parent forced 0700,
0600 before and after an atomic rename, control characters rejected, name-only
logging) — it is the *choice of destination* that was wrong, not the writing.

**6 — OAuth tokens.** `~/.wayland/oauth/{provider}.json`, 0700 dir / 0600 file,
atomic. A refresh token is a credential and it is at rest in cleartext. NOT
fixed here: the OAuth store has its own lifecycle (refresh-on-expiry, per-
provider files, an import path from the Codex CLI), and routing it through a
key/value credential store is a redesign rather than a redirect. Follow-up.

**7 — `migrate --include-credentials`.** Lifts a provider key out of a foreign
tool's `.env` and writes it into the migrated profile's `config.toml` as
`[providers.<slug>].api_key` — sink 3, reached from a different direction. NOT
fixed here only because the migrate planner is a preview/apply pipeline whose
plan is rendered and diffed before it runs; making one step of it write to the
ladder without also teaching the preview about the ladder would produce a plan
that does not describe what happens. Follow-up, and it should reuse `auth add`'s
path when it lands.

**8 — the ACP keychain surface.** `wcore_config::keychain` is keyring-only:
`store_secret`/`get_secret`/`delete_secret` with no plaintext fallback, so it is
already fail-closed and is NOT a cleartext sink. Two defects remain, both
recorded rather than fixed: (a) it has no vault tier, so on a keyring-less host
ACP auth is simply unavailable with an unactionable message, where the ladder
would give it the vault; (b) it writes under its own `SERVICE_PREFIX` and nothing
deletes those entries on profile removal — **the same P3 leak shape** closed for
the confidential blob key, in a second place. `wcore-acp/src/auth.rs:10-12`
additionally claims "the keychain has fallback env-var lookup behavior in
`wcore-config::keychain`"; it does not — the comment is stale and describes a
behaviour that is not in the code.

**9 — hand-edited cloud secrets.** `[bedrock].secret_access_key`,
`[vertex].service_account_json` and friends are read from `config.toml` and have
no store slot at all. Nothing in the product WRITES them — they are placed by
hand — so there is no write path to govern; the exposure is that they are read
from, and rendered from, a cleartext file. Partially addressed here: they were
also being rendered in CLEARTEXT by the effective-config preview, because
`is_secret_key`'s denylist matched neither `service_account_json` nor
`access_key_id` while both structs' hand-written `Debug` impls DID redact them.
The needles are added. **The denylist itself is the defect** — inverting it to an
allowlist of renderable keys is the structural fix and is deliberately deferred:
built in this change it would silently mask ordinary fields and trade a leak for
an unreadable preview.

### Residual, stated plainly

- Sinks 6, 7, 8 and 9 are open. Three of them can still put a credential in
  cleartext on disk (6, 7, 9-by-hand).
- Config-over-store precedence (sink 3) is unchanged.
- `is_secret_key` remains a denylist.
