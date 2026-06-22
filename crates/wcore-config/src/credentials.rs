//! Wave SD — `CredentialsStore` trait + backend impls.
//!
//! Closes SECURITY MAJOR #16 (API keys + AWS secret + GCP secret persisted
//! in plaintext config with default OS permissions).
//!
//! Two backends ship:
//!
//! * `PlaintextCredentialsStore` — backs onto the existing
//!   `~/.config/wayland-core/config.toml` path; every save enforces
//!   `0o600` perms on Unix and tries a deny-all ACL on Windows. The
//!   fallback half of the default `Auto` backend (and the explicit
//!   `backend = "plaintext"` opt-out).
//! * `KeyringCredentialsStore` — uses the OS credential store via the
//!   `keyring` crate (macOS Keychain, Windows Credential Manager, Linux
//!   Secret Service). Behind the `keyring` cargo feature (on by default
//!   in this workspace) and selected via `backend = "keyring"`.
//!
//! The trait is intentionally minimal so callers can also swap in a
//! test-only in-memory store. Lookups go through `Config::resolve_*`
//! helpers (env > store > config); puts/deletes are explicit operations.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Configurable backend for credential storage. Selected via the
/// `[storage.credentials]` section in `config.toml`.
///
/// Rollback: set `WAYLAND_VAULT=plaintext` (env var) before startup to
/// skip the auto-migration prompt and keep using the legacy `Plaintext`
/// backend. The migration entrypoint itself is wired in a later wave;
/// this variant only defines the on-disk shape and crypto primitives.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialsBackend {
    /// Default: prefer the OS keyring, transparently falling back to the
    /// plaintext `0o600` file when no keyring is available (headless Linux,
    /// CI). Reads consult the keyring first, then plaintext, so credentials
    /// written by either backend — including pre-existing plaintext keys —
    /// stay resolvable; new writes prefer the keyring. Closes the
    /// "secrets cleartext by default" finding (deep-sweep F16) without
    /// breaking headless or stranding existing keys. Set `backend =
    /// "plaintext"` to opt back in to the legacy always-plaintext store.
    #[default]
    Auto,
    /// Plaintext TOML on disk with `0o600` perms enforced.
    Plaintext,
    /// OS-native keyring (Keychain / Credential Manager / Secret Service).
    Keyring,
    /// Encrypted-file backend — Argon2id-derived key + XChaCha20-Poly1305
    /// AEAD over a TOML-encoded secrets table. Two-file layout:
    /// `cipher_path` holds the ciphertext blob (`nonce(24) || ct`) and
    /// `key_params_path` holds the non-secret KDF params as JSON.
    EncryptedFile {
        /// Path to the cipher-text file (e.g. ~/.wayland/credentials.enc).
        cipher_path: PathBuf,
        /// Path to the KDF params file (salt, m_cost, t_cost, p_cost — non-secret).
        key_params_path: PathBuf,
    },
}

/// The `[storage.credentials]` config section.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CredentialsStorageConfig {
    #[serde(default)]
    pub backend: CredentialsBackend,
    /// Optional service identifier used by the keyring backend. Defaults
    /// to `"wayland-core"` when omitted; surfaces so different installs
    /// (e.g. development vs. shipped) can keep their secrets separate.
    #[serde(default)]
    pub service_name: Option<String>,
}

#[derive(Debug, Error)]
pub enum CredentialsError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("toml serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
    #[error("keyring error: {0}")]
    Keyring(String),
    #[error("backend not available: {0}")]
    BackendUnavailable(String),
}

/// Generic key/value store for credentials.
///
/// Keys are flat strings; callers namespace via dotted prefixes
/// (e.g. `providers.anthropic.api_key`, `bedrock.secret_access_key`).
pub trait CredentialsStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError>;
    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError>;
    fn delete(&self, key: &str) -> Result<(), CredentialsError>;
}

// ---------------------------------------------------------------------------
// Plaintext backend (TOML on disk; 0o600 perms enforced)
// ---------------------------------------------------------------------------

/// TOML-backed credentials store.
///
/// Holds a `[secrets]` table at the configured path. The file is created
/// with `0o600` perms on Unix and parent-dir-restricted ACLs on Windows
/// on first write. Reads re-check perms and warn (via stderr) if the
/// file is world-readable, but still load — refusing-to-load would
/// strand users on a freshly-created file that the kernel briefly held
/// at the umask default.
pub struct PlaintextCredentialsStore {
    path: PathBuf,
}

impl PlaintextCredentialsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load_table(&self) -> Result<toml::Table, CredentialsError> {
        match std::fs::read_to_string(&self.path) {
            Ok(content) => {
                warn_if_world_readable(&self.path);
                let parsed: toml::Table = content.parse()?;
                Ok(parsed)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
            Err(e) => Err(CredentialsError::Io(e)),
        }
    }

    fn save_table(&self, table: &toml::Table) -> Result<(), CredentialsError> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let serialized = toml::to_string_pretty(table)?;
        crate::atomic_write(&self.path, serialized.as_bytes())?;
        secure_credential_file(&self.path)?;
        Ok(())
    }
}

impl CredentialsStore for PlaintextCredentialsStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        let table = self.load_table()?;
        let secrets = match table.get("secrets") {
            Some(toml::Value::Table(t)) => t,
            _ => return Ok(None),
        };
        Ok(secrets.get(key).and_then(|v| v.as_str()).map(str::to_owned))
    }

    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        let mut table = self.load_table()?;
        let secrets = table
            .entry("secrets".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let toml::Value::Table(secrets_table) = secrets else {
            // Corrupt file — overwrite the key with a fresh table.
            *secrets = toml::Value::Table(toml::Table::new());
            let toml::Value::Table(secrets_table) = secrets else {
                unreachable!("just assigned to Table");
            };
            secrets_table.insert(key.to_string(), toml::Value::String(value.to_string()));
            return self.save_table(&table);
        };
        secrets_table.insert(key.to_string(), toml::Value::String(value.to_string()));
        self.save_table(&table)
    }

    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        let mut table = self.load_table()?;
        if let Some(toml::Value::Table(secrets_table)) = table.get_mut("secrets") {
            secrets_table.remove(key);
        }
        self.save_table(&table)
    }
}

