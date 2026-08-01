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