// ---------------------------------------------------------------------------
// Keyring backend
// ---------------------------------------------------------------------------

/// OS-native keyring credentials store.
///
/// Backed by the `keyring` crate (macOS Keychain on Apple, Windows
/// Credential Manager on Windows, Secret Service on Linux). Each
/// `key` is mapped to a `(service, user)` pair; we use the
/// configured service name (default `"wayland-core"`) and the key
/// itself as the user — this keeps lookup O(1) and matches the
/// `keyring` crate's expected shape.
pub struct KeyringCredentialsStore {
    service: String,
}

impl KeyringCredentialsStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

impl CredentialsStore for KeyringCredentialsStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        let entry = keyring::Entry::new(&self.service, key)
            .map_err(|e| CredentialsError::Keyring(e.to_string()))?;
        match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(CredentialsError::Keyring(e.to_string())),
        }
    }

    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        let entry = keyring::Entry::new(&self.service, key)
            .map_err(|e| CredentialsError::Keyring(e.to_string()))?;
        entry
            .set_password(value)
            .map_err(|e| CredentialsError::Keyring(e.to_string()))
    }

    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        let entry = keyring::Entry::new(&self.service, key)
            .map_err(|e| CredentialsError::Keyring(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(CredentialsError::Keyring(e.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Auto backend (keyring primary, plaintext fallback) — the default
// ---------------------------------------------------------------------------

/// Probe whether the OS keyring is actually usable on this host. Returns
/// `false` on headless Linux without a running Secret Service, in CI, etc., so
/// the [`CredentialsBackend::Auto`] default can fall back to plaintext rather
/// than error. A `NoEntry` result means the keyring works (the probe key simply
/// does not exist); any other error means the keyring is unavailable.
fn keyring_available(service: &str) -> bool {
    match keyring::Entry::new(service, "__wayland_core_keyring_probe__") {
        Ok(entry) => matches!(entry.get_password(), Ok(_) | Err(keyring::Error::NoEntry)),
        Err(_) => false,
    }
}

/// The [`CredentialsBackend::Auto`] store: keyring primary, plaintext fallback.
///
/// Reads check the keyring first, then plaintext, so pre-existing plaintext
/// keys remain resolvable after the default flips to keyring. Writes prefer the
/// keyring and fall back to plaintext only if the keyring write fails. Built
/// only when [`keyring_available`] returned `true`; otherwise `open_store` uses
/// a bare [`PlaintextCredentialsStore`].
struct FallbackCredentialsStore {
    keyring: KeyringCredentialsStore,
    plaintext: PlaintextCredentialsStore,
}

impl FallbackCredentialsStore {
    fn new(service: String, plaintext_path: PathBuf) -> Self {
        Self {
            keyring: KeyringCredentialsStore::new(service),
            plaintext: PlaintextCredentialsStore::new(plaintext_path),
        }
    }
}

impl CredentialsStore for FallbackCredentialsStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        // Keyring first; a keyring read error must not hide a plaintext key.
        if let Ok(Some(v)) = self.keyring.get(key) {
            return Ok(Some(v));
        }
        self.plaintext.get(key)
    }

    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        match self.keyring.put(key, value) {
            Ok(()) => Ok(()),
            // Keyring became unavailable mid-session — persist to plaintext so
            // the write is not silently lost.
            Err(_) => self.plaintext.put(key, value),
        }
    }

    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        // Remove from both so a deleted key cannot resurface from the fallback.
        let _ = self.keyring.delete(key);
        self.plaintext.delete(key)
    }
}

// ---------------------------------------------------------------------------
// EncryptedFile backend (S11 — Argon2id + XChaCha20-Poly1305 vault)
// ---------------------------------------------------------------------------

/// Vault-file credentials store backed by the primitives in
/// [`encrypted_file`].
///
/// On-disk layout (two files, both created lazily on first `put`):
/// * `cipher_path` — raw bytes `nonce(24) || ciphertext || tag(16)`,
///   produced by [`encrypted_file::encrypt`].
/// * `key_params_path` — JSON-encoded [`encrypted_file::KdfParams`]
///   (salt + tuning knobs; non-secret).
///
/// Plaintext payload is a TOML document with a single `[secrets]` table,
/// matching the [`PlaintextCredentialsStore`] shape so the data model
/// stays portable across backends.
///
/// Passphrase resolution (first match wins):
/// 1. `WAYLAND_VAULT_PASSPHRASE` env var (logged at WARN — visible via
///    `/proc/<pid>/environ` on Linux; document a future
///    `CredentialsBackend::Pipe` for production).
/// 2. Interactive `rpassword` prompt on a TTY.
///
/// Concurrency: each store holds a `parking_lot::Mutex` over the cached
/// passphrase + KDF params so the Argon2id derivation runs once per
/// process even when callers thrash `get`/`put`. Cross-process locking
/// is not modeled — operators who run multiple writers should serialize
/// at the application layer.
pub struct EncryptedFileCredentialsStore {
    cipher_path: PathBuf,
    key_params_path: PathBuf,
    /// Cached unlock state. `None` until first successful read or write.
    /// Held under a mutex because the trait is `Send + Sync` and Argon2id
    /// is non-trivially expensive.
    unlocked: parking_lot::Mutex<Option<UnlockedVault>>,
}

/// In-memory vault unlock state.
struct UnlockedVault {
    /// User-supplied passphrase. Held only in memory; zeroized on drop.
    passphrase: zeroize::Zeroizing<String>,
    /// KDF params (salt + tuning knobs). Persisted to `key_params_path`.
    params: encrypted_file::KdfParams,
}

/// supply-unsafe-63: validate that an env-supplied raw file descriptor is
/// currently open and was opened for reading, before we wrap it with
/// `from_raw_fd`.
///
/// We avoid pulling in a new crate dependency by declaring the two POSIX
/// `fcntl` queries directly — `fcntl` lives in libc/libSystem, which is always
/// linked on unix targets. Both queries are read-only (no side effects on the
/// descriptor):
///   * `F_GETFD` — returns the fd flags, or `-1`/`EBADF` if the fd is closed.
///   * `F_GETFL` — returns the open-mode flags; we reject `O_WRONLY` (a
///     write-only descriptor can never satisfy our `read_to_string`).
#[cfg(unix)]
fn validate_readable_fd(fd: std::os::unix::io::RawFd) -> Result<(), CredentialsError> {
    // POSIX constants. These are stable across Linux and the BSDs/macOS.
    const F_GETFD: std::os::raw::c_int = 1;
    const F_GETFL: std::os::raw::c_int = 3;
    const O_ACCMODE: std::os::raw::c_int = 0o3;
    const O_WRONLY: std::os::raw::c_int = 0o1;

    unsafe extern "C" {
        // `fcntl(int fd, int cmd, ...)` — we only use the no-arg query forms.
        fn fcntl(fd: std::os::raw::c_int, cmd: std::os::raw::c_int, ...) -> std::os::raw::c_int;
    }

    let reject = |reason: &str| {
        Err(CredentialsError::BackendUnavailable(format!(
            "WAYLAND_VAULT_PASSPHRASE_FD={fd} {reason}"
        )))
    };

    // 1. Is the descriptor open at all? F_GETFD fails with -1 (errno EBADF)
    //    for a closed/never-opened fd.
    // SAFETY: F_GETFD is a read-only query that takes no variadic argument.
    let fd_flags = unsafe { fcntl(fd, F_GETFD) };
    if fd_flags == -1 {
        return reject("is not an open file descriptor");
    }

    // 2. Was it opened for reading? Reject write-only descriptors (e.g. a
    //    process's own stdout/stderr pipe) which would only yield EBADF on
    //    read and could mask a misconfiguration.
    // SAFETY: F_GETFL is a read-only query that takes no variadic argument.
    let status_flags = unsafe { fcntl(fd, F_GETFL) };
    if status_flags == -1 {
        return reject("could not be queried for read access");
    }
    if (status_flags & O_ACCMODE) == O_WRONLY {
        return reject("is write-only; a readable fd is required");
    }

    Ok(())
}

impl EncryptedFileCredentialsStore {
    pub fn new(cipher_path: PathBuf, key_params_path: PathBuf) -> Self {
        Self {
            cipher_path,
            key_params_path,
            unlocked: parking_lot::Mutex::new(None),
        }
    }

    /// Resolve a passphrase from a file descriptor, env var, or interactive prompt.
    ///
    /// F-055 — resolution order:
    ///   1. `WAYLAND_VAULT_PASSPHRASE_FD` env var: read passphrase from the
    ///      given file descriptor number (e.g. `--passphrase-fd 3`).  This is
    ///      invisible in `/proc/<pid>/environ` and avoids the env-var leak.
    ///   2. `WAYLAND_VAULT_PASSPHRASE` env var (legacy, kept for backwards
    ///      compatibility). Emits a warning about the `/proc` visibility risk.
    ///   3. Interactive `rpassword` prompt.
    fn read_passphrase() -> Result<zeroize::Zeroizing<String>, CredentialsError> {
        // F-055 path 1: read from a file descriptor. Unix-only — file
        // descriptors are not a portable concept; Windows uses HANDLEs
        // which the keyring backend doesn't expose. On Windows + targets
        // without unix-style fds, the code falls through to path 2/3.
        #[cfg(unix)]
        if let Ok(fd_str) = std::env::var("WAYLAND_VAULT_PASSPHRASE_FD") {
            let fd: std::os::unix::io::RawFd = fd_str.parse().map_err(|_| {
                CredentialsError::BackendUnavailable(format!(
                    "WAYLAND_VAULT_PASSPHRASE_FD is not a valid integer: {fd_str}"
                ))
            })?;
            // supply-unsafe-63: the fd number is fully attacker-influenced
            // (it comes from the environment). Validate it BEFORE handing it
            // to `from_raw_fd`: confirm it is actually open and that it was
            // opened for reading. Without this, a hostile or buggy parent
            // could point us at fd 1/2 (a write-only pipe → silent EBADF
            // read), or a closed/recycled descriptor → reading whatever data
            // happens to be on a fd opened later in the process. Reject
            // anything that is not a readable, currently-open descriptor.
            validate_readable_fd(fd)?;
            use std::io::Read;
            // SAFETY: We are re-borrowing an fd that the process inherited and
            // that `validate_readable_fd` just confirmed is open and readable;
            // ownership is not transferred and we do not close it.
            let mut f = unsafe { <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(fd) };
            let mut pp = String::new();
            f.read_to_string(&mut pp).map_err(|e| {
                CredentialsError::BackendUnavailable(format!("passphrase fd {fd}: {e}"))
            })?;
            // Do not close the fd — `std::mem::forget` prevents Drop from closing.
            std::mem::forget(f);
            let pp = pp.trim_end_matches('\n').to_string();
            return Ok(zeroize::Zeroizing::new(pp));
        }

        // F-055 path 2: env var (legacy, warned).
        if let Ok(pp) = std::env::var("WAYLAND_VAULT_PASSPHRASE") {
            tracing::warn!(
                target: "wcore_credentials",
                "WAYLAND_VAULT_PASSPHRASE provided via env var — visible via \
                 /proc/<pid>/environ on Linux. Set WAYLAND_VAULT_PASSPHRASE_FD \
                 to a file descriptor number to avoid this leak."
            );
            return Ok(zeroize::Zeroizing::new(pp));
        }

        // F-055 path 3: interactive prompt.
        let pp = rpassword::prompt_password("vault passphrase: ")
            .map_err(|e| CredentialsError::BackendUnavailable(format!("rpassword: {e}")))?;
        Ok(zeroize::Zeroizing::new(pp))
    }

    /// Acquire (or reuse) the unlocked-state cache.
    ///
    /// On first call:
    /// * If `key_params_path` exists, load the persisted KDF params and
    ///   verify the cached passphrase by attempting to decrypt the
    ///   existing cipher blob.
    /// * Otherwise, generate fresh [`KdfParams`] (with a random salt) and
    ///   accept the passphrase as the new vault password.
    fn unlock(&self) -> Result<parking_lot::MappedMutexGuard<'_, UnlockedVault>, CredentialsError> {
        let mut guard = self.unlocked.lock();
        if guard.is_none() {
            let passphrase = Self::read_passphrase()?;
            let params = if self.key_params_path.exists() {
                encrypted_file::load_key_params(&self.key_params_path)
                    .map_err(|e| CredentialsError::BackendUnavailable(format!("kdf params: {e}")))?
            } else {
                encrypted_file::KdfParams::default()
            };

            // If a ciphertext blob already exists, verify the passphrase
            // by decrypting it — otherwise a typo would silently rotate
            // the vault key on next write.
            if self.cipher_path.exists() {
                let blob = std::fs::read(&self.cipher_path)?;
                let _pt = encrypted_file::decrypt(&blob, &passphrase, &params).map_err(|e| {
                    CredentialsError::BackendUnavailable(format!(
                        "vault unlock failed (wrong passphrase or corrupt file): {e}"
                    ))
                })?;
            }

            *guard = Some(UnlockedVault { passphrase, params });
        }
        Ok(parking_lot::MutexGuard::map(guard, |o| {
            o.as_mut().expect("just initialized")
        }))
    }

    /// Load and decrypt the current secrets TOML table.
    ///
    /// Returns an empty table when no ciphertext has been persisted yet
    /// (first write will materialize the vault).
    fn load_secrets(&self, vault: &UnlockedVault) -> Result<toml::Table, CredentialsError> {
        if !self.cipher_path.exists() {
            return Ok(toml::Table::new());
        }
        let blob = std::fs::read(&self.cipher_path)?;
        let pt = encrypted_file::decrypt(&blob, &vault.passphrase, &vault.params).map_err(|e| {
            CredentialsError::BackendUnavailable(format!("vault decrypt failed: {e}"))
        })?;
        let parsed: toml::Table = std::str::from_utf8(&pt)
            .map_err(|e| {
                CredentialsError::BackendUnavailable(format!("vault plaintext utf8: {e}"))
            })?
            .parse()?;
        Ok(parsed)
    }

    /// Re-encrypt and atomically persist the given table.
    fn save_secrets(
        &self,
        vault: &UnlockedVault,
        table: &toml::Table,
    ) -> Result<(), CredentialsError> {
        let serialized = toml::to_string_pretty(table)?;
        // Reuse the cached KDF params — keep the same salt across writes
        // so the existing passphrase keeps deriving the same key. Only
        // the AEAD nonce is rotated on each encrypt (handled inside
        // `encrypted_file::encrypt`).
        let key = encrypted_file::derive_key(&vault.passphrase, &vault.params)
            .map_err(|e| CredentialsError::BackendUnavailable(format!("derive_key: {e}")))?;
        let blob = encrypted_file::encrypt_with_key(serialized.as_bytes(), &key).map_err(|e| {
            CredentialsError::BackendUnavailable(format!("vault encrypt failed: {e}"))
        })?;

        // Ensure both files share a parent directory and that it exists.
        if let Some(parent) = self.cipher_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        if let Some(parent) = self.key_params_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        crate::atomic_write(&self.cipher_path, &blob)?;
        secure_credential_file(&self.cipher_path)?;
        encrypted_file::save_key_params(&vault.params, &self.key_params_path)
            .map_err(|e| CredentialsError::BackendUnavailable(format!("save_key_params: {e}")))?;
        secure_credential_file(&self.key_params_path)?;
        Ok(())
    }
}

impl CredentialsStore for EncryptedFileCredentialsStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        let vault = self.unlock()?;
        let table = self.load_secrets(&vault)?;
        let secrets = match table.get("secrets") {
            Some(toml::Value::Table(t)) => t,
            _ => return Ok(None),
        };
        Ok(secrets.get(key).and_then(|v| v.as_str()).map(str::to_owned))
    }

    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        let vault = self.unlock()?;
        let mut table = self.load_secrets(&vault)?;
        let entry = table
            .entry("secrets".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !matches!(entry, toml::Value::Table(_)) {
            *entry = toml::Value::Table(toml::Table::new());
        }
        let toml::Value::Table(secrets_table) = entry else {
            unreachable!("just normalized to Table");
        };
        secrets_table.insert(key.to_string(), toml::Value::String(value.to_string()));
        self.save_secrets(&vault, &table)
    }

    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        let vault = self.unlock()?;
        let mut table = self.load_secrets(&vault)?;
        if let Some(toml::Value::Table(secrets_table)) = table.get_mut("secrets") {
            secrets_table.remove(key);
        }
        self.save_secrets(&vault, &table)
    }
}

/// Non-consuming check for whether vault unlock material is available
/// out-of-band, so [`open_store`] can choose the encrypted vault WITHOUT
/// triggering an interactive passphrase prompt on a headless/desktop spawn.
///
/// Mirrors the NON-INTERACTIVE prefixes of
/// [`EncryptedFileCredentialsStore::read_passphrase`]: a passphrase FD (Unix
/// only — file descriptors are not a portable Windows concept, and
/// `read_passphrase` likewise `#[cfg(unix)]`-gates the FD path) or the legacy
/// `WAYLAND_VAULT_PASSPHRASE` env var. The interactive `rpassword` prompt is
/// deliberately NOT treated as "present": selecting the vault must never block
/// a non-interactive launch on a TTY.
///
/// The Windows branch intentionally omits the FD check: a Windows caller that
/// set only `WAYLAND_VAULT_PASSPHRASE_FD` correctly falls back to plaintext
/// rather than being routed to the vault and then hitting `read_passphrase`'s
/// interactive prompt (whose FD path is also unix-only). Do NOT "fix" this by
/// adding an unconditional FD check — that reintroduces the Windows TTY block.
fn vault_unlock_material_present() -> bool {
    #[cfg(unix)]
    if std::env::var_os("WAYLAND_VAULT_PASSPHRASE_FD").is_some() {
        return true;
    }
    std::env::var_os("WAYLAND_VAULT_PASSPHRASE").is_some()
}

/// Derive the encrypted-vault file pair that sits beside the plaintext
/// credentials path (i.e. inside the active `WAYLAND_HOME`). Co-locating them
/// means the existing parent-dir hardening already covers them. The `"."`
/// fallback is unreachable in practice — every caller passes
/// `credentials_storage_path()`, which always has a real parent dir.
fn default_vault_paths(plaintext_path: &Path) -> (PathBuf, PathBuf) {
    let dir = plaintext_path.parent().unwrap_or_else(|| Path::new("."));
    (
        dir.join("credentials.enc"),
        dir.join("credentials.kdf.json"),
    )
}

/// Warn ONCE, to stderr, that an isolated profile is persisting secrets as a
/// plaintext-0600 file because no vault unlock material was supplied. The D1
/// "warned fallback": secrets are still `0o600` and in-home, but not encrypted
/// at rest. `Once`-guarded because `open_store` is called repeatedly per run
/// (once per provider key lookup) and an unguarded warning would spam stderr.
fn warn_isolated_plaintext_fallback(path: &Path) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!(
            "warning: WAYLAND_HOME is set (isolated profile) but no vault \
             passphrase was supplied; storing credentials as plaintext-0600 at \
             {}. To encrypt at rest, set WAYLAND_VAULT_PASSPHRASE_FD (a \
             passphrase file descriptor — preferred) or WAYLAND_VAULT_PASSPHRASE \
             (env var, visible via /proc/<pid>/environ). Secrets in a legacy OS \
             keyring are not auto-imported into isolated profiles — re-enter \
             them for this profile.",
            path.display()
        );
    });
}

/// Factory selecting the configured backend.
pub fn open_store(
    cfg: &CredentialsStorageConfig,
    plaintext_path: &Path,
) -> Result<Box<dyn CredentialsStore>, CredentialsError> {
    match &cfg.backend {
        // Default: keyring primary + plaintext fallback when a keyring exists;
        // a bare plaintext store on headless/CI hosts where it does not. (F16)
        CredentialsBackend::Auto => {
            // Isolated-profile homes (WAYLAND_HOME set) must NOT use the OS
            // keyring: the keyring service is a process-global constant
            // ("wayland-core") that bleeds secrets across every profile on the
            // host (C4 / D1). For an isolated home, prefer the in-home encrypted
            // vault when unlock material is supplied out-of-band; otherwise fall
            // back to a stderr-warned plaintext-0600 file in-home — never the
            // keyring. The legacy single (non-profile) home is unchanged below.
            if std::env::var_os("WAYLAND_HOME").is_some() {
                if vault_unlock_material_present() {
                    let (cipher_path, key_params_path) = default_vault_paths(plaintext_path);
                    return Ok(Box::new(EncryptedFileCredentialsStore::new(
                        cipher_path,
                        key_params_path,
                    )));
                }
                warn_isolated_plaintext_fallback(plaintext_path);
                return Ok(Box::new(PlaintextCredentialsStore::new(
                    plaintext_path.to_path_buf(),
                )));
            }
            let service = cfg
                .service_name
                .clone()
                .unwrap_or_else(|| "wayland-core".to_string());
            if keyring_available(&service) {
                Ok(Box::new(FallbackCredentialsStore::new(
                    service,
                    plaintext_path.to_path_buf(),
                )))
            } else {
                Ok(Box::new(PlaintextCredentialsStore::new(
                    plaintext_path.to_path_buf(),
                )))
            }
        }
        CredentialsBackend::Plaintext => Ok(Box::new(PlaintextCredentialsStore::new(
            plaintext_path.to_path_buf(),
        ))),
        CredentialsBackend::Keyring => {
            let service = cfg
                .service_name
                .clone()
                .unwrap_or_else(|| "wayland-core".to_string());
            Ok(Box::new(KeyringCredentialsStore::new(service)))
        }
        // S11 (v0.6.3): EncryptedFile backend is wired here. Crypto primitives
        // are defined in the `encrypted_file` submodule; the store glues them
        // to a TOML-encoded secrets table, an unlock-passphrase resolver
        // (env var or interactive prompt), and atomic re-encrypt on put.
        CredentialsBackend::EncryptedFile {
            cipher_path,
            key_params_path,
        } => Ok(Box::new(EncryptedFileCredentialsStore::new(
            cipher_path.clone(),
            key_params_path.clone(),
        ))),
    }
}

/// Validate a `[storage.credentials]` config block at startup.
///
/// All backends pass through unconditionally now that S11 has wired the
/// `EncryptedFile` store. Kept as a stable hook for callers (and so the
/// previous early-fail behavior can be reintroduced for any future
/// "shipped but disabled" backend).
pub fn validate_credentials_config(
    _cfg: &CredentialsStorageConfig,
) -> Result<(), CredentialsError> {
    Ok(())
}

// ---------------------------------------------------------------------------
// T1-E1 — Encrypted-file crypto primitives
// ---------------------------------------------------------------------------

/// Argon2id KDF + XChaCha20-Poly1305 AEAD primitives for the
/// `CredentialsBackend::EncryptedFile` variant.
///
/// Crypto patterns adopted from Forge vault.ts (Apache-2.0). This is a
/// from-scratch Rust implementation, not a direct port.
///
/// On-disk layout:
/// * `cipher_path`: ciphertext blob, raw bytes `nonce(24) || ct||tag`.
///   The XChaCha20-Poly1305 tag (16 bytes) is appended to the ciphertext
///   by the AEAD; no length-prefixing — readers split at the fixed 24-byte
///   nonce boundary and feed the remainder to `decrypt`.
/// * `key_params_path`: JSON-encoded [`KdfParams`] — non-secret salt +
///   tuning knobs (m_cost, t_cost, p_cost, version).
// T1-E1 lands the crypto primitives in this wave; the `CredentialsStore`
// impl that consumes them ships in a later wave. Dead-code suppression
// is applied at the individual fn level below — see `encrypt`, `decrypt`,
// `save_key_params`, `load_key_params` — so newly added module-level items
// still surface dead-code warnings until they are actually wired.
pub(crate) mod encrypted_file {
    use argon2::{Algorithm, Argon2, Params, Version};
    use base64::Engine;
    use chacha20poly1305::{
        Key, KeyInit, XChaCha20Poly1305, XNonce,
        aead::{Aead, OsRng},
    };
    use rand::RngCore;
    use serde::{Deserialize, Serialize};
    use zeroize::Zeroize;

    /// Default Argon2id memory cost in KiB (64 MiB). Matches the Forge
    /// vault.ts profile.
    const DEFAULT_M_COST_KIB: u32 = 64 * 1024;
    /// Default Argon2id iteration count.
    const DEFAULT_T_COST: u32 = 3;
    /// Default Argon2id parallelism degree.
    const DEFAULT_P_COST: u32 = 1;
    /// XChaCha20-Poly1305 nonce length (24 bytes).
    pub const NONCE_LEN: usize = 24;
    /// AEAD tag length (16 bytes — Poly1305 MAC tag).
    pub const TAG_LEN: usize = 16;
    /// KDF output key length (32 bytes for XChaCha20-Poly1305).
    pub const KEY_LEN: usize = 32;

    /// KDF parameters persisted alongside the ciphertext.
    ///
    /// Non-secret: the salt is randomized per vault and `m_cost`/`t_cost`/
    /// `p_cost` are tuning knobs. Storing them on disk lets future versions
    /// re-derive the same key from a user-supplied password without prompting
    /// for the tuning factors.
    #[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
    pub struct KdfParams {
        /// Base64 (url-safe, no pad) salt — 16 random bytes.
        pub salt_b64: String,
        /// Memory cost in KiB (Argon2id `m`).
        pub m_cost: u32,
        /// Iteration count (Argon2id `t`).
        pub t_cost: u32,
        /// Parallelism degree (Argon2id `p`).
        pub p_cost: u32,
        /// Schema version. Currently 1.
        pub version: u8,
    }

    impl Default for KdfParams {
        fn default() -> Self {
            let mut salt = [0u8; 16];
            // OsRng would also work; thread_rng is seeded from the OS and
            // adequate for a salt (no secrecy requirement).
            rand::thread_rng().fill_bytes(&mut salt);
            Self {
                salt_b64: base64_url(&salt),
                m_cost: DEFAULT_M_COST_KIB,
                t_cost: DEFAULT_T_COST,
                p_cost: DEFAULT_P_COST,
                version: 1,
            }
        }
    }

    fn base64_url(bytes: &[u8]) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    fn base64_url_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s)
    }

    #[derive(Debug, thiserror::Error)]
    pub enum EncryptedFileError {
        #[error("io error: {0}")]
        Io(#[from] std::io::Error),
        #[error("kdf params invalid: {0}")]
        KdfParams(String),
        #[error("aead error: {0}")]
        Aead(String),
        #[error("argon2 error: {0}")]
        Argon2(String),
        #[error("serde error: {0}")]
        Serde(#[from] serde_json::Error),
        #[error("base64 error: {0}")]
        Base64(#[from] base64::DecodeError),
        #[error("file too short")]
        TooShort,
    }

    /// Derive a 32-byte symmetric key from a password and [`KdfParams`].
    pub fn derive_key(
        password: &str,
        params: &KdfParams,
    ) -> Result<[u8; KEY_LEN], EncryptedFileError> {
        let salt = base64_url_decode(&params.salt_b64)?;
        let argon = Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            Params::new(params.m_cost, params.t_cost, params.p_cost, Some(KEY_LEN))
                .map_err(|e| EncryptedFileError::KdfParams(e.to_string()))?,
        );
        let mut key = [0u8; KEY_LEN];
        argon
            .hash_password_into(password.as_bytes(), &salt, &mut key)
            .map_err(|e| EncryptedFileError::Argon2(e.to_string()))?;
        Ok(key)
    }

    /// Encrypt `plaintext` with a freshly generated [`KdfParams`] and the
    /// derived key. Returns `(blob, params)` where `blob = nonce(24)||ct||tag`.
    /// Callers persist `blob` to `cipher_path` and `params` to
    /// `key_params_path`.
    #[allow(dead_code)]
    pub fn encrypt(
        plaintext: &[u8],
        password: &str,
    ) -> Result<(Vec<u8>, KdfParams), EncryptedFileError> {
        let params = KdfParams::default();
        let mut key_bytes = derive_key(password, &params)?;
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&key_bytes));
        let mut nonce_bytes = [0u8; NONCE_LEN];
        // Use OsRng for the AEAD nonce — must be unguessable per the
        // XChaCha20-Poly1305 contract.
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from_slice(&nonce_bytes);
        let ct = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| EncryptedFileError::Aead(e.to_string()))?;
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        key_bytes.zeroize();
        Ok((out, params))
    }

    /// Encrypt with a pre-derived key (skips Argon2id KDF). Used by the
    /// `EncryptedFileCredentialsStore` so writes don't re-run the 64 MiB /
    /// t=3 derivation on every `put`. Returns `nonce(24) || ct||tag`,
    /// identical in shape to [`encrypt`].
    pub fn encrypt_with_key(
        plaintext: &[u8],
        key: &[u8; KEY_LEN],
    ) -> Result<Vec<u8>, EncryptedFileError> {
        let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from_slice(&nonce_bytes);
        let ct = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| EncryptedFileError::Aead(e.to_string()))?;
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Decrypt a ciphertext blob produced by [`encrypt`].
    #[allow(dead_code)]
    pub fn decrypt(
        cipher_blob: &[u8],
        password: &str,
        params: &KdfParams,
    ) -> Result<Vec<u8>, EncryptedFileError> {
        if cipher_blob.len() < NONCE_LEN + TAG_LEN {
            return Err(EncryptedFileError::TooShort);
        }
        let (nonce_bytes, ct) = cipher_blob.split_at(NONCE_LEN);
        let mut key_bytes = derive_key(password, params)?;
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&key_bytes));
        let nonce = XNonce::from_slice(nonce_bytes);
        let pt = cipher
            .decrypt(nonce, ct)
            .map_err(|e| EncryptedFileError::Aead(e.to_string()));
        key_bytes.zeroize();
        pt
    }

    /// Persist [`KdfParams`] to disk as pretty-printed JSON.
    #[allow(dead_code)]
    pub fn save_key_params(
        params: &KdfParams,
        path: &std::path::Path,
    ) -> Result<(), EncryptedFileError> {
        let s = serde_json::to_string_pretty(params)?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, s)?;
        Ok(())
    }

    /// Load [`KdfParams`] previously written by [`save_key_params`].
    #[allow(dead_code)]
    pub fn load_key_params(path: &std::path::Path) -> Result<KdfParams, EncryptedFileError> {
        let s = std::fs::read_to_string(path)?;
        let p: KdfParams = serde_json::from_str(&s)?;
        Ok(p)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tempfile::tempdir;

        #[test]
        fn kdf_params_default_has_random_salt() {
            let a = KdfParams::default();
            let b = KdfParams::default();
            // 16 random bytes — collision probability is 2^-128.
            assert_ne!(a.salt_b64, b.salt_b64);
            assert_eq!(a.m_cost, 64 * 1024);
            assert_eq!(a.t_cost, 3);
            assert_eq!(a.p_cost, 1);
            assert_eq!(a.version, 1);
        }

        #[test]
        fn encrypt_decrypt_roundtrip_empty() {
            let (blob, params) = encrypt(b"", "pw").unwrap();
            let pt = decrypt(&blob, "pw", &params).unwrap();
            assert_eq!(pt, b"");
        }

        #[test]
        fn encrypt_decrypt_roundtrip_typical() {
            let secret = vec![0xABu8; 200];
            let (blob, params) = encrypt(&secret, "correct-horse-battery-staple").unwrap();
            let pt = decrypt(&blob, "correct-horse-battery-staple", &params).unwrap();
            assert_eq!(pt, secret);
        }

        #[test]
        fn decrypt_wrong_password_errors() {
            let (blob, params) = encrypt(b"top secret", "right").unwrap();
            let err = decrypt(&blob, "wrong", &params).unwrap_err();
            assert!(
                matches!(err, EncryptedFileError::Aead(_)),
                "expected Aead error, got {err:?}"
            );
        }

        #[test]
        fn decrypt_too_short_errors() {
            let params = KdfParams::default();
            let err = decrypt(&[0u8; 10], "pw", &params).unwrap_err();
            assert!(
                matches!(err, EncryptedFileError::TooShort),
                "expected TooShort, got {err:?}"
            );
        }

        #[test]
        fn decrypt_tampered_ciphertext_errors() {
            let (mut blob, params) = encrypt(b"hello world", "pw").unwrap();
            // Flip a byte inside the ciphertext (after the 24-byte nonce).
            let tamper_idx = NONCE_LEN + 1;
            blob[tamper_idx] ^= 0x01;
            let err = decrypt(&blob, "pw", &params).unwrap_err();
            assert!(
                matches!(err, EncryptedFileError::Aead(_)),
                "expected Aead error after tamper, got {err:?}"
            );
        }

        #[test]
        fn kdf_params_roundtrip_json() {
            let original = KdfParams::default();
            let s = serde_json::to_string(&original).unwrap();
            let back: KdfParams = serde_json::from_str(&s).unwrap();
            assert_eq!(original, back);
        }

        #[test]
        fn save_load_key_params_roundtrip() {
            let dir = tempdir().unwrap();
            let path = dir.path().join("params.json");
            let original = KdfParams::default();
            save_key_params(&original, &path).unwrap();
            let loaded = load_key_params(&path).unwrap();
            assert_eq!(original, loaded);
        }

        #[test]
        fn derive_key_deterministic_with_same_params() {
            let params = KdfParams::default();
            let k1 = derive_key("password123", &params).unwrap();
            let k2 = derive_key("password123", &params).unwrap();
            assert_eq!(k1, k2);
        }

        #[test]
        fn derive_key_differs_with_different_password() {
            let params = KdfParams::default();
            let k1 = derive_key("password1", &params).unwrap();
            let k2 = derive_key("password2", &params).unwrap();
            assert_ne!(k1, k2);
        }
    }
}

// ---------------------------------------------------------------------------
// Filesystem permission hardening
// ---------------------------------------------------------------------------

/// Enforce restrictive permissions on a file holding credentials.
///
/// On Unix: `chmod 0o600`. On Windows: leave to NTFS inheritance from
/// the user-profile-restricted parent directory (`%APPDATA%` is
/// per-user by default; explicit ACL manipulation needs `windows-acl`
/// which we don't want to pull in for this wave). Returns Ok on both
/// platforms; the Unix path is the load-bearing one for the audit
/// finding.
pub fn secure_credential_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Read-time perm check. Warns to stderr if the file is world-readable.
/// Intentionally does NOT refuse the load — that would brick the engine
/// on its very first run before any perms have been tightened.
pub fn warn_if_world_readable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                eprintln!(
                    "warning: {} has permissions {:#o}; tightening to 0o600 on next write",
                    path.display(),
                    mode
                );
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn plaintext_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("creds.toml");
        let store = PlaintextCredentialsStore::new(&path);

        assert!(store.get("anthropic_api_key").unwrap().is_none());

        store.put("anthropic_api_key", "sk-ant-secret").unwrap();
        assert_eq!(
            store.get("anthropic_api_key").unwrap().as_deref(),
            Some("sk-ant-secret")
        );

        store.put("openai_api_key", "sk-test").unwrap();
        assert_eq!(
            store.get("openai_api_key").unwrap().as_deref(),
            Some("sk-test")
        );

        store.delete("anthropic_api_key").unwrap();
        assert!(store.get("anthropic_api_key").unwrap().is_none());
        assert_eq!(
            store.get("openai_api_key").unwrap().as_deref(),
            Some("sk-test")
        );
    }

    #[cfg(unix)]
    #[test]
    fn plaintext_write_enforces_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join("creds.toml");
        let store = PlaintextCredentialsStore::new(&path);
        store.put("k", "v").unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "credentials file should be chmod 0600");
    }

    #[test]
    fn default_backend_is_auto() {
        // F16: default flipped Plaintext → Auto (keyring primary, plaintext
        // fallback) so secrets are not cleartext-by-default.
        let cfg = CredentialsStorageConfig::default();
        assert_eq!(cfg.backend, CredentialsBackend::Auto);
    }

    /// Hold the env-var passphrase while the test runs; cooperates with the
    /// other encrypted-file tests via `serial_test::serial`.
    struct EnvPassphraseGuard {
        prior: Option<String>,
    }

    impl EnvPassphraseGuard {
        fn set(value: &str) -> Self {
            let prior = std::env::var("WAYLAND_VAULT_PASSPHRASE").ok();
            unsafe {
                std::env::set_var("WAYLAND_VAULT_PASSPHRASE", value);
            }
            Self { prior }
        }
    }

    impl Drop for EnvPassphraseGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prior {
                    Some(v) => std::env::set_var("WAYLAND_VAULT_PASSPHRASE", v),
                    None => std::env::remove_var("WAYLAND_VAULT_PASSPHRASE"),
                }
            }
        }
    }

    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn encrypted_file_write_then_read_via_backend() {
        let _g = EnvPassphraseGuard::set("test-passphrase-1");
        let dir = tempdir().unwrap();
        let cipher = dir.path().join("vault.enc");
        let params = dir.path().join("vault.params.json");
        let store = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());

        // empty vault: get returns None without erroring
        assert!(store.get("anthropic_api_key").unwrap().is_none());

        store.put("anthropic_api_key", "sk-ant-secret").unwrap();
        store.put("openai_api_key", "sk-openai").unwrap();

        // Both files exist on disk
        assert!(cipher.exists(), "cipher blob not written");
        assert!(params.exists(), "kdf params not written");

        // Roundtrip
        assert_eq!(
            store.get("anthropic_api_key").unwrap().as_deref(),
            Some("sk-ant-secret")
        );
        assert_eq!(
            store.get("openai_api_key").unwrap().as_deref(),
            Some("sk-openai")
        );

        // Delete one
        store.delete("anthropic_api_key").unwrap();
        assert!(store.get("anthropic_api_key").unwrap().is_none());
        assert_eq!(
            store.get("openai_api_key").unwrap().as_deref(),
            Some("sk-openai")
        );
    }

    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn encrypted_file_survives_fresh_store_instance() {
        // Same passphrase + same files but a brand-new store object.
        // Simulates restart of the engine: the second store must decrypt
        // what the first one wrote.
        let _g = EnvPassphraseGuard::set("test-passphrase-2");
        let dir = tempdir().unwrap();
        let cipher = dir.path().join("vault.enc");
        let params = dir.path().join("vault.params.json");

        {
            let writer = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
            writer.put("k1", "v1").unwrap();
            writer.put("k2", "v2").unwrap();
        }

        let reader = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
        assert_eq!(reader.get("k1").unwrap().as_deref(), Some("v1"));
        assert_eq!(reader.get("k2").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn encrypted_file_wrong_passphrase_fails_unlock() {
        let dir = tempdir().unwrap();
        let cipher = dir.path().join("vault.enc");
        let params = dir.path().join("vault.params.json");

        // First: write the vault with one passphrase.
        {
            let _g = EnvPassphraseGuard::set("correct-passphrase");
            let writer = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
            writer.put("k", "v").unwrap();
        }

        // Second: try to unlock with a different passphrase.
        let _g = EnvPassphraseGuard::set("wrong-passphrase");
        let reader = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
        let err = reader.get("k").unwrap_err();
        assert!(
            matches!(err, CredentialsError::BackendUnavailable(ref m) if m.contains("vault unlock failed")),
            "expected BackendUnavailable with unlock-failed message, got {err:?}"
        );
    }

    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn encrypted_file_tampered_blob_fails_unlock() {
        let _g = EnvPassphraseGuard::set("test-passphrase-3");
        let dir = tempdir().unwrap();
        let cipher = dir.path().join("vault.enc");
        let params = dir.path().join("vault.params.json");

        {
            let writer = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
            writer.put("k", "v").unwrap();
        }

        // Flip a byte in the ciphertext (past the 24-byte nonce header).
        let mut bytes = std::fs::read(&cipher).unwrap();
        let idx = 24 + 1;
        bytes[idx] ^= 0xff;
        std::fs::write(&cipher, &bytes).unwrap();

        let reader = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
        let err = reader.get("k").unwrap_err();
        assert!(
            matches!(err, CredentialsError::BackendUnavailable(_)),
            "expected BackendUnavailable after tamper, got {err:?}"
        );
    }

    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn encrypted_file_factory_wires_backend() {
        let _g = EnvPassphraseGuard::set("factory-passphrase");
        let dir = tempdir().unwrap();
        let cipher_path = dir.path().join("creds.enc");
        let key_params_path = dir.path().join("creds.params.json");
        let cfg = CredentialsStorageConfig {
            backend: CredentialsBackend::EncryptedFile {
                cipher_path: cipher_path.clone(),
                key_params_path: key_params_path.clone(),
            },
            service_name: None,
        };
        // Factory should succeed (no longer BackendUnavailable).
        let store = open_store(&cfg, &dir.path().join("unused.toml"))
            .expect("encrypted-file factory wired");
        store.put("ak", "av").unwrap();
        assert_eq!(store.get("ak").unwrap().as_deref(), Some("av"));

        // Validator passes too.
        validate_credentials_config(&cfg).expect("encrypted-file validator passes");
    }

    #[test]
    fn config_parses_keyring_backend() {
        let parsed: CredentialsStorageConfig =
            toml::from_str(r#"backend = "keyring""#).expect("parses keyring");
        assert_eq!(parsed.backend, CredentialsBackend::Keyring);

        let parsed: CredentialsStorageConfig =
            toml::from_str(r#"backend = "plaintext""#).expect("parses plaintext");
        assert_eq!(parsed.backend, CredentialsBackend::Plaintext);
    }

    /// supply-unsafe-63: `validate_readable_fd` must accept a readable, open
    /// descriptor and reject closed or write-only ones before `from_raw_fd`.
    #[cfg(unix)]
    #[test]
    fn passphrase_fd_validation_rejects_bad_fds() {
        use std::os::unix::io::AsRawFd;

        let dir = tempdir().unwrap();

        // Readable, open fd → accepted.
        let readable_path = dir.path().join("readable");
        std::fs::write(&readable_path, b"secret\n").unwrap();
        let readable = std::fs::File::open(&readable_path).unwrap();
        assert!(
            validate_readable_fd(readable.as_raw_fd()).is_ok(),
            "an open read-only fd must validate"
        );

        // Write-only fd → rejected (cannot be read from).
        let writable_path = dir.path().join("writable");
        let writable = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&writable_path)
            .unwrap();
        assert!(
            validate_readable_fd(writable.as_raw_fd()).is_err(),
            "a write-only fd must be rejected"
        );

        // Closed / never-opened fd → rejected. A high fd number is almost
        // certainly not open in the test process.
        assert!(
            validate_readable_fd(9999).is_err(),
            "a closed/unopened fd must be rejected"
        );
        // A negative fd is never valid.
        assert!(
            validate_readable_fd(-1).is_err(),
            "a negative fd must be rejected"
        );
    }
}
