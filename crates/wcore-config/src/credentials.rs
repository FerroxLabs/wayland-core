//! Wave SD — `CredentialsStore` trait + backend impls.
//!
//! Closes SECURITY MAJOR #16 (API keys + AWS secret + GCP secret persisted
//! in plaintext config with default OS permissions).
//!
//! ## The default is FAIL-CLOSED. There is no plaintext fallback.
//!
//! Shipped as of v0.12.26 and stated here because the sentence this replaced
//! said the opposite. `build_ladder` mounts the OS keyring and the encrypted
//! vault and NOTHING ELSE; when neither is mounted, `put` REFUSES. It never
//! falls through to a cleartext write. FerroxLabs/wayland-core#397: the stale
//! claim sat on the type, where a reader looks first, while the truth sat
//! ~2,650 lines further down, and it produced a false "Core falls back to
//! plaintext credentials" reading three separate times in one afternoon.
//!
//! The plaintext store still exists, in two bounded roles, and saying so is
//! the point — deleting the false sentence without stating the true one only
//! invites the next reader to guess:
//!
//! * **Read-and-delete-only legacy.** The ladder's bottom rung is READ so that
//!   credentials written before a secure tier existed stay resolvable, and a
//!   value found there is promoted UP on the next read. Nothing new is ever
//!   written to it by the ladder.
//! * **An explicit, warned opt-out.** `backend = "plaintext"` selects it
//!   outright and prints a warning on stderr. Material that must never be
//!   cleartext (OAuth token sets) bypasses that opt-out by opening the ladder
//!   through [`open_secure_ladder_store`].
//!
//! Three stores ship:
//!
//! * `KeyringCredentialsStore` — the OS credential store via the `keyring`
//!   crate (macOS Keychain, Windows Credential Manager, Linux Secret
//!   Service). Behind the `keyring` cargo feature (on by default in this
//!   workspace); the ladder's top rung, and selectable via
//!   `backend = "keyring"`.
//! * `EncryptedFileCredentialsStore` — an Argon2id + XChaCha20-Poly1305 vault.
//!   The ladder's second rung, mounted when unlock material is present, and
//!   the top rung for an isolated profile (`WAYLAND_HOME`), which must not
//!   touch the process-global keyring service.
//! * `PlaintextCredentialsStore` — TOML on disk, `0o600` on Unix and a
//!   deny-all ACL attempt on Windows. Its two roles are the ones above.
//!
//! The trait is intentionally minimal so callers can also swap in a
//! test-only in-memory store. Lookups go through `Config::resolve_*`
//! helpers (env > store > config); puts/deletes are explicit operations.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    /// Default: the FAIL-CLOSED ladder built by [`build_ladder`].
    ///
    /// ladder: keyring -> encrypted_vault -> refuse
    ///
    /// The line above is machine-read by
    /// `the_documented_ladder_matches_the_rungs_build_ladder_mounts`; it is the
    /// claim, and the test is what stops it drifting from the code again.
    ///
    /// Writes descend the mounted rungs and then STOP. When neither the OS
    /// keyring nor the encrypted vault is available — headless Linux, CI, a
    /// locked vault — `put` returns an error. **It does not fall back to the
    /// plaintext file.** This doc comment claimed such a fallback until
    /// FerroxLabs/wayland-core#397; the fallback was removed in v0.12.26 and
    /// the sentence was not, and three independent readings then reported a
    /// plaintext-by-default weakness that had been fixed nine releases earlier.
    ///
    /// Reads descend one rung FURTHER, into the legacy plaintext file, so keys
    /// written before a secure tier existed stay resolvable; a value found
    /// there is promoted up on the next read and purged from below. That is a
    /// read path only — see the module header for the plaintext store's two
    /// remaining roles. Set `backend = "plaintext"` for the explicit, warned
    /// opt-out.
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

impl CredentialsBackend {
    /// Whether this backend may hold confidential material — encryption keys
    /// and sealed recovery requests, as opposed to ordinary API keys.
    ///
    /// This is the single source of the rule [`open_confidential_store`]
    /// enforces, exposed so callers can decide it from config alone instead of
    /// discovering it as a runtime failure. `Plaintext` never qualifies, by
    /// design; that refusal is the security property, not the defect.
    #[must_use]
    pub fn supports_confidential_material(&self) -> bool {
        !matches!(self, Self::Plaintext)
    }
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
    /// Resolve several keys from one logical snapshot when the backend can do
    /// so efficiently. The default preserves existing backend semantics;
    /// table-backed stores override it to avoid reloading or re-deriving their
    /// backing material once per key.
    fn get_many(&self, keys: &[&str]) -> Result<Vec<Option<String>>, CredentialsError> {
        keys.iter().map(|key| self.get(key)).collect()
    }
    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError>;
    fn delete(&self, key: &str) -> Result<(), CredentialsError>;
}

/// A [`CredentialsStore`] that keeps its secrets in process memory and nowhere
/// else.
///
/// Public because it is the only secure tier a hermetic test can mount: the OS
/// keyring is a host-global singleton (a test that wrote to it would collide
/// with the developer's real credentials and with every other test), and the
/// encrypted vault needs unlock material no headless runner has. Cloning shares
/// one backing map, so a test can prove that a value written through one handle
/// is readable through another — the "reopen the store" shape.
///
/// It is NOT a production backend: nothing persists past the process, so it can
/// never be selected by [`open_store`] or [`open_secure_ladder_store`].
#[derive(Clone, Default)]
pub struct InMemoryCredentialsStore {
    entries: std::sync::Arc<Mutex<HashMap<String, String>>>,
}

impl InMemoryCredentialsStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl CredentialsStore for InMemoryCredentialsStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        Ok(self.map().get(key).cloned())
    }

    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        self.map().insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        self.map().remove(key);
        Ok(())
    }
}

/// A credential store selected through the fail-closed confidential backend
/// policy. The private inner store prevents callers from constructing this
/// capability around a plaintext backend.
pub struct ConfidentialCredentialsStore {
    inner: Box<dyn CredentialsStore>,
    key_creation_lock_path: PathBuf,
    pin: Option<ConfidentialPinConfirmation>,
}

/// Everything needed to promote this profile's backend pin from advisory to
/// absolute the first time material is actually observed in it.
struct ConfidentialPinConfirmation {
    marker_path: PathBuf,
    selection: ConfidentialBackendSelection,
    recorded: std::sync::atomic::AtomicBool,
}

impl ConfidentialCredentialsStore {
    fn new(
        inner: Box<dyn CredentialsStore>,
        key_creation_lock_path: PathBuf,
        pin: Option<ConfidentialPinConfirmation>,
    ) -> Self {
        Self {
            inner,
            key_creation_lock_path,
            pin,
        }
    }

    pub(crate) fn key_creation_lock_path(&self) -> &Path {
        &self.key_creation_lock_path
    }

    /// Material was observed in the selected backend. Record it once per
    /// process; after this the pin is honoured absolutely.
    fn observed_material(&self) {
        let Some(pin) = &self.pin else {
            return;
        };
        if pin.recorded.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        confirm_confidential_backend_pin(&pin.marker_path, &pin.selection);
    }
}

impl CredentialsStore for ConfidentialCredentialsStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        let value = self.inner.get(key)?;
        if value.is_some() {
            self.observed_material();
        }
        Ok(value)
    }

    fn get_many(&self, keys: &[&str]) -> Result<Vec<Option<String>>, CredentialsError> {
        let values = self.inner.get_many(keys)?;
        if values.iter().any(Option::is_some) {
            self.observed_material();
        }
        Ok(values)
    }

    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        self.inner.put(key, value)?;
        self.observed_material();
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        self.inner.delete(key)
    }
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

    /// Enumerate the `[secrets]` table as flat `(key, value)` pairs, plus the
    /// raw entry count.
    ///
    /// Used by the #183 plaintext→vault migration. Non-string values (a
    /// corrupt/hand-edited file) are dropped from the returned pairs — they
    /// were never resolvable as credentials (`get` also does `.as_str()`) — but
    /// the raw count lets the migration detect that it dropped some and keep the
    /// plaintext file rather than destroy those hand-edited entries.
    fn load_all(&self) -> Result<(Vec<(String, String)>, usize), CredentialsError> {
        let table = self.load_table()?;
        let secrets = match table.get("secrets") {
            Some(toml::Value::Table(t)) => t,
            _ => return Ok((Vec::new(), 0)),
        };
        let raw_count = secrets.len();
        let entries = secrets
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect();
        Ok((entries, raw_count))
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

    fn get_many(&self, keys: &[&str]) -> Result<Vec<Option<String>>, CredentialsError> {
        let table = self.load_table()?;
        let secrets = match table.get("secrets") {
            Some(toml::Value::Table(table)) => Some(table),
            _ => None,
        };
        Ok(keys
            .iter()
            .map(|key| {
                secrets
                    .and_then(|table| table.get(*key))
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
            })
            .collect())
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

/// One keyring entry, addressed as the `keyring` crate addresses it.
///
/// Private because it is the *unbounded* view: a caller that writes through it
/// is subject to whatever per-entry size ceiling the host imposes. Everything
/// outside this module goes through [`KeyringCredentialsStore`], which spans
/// that ceiling.
struct RawKeyringEntries {
    service: String,
}

impl CredentialsStore for RawKeyringEntries {
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

/// Maximum UTF-16 code units this crate will put in ONE **Windows Credential
/// Manager** entry.
///
/// Windows Credential Manager caps a credential's `CredentialBlob` at
/// `CRED_MAX_CREDENTIAL_BLOB_SIZE` = 2560 bytes, and the `keyring` crate
/// encodes the value as UTF-16 — so the real per-entry ceiling on Windows is
/// **1280 code units**, roughly 1280 ASCII characters, and `set_password`
/// refuses anything larger.
///
/// That ceiling is far below one OAuth token set: a ChatGPT login is an access
/// JWT plus an id JWT plus a refresh token, routinely 3–5 KB of JSON. Once
/// OAuth tokens moved onto the credential ladder, `auth login` on Windows hit
/// the cap, the keyring rung refused, and — on the overwhelmingly common
/// Windows desktop that has no `WAYLAND_VAULT_PASSPHRASE` — the ladder had no
/// rung left and refused the login outright. A user who could sign in before
/// could not sign in after. Spanning entries is what makes the ceiling stop
/// being a functional limit; it does NOT relax the ladder, because every piece
/// still lands in the OS keyring and nothing is ever written in cleartext.
///
/// 1000 rather than 1280 leaves headroom for the non-ASCII case, where one
/// `char` costs two UTF-16 units and a split must still land on a `char`
/// boundary.
const WINDOWS_MAX_UTF16_UNITS_PER_ENTRY: usize = 1000;

/// The same ceiling everywhere Windows Credential Manager is not the backend.
///
/// macOS Keychain and Linux Secret Service were MEASURED against a bare keyring
/// entry (`crates/wcore-config/tests/keyring_cap_probe_live.rs`) and both
/// accepted 1,024,000 UTF-16 units — 1024× the Windows number. Applying the
/// Windows figure to them turned a ~4.3 KB OAuth token set into five parts plus
/// a manifest where it fits in ONE entry, which is six times the keyring round
/// trips and, more importantly, put two whole platforms on the spanned write
/// path for no reason at all.
///
/// 128,000 is deliberately an order of magnitude under the measured floor: the
/// probe proved those hosts accept at least 1,024,000, not that 1,024,000 is the
/// limit, and no credential this crate stores comes within range of 128,000
/// anyway. Reading is unaffected either way — [`chunked_get`] is driven by the
/// stored manifest, so values that an older build spanned still read back, and
/// the next rewrite collapses them to a single entry and purges the parts.
const NON_WINDOWS_MAX_UTF16_UNITS_PER_ENTRY: usize = 128_000;

/// Maximum UTF-16 code units this crate will put in ONE keyring entry on THIS
/// platform.
///
/// The single place the per-backend difference lives; every call site reads it
/// from here rather than branching on the platform itself.
const fn keyring_max_utf16_units_per_entry() -> usize {
    if cfg!(windows) {
        WINDOWS_MAX_UTF16_UNITS_PER_ENTRY
    } else {
        NON_WINDOWS_MAX_UTF16_UNITS_PER_ENTRY
    }
}

/// Sentinel that marks a primary entry as a manifest rather than a secret.
///
/// Leads with U+0001 (START OF HEADING), which no API key, token set or
/// passphrase this crate stores can begin with, so a literal value can never be
/// mistaken for a manifest.
const KEYRING_CHUNK_MANIFEST_PREFIX: &str = "\u{1}wayland-core-chunked-v1 ";

/// Where a spanned value's parts live: `<generation>` is `a` or `b` and
/// `<count>` is how many parts make up the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KeyringChunkManifest {
    generation: char,
    count: usize,
}

/// UTF-16 code units in `value` — the unit Windows Credential Manager actually
/// measures, which is neither `len()` (UTF-8 bytes) nor `chars().count()`.
fn utf16_units(value: &str) -> usize {
    value.chars().map(char::len_utf16).sum()
}

/// Split `value` into pieces of at most `max_units` UTF-16 code units each,
/// never splitting a `char`. `max_units` of 0 is treated as 1 so the loop
/// cannot spin.
fn split_by_utf16_units(value: &str, max_units: usize) -> Vec<&str> {
    let max_units = max_units.max(1);
    let mut parts = Vec::new();
    let mut start = 0;
    let mut units = 0;
    for (offset, ch) in value.char_indices() {
        let cost = ch.len_utf16();
        if units + cost > max_units && offset > start {
            parts.push(&value[start..offset]);
            start = offset;
            units = 0;
        }
        units += cost;
    }
    if start < value.len() || parts.is_empty() {
        parts.push(&value[start..]);
    }
    parts
}

fn render_chunk_manifest(manifest: KeyringChunkManifest) -> String {
    format!(
        "{KEYRING_CHUNK_MANIFEST_PREFIX}{} {}",
        manifest.generation, manifest.count
    )
}

/// Parse a primary entry as a manifest. `None` means "this is a literal
/// secret", which is what every value written before this scheme — and every
/// value small enough to fit one entry — looks like.
fn parse_chunk_manifest(primary: &str) -> Option<KeyringChunkManifest> {
    let rest = primary.strip_prefix(KEYRING_CHUNK_MANIFEST_PREFIX)?;
    let (generation, count) = rest.split_once(' ')?;
    let generation = match generation {
        "a" => 'a',
        "b" => 'b',
        _ => return None,
    };
    let count = count.parse::<usize>().ok()?;
    (count > 0).then_some(KeyringChunkManifest { generation, count })
}

fn chunk_key(key: &str, generation: char, index: usize) -> String {
    format!("{key}.__wlchunk{generation}{index}")
}

/// Where the cross-process lock for one keyring service's spanned writes lives.
///
/// The lock NAME is derived from what identifies the entry **in the OS
/// keyring** — the service and the key — and NOT from whichever credentials
/// path the caller happened to pass. The keyring is a host-global singleton:
/// two openers can reach one `(service, key)` pair through different
/// `plaintext_path`s (`build_ladder` and `open_store(Keyring)` both resolve the
/// same default service), and a name derived from the caller's path would let
/// exactly those two writers race.
///
/// The DIRECTORY is [`crate::config::wayland_config_dir`] — the canonical
/// `WAYLAND_HOME`-honouring credentials root, and already the home of the
/// migration lock and the confidential key-creation lock. It tracks the service
/// by construction: the keyring rung is only mounted with the bare service name
/// when `WAYLAND_HOME` is unset (one directory for every process on the host),
/// and a profile-scoped service is derived from a path under `WAYLAND_HOME`
/// (one directory per profile, matching the service's own profile digest).
#[derive(Clone, Debug)]
struct ChunkWriteLockSite {
    dir: PathBuf,
    /// Digested into the lock filename together with the key, so two keyring
    /// SERVICES sharing one directory never share a lock.
    namespace: String,
    policy: LockPolicy,
}

impl ChunkWriteLockSite {
    fn for_service(service: &str) -> Self {
        Self::anchored(service, crate::config::wayland_config_dir())
    }

    /// The same site with the credentials directory named explicitly rather than
    /// resolved from the environment. Only [`purge_profile_confidential_keys`]
    /// needs it: it runs against ANOTHER profile's keyring service while
    /// `WAYLAND_HOME` still points at the caller's, so the ambient directory
    /// would place its lock where no writer in the target profile looks.
    fn anchored(service: &str, dir: PathBuf) -> Self {
        Self {
            dir,
            namespace: service.to_string(),
            policy: LockPolicy::CREDENTIAL_WRITE,
        }
    }

    /// A site rooted at an explicit directory, so a test can exercise the real
    /// lock without writing lockfiles into the developer's credentials dir.
    #[cfg(test)]
    fn in_dir(dir: &Path, policy: LockPolicy) -> Self {
        Self {
            dir: dir.to_path_buf(),
            namespace: "test".to_string(),
            policy,
        }
    }

    /// One lock per `(service, key)` rather than one per store: two different
    /// credentials never contend, and the name is a fixed-length digest so a key
    /// containing characters no filesystem accepts still gets a lock.
    fn lock_path(&self, key: &str) -> PathBuf {
        let mut hasher = Sha256::new();
        hasher.update(self.namespace.as_bytes());
        hasher.update([0u8]);
        hasher.update(key.as_bytes());
        let digest = hasher.finalize();
        let name: String = digest
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.dir.join(format!(".credentials.chunk.{name}.lock"))
    }

    /// Hold the whole read-decide-write. Acquisition failure is a REFUSAL, never
    /// a fall-through to an unlocked write: proceeding without the lock is
    /// precisely the interleave this guards against.
    fn acquire(&self, key: &str) -> Result<ExclusiveFileLock, CredentialsError> {
        // The credentials directory is created lazily, and on a keyring-only
        // profile a credential write can be the first thing that needs it. This
        // is the same directory (and the same hardening) the migration lock and
        // the confidential key-creation lock already use.
        secure_credential_dir(&self.dir)?;
        ExclusiveFileLock::acquire(self.lock_path(key), self.policy, "credential write")
    }
}

/// The outcome of assembling a spanned value, separated from the error text so
/// [`chunked_get`] can tell "this generation is torn" from any other failure and
/// re-read under the write lock before giving up.
enum ChunkRead {
    Value(Option<String>),
    /// The live manifest claims `count` parts and part `index` is not there.
    Torn {
        count: usize,
        index: usize,
    },
}

/// One unsynchronised pass: read the manifest, then read the parts it names.
fn read_chunked(raw: &dyn CredentialsStore, key: &str) -> Result<ChunkRead, CredentialsError> {
    let Some(primary) = raw.get(key)? else {
        return Ok(ChunkRead::Value(None));
    };
    let Some(manifest) = parse_chunk_manifest(&primary) else {
        return Ok(ChunkRead::Value(Some(primary)));
    };
    let mut value = String::new();
    for index in 0..manifest.count {
        let Some(part) = raw.get(&chunk_key(key, manifest.generation, index))? else {
            return Ok(ChunkRead::Torn {
                count: manifest.count,
                index,
            });
        };
        value.push_str(&part);
    }
    Ok(ChunkRead::Value(Some(value)))
}

/// Read a value that may span several keyring entries.
///
/// The common path takes no lock — the commit order makes an unlocked read see
/// either the whole old value or the whole new one. The one gap it leaves is
/// narrow and transient: a reader that samples the manifest just BEFORE a writer
/// flips it, then reads the parts just AFTER that writer purged the superseded
/// generation, finds a part missing. That is a live, intact credential, so
/// re-reading once under the write lock (which by then is free, or is held by
/// the writer we are waiting out) settles it. Only a genuinely torn store
/// survives the retry, and that still refuses rather than returning a prefix.
fn chunked_get(
    raw: &dyn CredentialsStore,
    key: &str,
    locks: &ChunkWriteLockSite,
) -> Result<Option<String>, CredentialsError> {
    if let ChunkRead::Value(value) = read_chunked(raw, key)? {
        return Ok(value);
    }
    let _lock = locks.acquire(key)?;
    match read_chunked(raw, key)? {
        ChunkRead::Value(value) => Ok(value),
        // A missing part must be an ERROR, never a short string: returning the
        // prefix of a credential would hand the caller a secret that
        // authenticates as nothing and reads as a corrupted-token bug.
        ChunkRead::Torn { count, index } => Err(CredentialsError::Keyring(format!(
            "the credential for '{key}' spans {count} keyring entries and part {index} is \
             missing; refusing to return a truncated secret"
        ))),
    }
}

/// Write a value, spanning entries when it exceeds `max_units`.
///
/// SERIALIZED, and it is not negotiable: the whole read-decide-write runs under
/// one cross-process lock. The target generation is chosen by reading the live
/// manifest and taking the other letter — an unsynchronised read-modify-write.
/// Two writers that read the same manifest choose the SAME target and interleave
/// their parts into it, and because each writer then commits a manifest naming
/// its OWN part count, the loser's tail stays spliced onto the winner's head and
/// both writers return `Ok`. No crash and no injected fault is needed; an
/// ordinary preemption between the last part write and the manifest write is
/// enough. Two wayland-core processes on one profile (the CLI and the desktop
/// host, or two sessions) refreshing an OAuth token set near expiry is the
/// normal case, and those refresh tokens are single-use, so the splice costs the
/// user their login. The lock spans the manifest READ as well as the writes,
/// because the read is the half that makes the decision.
///
/// COMMIT ORDER, also not negotiable: **write the parts → flip the manifest →
/// purge the old parts.** The manifest is the only thing a reader consults
/// first, so until it flips the reader still sees the OLD value in full. The new
/// parts go under the OTHER generation for exactly this reason — reusing the
/// live generation's entry names would let a process killed mid-write leave a
/// manifest pointing at a mix of old and new parts, which is a silently corrupt
/// credential rather than an absent one. The lock does not change that ordering;
/// it only stops a second writer from being the thing that interleaves.
fn chunked_put(
    raw: &dyn CredentialsStore,
    key: &str,
    value: &str,
    max_units: usize,
    locks: &ChunkWriteLockSite,
) -> Result<(), CredentialsError> {
    let _lock = locks.acquire(key)?;
    let previous = read_previous_manifest(raw, key)?;

    if utf16_units(value) <= max_units {
        raw.put(key, value)?;
        purge_chunks(raw, key, previous);
        return Ok(());
    }

    let generation = match previous {
        Some(KeyringChunkManifest {
            generation: 'a', ..
        }) => 'b',
        _ => 'a',
    };
    let parts = split_by_utf16_units(value, max_units);
    for (index, part) in parts.iter().enumerate() {
        raw.put(&chunk_key(key, generation, index), part)?;
    }
    raw.put(
        key,
        &render_chunk_manifest(KeyringChunkManifest {
            generation,
            count: parts.len(),
        }),
    )?;
    purge_chunks(raw, key, previous);
    Ok(())
}

/// The live manifest, or `None` when the primary entry is genuinely absent or
/// holds a literal secret.
///
/// A read FAULT is neither of those and must abort the caller. Swallowing it
/// (the `raw.get(key).ok().flatten()` this replaces) reports "no previous
/// manifest", which selects generation `'a'` — and if the live value already IS
/// generation `'a'`, the writer then overwrites the live parts IN PLACE under a
/// manifest still counting the old part total. That is the corruption the commit
/// order exists to prevent, reached without any concurrency at all. It is
/// ordinary to reach: the keyring writability probe is cached for the life of
/// the process, so a keyring that locks AFTER startup — a screen-locked Secret
/// Service, a denied Keychain prompt, a transient Windows RPC failure — leaves
/// this the only guard.
fn read_previous_manifest(
    raw: &dyn CredentialsStore,
    key: &str,
) -> Result<Option<KeyringChunkManifest>, CredentialsError> {
    Ok(raw.get(key)?.as_deref().and_then(parse_chunk_manifest))
}

/// Remove a value and every entry it spans.
///
/// Under the same lock as [`chunked_put`], so a delete cannot land between a
/// concurrent writer's parts and its manifest, and cannot purge a generation
/// that writer is in the middle of publishing.
fn chunked_delete(
    raw: &dyn CredentialsStore,
    key: &str,
    locks: &ChunkWriteLockSite,
) -> Result<(), CredentialsError> {
    let _lock = locks.acquire(key)?;
    // Aborting on a read fault matters MORE here than on the write path: with
    // the manifest unread we would delete the primary and leave the parts, so a
    // logout would report success while the refresh token's fragments stayed in
    // the OS keyring, unreferenced and undeletable by any later call.
    let previous = read_previous_manifest(raw, key)?;
    // Manifest first: once it is gone no reader can reach the parts, so a
    // process killed here leaves orphans rather than a torn read.
    raw.delete(key)?;
    purge_chunks(raw, key, previous);
    Ok(())
}

/// Best-effort removal of a superseded generation's parts. A failure here
/// leaves unreferenced entries behind, which no reader can reach — never a
/// reason to fail the write that already succeeded.
fn purge_chunks(raw: &dyn CredentialsStore, key: &str, manifest: Option<KeyringChunkManifest>) {
    let Some(manifest) = manifest else {
        return;
    };
    for index in 0..manifest.count {
        if let Err(error) = raw.delete(&chunk_key(key, manifest.generation, index)) {
            tracing::debug!(
                target: "wcore_credentials",
                key,
                error = %error,
                "could not remove a superseded keyring chunk; it is unreferenced and harmless"
            );
        }
    }
}

/// OS-native keyring credentials store.
///
/// Backed by the `keyring` crate (macOS Keychain on Apple, Windows
/// Credential Manager on Windows, Secret Service on Linux). Each
/// `key` is mapped to a `(service, user)` pair; we use the
/// configured service name (default `"wayland-core"`) and the key
/// itself as the user — this keeps lookup O(1) and matches the
/// `keyring` crate's expected shape.
///
/// A value larger than one entry can hold
/// ([`keyring_max_utf16_units_per_entry`]) is spanned across sibling entries
/// under a manifest, so the Windows blob cap stops being a functional ceiling on
/// what can be stored. Values that fit are written literally, exactly as before,
/// so entries written by older builds keep reading back unchanged.
pub struct KeyringCredentialsStore {
    raw: RawKeyringEntries,
    /// The cross-process write lock for this service. Two wayland-core
    /// processes on one profile write to the SAME OS keyring entries, so
    /// serializing them cannot be done with an in-process mutex.
    locks: ChunkWriteLockSite,
}

impl KeyringCredentialsStore {
    pub fn new(service: impl Into<String>) -> Self {
        let service = service.into();
        Self {
            locks: ChunkWriteLockSite::for_service(&service),
            raw: RawKeyringEntries { service },
        }
    }

    /// [`Self::new`] with the write lock anchored at an explicit credentials
    /// directory. See [`ChunkWriteLockSite::anchored`] for the one caller.
    fn anchored_at(service: impl Into<String>, credentials_dir: PathBuf) -> Self {
        let service = service.into();
        Self {
            locks: ChunkWriteLockSite::anchored(&service, credentials_dir),
            raw: RawKeyringEntries { service },
        }
    }
}

impl CredentialsStore for KeyringCredentialsStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        chunked_get(&self.raw, key, &self.locks)
    }

    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        chunked_put(
            &self.raw,
            key,
            value,
            keyring_max_utf16_units_per_entry(),
            &self.locks,
        )
    }

    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        chunked_delete(&self.raw, key, &self.locks)
    }
}

// ---------------------------------------------------------------------------
// Auto backend (keyring primary, plaintext fallback) — the default
// ---------------------------------------------------------------------------

/// Key used by the keyring writability probe. Never holds a secret; the value
/// exists only long enough for the probe to prove a write landed.
const KEYRING_PROBE_KEY: &str = "__wayland_core_keyring_probe__";
const KEYRING_PROBE_VALUE: &str = "probe";

/// Probe whether the OS keyring is actually usable **for writes** on this host.
///
/// Returns `false` on headless Linux without a running Secret Service, in CI,
/// etc., so the [`CredentialsBackend::Auto`] default can fall back rather than
/// error.
///
/// The probe MUST write, not just read. A read-only probe (`get_password`,
/// accepting `NoEntry` as "works") reports the keyring available on a Windows
/// service account — `CredRead` succeeds there — while the very next
/// `CredWrite` fails with `Windows error code 8`. That combination made the
/// confidential opener pin the keyring and then refuse every durable-session
/// write with "secure recovery storage is unavailable", instead of falling
/// through to the already-unlocked encrypted vault. Proving writability is the
/// only probe that answers the question the caller is actually asking.
///
/// Cached per service for the life of the process: the answer is a property of
/// the host + account, and re-probing would put one write/delete round trip on
/// every credential open.
fn keyring_available(service: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(service)
    {
        return *cached;
    }
    let writable = keyring_probe_is_writable(service);
    cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(service.to_string(), writable);
    writable
}

/// The uncached round trip behind [`keyring_available`]: write a probe value,
/// then remove it.
fn keyring_probe_is_writable(service: &str) -> bool {
    let Ok(entry) = keyring::Entry::new(service, KEYRING_PROBE_KEY) else {
        return false;
    };
    if let Err(error) = entry.set_password(KEYRING_PROBE_VALUE) {
        tracing::debug!(
            target: "wcore_credentials",
            error = %error,
            "keyring write probe failed; treating the keyring as unavailable"
        );
        return false;
    }
    // Leave nothing behind. A probe entry that resists deletion is still proof
    // of a writable keyring, so a delete failure must NOT flip the verdict —
    // that would be the read-only probe's mistake in the other direction.
    if let Err(error) = entry.delete_credential()
        && !matches!(error, keyring::Error::NoEntry)
    {
        tracing::debug!(
            target: "wcore_credentials",
            error = %error,
            "keyring probe entry could not be removed"
        );
    }
    true
}

/// Build a stable, profile-isolated keyring service identity.
///
/// The credentials file may not exist yet, so canonicalize the longest
/// existing ancestor and append the missing suffix. This makes symlinked
/// profile paths converge while keeping new profiles deterministic. The path
/// itself is not exposed to the OS keyring UI; only its SHA-256 digest is.
fn profile_keyring_service(
    base_service: &str,
    credentials_path: &Path,
) -> Result<String, CredentialsError> {
    let canonical = absolute_confidential_path(credentials_path)?;
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    let digest_hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("{base_service}.profile.{digest_hex}"))
}

fn absolute_confidential_path(path: &Path) -> Result<PathBuf, CredentialsError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(canonicalize_with_missing_suffix(&absolute))
}

fn confidential_keyring_service(
    cfg: &CredentialsStorageConfig,
    credentials_path: &Path,
    isolated_home: bool,
) -> Result<String, CredentialsError> {
    let base_service = cfg
        .service_name
        .clone()
        .unwrap_or_else(|| "wayland-core".to_string());
    if isolated_home {
        profile_keyring_service(&base_service, credentials_path)
    } else {
        Ok(base_service)
    }
}

fn canonicalize_with_missing_suffix(path: &Path) -> PathBuf {
    let mut cursor = path;
    let mut missing = Vec::new();

    loop {
        if let Ok(mut canonical) = std::fs::canonicalize(cursor) {
            for component in missing.iter().rev() {
                canonical.push(component);
            }
            return canonical;
        }

        let Some(file_name) = cursor.file_name() else {
            return path.to_path_buf();
        };
        missing.push(file_name.to_os_string());
        let Some(parent) = cursor.parent() else {
            return path.to_path_buf();
        };
        cursor = parent;
    }
}

const CONFIDENTIAL_BACKEND_MARKER_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case", deny_unknown_fields)]
enum ConfidentialBackendSelection {
    Keyring {
        service: String,
    },
    EncryptedFile {
        cipher_path: PathBuf,
        key_params_path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfidentialBackendMarker {
    version: u8,
    selection: ConfidentialBackendSelection,
    /// `true` once the profile's confidential material has been OBSERVED in
    /// `selection` — a write that succeeded, or a read that returned a value.
    ///
    /// This is what makes the pin safe to honour absolutely. The pin exists to
    /// stop oscillation from orphaning secrets; if nothing has ever been
    /// observed in the pinned backend there is nothing to orphan, so an
    /// unconfirmed pin is advisory and may be re-selected when its backend is
    /// unavailable. That is the only thing that heals a profile which pinned
    /// the OS keyring on a host where `CredRead` succeeds and `CredWrite`
    /// fails: the pinning boot could not write, so it never confirmed.
    ///
    /// Absent in markers written before this field existed, hence
    /// `default` — those deserialize as unconfirmed, which is exactly the
    /// treatment the affected population needs. Only serialized when `true`,
    /// so an unconfirmed marker stays byte-identical to the pre-field shape.
    #[serde(default, skip_serializing_if = "is_false")]
    confirmed: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// A loaded pin plus whether it is backed by observed material.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PinnedConfidentialBackend {
    selection: ConfidentialBackendSelection,
    confirmed: bool,
}

#[derive(Debug, Clone)]
enum ConfidentialBackendMode {
    Auto {
        keyring: ConfidentialBackendSelection,
        vault: ConfidentialBackendSelection,
    },
    Explicit(ConfidentialBackendSelection),
}

fn confidential_backend_unavailable(message: &str) -> CredentialsError {
    CredentialsError::BackendUnavailable(message.to_string())
}

fn selection_is_available(
    selection: &ConfidentialBackendSelection,
    keyring_is_available: &impl Fn(&str) -> bool,
    vault_is_available: bool,
) -> bool {
    match selection {
        ConfidentialBackendSelection::Keyring { service } => keyring_is_available(service),
        ConfidentialBackendSelection::EncryptedFile { .. } => vault_is_available,
    }
}

/// Resolve one confidential backend, never replacing a pin that is backed by
/// observed material. Availability is injected so oscillation behavior can be
/// proven without touching an operator's real keyring.
///
/// A CONFIRMED pin is absolute: if its backend is unavailable this errors
/// rather than move, because moving would orphan the secrets already stored
/// there (`load_or_create_confidential_blob_key_from_store` would mint a fresh
/// key and every previously sealed blob would stop decrypting).
///
/// An UNCONFIRMED pin is advisory. Nothing has ever been observed in it, so
/// re-selecting costs nothing and is the only way a profile that pinned a
/// write-incapable keyring on its very first boot can ever recover.
fn select_confidential_backend(
    pinned: Option<&PinnedConfidentialBackend>,
    mode: &ConfidentialBackendMode,
    keyring_is_available: &impl Fn(&str) -> bool,
    vault_is_available: bool,
) -> Result<ConfidentialBackendSelection, CredentialsError> {
    if let Some(pin) = pinned {
        let pinned = &pin.selection;
        match mode {
            ConfidentialBackendMode::Auto { keyring, vault }
                if pinned != keyring && pinned != vault =>
            {
                return Err(confidential_backend_unavailable(
                    "pinned confidential backend conflicts with the current profile",
                ));
            }
            ConfidentialBackendMode::Explicit(required) if required != pinned => {
                return Err(confidential_backend_unavailable(
                    "configured confidential backend conflicts with the profile's pinned backend",
                ));
            }
            _ => {}
        }
        let available = match mode {
            ConfidentialBackendMode::Auto { .. } => {
                selection_is_available(pinned, keyring_is_available, vault_is_available)
            }
            // An explicitly configured encrypted file retains its existing
            // interactive unlock behavior; keyring availability is still
            // probed before the store is opened.
            ConfidentialBackendMode::Explicit(ConfidentialBackendSelection::EncryptedFile {
                ..
            }) => true,
            ConfidentialBackendMode::Explicit(_) => {
                selection_is_available(pinned, keyring_is_available, vault_is_available)
            }
        };
        if available {
            return Ok(pinned.clone());
        }
        // Unavailable. Only an unconfirmed Auto pin may be re-selected; an
        // explicit backend is an operator instruction, not a preference.
        if !(pin.confirmed || matches!(mode, ConfidentialBackendMode::Explicit(_))) {
            tracing::warn!(
                target: "wcore_credentials",
                "the profile's pinned confidential backend is unavailable and holds no \
                 observed material; re-selecting an available backend"
            );
        } else {
            return Err(confidential_backend_unavailable(
                "the profile's pinned confidential credential backend is unavailable",
            ));
        }
    }

    match mode {
        ConfidentialBackendMode::Auto { keyring, vault } => {
            if selection_is_available(keyring, keyring_is_available, vault_is_available) {
                Ok(keyring.clone())
            } else if selection_is_available(vault, keyring_is_available, vault_is_available) {
                Ok(vault.clone())
            } else {
                Err(confidential_backend_unavailable(
                    "no confidential credential backend is available",
                ))
            }
        }
        ConfidentialBackendMode::Explicit(selection) => {
            let available = match selection {
                ConfidentialBackendSelection::Keyring { .. } => {
                    selection_is_available(selection, keyring_is_available, vault_is_available)
                }
                ConfidentialBackendSelection::EncryptedFile { .. } => true,
            };
            if available {
                Ok(selection.clone())
            } else {
                Err(confidential_backend_unavailable(
                    "the configured confidential credential backend is unavailable",
                ))
            }
        }
    }
}

fn load_confidential_backend_marker(
    marker_path: &Path,
) -> Result<Option<ConfidentialBackendMarker>, CredentialsError> {
    let bytes = match std::fs::read(marker_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(CredentialsError::Io(error)),
    };
    let marker: ConfidentialBackendMarker = serde_json::from_slice(&bytes).map_err(|_| {
        confidential_backend_unavailable("confidential backend marker is malformed")
    })?;
    if marker.version != CONFIDENTIAL_BACKEND_MARKER_VERSION {
        return Err(confidential_backend_unavailable(
            "confidential backend marker version is unsupported",
        ));
    }
    Ok(Some(marker))
}

/// Load the pin and settle whether it is backed by material.
///
/// On-disk evidence outranks the recorded flag: a vault cipher file that
/// EXISTS holds the profile's secrets no matter what any earlier process
/// wrote, so such a pin is confirmed even if the marker predates the flag.
/// There is no equivalent cheap check for an OS keyring, which is precisely
/// why the flag exists.
fn load_pinned_confidential_backend(
    marker_path: &Path,
) -> Result<Option<PinnedConfidentialBackend>, CredentialsError> {
    Ok(
        load_confidential_backend_marker(marker_path)?.map(|marker| {
            let confirmed = marker.confirmed
                || match &marker.selection {
                    ConfidentialBackendSelection::EncryptedFile { cipher_path, .. } => {
                        cipher_path.exists()
                    }
                    ConfidentialBackendSelection::Keyring { .. } => false,
                };
            PinnedConfidentialBackend {
                selection: marker.selection,
                confirmed,
            }
        }),
    )
}

fn confidential_backend_marker_path(credentials_path: &Path) -> PathBuf {
    credentials_path.with_file_name(".credentials.confidential-backend.json")
}

/// Open the marker's advisory lock file. Held (via [`acquire_marker_lock`]) by
/// both the resolver and the confirmation path so the two can never interleave
/// a read-modify-write of the marker.
fn open_confidential_backend_marker_lock(
    marker_path: &Path,
) -> Result<fd_lock::RwLock<std::fs::File>, CredentialsError> {
    let lock_path = marker_path.with_extension("lock");
    if let Some(parent) = marker_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    Ok(fd_lock::RwLock::new(file))
}

/// Run `body` while holding the marker's exclusive advisory lock.
///
/// A closure rather than a returned guard: `fd_lock`'s guard borrows the
/// `RwLock` mutably, and a retry loop that returns the guard across a function
/// boundary is not expressible under NLL.
fn with_marker_lock<T>(
    marker_path: &Path,
    body: impl FnOnce() -> Result<T, CredentialsError>,
) -> Result<T, CredentialsError> {
    let mut lock = open_confidential_backend_marker_lock(marker_path)?;
    let _guard = loop {
        match lock.write() {
            Ok(guard) => break guard,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                return Err(confidential_backend_unavailable(
                    "confidential backend selection lock failed",
                ));
            }
        }
    };
    body()
}

fn write_confidential_backend_marker(
    marker_path: &Path,
    marker: &ConfidentialBackendMarker,
) -> Result<(), CredentialsError> {
    let bytes = serde_json::to_vec(marker).map_err(|_| {
        confidential_backend_unavailable("confidential backend marker serialization failed")
    })?;
    crate::atomic_write(marker_path, &bytes)?;
    Ok(())
}

/// Record that the profile's confidential material has been observed in
/// `selection`, making the pin absolute from here on.
///
/// Idempotent and best-effort: this is a durability hint, not a correctness
/// gate, so a failure to record it must never fail the caller's read or write.
fn confirm_confidential_backend_pin(marker_path: &Path, selection: &ConfidentialBackendSelection) {
    let record = || {
        with_marker_lock(marker_path, || {
            let Some(mut marker) = load_confidential_backend_marker(marker_path)? else {
                return Ok(());
            };
            if marker.confirmed || &marker.selection != selection {
                return Ok(());
            }
            marker.confirmed = true;
            write_confidential_backend_marker(marker_path, &marker)
        })
    };
    if let Err(error) = record() {
        tracing::debug!(
            target: "wcore_credentials",
            error = %error,
            "could not record confidential backend pin confirmation"
        );
    }
}

fn resolve_confidential_backend_with_availability(
    mode: &ConfidentialBackendMode,
    plaintext_path: &Path,
    keyring_is_available: &impl Fn(&str) -> bool,
    vault_is_available: bool,
) -> Result<ConfidentialBackendSelection, CredentialsError> {
    let marker_path = confidential_backend_marker_path(plaintext_path);
    with_marker_lock(&marker_path, || {
        let pinned = load_pinned_confidential_backend(&marker_path)?;
        let selected = select_confidential_backend(
            pinned.as_ref(),
            mode,
            keyring_is_available,
            vault_is_available,
        )?;
        // Write the marker on a first pin, and REWRITE it when an unconfirmed
        // pin was re-selected away from — otherwise the healed profile would
        // re-enter the same dead end on its next boot.
        if pinned.as_ref().map(|pin| &pin.selection) != Some(&selected) {
            write_confidential_backend_marker(
                &marker_path,
                &ConfidentialBackendMarker {
                    version: CONFIDENTIAL_BACKEND_MARKER_VERSION,
                    selection: selected.clone(),
                    confirmed: false,
                },
            )?;
        }
        Ok(selected)
    })
}

/// The actionable refusal emitted when no secure tier can hold a write.
///
/// GCM's shape (`fatal: No credential backing store has been selected`), with
/// the two ways forward named. A refusal that does not tell the operator how to
/// proceed is indistinguishable from a bug, and an operator who cannot proceed
/// reaches for the thing this refusal exists to prevent.
fn no_secure_backend_for_write(key: &str) -> CredentialsError {
    CredentialsError::BackendUnavailable(format!(
        "refusing to store credential '{key}': no secure credential backend is available on \
         this host. The OS keyring is not writable here, and the encrypted vault is locked. \
         To store it securely, unlock the vault by setting WAYLAND_VAULT_PASSPHRASE_FD (a \
         passphrase file descriptor — preferred) or WAYLAND_VAULT_PASSPHRASE; on a Linux \
         desktop, starting a Secret Service (gnome-keyring / KWallet) restores the keyring \
         instead. Unencrypted storage exists but is NOT recommended and is never selected \
         automatically: it writes your key in cleartext where any process or backup that \
         can read the file can read the key. Wayland will not do that on your behalf."
    ))
}

/// Warn ONCE that the operator explicitly opted in to unencrypted credentials.
///
/// Docker's model, and the only place in the codebase where it is acceptable:
/// the operator asked for it by name. `Once`-guarded because `open_store` runs
/// once per credential lookup.
fn warn_explicit_plaintext_backend(path: &Path) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!(
            "WARNING: [storage.credentials] backend = \"plaintext\" is configured. Secrets \
             are written UNENCRYPTED to {} (mode 0600). Anyone who can read that file, or a \
             backup of it, has your API keys. Remove the setting to use the OS keyring or \
             the encrypted vault instead.",
            path.display()
        );
    });
}

/// The [`CredentialsBackend::Auto`] store: the ordered credential ladder.
///
/// ```text
///   1. OS keyring          — present only when the WRITE probe succeeded.
///   2. Encrypted vault     — present only when unlock material was supplied.
///   3. legacy plaintext    — READ AND DELETE ONLY. Never written.
///   4. otherwise           — the write FAILS, with `no_secure_backend_for_write`.
/// ```
///
/// The plaintext tier exists solely so a host that already has a
/// `credentials.toml` from before this ladder keeps resolving its keys; it is
/// not a write target and there is no edge on which a `put` reaches it. Writing
/// cleartext requires `CredentialsBackend::Plaintext`, which the operator has to
/// name (see [`warn_explicit_plaintext_backend`]).
///
/// The probe is a HINT and the actual write is authoritative: probe→write is a
/// TOCTOU window by construction, so a keyring write that fails mid-session
/// descends the same ladder rather than taking a special branch.
///
/// RE-MIGRATION. A downgrade must not be permanent. When a read is satisfied by
/// a lower tier while a higher one is available, the value is promoted into the
/// higher tier and the lower copy removed — so a host whose keyring comes back
/// (a Windows service account that gains a logon session, a Linux box that
/// starts its Secret Service) heals on the next read instead of staying
/// downgraded forever.
///
/// The two upper tiers are trait objects (the shape
/// [`ConfidentialCredentialsStore`] already uses) rather than the concrete
/// backends. That is what makes the ORDERING provable: `KeyringCredentialsStore`
/// talks to the host's real credential store, so a ladder that could only be
/// built with one would have its keyring rungs untestable on precisely the
/// keyring-less hosts this ladder exists for — the tests would go quiet exactly
/// where the behaviour matters.
struct LadderCredentialsStore {
    /// `Some` iff [`keyring_available`] proved a write landed.
    keyring: Option<Box<dyn CredentialsStore>>,
    /// `Some` iff [`vault_unlock_material_present`] — otherwise opening it
    /// would block on an interactive passphrase prompt.
    vault: Option<Box<dyn CredentialsStore>>,
    /// Legacy cleartext file. Read and delete only.
    legacy: PlaintextCredentialsStore,
}

impl LadderCredentialsStore {
    fn new(
        keyring: Option<Box<dyn CredentialsStore>>,
        vault: Option<Box<dyn CredentialsStore>>,
        plaintext_path: PathBuf,
    ) -> Self {
        Self {
            keyring,
            vault,
            legacy: PlaintextCredentialsStore::new(plaintext_path),
        }
    }

    /// The highest tier that is mounted right now, or `None` for a ladder with
    /// no secure rung at all.
    fn top_tier(&self) -> Option<LadderTier> {
        if self.keyring.is_some() {
            Some(LadderTier::Keyring)
        } else if self.vault.is_some() {
            Some(LadderTier::Vault)
        } else {
            None
        }
    }

    fn tier(&self, tier: LadderTier) -> Option<&dyn CredentialsStore> {
        match tier {
            LadderTier::Keyring => self.keyring.as_deref(),
            LadderTier::Vault => self.vault.as_deref(),
            LadderTier::Legacy => Some(&self.legacy),
        }
    }

    /// Remove `key` from every tier strictly BELOW `above`, so the ladder holds
    /// at most one copy of a credential.
    ///
    /// This is what bounds the crash window in [`Self::promote`] and in
    /// [`CredentialsStore::put`]: a process killed after the new write and
    /// before this purge leaves a duplicate, and the very next successful write
    /// or promotion of that key removes it again. Without it a stale lower copy
    /// would persist indefinitely and could resurface — as the OLD value — if
    /// the upper tier later became unavailable.
    fn purge_below(&self, key: &str, above: LadderTier) -> Result<(), CredentialsError> {
        let mut first_error = None;
        for tier in [LadderTier::Keyring, LadderTier::Vault, LadderTier::Legacy] {
            if tier >= above {
                continue;
            }
            let removed = match tier {
                LadderTier::Legacy => self.delete_legacy(key),
                other => self.tier(other).map_or(Ok(()), |store| store.delete(key)),
            };
            if let Err(error) = removed {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Move `value` up to the highest available tier and drop the copies below.
    ///
    /// CRASH ORDER, and it is not negotiable: **write-new → verify-readback →
    /// delete-old.** A delete that precedes a verified write can lose the
    /// secret outright, and for a key that a caller re-creates on absence (the
    /// `load_or_create_*` shape) losing it does not fail — it silently MINTS a
    /// replacement and every artifact sealed under the old key stops opening.
    /// So the readback is a real re-read of the destination tier, not an
    /// inference from `put` returning `Ok`.
    ///
    /// Killed at any point, the state is recoverable and never empty:
    ///
    /// * before the write — unchanged; the next read retries.
    /// * after write, before verify — both tiers hold the SAME value; the next
    ///   read is served from the top and purges.
    /// * after verify, before purge — same, and `purge_below` is idempotent.
    ///
    /// Best-effort by design: a promotion that cannot complete leaves the lower
    /// copy in place and readable. A permanent-but-correct downgrade beats a
    /// lossy heal.
    fn promote(&self, key: &str, value: &str, found_in: LadderTier) {
        let Some(target) = self.top_tier() else {
            return;
        };
        if target <= found_in {
            return;
        }
        let Some(destination) = self.tier(target) else {
            return;
        };

        // 1. write-new.
        if let Err(error) = destination.put(key, value) {
            tracing::debug!(
                target: "wcore_credentials",
                key,
                error = %error,
                "could not promote a credential to a higher tier; the lower copy stands"
            );
            return;
        }
        // 2. verify-readback, from the destination itself.
        match destination.get(key) {
            Ok(Some(read_back)) if read_back == value => {}
            other => {
                tracing::warn!(
                    target: "wcore_credentials",
                    key,
                    readback_ok = other.is_ok(),
                    "a promoted credential did not read back from its new tier; leaving \
                     the lower-tier copy in place"
                );
                return;
            }
        }
        // 3. delete-old.
        match self.purge_below(key, target) {
            Ok(()) => tracing::info!(
                target: "wcore_credentials",
                key,
                "a secure credential tier became available again; the credential was \
                 promoted and the lower-tier copy removed"
            ),
            Err(error) => tracing::warn!(
                target: "wcore_credentials",
                key,
                error = %error,
                "credential promoted and verified, but the lower-tier copy could not be \
                 removed; it will be retried on the next read or write"
            ),
        }
    }
}

impl LadderCredentialsStore {
    /// Post-write hygiene for [`CredentialsStore::put`]: a value that has just
    /// been written to `above` makes any copy below it STALE, and a stale copy
    /// is worse than no copy — it is an OLD credential that a later tier
    /// regression would silently start serving. Best-effort and never fails the
    /// write, which has already succeeded.
    fn purge_stale_below(&self, key: &str, above: LadderTier) {
        if let Err(error) = self.purge_below(key, above) {
            tracing::warn!(
                target: "wcore_credentials",
                key,
                error = %error,
                "wrote the credential to a secure tier but could not remove a now-stale \
                 lower-tier copy; it will be retried on the next write"
            );
        }
    }

    /// Delete from the legacy cleartext file WITHOUT materializing it.
    ///
    /// `PlaintextCredentialsStore::delete` is a load-modify-save, and on a
    /// missing file the save leg CREATES an empty `credentials.toml`. On the
    /// ladder that would mean a delete (or a promotion off the legacy tier) on
    /// a host that has no cleartext file conjures one — a cleartext credentials
    /// file appearing as a side effect of the code whose job is to stop
    /// cleartext credentials files existing.
    fn delete_legacy(&self, key: &str) -> Result<(), CredentialsError> {
        if !self.legacy.path().exists() {
            return Ok(());
        }
        self.legacy.delete(key)
    }
}

/// Ordering is the ladder itself: `Keyring > Vault > Legacy`, so "is this tier
/// above the one the value was found in" is a comparison rather than a match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum LadderTier {
    Legacy,
    Vault,
    Keyring,
}

impl CredentialsStore for LadderCredentialsStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        // A read error in one tier must never hide a value held by a lower one.
        if let Some(keyring) = &self.keyring
            && let Ok(Some(value)) = keyring.get(key)
        {
            return Ok(Some(value));
        }
        if let Some(vault) = &self.vault
            && let Ok(Some(value)) = vault.get(key)
        {
            self.promote(key, &value, LadderTier::Vault);
            return Ok(Some(value));
        }
        let legacy = self.legacy.get(key)?;
        if let Some(value) = &legacy {
            self.promote(key, value, LadderTier::Legacy);
        }
        Ok(legacy)
    }

    /// One snapshot per tier rather than one per key. The vault re-derives its
    /// Argon2id key on every `load_secrets`, so the default `get_many` (a `get`
    /// per key) would put one KDF run per provider on the model picker's path.
    fn get_many(&self, keys: &[&str]) -> Result<Vec<Option<String>>, CredentialsError> {
        let mut out = vec![None; keys.len()];
        // Indices still unresolved. Each tier is queried for THIS list and then
        // the list shrinks, so every tier must be queried strictly after the one
        // above it has narrowed it — querying them up front and zipping later
        // mis-aligns the results against the reduced list.
        let mut missing: Vec<usize> = (0..keys.len()).collect();

        for tier in [LadderTier::Keyring, LadderTier::Vault] {
            if missing.is_empty() {
                return Ok(out);
            }
            let Some(store) = self.tier(tier) else {
                continue;
            };
            let subset: Vec<&str> = missing.iter().map(|index| keys[*index]).collect();
            // A tier that errors must not hide a value a lower tier holds.
            let Ok(values) = store.get_many(&subset) else {
                continue;
            };
            debug_assert_eq!(values.len(), missing.len());
            let mut still_missing = Vec::with_capacity(missing.len());
            for (slot, value) in missing.iter().copied().zip(values) {
                match value {
                    Some(value) => {
                        self.promote(keys[slot], &value, tier);
                        out[slot] = Some(value);
                    }
                    None => still_missing.push(slot),
                }
            }
            missing = still_missing;
        }

        if missing.is_empty() {
            return Ok(out);
        }
        let subset: Vec<&str> = missing.iter().map(|index| keys[*index]).collect();
        let values = self.legacy.get_many(&subset)?;
        debug_assert_eq!(values.len(), missing.len());
        for (slot, value) in missing.iter().copied().zip(values) {
            if let Some(value) = &value {
                self.promote(keys[slot], value, LadderTier::Legacy);
            }
            out[slot] = value;
        }
        Ok(out)
    }

    /// Descend the ladder and FAIL rather than downgrade. There is deliberately
    /// no plaintext arm here — that is the whole point of the type.
    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        if let Some(keyring) = &self.keyring {
            match keyring.put(key, value) {
                Ok(()) => {
                    self.purge_stale_below(key, LadderTier::Keyring);
                    return Ok(());
                }
                Err(error) => tracing::warn!(
                    target: "wcore_credentials",
                    error = %error,
                    "the OS keyring accepted a write probe but refused this write; \
                     descending to the encrypted vault"
                ),
            }
        }
        if let Some(vault) = &self.vault {
            match vault.put(key, value) {
                Ok(()) => {
                    self.purge_stale_below(key, LadderTier::Vault);
                    return Ok(());
                }
                Err(error) => tracing::warn!(
                    target: "wcore_credentials",
                    error = %error,
                    "the encrypted vault refused this write"
                ),
            }
        }
        Err(no_secure_backend_for_write(key))
    }

    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        // Remove from EVERY tier, including the read-only legacy one, so a
        // deleted key cannot resurface from below.
        if let Some(keyring) = &self.keyring {
            let _ = keyring.delete(key);
        }
        if let Some(vault) = &self.vault {
            let _ = vault.delete(key);
        }
        self.delete_legacy(key)
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
    /// Process-scoped passphrase authority. Held only in memory, redacted from
    /// debug output, shared across fresh store instances, and zeroized when the
    /// process authority is dropped.
    passphrase: std::sync::Arc<VaultPassphraseAuthority>,
    /// KDF params (salt + tuning knobs). Persisted to `key_params_path`.
    params: encrypted_file::KdfParams,
}

/// Process-scoped vault passphrase authority.
///
/// The secret is deliberately private and has a redacted `Debug`
/// implementation. `Arc` lets every encrypted-store instance in one process
/// share the same zeroizing allocation rather than cloning plaintext.
struct VaultPassphraseAuthority {
    secret: zeroize::Zeroizing<String>,
}

impl VaultPassphraseAuthority {
    fn new(secret: String) -> Self {
        Self {
            secret: zeroize::Zeroizing::new(secret),
        }
    }

    fn expose(&self) -> &str {
        self.secret.as_str()
    }
}

impl std::fmt::Debug for VaultPassphraseAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VaultPassphraseAuthority")
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct PassphraseFdIdentity {
    raw_fd: std::os::unix::io::RawFd,
    device: u64,
    inode: u64,
}

#[cfg(unix)]
type ProcessPassphraseAuthority = Option<(
    PassphraseFdIdentity,
    std::sync::Arc<VaultPassphraseAuthority>,
)>;

#[cfg(unix)]
fn passphrase_from_fd(
    fd: std::os::unix::io::RawFd,
) -> Result<std::sync::Arc<VaultPassphraseAuthority>, CredentialsError> {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::io::FromRawFd;

    validate_readable_fd(fd)?;

    // SAFETY: `validate_readable_fd` confirmed that this inherited descriptor
    // is open and readable. `ManuallyDrop` keeps this borrowed wrapper from
    // closing the descriptor, including on error paths.
    let mut file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
    let metadata = file.metadata().map_err(|error| {
        CredentialsError::BackendUnavailable(format!("passphrase fd {fd} metadata: {error}"))
    })?;
    let identity = PassphraseFdIdentity {
        raw_fd: fd,
        device: metadata.dev(),
        inode: metadata.ino(),
    };

    // A passphrase pipe is intentionally one-shot. Fresh recovery-store
    // instances in the same process must therefore share the authority created
    // by the first read. Holding the mutex across the initial read also prevents
    // concurrent openers from racing to consume the same descriptor.
    static AUTHORITY: std::sync::OnceLock<parking_lot::Mutex<ProcessPassphraseAuthority>> =
        std::sync::OnceLock::new();
    let mut authority = AUTHORITY
        .get_or_init(|| parking_lot::Mutex::new(None))
        .lock();
    if let Some((cached_identity, cached_authority)) = authority.as_ref() {
        if cached_identity == &identity {
            return Ok(std::sync::Arc::clone(cached_authority));
        }
        return Err(CredentialsError::BackendUnavailable(
            "WAYLAND_VAULT_PASSPHRASE_FD changed after the process vault authority was initialized"
                .to_string(),
        ));
    }

    let mut secret = zeroize::Zeroizing::new(String::new());
    file.read_to_string(&mut secret).map_err(|error| {
        CredentialsError::BackendUnavailable(format!("passphrase fd {fd}: {error}"))
    })?;
    while secret.ends_with('\n') {
        secret.pop();
    }
    let initialized = std::sync::Arc::new(VaultPassphraseAuthority { secret });
    *authority = Some((identity, std::sync::Arc::clone(&initialized)));
    Ok(initialized)
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
    fn read_passphrase() -> Result<std::sync::Arc<VaultPassphraseAuthority>, CredentialsError> {
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
            return passphrase_from_fd(fd);
        }

        // F-055 path 2: env var (legacy, warned).
        if let Ok(pp) = std::env::var("WAYLAND_VAULT_PASSPHRASE") {
            tracing::warn!(
                target: "wcore_credentials",
                "WAYLAND_VAULT_PASSPHRASE provided via env var — visible via \
                 /proc/<pid>/environ on Linux. Set WAYLAND_VAULT_PASSPHRASE_FD \
                 to a file descriptor number to avoid this leak."
            );
            return Ok(std::sync::Arc::new(VaultPassphraseAuthority::new(pp)));
        }

        // F-055 path 3: interactive prompt.
        let pp = rpassword::prompt_password("vault passphrase: ")
            .map_err(|e| CredentialsError::BackendUnavailable(format!("rpassword: {e}")))?;
        Ok(std::sync::Arc::new(VaultPassphraseAuthority::new(pp)))
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
            // Check perms BEFORE prompting for a passphrase: a vault that will
            // be refused must not first extract a secret from the operator.
            refuse_if_world_readable(&self.cipher_path)?;
            refuse_if_world_readable(&self.key_params_path)?;
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
                let _pt =
                    encrypted_file::decrypt(&blob, passphrase.expose(), &params).map_err(|e| {
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
        // Re-checked on every load, not only at unlock: the unlock cache is
        // process-lifetime, so a `chmod 644` applied by anything else while the
        // process runs would otherwise never be noticed.
        refuse_if_world_readable(&self.cipher_path)?;
        let blob = std::fs::read(&self.cipher_path)?;
        let pt = encrypted_file::decrypt(&blob, vault.passphrase.expose(), &vault.params).map_err(
            |e| CredentialsError::BackendUnavailable(format!("vault decrypt failed: {e}")),
        )?;
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
        let key = encrypted_file::derive_key(vault.passphrase.expose(), &vault.params)
            .map_err(|e| CredentialsError::BackendUnavailable(format!("derive_key: {e}")))?;
        let blob = encrypted_file::encrypt_with_key(serialized.as_bytes(), &key).map_err(|e| {
            CredentialsError::BackendUnavailable(format!("vault encrypt failed: {e}"))
        })?;

        // Ensure both files' parent directories exist AND are 0700 before any
        // ciphertext lands in them. `atomic_write` writes a sibling temp file,
        // so a loose parent dir is a window on the temp file too.
        if let Some(parent) = self.cipher_path.parent() {
            secure_credential_dir(parent)?;
        }
        if let Some(parent) = self.key_params_path.parent() {
            secure_credential_dir(parent)?;
        }
        // atomic_write → chmod AFTER, in that order: the tempfile round trip
        // creates the destination inode itself, and the process umask can strip
        // bits during that create. Re-applying 0600 afterwards is the same
        // discipline kimi-code applies at storage.ts:106-108.
        crate::atomic_write(&self.cipher_path, &blob)?;
        secure_credential_file(&self.cipher_path)?;
        encrypted_file::save_key_params(&vault.params, &self.key_params_path)
            .map_err(|e| CredentialsError::BackendUnavailable(format!("save_key_params: {e}")))?;
        secure_credential_file(&self.key_params_path)?;
        Ok(())
    }

    /// Import many secrets in a SINGLE atomic vault write (#183 migration).
    ///
    /// One `load → merge → save_secrets` means the whole batch lands via ONE
    /// `atomic_write` of the ciphertext, so an interrupted migration can never
    /// leave a partially-populated `.enc` (the per-key `put` loop it replaces
    /// could). Existing keys are PRESERVED (`or_insert`) — a pre-existing vault
    /// value is authoritative and never clobbered by an incoming plaintext one.
    fn import_secrets(&self, entries: &[(String, String)]) -> Result<(), CredentialsError> {
        let vault = self.unlock()?;
        let mut table = self.load_secrets(&vault)?;
        let secrets = table
            .entry("secrets".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !matches!(secrets, toml::Value::Table(_)) {
            *secrets = toml::Value::Table(toml::Table::new());
        }
        let toml::Value::Table(secrets_table) = secrets else {
            unreachable!("just normalized to Table");
        };
        for (k, v) in entries {
            secrets_table
                .entry(k.clone())
                .or_insert_with(|| toml::Value::String(v.clone()));
        }
        self.save_secrets(&vault, &table)
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

    fn get_many(&self, keys: &[&str]) -> Result<Vec<Option<String>>, CredentialsError> {
        let vault = self.unlock()?;
        let table = self.load_secrets(&vault)?;
        let secrets = match table.get("secrets") {
            Some(toml::Value::Table(table)) => Some(table),
            _ => None,
        };
        Ok(keys
            .iter()
            .map(|key| {
                secrets
                    .and_then(|table| table.get(*key))
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
            })
            .collect())
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

/// Warn ONCE, to stderr, that the ladder has no secure rung on this host, so
/// reads of any existing `credentials.toml` still work but WRITES will be
/// refused.
///
/// This replaces the old "storing credentials as plaintext-0600" notice, which
/// described the silent downgrade that has been removed. `Once`-guarded because
/// `open_store` is called repeatedly per run (once per provider key lookup) and
/// an unguarded warning would spam stderr.
fn warn_no_secure_credential_tier(path: &Path) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!(
            "warning: no secure credential backend is available here — the OS keyring is \
             not writable and the encrypted vault is locked. Existing secrets in {} remain \
             readable, but saving a new credential will be REFUSED rather than written in \
             cleartext. To store credentials, set WAYLAND_VAULT_PASSPHRASE_FD (a passphrase \
             file descriptor — preferred) or WAYLAND_VAULT_PASSPHRASE (env var, visible via \
             /proc/<pid>/environ) to unlock the encrypted vault.",
            path.display()
        );
    });
}

/// Timing contract for an [`ExclusiveFileLock`]: how long a lockfile may sit
/// untouched before a waiter treats its holder as crashed, how long a waiter
/// keeps trying, and whether the holder refreshes the lockfile while it works.
///
/// The two durations are not independent. `wait_ceiling` must sit ABOVE
/// `stale_after`, or a crashed holder becomes a hard refusal for every waiter
/// until the crash ages out: the waiter would give up before it ever reached
/// the steal. Sitting past `stale_after` means the steal is always reached and
/// the waiter proceeds holding the lock instead.
#[derive(Debug, Clone, Copy)]
pub struct LockPolicy {
    stale_after: std::time::Duration,
    wait_ceiling: std::time::Duration,
    /// When set, the holder spawns a dedicated OS thread that re-stamps the
    /// lockfile at this interval so its mtime tracks liveness rather than
    /// acquisition time. See [`LockPolicy::with_heartbeat`].
    heartbeat: Option<std::time::Duration>,
}

impl LockPolicy {
    /// A holder that has not finished in a minute is treated as crashed. Long
    /// enough that no healthy holder is ever stolen from (both the migration and
    /// a spanned keyring write are sub-second), short enough that a crash costs
    /// at most that long.
    /// The OAuth refresh holds for tens of seconds and does NOT raise this:
    /// it carries a heartbeat, so its lockfile mtime tracks liveness and
    /// staleness is judged against the beat rather than the hold.
    const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(60);
    /// Poll interval while waiting. The uncontended path never sleeps at all —
    /// the first `create_new` succeeds.
    const POLL: std::time::Duration = std::time::Duration::from_millis(50);

    /// Migration: give up quickly. The plaintext store keeps serving in the
    /// meantime, so deferring to the next open loses nothing.
    pub const MIGRATION: Self = Self {
        stale_after: Self::STALE_AFTER,
        wait_ceiling: std::time::Duration::from_secs(10),
        heartbeat: None,
    };

    /// Credential writes: out-wait the staleness threshold. A ceiling BELOW
    /// `stale_after` would turn a crashed holder into a hard refusal for every
    /// writer until the crash aged out; sitting just past it means the stale
    /// steal is always reached and the write proceeds instead.
    pub const CREDENTIAL_WRITE: Self = Self {
        stale_after: Self::STALE_AFTER,
        wait_ceiling: std::time::Duration::from_secs(65),
        heartbeat: None,
    };

    /// A policy whose durations a caller derives from its own hold time. The
    /// caller owns the derivation because only it knows the maximum hold — see
    /// `wcore_agent::oauth::refresh_lock` for a worked example.
    pub const fn new(stale_after: std::time::Duration, wait_ceiling: std::time::Duration) -> Self {
        Self {
            stale_after,
            wait_ceiling,
            heartbeat: None,
        }
    }

    /// Re-stamp the lockfile every `every` while the lock is held.
    ///
    /// Without a heartbeat, `stale_after` has to exceed the maximum hold, so a
    /// hold measured in tens of seconds forces a staleness measured in minutes
    /// — and a crashed holder wedges every waiter for that long. With one, the
    /// lockfile's mtime tracks LIVENESS rather than acquisition time, so
    /// `stale_after` is sized against the heartbeat interval instead and a
    /// crash is detected in seconds regardless of how long the work takes.
    ///
    /// The heartbeat runs on a dedicated OS thread, never on a task executor:
    /// a heartbeat that can be starved by the same executor the holder is
    /// blocking would let a LIVE holder be judged stale and stolen from, which
    /// is the exact failure the lock exists to prevent.
    pub const fn with_heartbeat(mut self, every: std::time::Duration) -> Self {
        self.heartbeat = Some(every);
        self
    }
}

/// Exclusive, self-recovering cross-process lock: a create-`O_EXCL` lockfile,
/// which is atomic on every platform.
/// Three callers, one mechanism:
/// * the one-shot plaintext→vault migration ([`migrate_plaintext_into_vault`]),
///   because two migrators that both saw no `.enc`/`.kdf` would generate
///   DIFFERENT random salts and interleave their two-file writes into a
///   mismatched (undecryptable) vault;
/// * every spanned keyring write ([`chunked_put`] / [`chunked_delete`]),
///   because the target generation is chosen by READING the live manifest, and
///   two writers that read the same manifest would choose the same target and
///   interleave their parts into it;
/// * the OAuth refresh critical section (`wcore-agent`), because a rotating
///   refresh token is single-use and two processes that both POST it burn the
///   whole authorization grant.
pub struct ExclusiveFileLock {
    path: PathBuf,
    /// Unique per-acquisition token stamped into the lockfile, so `drop` only
    /// removes a lockfile that is STILL ours — never one a concurrent stealer
    /// created after our lock was (wrongly) judged stale.
    nonce: String,
    heartbeat: Option<Heartbeat>,
}

impl std::fmt::Debug for ExclusiveFileLock {
    /// Path only. The nonce is deliberately omitted: it is the token that
    /// decides whether `drop` may remove the lockfile, and a `{:?}` in a log
    /// line is not where that belongs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExclusiveFileLock")
            .field("path", &self.path)
            .field("heartbeat", &self.heartbeat.is_some())
            .finish()
    }
}

impl ExclusiveFileLock {
    /// `label` names the lock in the busy error, so a caller can tell a wedged
    /// migration from a wedged refresh.
    pub fn acquire(
        path: PathBuf,
        policy: LockPolicy,
        label: &str,
    ) -> Result<Self, CredentialsError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        // Unique per acquisition (pid + a process-local sequence) so different
        // processes/acquisitions never collide.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = format!(
            "{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let deadline = std::time::Instant::now() + policy.wait_ceiling;
        loop {
            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    // Best-effort stamp. Even if the write fails the lock (the
                    // file's existence) still holds; we simply won't nonce-match
                    // on drop and will conservatively leave the file.
                    let _ = f.write_all(nonce.as_bytes());
                    drop(f);
                    // A caller that asked for a heartbeat sized `stale_after`
                    // against it. If we cannot start one we must NOT hand back
                    // a lock that claims it — release the file we just created
                    // (it is ours, the nonce matches) and fail honestly.
                    let heartbeat = match policy.heartbeat {
                        None => None,
                        Some(every) => match Heartbeat::start(path.clone(), nonce.clone(), every) {
                            Ok(started) => Some(started),
                            Err(error) => {
                                let _ = std::fs::remove_file(&path);
                                return Err(CredentialsError::BackendUnavailable(format!(
                                    "the {label} lock at {} could not start its liveness \
                                         heartbeat ({error}); refusing to hold a lock that \
                                         reports itself heartbeated while nothing refreshes \
                                         it, because a waiter would then judge this live \
                                         holder stale and steal the lock",
                                    path.display()
                                )));
                            }
                        },
                    };
                    return Ok(Self {
                        path,
                        nonce,
                        heartbeat,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if Self::is_stale(&path, policy.stale_after) {
                        // Crashed holder — steal it and re-race the create_new
                        // (whoever wins the atomic create proceeds).
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(CredentialsError::BackendUnavailable(format!(
                            "the {label} lock at {} is held by another process and did not \
                             free within {:?}",
                            path.display(),
                            policy.wait_ceiling
                        )));
                    }
                    std::thread::sleep(LockPolicy::POLL);
                }
                Err(e) => return Err(CredentialsError::Io(e)),
            }
        }
    }

    /// A lockfile untouched for longer than `stale_after` is treated as
    /// abandoned by a crashed holder. Any error reading the mtime (clock skew,
    /// missing) → not stale.
    fn is_stale(path: &Path, stale_after: std::time::Duration) -> bool {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().map(|age| age > stale_after))
            .map(|r| r.unwrap_or(false))
            .unwrap_or(false)
    }
}

impl Drop for ExclusiveFileLock {
    fn drop(&mut self) {
        // Stop the heartbeat FIRST, so it cannot re-stamp (and so resurrect)
        // a lockfile we are about to release.
        self.heartbeat.take();
        // Remove ONLY if the lockfile still carries our nonce. If a stale-steal
        // replaced it with another holder's token, deleting it would let a third
        // holder in concurrently — so leave it for the current owner.
        if let Ok(contents) = std::fs::read_to_string(&self.path)
            && contents == self.nonce
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Dedicated OS thread that re-stamps a held lockfile so its mtime tracks the
/// holder's liveness. Stopping is immediate (condvar), not a sleep the drop has
/// to wait out.
struct Heartbeat {
    stop: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Heartbeat {
    /// Fallible ON PURPOSE.
    ///
    /// This used to end in `.spawn(...).ok()`, which turned an OS refusal to
    /// create the thread into `handle: None` — a lock that reports itself
    /// heartbeated to its holder while nothing is refreshing its mtime. The
    /// holder then believes it is protected, the next waiter correctly judges
    /// the lockfile stale, and a LIVE credential lock is stolen with no error
    /// raised anywhere. A protection that can silently not exist is worse than
    /// one that is absent, because callers size `stale_after` against it.
    ///
    /// Thread creation fails under exactly the conditions that also make the
    /// heartbeat matter — resource pressure — so this is not a theoretical arm.
    fn start(path: PathBuf, nonce: String, every: std::time::Duration) -> std::io::Result<Self> {
        let stop = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let signal = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("wayland-lock-heartbeat".into())
            .spawn(move || {
                let (lock, cvar) = &*signal;
                loop {
                    let Ok(guard) = lock.lock() else { return };
                    let Ok((guard, _)) = cvar.wait_timeout(guard, every) else {
                        return;
                    };
                    if *guard {
                        return;
                    }
                    drop(guard);
                    // Re-stamp only while the file is still OURS. A lockfile a
                    // stealer already replaced belongs to them; refreshing its
                    // mtime would hide their crash from the next waiter.
                    if std::fs::read_to_string(&path).ok().as_deref() == Some(nonce.as_str()) {
                        let _ = std::fs::write(&path, nonce.as_bytes());
                    }
                }
            })?;
        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.stop;
        if let Ok(mut stopped) = lock.lock() {
            *stopped = true;
        }
        cvar.notify_all();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// #183 — one-shot plaintext→encrypted-vault migration.
///
/// The encrypted store only ever reads `credentials.enc`; it never consulted an
/// existing plaintext `credentials.toml`. So the first time a profile that had
/// stored secrets in plaintext gains vault unlock material, those secrets would
/// silently vanish (apparent credential loss — the reason the desktop
/// wayland#710 fix gated existing-plaintext profiles to stay plaintext until
/// this shipped). This imports them.
///
/// Crash-atomic and concurrency-safe (both were review BLOCKERs):
///   * The guard is driven by PLAINTEXT PRESENCE, not `.enc` absence. The
///     plaintext file is removed only AFTER a full verified import, so an
///     interrupted run is simply retried on the next open — a partial `.enc` is
///     never trusted as the source of truth.
///   * Import is a SINGLE atomic vault write (`import_secrets`), so no partial
///     `.enc` exists mid-run. Existing vault keys are preserved, so the import
///     is idempotent — re-running after an interruption converges.
///   * A `.enc` with no `.kdf` can only be a crash artifact of an interrupted
///     write (a healthy vault has both); it is permanently undecryptable, and
///     the plaintext still holds the truth, so it is discarded and rebuilt.
///   * The whole sequence runs under an [`ExclusiveFileLock`] so two concurrent
///     openers cannot corrupt the vault with mismatched salts.
///
/// Only runs when non-interactive unlock material is present, so `open_store`
/// never blocks on an interactive passphrase prompt. On failure it returns the
/// error: the isolated-profile `Auto` path then keeps serving plaintext (secrets
/// stay resolvable); an operator who explicitly chose `EncryptedFile` sees it.
fn migrate_plaintext_into_vault(
    plaintext_path: &Path,
    store: &EncryptedFileCredentialsStore,
) -> Result<(), CredentialsError> {
    // Cheap guards BEFORE any unlock (so a no-op never prompts): need unlock
    // material and a plaintext source to migrate at all.
    if !vault_unlock_material_present() || !plaintext_path.exists() {
        return Ok(());
    }
    let dir = plaintext_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // Serialize against a concurrent opener on the same home for the whole
    // import → verify → delete window.
    let _lock = ExclusiveFileLock::acquire(
        dir.join(".credentials.migrate.lock"),
        LockPolicy::MIGRATION,
        "credentials migration",
    )?;

    // Re-read UNDER the lock — a migrator we waited on may have already
    // finished and removed the plaintext file.
    let plaintext = PlaintextCredentialsStore::new(plaintext_path.to_path_buf());
    let (entries, raw_count) = plaintext.load_all()?;
    if entries.is_empty() {
        return Ok(());
    }

    // A ciphertext whose KDF-params file is missing OR unparseable is a crash
    // artifact from an interrupted write (a healthy vault always has both, and a
    // valid params file) — permanently undecryptable, and the plaintext still
    // holds the authoritative secrets, so discard it and rebuild. (A `.enc` with
    // a VALID `.kdf` that simply won't decrypt — e.g. a real vault under a
    // different passphrase — is left alone: `import_secrets` surfaces the unlock
    // error and we fall back to plaintext rather than destroying it.)
    if store.cipher_path.exists() {
        let kdf_unusable = !store.key_params_path.exists()
            || encrypted_file::load_key_params(&store.key_params_path).is_err();
        if kdf_unusable {
            let _ = std::fs::remove_file(&store.cipher_path);
            let _ = std::fs::remove_file(&store.key_params_path);
        }
    }

    // ONE atomic vault write, then verify every plaintext key resolves before
    // touching the original.
    store.import_secrets(&entries)?;
    for (k, _v) in &entries {
        if store.get(k)?.is_none() {
            return Err(CredentialsError::BackendUnavailable(format!(
                "vault migration readback missing key '{k}'"
            )));
        }
    }

    // Remove the plaintext original only if EVERY entry migrated. If some
    // non-string (hand-edited, non-credential) values were dropped by
    // `load_all`, keep the file so that data is not destroyed.
    if entries.len() == raw_count {
        if let Err(e) = std::fs::remove_file(plaintext_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            // The vault holds every secret; a lingering plaintext file is
            // retried (and re-removed) on the next open. Log, don't fail.
            tracing::warn!(
                target: "wcore_credentials",
                error = %e,
                "vault migration succeeded but could not remove the plaintext file; \
                 it will be retried on the next open"
            );
        }
    } else {
        tracing::warn!(
            target: "wcore_credentials",
            skipped = raw_count - entries.len(),
            "vault migration imported the string secrets but kept the plaintext file \
             because it also holds non-string entries"
        );
    }
    tracing::info!(
        target: "wcore_credentials",
        count = entries.len(),
        "migrated existing plaintext credentials into the encrypted vault"
    );
    Ok(())
}

/// Build the ordered ladder — keyring, then encrypted vault, then a refusal.
/// Cleartext is never a rung; the legacy plaintext file is mounted read-only so
/// pre-existing keys stay resolvable. (F16, P1)
///
/// Shared by the [`CredentialsBackend::Auto`] arm of [`open_store`] and by
/// [`open_secure_ladder_store`], so the two cannot drift in which rungs they
/// mount or in the order they try them.
fn build_ladder(cfg: &CredentialsStorageConfig, plaintext_path: &Path) -> LadderCredentialsStore {
    // Isolated-profile homes (WAYLAND_HOME set) must NOT use the OS
    // keyring: the keyring service is a process-global constant
    // ("wayland-core") that bleeds secrets across every profile on the
    // host (C4 / D1). Such a profile's top rung is the in-home vault.
    let isolated = std::env::var_os("WAYLAND_HOME").is_some();

    let keyring: Option<Box<dyn CredentialsStore>> = if isolated {
        None
    } else {
        let service = cfg
            .service_name
            .clone()
            .unwrap_or_else(|| "wayland-core".to_string());
        keyring_available(&service)
            .then(|| Box::new(KeyringCredentialsStore::new(service)) as Box<dyn CredentialsStore>)
    };

    let vault: Option<Box<dyn CredentialsStore>> = if vault_unlock_material_present() {
        // An operator who named explicit vault paths gets THOSE, so the ladder
        // and an explicit `backend = "encrypted_file"` never open two different
        // vaults for the same profile.
        let (cipher_path, key_params_path) = match &cfg.backend {
            CredentialsBackend::EncryptedFile {
                cipher_path,
                key_params_path,
            } => (cipher_path.clone(), key_params_path.clone()),
            _ => default_vault_paths(plaintext_path),
        };
        let store = EncryptedFileCredentialsStore::new(cipher_path, key_params_path);
        // #183: import any pre-existing plaintext secrets into the
        // vault once. On failure the legacy tier keeps serving them, so
        // no secret is ever lost — but the vault stays mounted, because
        // dropping it would turn a migration hiccup into a refusal to
        // write at all.
        if let Err(error) = migrate_plaintext_into_vault(plaintext_path, &store) {
            tracing::warn!(
                target: "wcore_credentials",
                error = %error,
                "plaintext→vault migration failed; the existing plaintext file \
                 stays readable and the vault remains the write target"
            );
        }
        Some(Box::new(store) as Box<dyn CredentialsStore>)
    } else {
        None
    };

    if keyring.is_none() && vault.is_none() {
        warn_no_secure_credential_tier(plaintext_path);
    }

    LadderCredentialsStore::new(keyring, vault, plaintext_path.to_path_buf())
}

/// The keyring → encrypted-vault → REFUSE ladder, built regardless of
/// `cfg.backend`.
///
/// [`open_store`] honours an explicit `backend = "plaintext"` opt-out, because
/// an operator may legitimately choose that for an ordinary API key. Material
/// that must NEVER be written in cleartext — OAuth token sets, whose refresh
/// token is a long-lived bearer credential for the user's account — opens the
/// ladder through this entry point instead, so that opt-out cannot downgrade
/// it. When no secure rung is mounted, `put` refuses; it never falls through to
/// a cleartext write.
///
/// Reads still descend to the legacy `credentials.toml`, so credentials written
/// before a secure tier existed stay resolvable and are promoted up on the next
/// read.
#[must_use]
pub fn open_secure_ladder_store(
    cfg: &CredentialsStorageConfig,
    plaintext_path: &Path,
) -> Box<dyn CredentialsStore> {
    Box::new(build_ladder(cfg, plaintext_path))
}

/// The credentials-store key holding `provider`'s OAuth token set.
///
/// Defined in the crate that owns the store rather than in the writer, so the
/// writer (`wcore_agent::oauth::OAuthStorage`) and the connectivity readers
/// ([`crate::config::provider_connected`] and the xAI key resolver) name the
/// same string by construction. A reader that re-spells this is a reader that
/// reports a signed-in user as signed out.
///
/// `provider` is sanitized on the same rule as `OAuthStorage::path_for` so a
/// hostile provider name cannot forge another key's namespace.
#[must_use]
pub fn oauth_tokens_key(provider: &str) -> String {
    let safe = provider.replace(['/', '\\', '\0', '.'], "_");
    format!("oauth.{safe}.tokens")
}

/// Factory selecting the configured backend.
pub fn open_store(
    cfg: &CredentialsStorageConfig,
    plaintext_path: &Path,
) -> Result<Box<dyn CredentialsStore>, CredentialsError> {
    match &cfg.backend {
        CredentialsBackend::Auto => Ok(Box::new(build_ladder(cfg, plaintext_path))),
        CredentialsBackend::Plaintext => {
            warn_explicit_plaintext_backend(plaintext_path);
            Ok(Box::new(PlaintextCredentialsStore::new(
                plaintext_path.to_path_buf(),
            )))
        }
        CredentialsBackend::Keyring => {
            let base_service = cfg
                .service_name
                .clone()
                .unwrap_or_else(|| "wayland-core".to_string());
            let service = if std::env::var_os("WAYLAND_HOME").is_some() {
                profile_keyring_service(&base_service, plaintext_path)?
            } else {
                base_service
            };
            Ok(Box::new(KeyringCredentialsStore::new(service)))
        }
        // S11 (v0.6.3): EncryptedFile backend is wired here. Crypto primitives
        // are defined in the `encrypted_file` submodule; the store glues them
        // to a TOML-encoded secrets table, an unlock-passphrase resolver
        // (env var or interactive prompt), and atomic re-encrypt on put.
        CredentialsBackend::EncryptedFile {
            cipher_path,
            key_params_path,
        } => {
            let store =
                EncryptedFileCredentialsStore::new(cipher_path.clone(), key_params_path.clone());
            // #183: import pre-existing plaintext secrets once. The operator
            // explicitly chose encryption here, so surface any migration error
            // rather than silently downgrading to plaintext.
            migrate_plaintext_into_vault(plaintext_path, &store)?;
            Ok(Box::new(store))
        }
    }
}

/// The confidential-backend decision inputs for one profile: the absolute
/// credentials path and the [`ConfidentialBackendMode`] its configured backend
/// implies.
///
/// Extracted so [`open_confidential_store`] and [`confidential_backend_available`]
/// derive the mode from ONE piece of code. They answer the same question and
/// must never be able to disagree about which backends are candidates.
///
/// Pure: reads config and the environment, writes nothing.
fn confidential_backend_plan(
    cfg: &CredentialsStorageConfig,
    plaintext_path: &Path,
) -> Result<(PathBuf, ConfidentialBackendMode), CredentialsError> {
    if !cfg.backend.supports_confidential_material() {
        return Err(CredentialsError::BackendUnavailable(
            "plaintext credentials are not permitted for confidential material".to_string(),
        ));
    }

    let credentials_path = absolute_confidential_path(plaintext_path)?;
    let isolated_home = std::env::var_os("WAYLAND_HOME").is_some();
    let service = confidential_keyring_service(cfg, &credentials_path, isolated_home)?;
    let keyring = ConfidentialBackendSelection::Keyring { service };
    let (default_cipher_path, default_key_params_path) = default_vault_paths(&credentials_path);
    let vault = ConfidentialBackendSelection::EncryptedFile {
        cipher_path: absolute_confidential_path(&default_cipher_path)?,
        key_params_path: absolute_confidential_path(&default_key_params_path)?,
    };
    let mode = match &cfg.backend {
        CredentialsBackend::Auto => ConfidentialBackendMode::Auto { keyring, vault },
        CredentialsBackend::Keyring => ConfidentialBackendMode::Explicit(keyring),
        CredentialsBackend::EncryptedFile {
            cipher_path,
            key_params_path,
        } => ConfidentialBackendMode::Explicit(ConfidentialBackendSelection::EncryptedFile {
            cipher_path: absolute_confidential_path(cipher_path)?,
            key_params_path: absolute_confidential_path(key_params_path)?,
        }),
        CredentialsBackend::Plaintext => unreachable!("handled above"),
    };
    Ok((credentials_path, mode))
}

/// Read-only twin of [`open_confidential_store`]: can this profile reach a
/// confidential-capable backend *right now*?
///
/// This exists so a caller can decide at STARTUP whether a capability that
/// requires confidential storage is supportable, instead of promising it and
/// discovering the answer on the user's first turn. It creates no directory,
/// takes no lock, pins no backend marker and migrates nothing, so it is safe
/// to call on every launch and cannot itself change the answer.
///
/// It routes through the same [`confidential_backend_plan`] and
/// [`select_confidential_backend`] the opener uses, so the two cannot drift.
///
/// Returns `false` for `backend = "plaintext"`. That is correct but is NOT the
/// headless case: a caller that must tell "the operator configured something
/// that cannot work" apart from "this host has no secure store at all" has to
/// consult [`CredentialsBackend::supports_confidential_material`] first.
#[must_use]
pub fn confidential_backend_available(
    cfg: &CredentialsStorageConfig,
    plaintext_path: &Path,
) -> bool {
    let Ok((credentials_path, mode)) = confidential_backend_plan(cfg, plaintext_path) else {
        return false;
    };
    let marker_path = confidential_backend_marker_path(&credentials_path);
    // A marker that cannot be read is treated as absent: this is a query, and
    // the opener holds the lock that makes marker reads authoritative.
    let pinned = load_pinned_confidential_backend(&marker_path).unwrap_or_default();
    select_confidential_backend(
        pinned.as_ref(),
        &mode,
        &keyring_available,
        vault_unlock_material_present(),
    )
    .is_ok()
}

/// Open a credentials store for material that must never be written in
/// plaintext.
///
/// Unlike [`open_store`], `Auto` is fail-closed: it selects the OS keyring when
/// it is usable, using a stable profile-namespaced service for isolated
/// `WAYLAND_HOME` profiles. Otherwise it selects the encrypted-file vault only
/// when unlock material is available.
/// It never constructs [`PlaintextCredentialsStore`] or
/// [`FallbackCredentialsStore`].
pub fn open_confidential_store(
    cfg: &CredentialsStorageConfig,
    plaintext_path: &Path,
) -> Result<ConfidentialCredentialsStore, CredentialsError> {
    open_confidential_store_with_availability(cfg, plaintext_path, &keyring_available)
}

/// [`open_confidential_store`] with keyring availability injected.
///
/// Mirrors the seam [`resolve_confidential_backend_with_availability`] already
/// exposes, so the vault branch can be exercised on a host whose OS keyring
/// genuinely works (macOS, a logged-in Windows desktop) without writing into
/// the operator's real credential store. Without this, a "the vault is used"
/// test only holds on headless Linux and silently asserts nothing elsewhere.
fn open_confidential_store_with_availability(
    cfg: &CredentialsStorageConfig,
    plaintext_path: &Path,
    keyring_is_available: &impl Fn(&str) -> bool,
) -> Result<ConfidentialCredentialsStore, CredentialsError> {
    let (credentials_path, mode) = confidential_backend_plan(cfg, plaintext_path)?;
    let selected = resolve_confidential_backend_with_availability(
        &mode,
        &credentials_path,
        keyring_is_available,
        vault_unlock_material_present(),
    )?;

    let pin = Some(ConfidentialPinConfirmation {
        marker_path: confidential_backend_marker_path(&credentials_path),
        selection: selected.clone(),
        recorded: std::sync::atomic::AtomicBool::new(false),
    });

    match selected {
        ConfidentialBackendSelection::Keyring { service } => {
            let key_creation_lock_path =
                credentials_path.with_file_name(".credentials.confidential-key.lock");
            Ok(ConfidentialCredentialsStore::new(
                Box::new(KeyringCredentialsStore::new(service)),
                key_creation_lock_path,
                pin,
            ))
        }
        ConfidentialBackendSelection::EncryptedFile {
            cipher_path,
            key_params_path,
        } => {
            let key_creation_lock_path = cipher_path.with_extension("confidential-key.lock");
            let store = EncryptedFileCredentialsStore::new(cipher_path, key_params_path);
            migrate_plaintext_into_vault(&credentials_path, &store)?;
            Ok(ConfidentialCredentialsStore::new(
                Box::new(store),
                key_creation_lock_path,
                pin,
            ))
        }
    }
}

/// The credentials-store key under which durable session recovery persists its
/// prepared-request sealing key.
///
/// Lives here, in the crate that owns the store, rather than in the consumer
/// (`wcore_agent::recovery_confidential`) so that the WRITER and the DELETER
/// name the same string by construction. A per-profile key whose deleter has to
/// re-spell its identifier is a deleter that stops matching the day the writer
/// is renamed — and that silent mismatch is the exact shape of the P3 leak.
pub const RECOVERY_PREPARED_REQUEST_KEY_REF: &str = "wayland-core.recovery.prepared-request.v1";

/// Every key ref [`purge_profile_confidential_keys`] must remove. One stable
/// target name per logical key; a new confidential key ref MUST be added here in
/// the same change that introduces it.
const CONFIDENTIAL_KEY_REFS: &[&str] = &[RECOVERY_PREPARED_REQUEST_KEY_REF];

/// Remove a profile's confidential keys from the OS keyring before its home
/// directory is deleted (P3).
///
/// Driven by the profile's own backend marker, which records the exact
/// `ConfidentialBackendSelection` that was pinned — so this deletes from the
/// service the profile actually used, not from a service name recomputed from
/// ambient state (`WAYLAND_HOME` points at the CALLER's profile during a
/// `profile delete`, so a recomputed name would purge the wrong profile, or
/// nothing).
///
/// Only the keyring selection is purged, deliberately. A vault-pinned profile
/// keeps its key in `credentials.enc` beside `credentials.toml`, which the
/// caller is about to remove with the rest of the tree; and if an operator has
/// explicitly pointed `EncryptedFile` at a path outside the profile home, then
/// deleting the profile is not licence to write to a file they placed elsewhere
/// (nor to prompt them for its passphrase to do so).
///
/// Best-effort by contract: it returns the first error for the caller to log,
/// but a profile whose keyring entries cannot be reached must still be
/// deletable. Never errors when there is no marker — a profile that never
/// opened a confidential store has nothing to purge.
pub fn purge_profile_confidential_keys(credentials_path: &Path) -> Result<(), CredentialsError> {
    let credentials_path = absolute_confidential_path(credentials_path)?;
    let marker_path = confidential_backend_marker_path(&credentials_path);
    let Some(marker) = load_confidential_backend_marker(&marker_path)? else {
        return Ok(());
    };
    let ConfidentialBackendSelection::Keyring { service } = marker.selection else {
        return Ok(());
    };

    // The TARGET profile's credentials directory, not the caller's:
    // `WAYLAND_HOME` still points at whoever is running `profile delete`.
    let target_dir = credentials_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let store = ConfidentialCredentialsStore::new(
        Box::new(KeyringCredentialsStore::anchored_at(service, target_dir)),
        credentials_path.with_file_name(".credentials.confidential-key.lock"),
        None,
    );
    purge_confidential_keys_from(&store)
}

/// The delete set, split from the marker resolution so it can be measured
/// without an OS keyring.
///
/// The keyring binding in [`purge_profile_confidential_keys`] cannot be
/// exercised on a headless host — which is most of them, and all of the gate
/// hosts. Splitting here means the thing that can actually be WRONG (which key
/// refs get deleted, and whether a failure on one still attempts the rest) is
/// unconditionally provable, and only the literal
/// `KeyringCredentialsStore::new(service)` line is left untested.
fn purge_confidential_keys_from(
    store: &ConfidentialCredentialsStore,
) -> Result<(), CredentialsError> {
    let mut first_error = None;
    for key_ref in CONFIDENTIAL_KEY_REFS {
        // Every ref is attempted even after one fails: stopping early would
        // leave later keys orphaned for the sake of an error we are already
        // reporting.
        if let Err(error) = crate::confidential_blob::delete_confidential_blob_key(store, key_ref) {
            tracing::warn!(
                target: "wcore_credentials",
                key_ref,
                error = %error,
                "could not remove a profile confidential key from the OS keyring; it will \
                 be orphaned there"
            );
            first_error.get_or_insert(CredentialsError::Keyring(error.to_string()));
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
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

/// Create (if needed) and harden the directory that holds credential material.
///
/// `0o700` on Unix, applied AFTER `create_dir_all` and not only through its
/// `mode` argument: `create_dir_all` applies its mode only to directories it
/// actually creates, and the process umask masks the bits on the way in. This
/// is kimi-code's lesson (`resources/kimi-code/packages/oauth/src/storage.ts:49-53`
/// — `mkdirSync(dir, { mode: 0o700 })` followed by an unconditional
/// `chmodSync(dir, 0o700)`), and it is the difference between a vault directory
/// that is 0700 and one that is 0755 on any host with a permissive umask.
///
/// On Windows this is `create_dir_all` only, for the same reason
/// [`secure_credential_file`] is a no-op there: `%APPDATA%` is already
/// per-user, and explicit ACL work needs a dependency this crate does not take.
pub fn secure_credential_dir(dir: &Path) -> std::io::Result<()> {
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// REFUSE to read credential material out of a file any other account can read.
///
/// The counterpart to [`warn_if_world_readable`], and deliberately a different
/// decision. Warning is right for the legacy cleartext file, whose contents are
/// already exposed and where refusing would only strand the operator. It is
/// wrong for the encrypted vault: a world-readable vault means the ciphertext,
/// the KDF params and the salt are all harvestable, so every future write to it
/// is an offline-crackable artifact handed to whoever is watching. Loading it
/// anyway would let the ladder keep reporting "stored securely" while it is not.
///
/// Group and other bits are both checked (`0o077`): "world-readable" in the
/// threat model is "readable by an account that is not the owner", and a shared
/// group on a build host is exactly such an account.
#[cfg(unix)]
fn refuse_if_world_readable(path: &Path) -> Result<(), CredentialsError> {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(path) else {
        // Absent (or unstattable) is not "insecure" — a vault that does not
        // exist yet is the ordinary first-write case.
        return Ok(());
    };
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(CredentialsError::BackendUnavailable(format!(
            "refusing to open the encrypted credential vault at {}: it has permissions \
             {mode:#o} and is readable by accounts other than its owner. Run `chmod 600 {}` \
             (and `chmod 700` on its directory) after confirming no other account has \
             already copied it; if one may have, rotate the secrets it holds.",
            path.display(),
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn refuse_if_world_readable(path: &Path) -> Result<(), CredentialsError> {
    let _ = path;
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
    use tempfile::{TempDir, tempdir};

    // =======================================================================
    // FerroxLabs/wayland-core#397 — the documented ladder and the built ladder
    // =======================================================================

    /// The rung list in `CredentialsBackend::Auto`'s doc comment is the rung
    /// list `build_ladder` mounts.
    ///
    /// #397 c2. The failure mode this guards is not a typo, it is DISTANCE:
    /// the claim lives on the type at the top of the file and the code lives
    /// ~2,650 lines down, and distance does not shrink on its own. A prose
    /// rule ("keep these in sync") does not do this — the same file already
    /// carried one and the comment still went stale for nine releases.
    ///
    /// The instrument reads the `ladder:` line as the DECLARATION and derives
    /// the ACTUAL rungs from the store types `build_ladder`'s body constructs,
    /// so adding a rung, removing one, or reordering them fails here.
    #[test]
    fn the_documented_ladder_matches_the_rungs_build_ladder_mounts() {
        const SOURCE: &str = include_str!("credentials.rs");

        // 1. The declaration.
        let declared: Vec<String> = SOURCE
            .lines()
            .find_map(|line| {
                line.trim_start()
                    .strip_prefix("/// ladder: ")
                    .map(|rest| rest.split(" -> ").map(|r| r.trim().to_string()).collect())
            })
            .expect(
                "the `Auto` doc comment must declare its ladder as a \
                 `/// ladder: a -> b -> refuse` line — that line IS the claim",
            );

        // 2. What `build_ladder` actually mounts, in body order.
        let body_start = SOURCE
            .find("fn build_ladder(")
            .expect("known-positive control: `build_ladder` must be in this file");
        // The first column-zero closing brace after the signature: every
        // block inside the function is indented, so this is its end.
        let body_end = SOURCE[body_start..]
            .find("\n}\n")
            .map_or(SOURCE.len(), |at| body_start + at);
        let body = &SOURCE[body_start..body_end];

        // Every credentials store this crate defines, so a rung added from a
        // NEW store type cannot slip through by not being on a short list.
        let mut stores: Vec<&str> = SOURCE
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim_start();
                let rest = trimmed
                    .strip_prefix("pub struct ")
                    .or_else(|| trimmed.strip_prefix("struct "))?;
                let name = rest.split(['<', ' ', '{', '(']).next()?;
                name.ends_with("CredentialsStore").then_some(name)
            })
            .collect();
        stores.sort_unstable();
        stores.dedup();
        assert!(
            stores.len() >= 4,
            "known-positive control: the scan found only {stores:?} — it is \
             not seeing this file's credential stores"
        );

        fn rung_name(store: &str) -> &str {
            match store {
                "KeyringCredentialsStore" => "keyring",
                "EncryptedFileCredentialsStore" => "encrypted_vault",
                "PlaintextCredentialsStore" => "plaintext",
                other => other,
            }
        }
        let mut mounted: Vec<String> = Vec::new();
        for (offset, _) in body.match_indices("CredentialsStore::new(") {
            let head = &body[..offset];
            let Some(store) = stores
                .iter()
                .find(|store| head.ends_with(&store[..store.len() - "CredentialsStore".len()]))
            else {
                continue;
            };
            // The ladder type itself is the container, not a rung.
            if *store == "LadderCredentialsStore" {
                continue;
            }
            let name = rung_name(store).to_string();
            if !mounted.contains(&name) {
                mounted.push(name);
            }
        }
        assert!(
            !mounted.is_empty(),
            "known-positive control: `build_ladder` constructs no store at all \
             — the body slice is wrong, not the code"
        );

        // 3. Compare. The declaration's terminal word is the behaviour on an
        //    empty ladder and is checked by the sibling test below, so it is
        //    dropped before the rung comparison.
        let (terminal, declared_rungs) = declared
            .split_last()
            .expect("the declaration must name at least a terminal");
        assert_eq!(
            terminal, "refuse",
            "#397: the ladder's terminal behaviour is REFUSE. A declaration \
             ending any other way is claiming a fallback the code does not have"
        );
        assert_eq!(
            declared_rungs.to_vec(),
            mounted,
            "the `ladder:` line in `CredentialsBackend::Auto`'s doc comment and \
             the rungs `build_ladder` mounts disagree (FerroxLabs/wayland-core#397)"
        );
        assert!(
            !declared_rungs.iter().any(|rung| rung == "plaintext"),
            "the documented ladder names `plaintext` as a WRITE rung — that is \
             exactly the claim #397 was filed to remove"
        );
    }

    /// The terminal word is a behaviour, so it is measured as one: a ladder
    /// with no secure rung REFUSES the write and leaves no cleartext behind.
    ///
    /// #397 c2's other arm. Reading `Err(no_secure_backend_for_write(key))` in
    /// the source proves the branch is written; running it proves the branch
    /// is reached and that nothing else wrote the value on the way past.
    #[test]
    fn a_ladder_with_no_secure_rung_refuses_the_write_and_writes_no_cleartext() {
        let dir = tempdir().unwrap();
        let plaintext_path = dir.path().join("credentials.toml");
        let ladder = LadderCredentialsStore::new(None, None, plaintext_path.clone());

        let refused = ladder.put("anthropic.api_key", "sk-ant-MUST-NOT-BE-WRITTEN");
        assert!(
            refused.is_err(),
            "a ladder with no keyring and no vault must REFUSE the write, not \
             fall through to cleartext"
        );
        assert!(
            !plaintext_path.exists(),
            "the refused write left a plaintext credentials file behind"
        );

        // WRONG-REFUSAL CONTROL: the legacy READ rung still works, or this
        // would be passing because the ladder refuses everything, and every
        // pre-existing key would be stranded.
        std::fs::write(
            &plaintext_path,
            "[secrets]\n\"legacy.key\" = \"sk-legacy\"\n",
        )
        .unwrap();
        let ladder = LadderCredentialsStore::new(None, None, plaintext_path.clone());
        assert_eq!(
            ladder.get("legacy.key").unwrap().as_deref(),
            Some("sk-legacy"),
            "reads must still descend to the legacy plaintext file — that is \
             the role the module header states it keeps"
        );
    }

    /// The startup availability probe must agree with the opener and must
    /// change nothing while answering.
    ///
    /// Both arms are host-independent on purpose. An explicitly configured
    /// encrypted-file backend is available on every host (interactive unlock is
    /// retained, so selection never rejects it) and the plaintext backend is
    /// refused on every host — so this gate has a reachable pass state AND a
    /// reachable fail state everywhere it runs, including a machine that does
    /// have an OS keyring.
    ///
    /// The `Auto`-with-no-keyring-and-no-vault case — the actual headless-server
    /// defect — cannot be asserted here, because a developer machine WITH a
    /// keyring would legitimately answer `true`. It is proven live on a
    /// keyring-less host instead; see
    /// `.planning/evidence/fix-headless-keyring/`.
    #[test]
    fn the_availability_probe_agrees_with_the_opener_and_writes_nothing() {
        let dir = tempdir().unwrap();
        let plaintext_path = dir.path().join("credentials.toml");
        let marker_path = dir.path().join(".credentials.confidential-backend.json");
        let cfg = CredentialsStorageConfig {
            backend: CredentialsBackend::EncryptedFile {
                cipher_path: dir.path().join("vault.enc"),
                key_params_path: dir.path().join("vault.params.json"),
            },
            service_name: None,
        };

        assert!(
            confidential_backend_available(&cfg, &plaintext_path),
            "an explicitly configured encrypted-file backend is reachable on every host"
        );
        assert!(
            !marker_path.exists(),
            "the probe pinned a backend; it must be read-only"
        );

        // Known-positive for the assertion above: the opener DOES write that
        // exact marker path. Without this, `!marker_path.exists()` would also
        // pass on a typo'd path, i.e. for free.
        assert!(
            open_confidential_store(&cfg, &plaintext_path).is_ok(),
            "opener and probe must agree"
        );
        assert!(
            marker_path.exists(),
            "the opener must write the marker the probe deliberately skipped — \
             otherwise the read-only assertion above is vacuous"
        );

        // The other direction, on the same host, in the same test.
        let plaintext_cfg = CredentialsStorageConfig {
            backend: CredentialsBackend::Plaintext,
            service_name: None,
        };
        assert!(
            !confidential_backend_available(&plaintext_cfg, &plaintext_path),
            "plaintext can never hold confidential material"
        );
        assert!(
            open_confidential_store(&plaintext_cfg, &plaintext_path).is_err(),
            "opener and probe must agree in this direction too"
        );
    }

    /// `WAYLAND_VAULT_PASSPHRASE` was the workaround one live UAT lane found for
    /// the headless failure. This pins the mechanism it works by: with unlock
    /// material present, the `Auto` backend is reachable on ANY host, keyring or
    /// no keyring — which is why supplying it makes a default install able to
    /// hold a durable session again.
    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn vault_unlock_material_makes_the_auto_backend_reachable_on_any_host() {
        let dir = tempdir().unwrap();
        let plaintext_path = dir.path().join("credentials.toml");
        let cfg = CredentialsStorageConfig {
            backend: CredentialsBackend::Auto,
            service_name: None,
        };

        let _guard = EnvPassphraseGuard::set("probe-pass-1");
        assert!(
            confidential_backend_available(&cfg, &plaintext_path),
            "an unlocked vault satisfies Auto even where no OS keyring exists"
        );
    }

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
        assert_eq!(
            store
                .get_many(&["anthropic_api_key", "missing", "openai_api_key"])
                .unwrap(),
            vec![
                Some("sk-ant-secret".to_string()),
                None,
                Some("sk-test".to_string())
            ]
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
        assert_eq!(
            store
                .get_many(&["anthropic_api_key", "missing", "openai_api_key"])
                .unwrap(),
            vec![
                Some("sk-ant-secret".to_string()),
                None,
                Some("sk-openai".to_string())
            ]
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

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn passphrase_fd_authority_survives_provider_then_confidential_store_open() {
        use std::io::Write;
        use std::os::unix::io::AsRawFd;
        use std::os::unix::net::UnixStream;

        let _passphrase = EnvVarGuard::remove("WAYLAND_VAULT_PASSPHRASE");
        let dir = tempdir().unwrap();
        let plaintext_path = dir.path().join("credentials.toml");
        let cipher_path = dir.path().join("credentials.enc");
        let key_params_path = dir.path().join("credentials.kdf.json");
        let cfg = CredentialsStorageConfig {
            backend: CredentialsBackend::EncryptedFile {
                cipher_path,
                key_params_path,
            },
            service_name: None,
        };

        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"one-shot-recovery-passphrase\n").unwrap();
        writer.shutdown(std::net::Shutdown::Write).unwrap();
        let _passphrase_fd = EnvVarGuard::set(
            "WAYLAND_VAULT_PASSPHRASE_FD",
            &reader.as_raw_fd().to_string(),
        );

        // Config/provider resolution opens the ordinary store first and
        // consumes the one-shot passphrase descriptor even when no provider
        // credential exists.
        let provider_store = open_store(&cfg, &plaintext_path).unwrap();
        assert!(
            provider_store
                .get("providers.openai.api_key")
                .unwrap()
                .is_none()
        );

        // Recovery protection opens a fresh fail-closed store later in the
        // same process. It must reuse the in-memory authority rather than read
        // the now-at-EOF descriptor again.
        let recovery_store = open_confidential_store(&cfg, &plaintext_path).unwrap();
        recovery_store
            .put("recovery.sealing_key", "sealed-key-material")
            .unwrap();
        assert_eq!(
            recovery_store
                .get("recovery.sealing_key")
                .unwrap()
                .as_deref(),
            Some("sealed-key-material")
        );

        // A launch authority is immutable. Repointing the environment at a
        // different live descriptor cannot silently switch vault keys.
        let (replacement_reader, mut replacement_writer) = UnixStream::pair().unwrap();
        replacement_writer
            .write_all(b"attacker-selected-replacement\n")
            .unwrap();
        replacement_writer
            .shutdown(std::net::Shutdown::Write)
            .unwrap();
        let _replacement_fd = EnvVarGuard::set(
            "WAYLAND_VAULT_PASSPHRASE_FD",
            &replacement_reader.as_raw_fd().to_string(),
        );
        let reopened = open_store(&cfg, &plaintext_path).unwrap();
        let error = reopened
            .get("providers.openai.api_key")
            .expect_err("mid-process passphrase authority replacement must fail closed");
        assert!(
            matches!(error, CredentialsError::BackendUnavailable(ref message) if message.contains("changed after the process vault authority was initialized"))
        );
    }

    #[test]
    fn vault_passphrase_authority_debug_is_redacted() {
        let authority = VaultPassphraseAuthority::new("must-not-appear".to_string());
        let rendered = format!("{authority:?}");
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains("must-not-appear"));
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

    /// Set/restore an arbitrary process-global env var for a test. Mirrors
    /// [`EnvPassphraseGuard`] for `WAYLAND_HOME` (the isolated-profile switch).
    struct EnvVarGuard {
        key: &'static str,
        prior: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prior = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, prior }
        }

        fn remove(key: &'static str) -> Self {
            let prior = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, prior }
        }
    }

    #[test]
    #[serial_test::serial(vault_passphrase_env, wayland_home_env)]
    fn confidential_auto_never_downgrades_to_plaintext() {
        let dir = tempdir().unwrap();
        let _home = EnvVarGuard::set("WAYLAND_HOME", dir.path().to_str().unwrap());
        let _passphrase = EnvVarGuard::remove("WAYLAND_VAULT_PASSPHRASE");
        let _passphrase_fd = EnvVarGuard::remove("WAYLAND_VAULT_PASSPHRASE_FD");
        let plaintext_path = dir.path().join("credentials.toml");

        match open_confidential_store(&CredentialsStorageConfig::default(), &plaintext_path) {
            // A usable OS keyring is a valid confidential backend, including
            // for an isolated profile. Do not write a probe value into the
            // operator's keyring from this test.
            Ok(_) => {}
            Err(err) => assert!(matches!(err, CredentialsError::BackendUnavailable(_))),
        }
        assert!(
            !plaintext_path.exists(),
            "confidential Auto must never materialize a plaintext store"
        );

        let plaintext_cfg = CredentialsStorageConfig {
            backend: CredentialsBackend::Plaintext,
            service_name: None,
        };
        assert!(matches!(
            open_confidential_store(&plaintext_cfg, &plaintext_path),
            Err(CredentialsError::BackendUnavailable(_))
        ));
    }

    #[test]
    fn profile_keyring_service_is_stable_and_profile_isolated() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("profile-a").join("credentials.toml");
        let second = dir.path().join("profile-b").join("credentials.toml");

        let first_service = profile_keyring_service("wayland-core", &first).unwrap();
        assert_eq!(
            first_service,
            profile_keyring_service("wayland-core", &first).unwrap()
        );
        assert_ne!(
            first_service,
            profile_keyring_service("wayland-core", &second).unwrap()
        );
        assert!(first_service.starts_with("wayland-core.profile."));
        assert_eq!(first_service.len(), "wayland-core.profile.".len() + 64);
    }

    #[test]
    fn profile_keyring_service_preserves_configured_namespace() {
        let dir = tempdir().unwrap();
        let credentials_path = dir.path().join("credentials.toml");

        let default = profile_keyring_service("wayland-core", &credentials_path).unwrap();
        let configured = profile_keyring_service("wayland-core-dev", &credentials_path).unwrap();

        assert_ne!(default, configured);
        assert!(configured.starts_with("wayland-core-dev.profile."));
    }

    #[test]
    fn explicit_keyring_uses_the_same_isolated_profile_namespace() {
        let dir = tempdir().unwrap();
        let credentials_path = dir.path().join("credentials.toml");
        let cfg = CredentialsStorageConfig {
            backend: CredentialsBackend::Keyring,
            service_name: Some("wayland-core-explicit".to_string()),
        };

        let isolated = confidential_keyring_service(&cfg, &credentials_path, true).unwrap();
        let non_isolated = confidential_keyring_service(&cfg, &credentials_path, false).unwrap();

        assert!(isolated.starts_with("wayland-core-explicit.profile."));
        assert_eq!(non_isolated, "wayland-core-explicit");
    }

    fn test_keyring_selection(service: &str) -> ConfidentialBackendSelection {
        ConfidentialBackendSelection::Keyring {
            service: service.to_string(),
        }
    }

    fn test_vault_selection(root: &Path, name: &str) -> ConfidentialBackendSelection {
        ConfidentialBackendSelection::EncryptedFile {
            cipher_path: root.join(format!("{name}.enc")),
            key_params_path: root.join(format!("{name}.kdf.json")),
        }
    }

    /// A pin that is backed by observed material.
    fn confirmed_pin(selection: &ConfidentialBackendSelection) -> PinnedConfidentialBackend {
        PinnedConfidentialBackend {
            selection: selection.clone(),
            confirmed: true,
        }
    }

    /// A pin recorded at selection time that has never held anything — the
    /// shape a Windows service account's first boot leaves behind.
    fn unconfirmed_pin(selection: &ConfidentialBackendSelection) -> PinnedConfidentialBackend {
        PinnedConfidentialBackend {
            selection: selection.clone(),
            confirmed: false,
        }
    }

    #[test]
    fn confidential_auto_vault_pin_refuses_keyring_appearance() {
        let dir = tempdir().unwrap();
        let original_keyring = test_keyring_selection("keyring-original");
        let original_vault = test_vault_selection(dir.path(), "vault-original");
        let initial_mode = ConfidentialBackendMode::Auto {
            keyring: original_keyring,
            vault: original_vault.clone(),
        };
        let selected = select_confidential_backend(None, &initial_mode, &|_| false, true).unwrap();
        assert_eq!(selected, original_vault);
        // The pin only becomes authoritative once material has been observed
        // in it; that is the state this anti-oscillation guarantee protects.
        let pinned = confirmed_pin(&selected);

        let restart_mode = ConfidentialBackendMode::Auto {
            keyring: test_keyring_selection("keyring-original"),
            vault: test_vault_selection(dir.path(), "vault-original"),
        };
        assert!(matches!(
            select_confidential_backend(Some(&pinned), &restart_mode, &|_| true, false),
            Err(CredentialsError::BackendUnavailable(_))
        ));
        assert_eq!(
            select_confidential_backend(Some(&pinned), &restart_mode, &|_| true, true).unwrap(),
            selected,
            "the original vault paths remain authoritative"
        );
    }

    #[test]
    fn confidential_auto_keyring_pin_refuses_vault_fallback() {
        let dir = tempdir().unwrap();
        let original_keyring = test_keyring_selection("keyring-original");
        let initial_mode = ConfidentialBackendMode::Auto {
            keyring: original_keyring.clone(),
            vault: test_vault_selection(dir.path(), "vault-original"),
        };
        let selected = select_confidential_backend(None, &initial_mode, &|_| true, true).unwrap();
        assert_eq!(selected, original_keyring);
        let pinned = confirmed_pin(&selected);

        let restart_mode = ConfidentialBackendMode::Auto {
            keyring: test_keyring_selection("keyring-original"),
            vault: test_vault_selection(dir.path(), "vault-original"),
        };
        assert!(matches!(
            select_confidential_backend(Some(&pinned), &restart_mode, &|_| false, true),
            Err(CredentialsError::BackendUnavailable(_))
        ));
        assert_eq!(
            select_confidential_backend(
                Some(&pinned),
                &restart_mode,
                &|service| service == "keyring-original",
                true,
            )
            .unwrap(),
            selected,
            "the original keyring service remains authoritative"
        );
    }

    /// P2. An UNCONFIRMED pin on an unavailable backend must be re-selected.
    ///
    /// Without this, the write-capable probe fixes nothing for the population
    /// that already has the problem: their profile pinned the keyring on the
    /// boot whose `CredWrite` failed, and every later boot re-validated that
    /// pin and refused again. Nothing was ever stored there, so there is
    /// nothing the move can orphan.
    #[test]
    fn confidential_auto_unconfirmed_pin_heals_onto_an_available_backend() {
        let dir = tempdir().unwrap();
        let keyring = test_keyring_selection("keyring-unwritable");
        let vault = test_vault_selection(dir.path(), "vault-fallback");
        let mode = ConfidentialBackendMode::Auto {
            keyring: keyring.clone(),
            vault: vault.clone(),
        };

        // Keyring unwritable, vault unlocked → heal onto the vault.
        assert_eq!(
            select_confidential_backend(Some(&unconfirmed_pin(&keyring)), &mode, &|_| false, true)
                .unwrap(),
            vault,
            "an unconfirmed pin on an unavailable backend must not be a dead end"
        );
        // ...but still fail closed when there is nowhere to heal TO.
        assert!(matches!(
            select_confidential_backend(Some(&unconfirmed_pin(&keyring)), &mode, &|_| false, false),
            Err(CredentialsError::BackendUnavailable(_))
        ));
        // ...and never move a pin whose backend still works.
        assert_eq!(
            select_confidential_backend(Some(&unconfirmed_pin(&keyring)), &mode, &|_| true, true)
                .unwrap(),
            keyring,
            "an available pin must be honoured whether or not it is confirmed"
        );
        // An EXPLICIT backend is an operator instruction, not a preference:
        // healing must not silently override it.
        assert!(matches!(
            select_confidential_backend(
                Some(&unconfirmed_pin(&keyring)),
                &ConfidentialBackendMode::Explicit(keyring.clone()),
                &|_| false,
                true
            ),
            Err(CredentialsError::BackendUnavailable(_))
        ));
    }

    /// P2, end to end through the marker file: a profile that is ALREADY
    /// pinned to a write-incapable keyring on disk boots onto the vault, and
    /// the marker is rewritten so it does not re-enter the dead end.
    #[test]
    #[serial_test::serial(vault_passphrase_env, wayland_home_env)]
    fn an_already_pinned_write_incapable_profile_recovers_on_next_boot() {
        let _passphrase = EnvPassphraseGuard::set("heal-pin-passphrase");
        let dir = tempdir().unwrap();
        let _home = EnvVarGuard::set("WAYLAND_HOME", dir.path().to_str().unwrap());
        let _passphrase_fd = EnvVarGuard::remove("WAYLAND_VAULT_PASSPHRASE_FD");
        let plaintext_path = dir.path().join("credentials.toml");
        let (cipher_path, _params_path) = default_vault_paths(&plaintext_path);
        let marker_path = confidential_backend_marker_path(&plaintext_path);

        // Boot 1, on the old read-only probe: the keyring looks available, so
        // the profile pins it. The write then fails, so nothing is confirmed.
        let (credentials_path, mode) =
            confidential_backend_plan(&CredentialsStorageConfig::default(), &plaintext_path)
                .unwrap();
        let ConfidentialBackendMode::Auto { keyring, .. } = &mode else {
            panic!("default config must plan Auto");
        };
        write_confidential_backend_marker(
            &confidential_backend_marker_path(&credentials_path),
            &ConfidentialBackendMarker {
                version: CONFIDENTIAL_BACKEND_MARKER_VERSION,
                selection: keyring.clone(),
                confirmed: false,
            },
        )
        .unwrap();

        // Boot 2, with the write-capable probe reporting the truth.
        let store = open_confidential_store_with_availability(
            &CredentialsStorageConfig::default(),
            &plaintext_path,
            &|_| false,
        )
        .expect("an unconfirmed keyring pin must not strand the profile");
        store.put("recovery.sealing_key", "healed-key").unwrap();

        assert!(
            cipher_path.exists(),
            "the healed profile must actually be using the vault"
        );
        assert_eq!(
            store.get("recovery.sealing_key").unwrap().as_deref(),
            Some("healed-key")
        );

        // The marker was rewritten to the vault AND confirmed by the write, so
        // the keyring can never reclaim this profile. Read the RAW marker, not
        // the derived pin: `load_pinned_confidential_backend` also infers
        // confirmation from an existing cipher file, which would mask a
        // regression in the confirm-on-observation path.
        let healed = load_confidential_backend_marker(&marker_path)
            .unwrap()
            .expect("marker must still exist");
        assert!(
            matches!(
                healed.selection,
                ConfidentialBackendSelection::EncryptedFile { .. }
            ),
            "marker was not rewritten: {:?}",
            healed.selection
        );
        assert!(
            healed.confirmed,
            "a successful write must confirm the pin so it stops being advisory"
        );
    }

    #[test]
    fn confidential_auto_rejects_foreign_or_relative_backend_markers() {
        let dir = tempdir().unwrap();
        let credentials_path = dir.path().join("credentials.toml");
        let marker_path = dir.path().join(".credentials.confidential-backend.json");
        let mode = ConfidentialBackendMode::Auto {
            keyring: test_keyring_selection("current-profile-keyring"),
            vault: test_vault_selection(dir.path(), "current-profile-vault"),
        };
        let foreign = [
            test_keyring_selection("copied-foreign-profile-keyring"),
            ConfidentialBackendSelection::EncryptedFile {
                cipher_path: PathBuf::from("relative-vault.enc"),
                key_params_path: PathBuf::from("relative-vault.kdf.json"),
            },
            test_vault_selection(&dir.path().join("foreign-profile"), "foreign-vault"),
        ];

        for selection in foreign {
            let marker = ConfidentialBackendMarker {
                version: CONFIDENTIAL_BACKEND_MARKER_VERSION,
                selection,
                confirmed: true,
            };
            crate::atomic_write(&marker_path, &serde_json::to_vec(&marker).unwrap()).unwrap();
            assert!(matches!(
                resolve_confidential_backend_with_availability(
                    &mode,
                    &credentials_path,
                    &|_| true,
                    true,
                ),
                Err(CredentialsError::BackendUnavailable(_))
            ));
        }
    }

    #[test]
    fn confidential_backend_marker_is_strict_and_fail_closed() {
        let dir = tempdir().unwrap();
        let credentials_path = dir.path().join("credentials.toml");
        let marker_path = dir.path().join(".credentials.confidential-backend.json");
        let mode = ConfidentialBackendMode::Explicit(test_keyring_selection("strict-marker"));

        std::fs::write(
            &marker_path,
            br#"{"version":1,"selection":{"backend":"keyring"}}"#,
        )
        .unwrap();
        assert!(matches!(
            resolve_confidential_backend_with_availability(
                &mode,
                &credentials_path,
                &|_| true,
                false,
            ),
            Err(CredentialsError::BackendUnavailable(_))
        ));

        let unsupported = ConfidentialBackendMarker {
            version: CONFIDENTIAL_BACKEND_MARKER_VERSION + 1,
            selection: test_keyring_selection("strict-marker"),
            confirmed: false,
        };
        crate::atomic_write(&marker_path, &serde_json::to_vec(&unsupported).unwrap()).unwrap();
        assert!(matches!(
            resolve_confidential_backend_with_availability(
                &mode,
                &credentials_path,
                &|_| true,
                false,
            ),
            Err(CredentialsError::BackendUnavailable(_))
        ));
    }

    /// Race two selectors with OPPOSITE availability views over one profile.
    ///
    /// DELIBERATE GUARANTEE CHANGE (P2). This case used to assert that exactly
    /// one of the two ever succeeds, for a brand-new profile. That is no longer
    /// true and cannot be: a pin that has never held material is advisory, and
    /// making it absolute from the instant it is written is precisely what
    /// stranded every Windows service-account profile — the boot that pinned
    /// the keyring is the boot whose `CredWrite` failed.
    ///
    /// What still holds, and is asserted here:
    ///  * once the pin is CONFIRMED (material observed), it is absolute and
    ///    exactly one of the two concurrent selectors succeeds — that is the
    ///    anti-oscillation guarantee, undamaged;
    ///  * while UNCONFIRMED, both may proceed, but the marker is never torn:
    ///    it ends as exactly one of the profile's two candidate backends, and
    ///    it matches one of the returned selections.
    ///
    /// The unconfirmed divergence is only reachable when two processes on one
    /// host disagree about keyring availability, which this test manufactures
    /// and a real host does not produce — availability is a property of the
    /// host and account, not of the process.
    fn race_two_selectors(
        credentials_path: &Path,
        mode: &ConfidentialBackendMode,
    ) -> [Result<ConfidentialBackendSelection, CredentialsError>; 2] {
        use std::sync::{Arc, Barrier};

        let credentials_path = Arc::new(credentials_path.to_path_buf());
        let mode = Arc::new(mode.clone());
        let barrier = Arc::new(Barrier::new(2));

        let keyring_thread = {
            let credentials_path = Arc::clone(&credentials_path);
            let mode = Arc::clone(&mode);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                resolve_confidential_backend_with_availability(
                    &mode,
                    &credentials_path,
                    &|_| true,
                    false,
                )
            })
        };
        let vault_thread = {
            let credentials_path = Arc::clone(&credentials_path);
            let mode = Arc::clone(&mode);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                resolve_confidential_backend_with_availability(
                    &mode,
                    &credentials_path,
                    &|_| false,
                    true,
                )
            })
        };
        [keyring_thread.join().unwrap(), vault_thread.join().unwrap()]
    }

    #[test]
    fn concurrent_confidential_backend_selection_creates_one_authority() {
        let dir = tempdir().unwrap();
        let credentials_path = dir.path().join("credentials.toml");
        let keyring = test_keyring_selection("concurrent-keyring");
        let vault = test_vault_selection(dir.path(), "concurrent-vault");
        let mode = ConfidentialBackendMode::Auto {
            keyring: keyring.clone(),
            vault: vault.clone(),
        };
        let marker_path = confidential_backend_marker_path(&credentials_path);

        // A CONFIRMED pin is absolute: the selector whose view contradicts it
        // must fail rather than open a second authority.
        write_confidential_backend_marker(
            &marker_path,
            &ConfidentialBackendMarker {
                version: CONFIDENTIAL_BACKEND_MARKER_VERSION,
                selection: keyring.clone(),
                confirmed: true,
            },
        )
        .unwrap();
        let results = race_two_selectors(&credentials_path, &mode);
        let successful = results
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .collect::<Vec<_>>();
        assert_eq!(
            successful.len(),
            1,
            "exactly one concurrent selector owns a confirmed pin"
        );
        assert_eq!(*successful[0], keyring);
        let after = load_confidential_backend_marker(&marker_path)
            .unwrap()
            .expect("marker survives the race");
        assert_eq!(after.selection, keyring, "a confirmed pin never moves");
        assert!(after.confirmed);
    }

    #[test]
    fn concurrent_unconfirmed_selection_leaves_one_untorn_marker() {
        let dir = tempdir().unwrap();
        let credentials_path = dir.path().join("credentials.toml");
        let keyring = test_keyring_selection("concurrent-keyring");
        let vault = test_vault_selection(dir.path(), "concurrent-vault");
        let mode = ConfidentialBackendMode::Auto {
            keyring: keyring.clone(),
            vault: vault.clone(),
        };
        let marker_path = confidential_backend_marker_path(&credentials_path);

        let results = race_two_selectors(&credentials_path, &mode);
        let successful = results
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .collect::<Vec<_>>();
        assert!(
            !successful.is_empty(),
            "an empty profile with an available backend must not strand every selector"
        );
        let marker = load_confidential_backend_marker(&marker_path)
            .unwrap()
            .expect("a marker is always written");
        assert!(
            marker.selection == keyring || marker.selection == vault,
            "marker is torn: {:?}",
            marker.selection
        );
        assert!(
            successful.iter().any(|s| **s == marker.selection),
            "the marker must match a selection some selector actually returned"
        );
        assert!(
            !marker.confirmed,
            "selection alone must never confirm a pin — only observed material does"
        );
    }

    #[cfg(unix)]
    #[test]
    fn profile_keyring_service_canonicalizes_symlinked_profile_path() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let profile = dir.path().join("profile");
        std::fs::create_dir(&profile).unwrap();
        let alias = dir.path().join("profile-alias");
        symlink(&profile, &alias).unwrap();

        let canonical = profile.join("credentials.toml");
        let aliased = alias.join("credentials.toml");
        assert_eq!(
            profile_keyring_service("wayland-core", &canonical).unwrap(),
            profile_keyring_service("wayland-core", &aliased).unwrap()
        );
    }

    /// The keyring is pinned UNAVAILABLE here on purpose. The assertion is
    /// "when Auto falls to the vault, it really uses the vault and writes no
    /// plaintext" — but `open_confidential_store` consults the host's real
    /// keyring, so on macOS and on a logged-in Windows desktop Auto correctly
    /// picks the keyring and no cipher file is ever written. The test was
    /// therefore Linux-headless-specific and failed on macOS at the
    /// `cipher_path.exists()` line for a reason that was not a defect. Pinning
    /// availability makes the case mean the same thing on all three platforms.
    /// (Naming: this exercises no plaintext path at all — the never-downgrade
    /// guarantee is proven by `confidential_auto_never_downgrades_to_plaintext`.)
    #[test]
    #[serial_test::serial(vault_passphrase_env, wayland_home_env)]
    fn confidential_auto_uses_encrypted_vault_when_the_keyring_is_unavailable() {
        let _passphrase = EnvPassphraseGuard::set("confidential-auto-passphrase");
        let dir = tempdir().unwrap();
        let _home = EnvVarGuard::set("WAYLAND_HOME", dir.path().to_str().unwrap());
        let _passphrase_fd = EnvVarGuard::remove("WAYLAND_VAULT_PASSPHRASE_FD");
        let plaintext_path = dir.path().join("credentials.toml");
        let (cipher_path, params_path) = default_vault_paths(&plaintext_path);

        let store = open_confidential_store_with_availability(
            &CredentialsStorageConfig::default(),
            &plaintext_path,
            &|_| false,
        )
        .unwrap();
        store
            .put("recovery.sealing_key", "base64-key-material")
            .unwrap();

        assert!(cipher_path.exists());
        assert!(params_path.exists());
        assert!(!plaintext_path.exists());
        assert_eq!(
            store.get("recovery.sealing_key").unwrap().as_deref(),
            Some("base64-key-material")
        );
    }

    /// NON-VACUITY for the write-capable probe.
    ///
    /// A probe that always answers "unavailable" would make every
    /// keyring-falls-to-vault test pass and be completely wrong. This case is
    /// two-sided and runs on every platform: it establishes ground truth by
    /// doing the set/get/delete round trip INLINE on a different key, then
    /// requires [`keyring_available`] to agree with it.
    ///
    /// * headless Linux / CI  → ground truth false → the probe MUST say false
    /// * macOS / logged-in Windows → ground truth true → the probe MUST say true
    ///
    /// So neither a probe stuck on `false` nor one stuck on `true` can pass it.
    #[test]
    fn keyring_probe_agrees_with_an_independent_write_round_trip() {
        let service = format!(
            "wayland-core.probe-nonvacuity.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );

        // Ground truth, established WITHOUT calling the probe.
        let ground_truth = match keyring::Entry::new(&service, "__independent_round_trip__") {
            Ok(entry) => match entry.set_password("round-trip") {
                Ok(()) => {
                    let read_back = entry.get_password().ok();
                    let _ = entry.delete_credential();
                    read_back.as_deref() == Some("round-trip")
                }
                Err(_) => false,
            },
            Err(_) => false,
        };

        assert_eq!(
            keyring_available(&service),
            ground_truth,
            "the availability probe disagrees with a real write round trip on this host; \
             a probe that is not write-capable (or is hardwired) is exactly the F20 defect"
        );

        // Cached per service: the second call must not re-probe, and must not
        // change its mind.
        assert_eq!(keyring_available(&service), ground_truth);
    }

    /// A working keyring is still PREFERRED over the vault, and the selector
    /// still fails closed when neither backend is usable. Together with the
    /// round-trip test above this pins both halves: the probe tells the truth,
    /// and a truthful "available" still wins.
    #[test]
    fn confidential_auto_prefers_a_working_keyring_and_fails_closed_without_one() {
        let dir = tempdir().unwrap();
        let keyring = test_keyring_selection("keyring-writable");
        let vault = test_vault_selection(dir.path(), "vault-fallback");
        let mode = ConfidentialBackendMode::Auto {
            keyring: keyring.clone(),
            vault: vault.clone(),
        };

        // Keyring writable + vault unlocked → keyring wins.
        assert_eq!(
            select_confidential_backend(None, &mode, &|_| true, true).unwrap(),
            keyring,
            "a genuinely writable keyring must still be preferred"
        );
        // Keyring writable, no vault material → still the keyring.
        assert_eq!(
            select_confidential_backend(None, &mode, &|_| true, false).unwrap(),
            keyring
        );
        // Keyring NOT writable (the Windows service-account case) → vault.
        assert_eq!(
            select_confidential_backend(None, &mode, &|_| false, true).unwrap(),
            vault,
            "an unwritable keyring must fall through to the unlocked vault"
        );
        // Neither → fail closed, never plaintext.
        assert!(matches!(
            select_confidential_backend(None, &mode, &|_| false, false),
            Err(CredentialsError::BackendUnavailable(_))
        ));
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prior {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    // #183 — plaintext→vault migration entrypoint.

    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn migrate_plaintext_into_vault_imports_verifies_and_removes() {
        let _g = EnvPassphraseGuard::set("migrate-pass-1");
        let dir = tempdir().unwrap();
        let plaintext_path = dir.path().join("credentials.toml");
        let seed = PlaintextCredentialsStore::new(&plaintext_path);
        seed.put("anthropic_api_key", "sk-ant-1").unwrap();
        seed.put("openai_api_key", "sk-oai-2").unwrap();
        assert!(plaintext_path.exists());

        let (cipher, params) = default_vault_paths(&plaintext_path);
        let store = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
        migrate_plaintext_into_vault(&plaintext_path, &store).unwrap();

        // Secrets now resolve through the vault...
        assert_eq!(
            store.get("anthropic_api_key").unwrap().as_deref(),
            Some("sk-ant-1")
        );
        assert_eq!(
            store.get("openai_api_key").unwrap().as_deref(),
            Some("sk-oai-2")
        );
        // ...the ciphertext exists, and the plaintext original is gone.
        assert!(cipher.exists(), "vault ciphertext should be written");
        assert!(
            !plaintext_path.exists(),
            "plaintext file should be removed after a verified migration"
        );
    }

    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn migrate_merges_without_clobbering_existing_vault_keys() {
        let _g = EnvPassphraseGuard::set("migrate-pass-2");
        let dir = tempdir().unwrap();
        let plaintext_path = dir.path().join("credentials.toml");
        let seed = PlaintextCredentialsStore::new(&plaintext_path);
        seed.put("shared", "plain-shared").unwrap();
        seed.put("plaintext_only", "plain-only").unwrap();

        let (cipher, params) = default_vault_paths(&plaintext_path);
        let store = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
        store.put("shared", "vault-shared").unwrap();
        assert!(cipher.exists());

        migrate_plaintext_into_vault(&plaintext_path, &store).unwrap();
        // Existing vault key is authoritative (NOT clobbered by plaintext)...
        assert_eq!(
            store.get("shared").unwrap().as_deref(),
            Some("vault-shared")
        );
        // ...the plaintext-only key is imported...
        assert_eq!(
            store.get("plaintext_only").unwrap().as_deref(),
            Some("plain-only")
        );
        // ...and the plaintext file is consolidated away.
        assert!(
            !plaintext_path.exists(),
            "plaintext should be removed after every key is resolvable in the vault"
        );
    }

    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn migrate_discards_orphaned_ciphertext_without_kdf() {
        // BLOCKER-1 regression: an interrupted migration can leave a `.enc`
        // with no `.kdf` (crash between the two writes). It is permanently
        // undecryptable, so the migration must discard it and rebuild from the
        // still-present plaintext — never trust the orphan and lose secrets.
        let _g = EnvPassphraseGuard::set("migrate-pass-orphan");
        let dir = tempdir().unwrap();
        let plaintext_path = dir.path().join("credentials.toml");
        let seed = PlaintextCredentialsStore::new(&plaintext_path);
        seed.put("k1", "v1").unwrap();
        seed.put("k2", "v2").unwrap();

        let (cipher, params) = default_vault_paths(&plaintext_path);
        // Simulate the crash artifact: a ciphertext with NO params file.
        std::fs::write(&cipher, b"orphaned-unreadable-ciphertext").unwrap();
        assert!(cipher.exists() && !params.exists());

        let store = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
        migrate_plaintext_into_vault(&plaintext_path, &store).unwrap();

        // Rebuilt from plaintext: both keys resolve, params now exist, plaintext gone.
        assert_eq!(store.get("k1").unwrap().as_deref(), Some("v1"));
        assert_eq!(store.get("k2").unwrap().as_deref(), Some("v2"));
        assert!(params.exists(), "kdf params should be rebuilt");
        assert!(!plaintext_path.exists());
    }

    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn migrate_discards_ciphertext_with_corrupt_kdf() {
        // F3 regression: a present-but-unparseable `.kdf` (crash mid-write) is
        // also a dead artifact — discard both and rebuild from plaintext rather
        // than hard-failing every open forever.
        let _g = EnvPassphraseGuard::set("migrate-pass-corruptkdf");
        let dir = tempdir().unwrap();
        let plaintext_path = dir.path().join("credentials.toml");
        let seed = PlaintextCredentialsStore::new(&plaintext_path);
        seed.put("k", "v").unwrap();

        let (cipher, params) = default_vault_paths(&plaintext_path);
        std::fs::write(&cipher, b"orphaned-ciphertext").unwrap();
        std::fs::write(&params, b"not-valid-json{{{").unwrap();

        let store = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
        migrate_plaintext_into_vault(&plaintext_path, &store).unwrap();

        assert_eq!(store.get("k").unwrap().as_deref(), Some("v"));
        assert!(!plaintext_path.exists());
    }

    #[test]
    fn migration_lock_drop_removes_only_our_own_lock() {
        // F1 regression: after a stale-steal replaces our lockfile with another
        // holder's, our drop must NOT delete the stealer's lock (which would let
        // a third concurrent migrator in).
        let dir = tempdir().unwrap();
        let path = dir.path().join(".credentials.migrate.lock");
        {
            let _lock =
                ExclusiveFileLock::acquire(path.clone(), LockPolicy::MIGRATION, "test").unwrap();
            assert!(path.exists());
            std::fs::write(&path, "another-process-nonce").unwrap();
            // _lock drops here.
        }
        assert!(
            path.exists(),
            "drop must leave a lockfile that carries another holder's nonce"
        );

        // Clear the foreign lock, then confirm a normal acquire DOES clean up
        // its own lock on drop.
        std::fs::remove_file(&path).unwrap();
        {
            let _lock =
                ExclusiveFileLock::acquire(path.clone(), LockPolicy::MIGRATION, "test").unwrap();
            assert!(path.exists());
        }
        assert!(!path.exists(), "drop removes our own lockfile");
    }

    #[test]
    fn exclusive_lock_wait_ceiling_refuses_a_live_holder() {
        // A second acquirer must NOT get in while a live holder is inside its
        // critical section, and must surface a labelled busy error rather than
        // silently proceeding.
        let dir = tempdir().unwrap();
        let path = dir.path().join("refresh.lock");
        let policy = LockPolicy::new(
            std::time::Duration::from_secs(60),
            std::time::Duration::from_millis(150),
        );
        let _held = ExclusiveFileLock::acquire(path.clone(), policy, "refresh").unwrap();
        let error = ExclusiveFileLock::acquire(path.clone(), policy, "refresh")
            .expect_err("a live holder must not be displaced");
        assert!(
            error.to_string().contains("refresh"),
            "the busy error must name the lock: {error}"
        );
    }

    #[test]
    fn exclusive_lock_steals_a_stale_holder_but_never_a_heartbeating_one() {
        // Two halves of one invariant, which is why they are one test: an
        // ABANDONED lockfile must be stolen, and a LIVE one must not be — and
        // the only thing separating them is the heartbeat.
        let dir = tempdir().unwrap();

        // (a) Abandoned: a lockfile nobody is refreshing ages out and is stolen.
        let abandoned = dir.path().join("abandoned.lock");
        std::fs::write(&abandoned, "dead-holder-nonce").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(120));
        let stale_policy = LockPolicy::new(
            std::time::Duration::from_millis(100),
            std::time::Duration::from_secs(2),
        );
        let stolen = ExclusiveFileLock::acquire(abandoned.clone(), stale_policy, "refresh");
        assert!(stolen.is_ok(), "a stale lockfile must be stealable");
        drop(stolen);

        // (b) Live: the holder heartbeats, so the waiter must time out instead
        // of stealing.
        //
        // These timings are 10x what this leg shipped with (a 20ms heartbeat
        // against a 100ms staleness), and the change is measured rather than
        // defensive. Those values required the heartbeat thread to be scheduled
        // at least once every five 20ms ticks; on a GitHub-hosted macOS runner
        // executing 13870 tests in parallel it is not, and this test failed
        // NINE consecutive tries across three runs while passing on Linux 10/10
        // idle AND 10/10 under 96 CPU burners.
        //
        // It was NOT widened until it went green. The competing explanation —
        // `Heartbeat::start` swallowing a thread-spawn failure with `.ok()` and
        // returning a lock that reports itself heartbeated while nothing
        // refreshes it — was a real defect, is fixed above, and was ELIMINATED
        // as this failure's cause by observing that the explicit error it now
        // raises never appears in the failing run. What remains is scheduling.
        //
        // The invariant is the RATIO, never the absolute values: a heartbeating
        // holder must not be judged stale. Scaling both sides preserves it and
        // buys 20 ticks of scheduling slack instead of 5.
        //
        // `wait_ceiling` MUST stay greater than `stale_after`, or this proves
        // nothing — a waiter that gives up before the lock could ever age out
        // would pass with no heartbeat at all. 2000ms > 1000ms keeps the steal
        // genuinely reachable, so the heartbeat is the only thing preventing it.
        let live = dir.path().join("live.lock");
        let held_policy = LockPolicy::new(
            std::time::Duration::from_millis(1000),
            std::time::Duration::from_secs(3),
        )
        .with_heartbeat(std::time::Duration::from_millis(50));
        let _held = ExclusiveFileLock::acquire(live.clone(), held_policy, "refresh").unwrap();
        let waiter_policy = LockPolicy::new(
            std::time::Duration::from_millis(1000),
            std::time::Duration::from_millis(2000),
        );
        let error = ExclusiveFileLock::acquire(live.clone(), waiter_policy, "refresh")
            .expect_err("a heartbeating holder must never be judged stale");
        assert!(error.to_string().contains("did not free within"));
    }

    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn migrate_keeps_plaintext_when_non_string_entries_present() {
        // NIT-6 regression: a non-string (hand-edited) entry is not a credential
        // and cannot migrate; the plaintext file must be KEPT so that data is
        // not silently destroyed, while the real string secret still migrates.
        let _g = EnvPassphraseGuard::set("migrate-pass-nonstr");
        let dir = tempdir().unwrap();
        let plaintext_path = dir.path().join("credentials.toml");
        std::fs::write(
            &plaintext_path,
            "[secrets]\napi_key = \"sk-real\"\nport = 8080\n",
        )
        .unwrap();

        let (cipher, params) = default_vault_paths(&plaintext_path);
        let store = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
        migrate_plaintext_into_vault(&plaintext_path, &store).unwrap();

        assert_eq!(store.get("api_key").unwrap().as_deref(), Some("sk-real"));
        assert!(
            plaintext_path.exists(),
            "plaintext must be kept when it holds a non-string entry that cannot migrate"
        );
    }

    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn migrate_is_noop_without_plaintext_secrets() {
        let _g = EnvPassphraseGuard::set("migrate-pass-3");
        let dir = tempdir().unwrap();
        let plaintext_path = dir.path().join("credentials.toml");
        let (cipher, params) = default_vault_paths(&plaintext_path);
        let store = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());

        // (a) missing plaintext file → no-op, no vault materialized.
        migrate_plaintext_into_vault(&plaintext_path, &store).unwrap();
        assert!(
            !cipher.exists(),
            "no vault should be created when there is nothing to migrate"
        );

        // (b) present-but-empty plaintext file → still a no-op.
        std::fs::write(&plaintext_path, "").unwrap();
        migrate_plaintext_into_vault(&plaintext_path, &store).unwrap();
        assert!(!cipher.exists());
        assert!(plaintext_path.exists());
    }

    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn open_store_encrypted_file_migrates_plaintext_once() {
        let _g = EnvPassphraseGuard::set("migrate-pass-4");
        let dir = tempdir().unwrap();
        let plaintext_path = dir.path().join("credentials.toml");
        let seed = PlaintextCredentialsStore::new(&plaintext_path);
        seed.put("provider_key", "sk-live-xyz").unwrap();

        let cipher = dir.path().join("credentials.enc");
        let params = dir.path().join("credentials.kdf.json");
        let cfg = CredentialsStorageConfig {
            backend: CredentialsBackend::EncryptedFile {
                cipher_path: cipher.clone(),
                key_params_path: params.clone(),
            },
            service_name: None,
        };

        // First open migrates plaintext → vault.
        let store = open_store(&cfg, &plaintext_path).unwrap();
        assert_eq!(
            store.get("provider_key").unwrap().as_deref(),
            Some("sk-live-xyz")
        );
        assert!(cipher.exists());
        assert!(
            !plaintext_path.exists(),
            "plaintext removed after migrating via open_store"
        );

        // Second open (simulated restart) is a no-op and still reads.
        let store2 = open_store(&cfg, &plaintext_path).unwrap();
        assert_eq!(
            store2.get("provider_key").unwrap().as_deref(),
            Some("sk-live-xyz")
        );
    }

    #[test]
    #[serial_test::serial(vault_passphrase_env, wayland_home_env)]
    fn open_store_auto_isolated_migrates_plaintext_to_vault() {
        let _pass = EnvPassphraseGuard::set("migrate-pass-5");
        let dir = tempdir().unwrap();
        let _home = EnvVarGuard::set("WAYLAND_HOME", dir.path().to_str().unwrap());
        let plaintext_path = dir.path().join("credentials.toml");
        let seed = PlaintextCredentialsStore::new(&plaintext_path);
        seed.put("isolated_key", "sk-iso").unwrap();

        // Auto backend + WAYLAND_HOME + passphrase present ⇒ the isolated-profile
        // branch builds the in-home vault and migrates into it.
        let cfg = CredentialsStorageConfig::default();
        let store = open_store(&cfg, &plaintext_path).unwrap();
        assert_eq!(
            store.get("isolated_key").unwrap().as_deref(),
            Some("sk-iso")
        );

        let (cipher, _params) = default_vault_paths(&plaintext_path);
        assert!(
            cipher.exists(),
            "auto-isolated path should have created the vault"
        );
        assert!(
            !plaintext_path.exists(),
            "plaintext removed after auto-isolated migration"
        );
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

    // -----------------------------------------------------------------------
    // The ladder's ordering, against injected tiers.
    //
    // `KeyringCredentialsStore` talks to the host's real credential store, so
    // the keyring rungs can only be exercised on a host that HAS one — i.e.
    // never on the headless gate host these rungs exist to serve. These cases
    // inject the tiers instead, so the ORDER, the promotion sequence and the
    // refusal are measured everywhere.
    // -----------------------------------------------------------------------

    /// An in-memory tier that can be made to fail on demand, and that COUNTS
    /// its operations — so a test can prove not just the end state but the
    /// sequence that produced it (the write-before-delete ordering is the whole
    /// crash-safety argument, and an end-state assertion cannot see it).
    #[derive(Default)]
    struct FakeTier {
        entries: Mutex<HashMap<String, String>>,
        log: Mutex<Vec<String>>,
        fail_put: std::sync::atomic::AtomicBool,
    }

    impl FakeTier {
        fn with(pairs: &[(&str, &str)]) -> Self {
            let tier = Self::default();
            for (key, value) in pairs {
                tier.entries
                    .lock()
                    .unwrap()
                    .insert((*key).to_string(), (*value).to_string());
            }
            // Seeding is fixture, not behaviour under test.
            tier.log.lock().unwrap().clear();
            tier
        }

        fn record(&self, event: &str) {
            self.log.lock().unwrap().push(event.to_string());
        }

        fn ops(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }

        fn snapshot(&self) -> Vec<(String, String)> {
            let mut items: Vec<_> = self
                .entries
                .lock()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            items.sort();
            items
        }
    }

    impl CredentialsStore for std::sync::Arc<FakeTier> {
        fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
            self.record(&format!("get:{key}"));
            Ok(self.entries.lock().unwrap().get(key).cloned())
        }

        fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
            if self.fail_put.load(std::sync::atomic::Ordering::SeqCst) {
                self.record(&format!("put-FAILED:{key}"));
                return Err(CredentialsError::Keyring("injected write failure".into()));
            }
            self.record(&format!("put:{key}"));
            self.entries
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), CredentialsError> {
            self.record(&format!("delete:{key}"));
            self.entries.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn tier(pairs: &[(&str, &str)]) -> std::sync::Arc<FakeTier> {
        std::sync::Arc::new(FakeTier::with(pairs))
    }

    fn boxed(tier: &std::sync::Arc<FakeTier>) -> Box<dyn CredentialsStore> {
        Box::new(std::sync::Arc::clone(tier))
    }

    // -----------------------------------------------------------------------
    // Spanning the Windows Credential Manager blob cap.
    // -----------------------------------------------------------------------

    /// A store that enforces the cap Windows Credential Manager enforces, in
    /// the unit Windows measures: `CRED_MAX_CREDENTIAL_BLOB_SIZE` = 2560
    /// bytes of UTF-16, i.e. 1280 code units. The `keyring` crate refuses the
    /// write itself, before the syscall, with exactly this arithmetic.
    ///
    /// This is the ONLY shape that can tell the spanning writer apart from a
    /// writer that merely happens to work: against an uncapped store, deleting
    /// the spanning logic changes nothing observable.
    #[derive(Default)]
    struct CapsBlobLikeWindows {
        entries: Mutex<HashMap<String, String>>,
        max_utf16_units: usize,
    }

    impl CapsBlobLikeWindows {
        fn new(max_utf16_units: usize) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self {
                entries: Mutex::new(HashMap::new()),
                max_utf16_units,
            })
        }
    }

    impl CredentialsStore for std::sync::Arc<CapsBlobLikeWindows> {
        fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
            Ok(self.entries.lock().unwrap().get(key).cloned())
        }

        fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
            if utf16_units(value) > self.max_utf16_units {
                return Err(CredentialsError::Keyring(format!(
                    "Password too long: {} > {} UTF-16 code units",
                    utf16_units(value),
                    self.max_utf16_units
                )));
            }
            self.entries
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), CredentialsError> {
            self.entries.lock().unwrap().remove(key);
            Ok(())
        }
    }

    /// A write-lock site rooted in a fresh temp dir. The real cross-process
    /// lock is exercised — only its LOCATION is redirected, so no test writes a
    /// lockfile into the developer's profile home.
    fn chunk_locks() -> (TempDir, ChunkWriteLockSite) {
        let dir = tempdir().unwrap();
        let locks = ChunkWriteLockSite::in_dir(dir.path(), LockPolicy::CREDENTIAL_WRITE);
        (dir, locks)
    }

    /// A realistically sized OAuth token set: access JWT + id JWT + refresh
    /// token. ~3.4 KB, which is what makes this defect a real Windows
    /// regression rather than a theoretical one.
    fn oauth_token_set_json() -> String {
        let jwt = format!("hdr.{}.sig", "P".repeat(1400));
        format!(
            r#"{{"access_token":"{jwt}","id_token":"{jwt}","refresh_token":"{}","token_type":"Bearer","expires_at_unix_secs":1700000000}}"#,
            "R".repeat(500)
        )
    }

    /// POSITIVE CONTROL. The cap is real: writing the token set as ONE entry
    /// is refused by the capped store, which is precisely what the Windows
    /// keyring rung did to `auth login`.
    #[test]
    fn one_entry_cannot_hold_an_oauth_token_set_under_the_windows_cap() {
        let raw = CapsBlobLikeWindows::new(1280);
        let error = raw.put("oauth.chatgpt.tokens", &oauth_token_set_json());
        assert!(
            error.is_err(),
            "the Windows blob cap must reject a whole token set in one entry; \
             if it does not, this test is not exercising the defect"
        );
    }

    /// MUTATION TARGET. Replace `chunked_put`/`chunked_get` with a direct
    /// `raw.put`/`raw.get` and this fails on the write: the token set does not
    /// fit, and a Windows user cannot log in.
    #[test]
    fn an_oauth_token_set_round_trips_across_the_windows_blob_cap() {
        let (_lock_dir, locks) = chunk_locks();
        let raw = CapsBlobLikeWindows::new(1280);
        let value = oauth_token_set_json();

        chunked_put(&raw, "oauth.chatgpt.tokens", &value, 1000, &locks)
            .expect("spanned write must land");
        let read_back = chunked_get(&raw, "oauth.chatgpt.tokens", &locks).expect("read");

        assert_eq!(
            read_back.as_deref(),
            Some(value.as_str()),
            "a spanned value must read back byte-identical"
        );
        assert!(
            raw.entries.lock().unwrap().len() > 1,
            "the value must actually span entries, not fit in one"
        );
        for (entry_key, entry_value) in raw.entries.lock().unwrap().iter() {
            assert!(
                utf16_units(entry_value) <= 1280,
                "{entry_key} is over the cap at {} units",
                utf16_units(entry_value)
            );
        }
    }

    /// A value that fits is still written literally, so entries written by
    /// builds that predate spanning keep reading back unchanged.
    #[test]
    fn a_small_value_is_stored_literally_and_reads_back() {
        let (_lock_dir, locks) = chunk_locks();
        let raw = CapsBlobLikeWindows::new(1280);
        chunked_put(
            &raw,
            "providers.anthropic.api_key",
            "sk-ant-123",
            1000,
            &locks,
        )
        .unwrap();
        assert_eq!(
            raw.entries
                .lock()
                .unwrap()
                .get("providers.anthropic.api_key")
                .map(String::as_str),
            Some("sk-ant-123"),
            "a value that fits must not be wrapped in a manifest"
        );
        assert_eq!(
            chunked_get(&raw, "providers.anthropic.api_key", &locks)
                .unwrap()
                .as_deref(),
            Some("sk-ant-123")
        );
    }

    /// Rewriting a spanned value must never leave a reader looking at a mix of
    /// old and new parts. The new generation is written under different entry
    /// names and the manifest flip is the commit point.
    #[test]
    fn rewriting_a_spanned_value_alternates_generations_and_purges_the_old_one() {
        let (_lock_dir, locks) = chunk_locks();
        let raw = CapsBlobLikeWindows::new(1280);
        let first = "A".repeat(4000);
        let second = "B".repeat(5000);

        chunked_put(&raw, "k", &first, 1000, &locks).unwrap();
        let gen_a = parse_chunk_manifest(&raw.get("k").unwrap().unwrap()).expect("manifest");
        assert_eq!(gen_a.generation, 'a');

        chunked_put(&raw, "k", &second, 1000, &locks).unwrap();
        let gen_b = parse_chunk_manifest(&raw.get("k").unwrap().unwrap()).expect("manifest");
        assert_eq!(
            gen_b.generation, 'b',
            "the rewrite must use the other generation"
        );

        assert_eq!(
            chunked_get(&raw, "k", &locks).unwrap().as_deref(),
            Some(second.as_str())
        );
        assert!(
            !raw.entries
                .lock()
                .unwrap()
                .keys()
                .any(|k| k.contains("__wlchunka")),
            "the superseded generation's parts must be purged"
        );
    }

    /// A missing part is an ERROR, never a short string. Returning the prefix
    /// of a credential hands the caller a secret that authenticates as nothing
    /// and reads to them as a corrupted-token bug rather than a storage fault.
    #[test]
    fn a_missing_part_refuses_rather_than_returning_a_truncated_secret() {
        let (_lock_dir, locks) = chunk_locks();
        let raw = CapsBlobLikeWindows::new(1280);
        let value = "Z".repeat(4000);
        chunked_put(&raw, "k", &value, 1000, &locks).unwrap();
        raw.entries.lock().unwrap().remove(&chunk_key("k", 'a', 2));

        let error = chunked_get(&raw, "k", &locks).expect_err("a torn value must not read back");
        assert!(
            error
                .to_string()
                .contains("refusing to return a truncated secret"),
            "unexpected error: {error}"
        );
    }

    /// Deleting a spanned value must remove every entry it occupies; a
    /// leftover part is a secret that survived a logout.
    #[test]
    fn deleting_a_spanned_value_removes_every_entry() {
        let (_lock_dir, locks) = chunk_locks();
        let raw = CapsBlobLikeWindows::new(1280);
        chunked_put(&raw, "k", &"Q".repeat(4000), 1000, &locks).unwrap();
        assert!(raw.entries.lock().unwrap().len() > 1);

        chunked_delete(&raw, "k", &locks).unwrap();
        assert!(
            raw.entries.lock().unwrap().is_empty(),
            "logout left {:?} behind",
            raw.entries.lock().unwrap().keys().collect::<Vec<_>>()
        );
        assert!(chunked_get(&raw, "k", &locks).unwrap().is_none());
    }

    /// The split must land on `char` boundaries and must count UTF-16 units,
    /// not bytes and not chars — the unit Windows measures. A 4-byte emoji
    /// costs TWO units.
    #[test]
    fn splitting_counts_utf16_units_and_never_breaks_a_char() {
        let value = "🔑".repeat(10); // 10 chars, 20 UTF-16 units, 40 bytes
        assert_eq!(utf16_units(&value), 20);
        let parts = split_by_utf16_units(&value, 5);
        assert_eq!(parts.concat(), value, "the split must be lossless");
        for part in &parts {
            assert!(utf16_units(part) <= 5, "part over budget: {part}");
            assert!(part.chars().count() * 2 == utf16_units(part));
        }
    }

    /// A literal secret must never be mistaken for a manifest, and a manifest
    /// must never be mistaken for a secret.
    #[test]
    fn manifest_parsing_does_not_collide_with_real_secrets() {
        assert!(parse_chunk_manifest("sk-ant-123").is_none());
        assert!(parse_chunk_manifest(r#"{"access_token":"x"}"#).is_none());
        assert!(parse_chunk_manifest("wayland-core-chunked-v1 a 3").is_none());
        assert_eq!(
            parse_chunk_manifest(&render_chunk_manifest(KeyringChunkManifest {
                generation: 'b',
                count: 7,
            })),
            Some(KeyringChunkManifest {
                generation: 'b',
                count: 7,
            })
        );
    }

    /// ABLATION (d). The downgrade must not be permanent: when the keyring
    /// comes back, the secret moves UP and the lower copy is GONE.
    ///
    /// The sequence is asserted, not only the end state. `write-new →
    /// verify-readback → delete-old` is what makes a kill at any point
    /// recoverable; a delete that preceded the write could lose the secret
    /// outright, and for a caller with `load_or_create` semantics losing it
    /// silently MINTS a replacement key instead of failing.
    #[test]
    fn the_ladder_promotes_a_credential_when_a_higher_tier_returns() {
        let keyring = tier(&[]);
        let vault = tier(&[("k", "v-from-vault")]);
        let dir = tempdir().unwrap();
        let legacy_path = dir.path().join("credentials.toml");
        let ladder = LadderCredentialsStore::new(
            Some(boxed(&keyring)),
            Some(boxed(&vault)),
            legacy_path.clone(),
        );

        assert_eq!(
            ladder.get("k").unwrap().as_deref(),
            Some("v-from-vault"),
            "non-vacuity: the read must still return the value while it moves"
        );

        // End state: exactly one copy, in the TOP tier.
        assert_eq!(
            keyring.snapshot(),
            vec![("k".to_string(), "v-from-vault".to_string())],
            "the credential must have been promoted into the keyring"
        );
        assert!(
            vault.snapshot().is_empty(),
            "the lower-tier copy must be gone, not merely shadowed"
        );
        assert!(
            !legacy_path.exists(),
            "promotion must not conjure a cleartext credentials file"
        );

        // Sequence: the keyring is WRITTEN and READ BACK before the vault is
        // touched by a delete.
        let keyring_ops = keyring.ops();
        assert_eq!(
            keyring_ops,
            vec![
                "get:k".to_string(), // ladder read: keyring miss
                "put:k".to_string(), // 1. write-new
                "get:k".to_string(), // 2. verify-readback
            ],
            "unexpected keyring op sequence: {keyring_ops:?}"
        );
        let vault_ops = vault.ops();
        assert_eq!(
            vault_ops,
            vec!["get:k".to_string(), "delete:k".to_string()],
            "the vault must be deleted from only AFTER the readback: {vault_ops:?}"
        );

        // And the value is still readable afterwards, from the new tier.
        assert_eq!(ladder.get("k").unwrap().as_deref(), Some("v-from-vault"));
    }

    /// The other direction of the same rule: a promotion whose destination
    /// write FAILS must leave the lower copy untouched and still readable. A
    /// heal that can lose the secret is worse than no heal.
    #[test]
    fn a_failed_promotion_leaves_the_lower_copy_intact() {
        let keyring = tier(&[]);
        keyring
            .fail_put
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let vault = tier(&[("k", "v-from-vault")]);
        let dir = tempdir().unwrap();
        let ladder = LadderCredentialsStore::new(
            Some(boxed(&keyring)),
            Some(boxed(&vault)),
            dir.path().join("credentials.toml"),
        );

        assert_eq!(
            ladder.get("k").unwrap().as_deref(),
            Some("v-from-vault"),
            "the read must succeed even though the promotion could not"
        );
        assert_eq!(
            vault.snapshot(),
            vec![("k".to_string(), "v-from-vault".to_string())],
            "a failed promotion must NOT delete the only copy"
        );
        assert!(
            !vault.ops().contains(&"delete:k".to_string()),
            "the vault must never be deleted from when the destination write failed"
        );
    }

    /// A `put` maintains the single-copy invariant: after a write to the top
    /// tier, no stale copy is left below. Without this a crash-created (or
    /// migration-created) duplicate would linger as an OLD value that a later
    /// tier regression would silently start serving.
    #[test]
    fn a_put_removes_the_now_stale_copies_below_it() {
        let keyring = tier(&[]);
        let vault = tier(&[("k", "OLD-value")]);
        let dir = tempdir().unwrap();
        let legacy_path = dir.path().join("credentials.toml");
        PlaintextCredentialsStore::new(&legacy_path)
            .put("k", "OLDER-value")
            .unwrap();

        let ladder = LadderCredentialsStore::new(
            Some(boxed(&keyring)),
            Some(boxed(&vault)),
            legacy_path.clone(),
        );
        ladder.put("k", "NEW-value").unwrap();

        assert_eq!(
            keyring.snapshot(),
            vec![("k".to_string(), "NEW-value".to_string())]
        );
        assert!(
            vault.snapshot().is_empty(),
            "the stale vault copy must be removed by the write that superseded it"
        );
        let legacy_body = std::fs::read_to_string(&legacy_path).unwrap();
        assert!(
            !legacy_body.contains("OLDER-value"),
            "the stale cleartext copy must be removed too: {legacy_body}"
        );
        assert_eq!(
            ladder.get("k").unwrap().as_deref(),
            Some("NEW-value"),
            "non-vacuity: the surviving copy is the new one"
        );
    }

    /// `get_many` must return values POSITIONALLY aligned with `keys` when the
    /// answers come from different tiers.
    ///
    /// Written after the first version of the batched path was wrong: it
    /// queried both upper tiers up front against the same index list, then
    /// zipped the vault's answers against a list the keyring pass had already
    /// shortened, so every value below the first keyring hit came back attached
    /// to the WRONG key. That is a credential mix-up, not a cosmetic bug, and it
    /// was invisible to every existing case because they all ran with the
    /// keyring tier absent (the isolated-profile fixture) — the one arrangement
    /// in which no reduction happens. This case interleaves the tiers so the
    /// reduction is forced.
    #[test]
    fn get_many_keeps_values_aligned_when_tiers_answer_different_keys() {
        let keyring = tier(&[("b", "B-keyring"), ("d", "D-keyring")]);
        let vault = tier(&[("c", "C-vault")]);
        let dir = tempdir().unwrap();
        let legacy_path = dir.path().join("credentials.toml");
        let legacy = PlaintextCredentialsStore::new(&legacy_path);
        legacy.put("e", "E-legacy").unwrap();

        let ladder = LadderCredentialsStore::new(
            Some(boxed(&keyring)),
            Some(boxed(&vault)),
            legacy_path.clone(),
        );

        // a: nowhere. b: keyring. c: vault. d: keyring. e: legacy.
        // The keyring answers positions 1 and 3, so the remaining list the
        // vault sees is [0, 2, 4] — a different shape from the original.
        assert_eq!(
            ladder.get_many(&["a", "b", "c", "d", "e"]).unwrap(),
            vec![
                None,
                Some("B-keyring".to_string()),
                Some("C-vault".to_string()),
                Some("D-keyring".to_string()),
                Some("E-legacy".to_string()),
            ]
        );

        // And the batched path re-migrates exactly like the single-key one: the
        // lower-tier answers moved up, and their old homes are empty.
        assert!(
            vault.snapshot().is_empty(),
            "the vault answer must have been promoted out: {:?}",
            vault.snapshot()
        );
        assert_eq!(legacy.get("e").unwrap(), None, "legacy copy must be gone");
        assert_eq!(
            keyring.snapshot(),
            vec![
                ("b".to_string(), "B-keyring".to_string()),
                ("c".to_string(), "C-vault".to_string()),
                ("d".to_string(), "D-keyring".to_string()),
                ("e".to_string(), "E-legacy".to_string()),
            ],
            "every resolved value must now live in the top tier"
        );
    }

    /// Mid-session revocation. The write probe is a HINT — probe→write is a
    /// TOCTOU window by construction — so a keyring that refuses the actual
    /// write must descend the SAME ladder, not take a plaintext branch.
    #[test]
    fn a_keyring_that_fails_mid_session_descends_to_the_vault_not_to_cleartext() {
        let keyring = tier(&[]);
        keyring
            .fail_put
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let vault = tier(&[]);
        let dir = tempdir().unwrap();
        let legacy_path = dir.path().join("credentials.toml");
        let ladder = LadderCredentialsStore::new(
            Some(boxed(&keyring)),
            Some(boxed(&vault)),
            legacy_path.clone(),
        );

        ladder
            .put("k", "v")
            .expect("the vault must catch the write");
        assert_eq!(
            vault.snapshot(),
            vec![("k".to_string(), "v".to_string())],
            "the write must land in the vault"
        );
        assert!(
            !legacy_path.exists(),
            "a mid-session keyring failure must NOT produce a cleartext file — that is \
             the exact edge FallbackCredentialsStore::put took"
        );
        assert_eq!(
            ladder.get("k").unwrap().as_deref(),
            Some("v"),
            "non-vacuity: readable back from the tier that accepted it"
        );
    }

    /// Both secure tiers refuse → the ladder refuses. There is no cleartext arm
    /// to fall into, and the refusal is actionable.
    #[test]
    fn the_ladder_refuses_rather_than_writing_cleartext_when_every_tier_fails() {
        let keyring = tier(&[]);
        keyring
            .fail_put
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let vault = tier(&[]);
        vault
            .fail_put
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let dir = tempdir().unwrap();
        let legacy_path = dir.path().join("credentials.toml");
        let ladder = LadderCredentialsStore::new(
            Some(boxed(&keyring)),
            Some(boxed(&vault)),
            legacy_path.clone(),
        );

        let error = ladder.put("k", "v").expect_err("both tiers refused");
        assert!(
            error.to_string().contains("WAYLAND_VAULT_PASSPHRASE"),
            "the refusal must be actionable: {error}"
        );
        assert!(
            !legacy_path.exists(),
            "a fully-refused write must leave no cleartext file behind"
        );
        // Known-positive for the assertion above: the same ladder DOES write
        // when a tier accepts, so `!exists` is not passing for free.
        vault
            .fail_put
            .store(false, std::sync::atomic::Ordering::SeqCst);
        ladder.put("k", "v").expect("the vault now accepts");
        assert_eq!(ladder.get("k").unwrap().as_deref(), Some("v"));
    }

    /// P3. `purge_profile_confidential_keys` must delete from the service the
    /// profile's own marker records, and must be a no-op (never an error) for a
    /// profile that never opened a confidential store.
    #[test]
    fn purging_a_profile_is_driven_by_its_own_marker_and_is_a_noop_without_one() {
        let dir = tempdir().unwrap();
        let credentials_path = dir.path().join("credentials.toml");

        // No marker → nothing to purge, and specifically NOT an error: a
        // profile that never held confidential material must stay deletable.
        purge_profile_confidential_keys(&credentials_path)
            .expect("a profile with no marker has nothing to purge");

        // A vault-pinned profile is also a no-op here: its key lives in files
        // inside the profile tree, which the caller is about to remove.
        let marker_path = confidential_backend_marker_path(
            &absolute_confidential_path(&credentials_path).unwrap(),
        );
        write_confidential_backend_marker(
            &marker_path,
            &ConfidentialBackendMarker {
                version: CONFIDENTIAL_BACKEND_MARKER_VERSION,
                selection: ConfidentialBackendSelection::EncryptedFile {
                    cipher_path: dir.path().join("credentials.enc"),
                    key_params_path: dir.path().join("credentials.kdf.json"),
                },
                confirmed: true,
            },
        )
        .unwrap();
        purge_profile_confidential_keys(&credentials_path)
            .expect("a vault-pinned profile needs no keyring purge");

        // The key ref the purge targets must be the one the writer uses. This
        // is the whole reason the constant lives in this crate: two spellings
        // is a writer with no deleter, which is the P3 leak.
        assert!(
            CONFIDENTIAL_KEY_REFS.contains(&RECOVERY_PREPARED_REQUEST_KEY_REF),
            "the recovery key ref must be in the purge set"
        );
    }

    /// P3, the half that can actually be wrong, measured without a keyring: the
    /// purge REMOVES the confidential key rather than merely being called.
    ///
    /// The seeded key is written by the same `load_or_create_confidential_blob_key`
    /// path production uses, so this is the real writer's output being deleted
    /// — not a hand-rolled approximation of it that could disagree about the
    /// key ref and pass anyway.
    #[test]
    fn purging_removes_the_confidential_key_the_writer_created() {
        let dir = tempdir().unwrap();
        let backing = tier(&[]);
        let store = ConfidentialCredentialsStore::new(
            boxed(&backing),
            dir.path().join(".credentials.confidential-key.lock"),
            None,
        );

        // The production writer mints and persists the key.
        crate::confidential_blob::load_or_create_confidential_blob_key(
            &store,
            RECOVERY_PREPARED_REQUEST_KEY_REF,
        )
        .expect("the writer must create a key");
        // NON-VACUITY: it is really there, under the ref the purge targets, and
        // really readable, BEFORE the purge. Without this the assertions below
        // pass on a store that was empty all along.
        assert_eq!(
            backing.snapshot().len(),
            1,
            "the writer must have persisted exactly one entry: {:?}",
            backing.snapshot()
        );
        assert_eq!(
            backing.snapshot()[0].0,
            RECOVERY_PREPARED_REQUEST_KEY_REF,
            "the writer must use the ref the purge deletes — two spellings is the P3 leak"
        );
        crate::confidential_blob::load_confidential_blob_key(
            &store,
            RECOVERY_PREPARED_REQUEST_KEY_REF,
        )
        .expect("the key must load back before the purge");

        purge_confidential_keys_from(&store).expect("purge");

        assert!(
            backing.snapshot().is_empty(),
            "the confidential key must be GONE from the backing store: {:?}",
            backing.snapshot()
        );
        assert!(
            crate::confidential_blob::load_confidential_blob_key(
                &store,
                RECOVERY_PREPARED_REQUEST_KEY_REF,
            )
            .is_err(),
            "a purged key must no longer load"
        );

        // Idempotent: deleting an already-absent key is success, so a second
        // profile-delete attempt cannot fail on the first one's work.
        purge_confidential_keys_from(&store).expect("purge is idempotent");
    }

    /// Vault hygiene: a world-readable vault is REFUSED, not loaded. Both
    /// directions in one case, so neither a hardwired refusal nor a hardwired
    /// acceptance can pass it.
    #[cfg(unix)]
    #[test]
    #[serial_test::serial(vault_passphrase_env)]
    fn a_world_readable_vault_is_refused_and_a_0600_one_is_not() {
        use std::os::unix::fs::PermissionsExt;
        let _pass = EnvPassphraseGuard::set("hygiene-pass");
        let _fd = EnvVarGuard::remove("WAYLAND_VAULT_PASSPHRASE_FD");
        let dir = tempdir().unwrap();
        let cipher = dir.path().join("v.enc");
        let params = dir.path().join("v.kdf.json");

        let store = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
        store.put("k", "v").unwrap();

        // Known-positive: at the perms the store itself writes, it loads.
        assert_eq!(
            EncryptedFileCredentialsStore::new(cipher.clone(), params.clone())
                .get("k")
                .unwrap()
                .as_deref(),
            Some("v"),
            "a 0600 vault must load — otherwise the refusal below is not discriminating"
        );

        // The directory the store created must be 0700 (kimi-code's umask
        // lesson: create_dir_all's mode is masked and only applies on create).
        let dir_mode = std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "the vault directory must be 0700, got {dir_mode:#o}"
        );

        // Now loosen it and require a refusal.
        std::fs::set_permissions(&cipher, std::fs::Permissions::from_mode(0o644)).unwrap();
        let error = EncryptedFileCredentialsStore::new(cipher.clone(), params)
            .get("k")
            .expect_err("a world-readable vault must be refused");
        assert!(
            error
                .to_string()
                .contains("readable by accounts other than its owner"),
            "the refusal must say why and how to fix it: {error}"
        );
    }
}

// ===========================================================================
// Spanned-write concurrency proofs.
//
// `chunked_put` chooses its target generation by READING the live manifest and
// taking the other letter. Without a lock that is a read-modify-write with no
// compare-and-swap, so two writers that read the same manifest choose the SAME
// target and interleave their parts into it — and both return `Ok`. These tests
// pin the schedule that does it. They contain no injected faults: an ordinary
// preemption between a writer's last part write and its manifest write is all
// it takes, and any OS may impose one at any time.
//
// Delete the `locks.acquire(key)?` line from `chunked_put` and
// `a_second_writer_cannot_commit_over_a_parked_writers_parts` fails with
// `len=9000 tags=[BA]` — B's first 3000 bytes spliced onto A's last 6000.
// ===========================================================================
#[cfg(test)]
mod chunk_write_lock_verification {
    use super::*;
    use std::collections::HashMap;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Mutex};

    const KEY: &str = "oauth.chatgpt.tokens";
    const UNITS: usize = 1000;

    #[derive(Default)]
    struct Shared {
        entries: Mutex<HashMap<String, String>>,
    }

    /// A view of the shared store that can hand control to the other writer at a
    /// named point. Each individual entry op is atomic, exactly as a keyring
    /// daemon makes it; only the multi-op SEQUENCE is left unserialised, which is
    /// the whole question.
    struct Scheduled {
        shared: Arc<Shared>,
        /// Fired the first time this writer is about to commit its manifest.
        at_manifest: Mutex<Option<Sender<()>>>,
        /// Waited on before that commit proceeds.
        resume: Mutex<Option<Receiver<()>>>,
    }

    impl Scheduled {
        fn plain(shared: &Arc<Shared>) -> Self {
            Self {
                shared: Arc::clone(shared),
                at_manifest: Mutex::new(None),
                resume: Mutex::new(None),
            }
        }
    }

    impl CredentialsStore for Scheduled {
        fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
            Ok(self.shared.entries.lock().unwrap().get(key).cloned())
        }
        fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
            if value.starts_with(KEYRING_CHUNK_MANIFEST_PREFIX) {
                // Writer A is preempted here: its parts are all committed, its
                // manifest is not.
                if let Some(tx) = self.at_manifest.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                if let Some(rx) = self.resume.lock().unwrap().take() {
                    let _ = rx.recv();
                }
            }
            self.shared
                .entries
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), CredentialsError> {
            self.shared.entries.lock().unwrap().remove(key);
            Ok(())
        }
    }

    /// THE RELEASE BLOCKER. Two token refreshes of one provider, scheduled so
    /// that the second writer runs entirely inside the first writer's
    /// read-decide-write window.
    ///
    /// The invariant is not "one of them wins" but "the committed value is one
    /// whole token set". Splicing is worse than losing a write: ChatGPT refresh
    /// tokens rotate and are single-use, so both are burned server-side, the next
    /// `load` gets malformed JSON, and the user must re-authenticate.
    #[test]
    fn a_second_writer_cannot_commit_over_a_parked_writers_parts() {
        let lock_dir = tempfile::tempdir().unwrap();
        let locks = ChunkWriteLockSite::in_dir(lock_dir.path(), LockPolicy::CREDENTIAL_WRITE);
        let shared = Arc::new(Shared::default());
        let seeded = "S".repeat(6000);

        // A live spanned value, generation 'a'.
        chunked_put(&Scheduled::plain(&shared), KEY, &seeded, UNITS, &locks).unwrap();

        let (reached_tx, reached_rx) = channel::<()>();
        let (resume_tx, resume_rx) = channel::<()>();

        // Writer A: the long token set. Parked just before its manifest commit,
        // after all nine of its parts have landed.
        let a_shared = Arc::clone(&shared);
        let a_locks = locks.clone();
        let writer_a = std::thread::spawn(move || {
            let store = Scheduled {
                shared: a_shared,
                at_manifest: Mutex::new(Some(reached_tx)),
                resume: Mutex::new(Some(resume_rx)),
            };
            chunked_put(&store, KEY, &"A".repeat(9000), UNITS, &a_locks)
        });
        reached_rx
            .recv()
            .expect("writer A reaches its manifest commit");

        // Writer B: a short token set, launched while A is parked.
        let b_shared = Arc::clone(&shared);
        let b_locks = locks.clone();
        let (b_entered_tx, b_entered_rx) = channel::<()>();
        let writer_b = std::thread::spawn(move || {
            let store = Scheduled::plain(&b_shared);
            b_entered_tx.send(()).unwrap();
            chunked_put(&store, KEY, &"B".repeat(3000), UNITS, &b_locks)
        });
        b_entered_rx.recv().expect("writer B starts");

        // B must make no progress while A holds the lock. The lock polls every
        // 50ms, so this gives it several real chances to break in. This can only
        // fail if the lock does not hold; a slow host makes it MORE likely to
        // pass, so it is not a timing flake.
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(
            chunked_get(&Scheduled::plain(&shared), KEY, &locks)
                .unwrap()
                .as_deref(),
            Some(seeded.as_str()),
            "while writer A is inside its read-decide-write, no other writer may commit"
        );

        resume_tx.send(()).unwrap();
        writer_a.join().unwrap().expect("writer A reports success");
        writer_b.join().unwrap().expect("writer B reports success");

        let final_value = chunked_get(&Scheduled::plain(&shared), KEY, &locks)
            .unwrap()
            .expect("a credential must still be present");
        let mut tags: Vec<char> = final_value.chars().collect();
        tags.dedup();
        let tags: String = tags.into_iter().collect();
        assert_eq!(
            final_value,
            "B".repeat(3000),
            "the writers must serialize (A whole, then B whole); got len={} tags=[{tags}]",
            final_value.len()
        );
    }

    /// Free-running version of the same thing: no scheduling at all, just many
    /// rounds of two racing refreshes. Weaker than the pinned schedule above and
    /// kept for exactly that reason — it is the shape a real host produces.
    #[test]
    fn racing_writers_never_yield_a_spliced_credential() {
        let lock_dir = tempfile::tempdir().unwrap();
        let locks = ChunkWriteLockSite::in_dir(lock_dir.path(), LockPolicy::CREDENTIAL_WRITE);
        let candidates = ["A".repeat(9000), "B".repeat(3000)];

        for attempt in 0..50 {
            let shared = Arc::new(Shared::default());
            chunked_put(
                &Scheduled::plain(&shared),
                KEY,
                &"S".repeat(6000),
                UNITS,
                &locks,
            )
            .unwrap();

            let barrier = Arc::new(std::sync::Barrier::new(candidates.len()));
            let mut handles = Vec::new();
            for value in candidates.clone() {
                let shared = Arc::clone(&shared);
                let barrier = Arc::clone(&barrier);
                let locks = locks.clone();
                handles.push(std::thread::spawn(move || {
                    barrier.wait();
                    chunked_put(&Scheduled::plain(&shared), KEY, &value, UNITS, &locks)
                }));
            }
            for handle in handles {
                handle.join().unwrap().expect("both writers must succeed");
            }

            let value = chunked_get(&Scheduled::plain(&shared), KEY, &locks)
                .unwrap()
                .expect("a credential must still be present");
            assert!(
                candidates.contains(&value),
                "attempt {attempt}: the store committed a value that is NEITHER token set \
                 (len={})",
                value.len()
            );
        }
    }

    /// A holder that DIED still holds its lockfile — `abort`, SIGKILL and a power
    /// cut all skip `Drop`. The lock must recover itself rather than wedge every
    /// future credential write on the profile.
    #[test]
    fn a_dead_holders_lock_is_recovered_rather_than_wedging_writes() {
        let lock_dir = tempfile::tempdir().unwrap();
        let policy = LockPolicy {
            stale_after: std::time::Duration::from_millis(50),
            wait_ceiling: std::time::Duration::from_secs(5),
            heartbeat: None,
        };
        let locks = ChunkWriteLockSite::in_dir(lock_dir.path(), policy);

        // Exactly what a killed holder leaves behind: the lockfile, another
        // process's nonce in it, and nobody to remove it.
        let abandoned = locks.lock_path(KEY);
        std::fs::create_dir_all(lock_dir.path()).unwrap();
        std::fs::write(&abandoned, "9999999-0").unwrap();

        let shared = Arc::new(Shared::default());
        chunked_put(
            &Scheduled::plain(&shared),
            KEY,
            &"R".repeat(4000),
            UNITS,
            &locks,
        )
        .expect("a write must not be wedged forever by a dead holder");
        assert_eq!(
            chunked_get(&Scheduled::plain(&shared), KEY, &locks)
                .unwrap()
                .as_deref(),
            Some("R".repeat(4000).as_str())
        );
        assert!(
            !abandoned.exists(),
            "the recovered lock must be released again on drop"
        );
    }

    /// A lock that cannot be taken is a REFUSAL. Falling through to an unlocked
    /// write would restore the defect precisely when contention is highest.
    #[test]
    fn a_write_that_cannot_take_the_lock_refuses_rather_than_racing() {
        let lock_dir = tempfile::tempdir().unwrap();
        let policy = LockPolicy {
            // Long enough that the live holder below is never judged stale.
            stale_after: std::time::Duration::from_secs(60),
            wait_ceiling: std::time::Duration::from_millis(120),
            heartbeat: None,
        };
        let locks = ChunkWriteLockSite::in_dir(lock_dir.path(), policy);
        let shared = Arc::new(Shared::default());
        chunked_put(
            &Scheduled::plain(&shared),
            KEY,
            &"L".repeat(4000),
            UNITS,
            &locks,
        )
        .unwrap();

        let _held = locks.acquire(KEY).expect("first acquisition");
        let error = chunked_put(
            &Scheduled::plain(&shared),
            KEY,
            &"M".repeat(4000),
            UNITS,
            &locks,
        )
        .expect_err("a write that cannot serialize must not proceed");
        assert!(
            error.to_string().contains("credential write"),
            "the refusal must name the lock it could not take: {error}"
        );
        assert_eq!(
            chunked_get(&Scheduled::plain(&shared), KEY, &locks)
                .unwrap()
                .as_deref(),
            Some("L".repeat(4000).as_str()),
            "the refused write must leave the live credential untouched"
        );
    }

    /// Lock identity must track the OS keyring entry, not the caller. Two
    /// services, or two keys, are two locks; the same pair is one lock however
    /// many store handles exist.
    #[test]
    fn lock_identity_follows_the_service_and_key() {
        let one = ChunkWriteLockSite::for_service("wayland-core");
        let two = ChunkWriteLockSite::for_service("wayland-core");
        let other = ChunkWriteLockSite::for_service("wayland-core.profile.deadbeef");

        assert_eq!(
            one.lock_path(KEY),
            two.lock_path(KEY),
            "two handles on one service+key must contend for ONE lock"
        );
        assert_ne!(
            one.lock_path(KEY),
            one.lock_path("oauth.claude.tokens"),
            "different credentials must not contend"
        );
        assert_ne!(
            one.lock_path(KEY),
            other.lock_path(KEY),
            "different keyring services address different entries"
        );
        // A key that is not a legal filename must still get a lock.
        let awkward = one.lock_path("oauth./../../etc/passwd\0.tokens");
        assert_eq!(awkward.parent(), Some(one.dir.as_path()));
    }
}

// ===========================================================================
// A read fault on the primary entry must ABORT the write.
//
// `chunked_put` used to open with `raw.get(key).ok().flatten()`, so a read ERROR
// was indistinguishable from "there is no previous manifest" and selected
// generation 'a'. When the live value already IS generation 'a' that overwrites
// the live parts IN PLACE, under a manifest still counting the old part total —
// the one thing the commit order exists to prevent, reached with no concurrency
// at all.
//
// It is ordinary to reach: `keyring_available` is cached for the life of the
// process, so a keyring that locks AFTER startup (screen-locked Secret Service,
// a denied Keychain prompt, a transient Windows RPC failure) lands here.
// ===========================================================================
#[cfg(test)]
mod chunk_read_fault_verification {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    const KEY: &str = "oauth.chatgpt.tokens";
    const UNITS: usize = 1000;

    #[derive(Default)]
    struct FaultingStore {
        entries: Mutex<HashMap<String, String>>,
        fail_primary_get: AtomicBool,
    }

    impl CredentialsStore for FaultingStore {
        fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
            if self.fail_primary_get.load(Ordering::SeqCst) && !key.contains("__wlchunk") {
                return Err(CredentialsError::Keyring("injected read fault".into()));
            }
            Ok(self.entries.lock().unwrap().get(key).cloned())
        }
        fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
            self.entries
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), CredentialsError> {
            self.entries.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn locks(dir: &std::path::Path) -> ChunkWriteLockSite {
        ChunkWriteLockSite::in_dir(dir, LockPolicy::CREDENTIAL_WRITE)
    }

    /// Shrinking is the sharp case: the new value needs FEWER parts than the live
    /// manifest counts, so an in-place reuse of generation 'a' leaves the reader
    /// splicing the new head onto the old tail.
    #[test]
    fn a_read_fault_on_the_primary_entry_aborts_the_write() {
        let lock_dir = tempfile::tempdir().unwrap();
        let locks = locks(lock_dir.path());
        let raw = FaultingStore::default();
        let live = "O".repeat(9000);
        chunked_put(&raw, KEY, &live, UNITS, &locks).unwrap();

        raw.fail_primary_get.store(true, Ordering::SeqCst);
        let error = chunked_put(&raw, KEY, &"N".repeat(2500), UNITS, &locks)
            .expect_err("a write that cannot read the live manifest must abort");
        assert!(
            error.to_string().contains("injected read fault"),
            "the write must surface the read fault, not invent a fresh generation: {error}"
        );

        raw.fail_primary_get.store(false, Ordering::SeqCst);
        assert_eq!(
            chunked_get(&raw, KEY, &locks).unwrap().as_deref(),
            Some(live.as_str()),
            "the live credential must be byte-identical after the aborted write"
        );
    }

    /// The same for delete. Proceeding without the manifest would remove the
    /// primary and strand the parts: a logout reporting success while the refresh
    /// token's fragments stay in the OS keyring, unreferenced and unreachable by
    /// any later call.
    #[test]
    fn a_read_fault_aborts_a_delete_rather_than_stranding_the_parts() {
        let lock_dir = tempfile::tempdir().unwrap();
        let locks = locks(lock_dir.path());
        let raw = FaultingStore::default();
        chunked_put(&raw, KEY, &"O".repeat(9000), UNITS, &locks).unwrap();
        let before = raw.entries.lock().unwrap().len();

        raw.fail_primary_get.store(true, Ordering::SeqCst);
        chunked_delete(&raw, KEY, &locks).expect_err("a blind delete must not proceed");

        raw.fail_primary_get.store(false, Ordering::SeqCst);
        assert_eq!(
            raw.entries.lock().unwrap().len(),
            before,
            "the refused delete must not have removed anything"
        );

        // And once the keyring answers again, the delete removes every entry.
        chunked_delete(&raw, KEY, &locks).unwrap();
        assert!(
            raw.entries.lock().unwrap().is_empty(),
            "logout left {:?} behind",
            raw.entries.lock().unwrap().keys().collect::<Vec<_>>()
        );
    }

    /// An absent primary entry is NOT a fault: a first write must still work.
    #[test]
    fn an_absent_primary_entry_is_not_a_read_fault() {
        let lock_dir = tempfile::tempdir().unwrap();
        let locks = locks(lock_dir.path());
        let raw = FaultingStore::default();
        chunked_put(&raw, KEY, &"F".repeat(4000), UNITS, &locks)
            .expect("a first write has no previous manifest and must succeed");
        assert_eq!(
            chunked_get(&raw, KEY, &locks).unwrap().as_deref(),
            Some("F".repeat(4000).as_str())
        );
    }

    /// The per-backend ceiling (c): the Windows number is 1024× too small for
    /// macOS and Linux, and applying it there put both platforms on the spanned
    /// path — where the two defects above live — for a value that fits in one
    /// entry.
    #[test]
    fn the_per_entry_ceiling_is_the_backends_own() {
        let ceiling = keyring_max_utf16_units_per_entry();
        if cfg!(windows) {
            // MEASURED: Windows Credential Manager refuses above 1280 UTF-16
            // units. A threshold at or above that makes `auth login` fail.
            assert!(
                ceiling < 1280,
                "the threshold must stay under the measured Windows blob cap"
            );
            assert_eq!(ceiling, WINDOWS_MAX_UTF16_UNITS_PER_ENTRY);
        } else {
            // MEASURED: macOS Keychain and Linux Secret Service both accepted
            // 1,024,000 units in one entry. Stay an order of magnitude under
            // that floor — the probe proved they accept AT LEAST that much, not
            // that it is the limit...
            assert!(
                ceiling <= 1_024_000 / 8,
                "keep a wide margin under the measured floor"
            );
            // ...and far enough above the Windows figure that no realistic
            // credential is spanned at all.
            assert!(
                ceiling > 20_000,
                "applying the Windows figure here is what put macOS and Linux on \
                 the spanned path for a value that fits in one entry"
            );
        }
    }

    /// Defence in depth, stated as a property: an OAuth-sized token set does not
    /// span on macOS or Linux, and still does on Windows.
    #[test]
    fn an_oauth_token_set_only_spans_where_the_backend_forces_it() {
        let lock_dir = tempfile::tempdir().unwrap();
        let locks = locks(lock_dir.path());
        let raw = FaultingStore::default();
        let jwt = format!("hdr.{}.sig", "P".repeat(1400));
        let token_set = format!(
            r#"{{"access_token":"{jwt}","id_token":"{jwt}","refresh_token":"{}"}}"#,
            "R".repeat(500)
        );
        assert!(
            utf16_units(&token_set) > 1280,
            "the fixture must exceed the Windows cap"
        );

        chunked_put(
            &raw,
            KEY,
            &token_set,
            keyring_max_utf16_units_per_entry(),
            &locks,
        )
        .unwrap();
        let spanned = raw
            .entries
            .lock()
            .unwrap()
            .keys()
            .any(|key| key.contains("__wlchunk"));
        assert_eq!(
            spanned,
            cfg!(windows),
            "a token set must span only where the backend's own ceiling forces it"
        );
        assert_eq!(
            chunked_get(&raw, KEY, &locks).unwrap().as_deref(),
            Some(token_set.as_str())
        );
    }

    /// A value an older build spanned under the 1000-unit threshold must still
    /// read back after the threshold is raised, and the next rewrite must collapse
    /// it to one entry and purge the parts.
    #[test]
    fn values_spanned_by_an_older_build_still_read_and_then_collapse() {
        let lock_dir = tempfile::tempdir().unwrap();
        let locks = locks(lock_dir.path());
        let raw = FaultingStore::default();
        let legacy = "V".repeat(4000);

        // Written the way the previous build wrote it, everywhere.
        chunked_put(&raw, KEY, &legacy, 1000, &locks).unwrap();
        assert!(
            raw.entries
                .lock()
                .unwrap()
                .keys()
                .any(|key| key.contains("__wlchunk")),
            "the fixture must actually be spanned"
        );
        assert_eq!(
            chunked_get(&raw, KEY, &locks).unwrap().as_deref(),
            Some(legacy.as_str()),
            "raising the threshold must not orphan values written under the old one"
        );

        chunked_put(
            &raw,
            KEY,
            &legacy,
            keyring_max_utf16_units_per_entry(),
            &locks,
        )
        .unwrap();
        let still_spanned = raw
            .entries
            .lock()
            .unwrap()
            .keys()
            .any(|key| key.contains("__wlchunk"));
        assert_eq!(
            still_spanned,
            cfg!(windows),
            "off Windows the rewrite must collapse to one entry and purge the parts"
        );
        assert_eq!(
            chunked_get(&raw, KEY, &locks).unwrap().as_deref(),
            Some(legacy.as_str())
        );
    }
}

// ===========================================================================
// Crash safety, asserted by EXECUTION rather than by reading the commit order.
//
// A child process is killed with `abort()` at every mutating step of a spanned
// write; the parent then reads the store back and demands the complete OLD
// value, the complete NEW value, or a clean error — never a mix and never a
// prefix. Ported from the verify-cred lane so the property the write lock must
// not break keeps being measured after every change to this path.
// ===========================================================================
#[cfg(test)]
mod chunk_crash_injection {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// One file per entry, each written temp+rename.
    ///
    /// This models what a keyring daemon actually gives us: an individual entry
    /// commit is atomic and out of our address space, so killing OUR process can
    /// never leave one ENTRY half-written — it can only leave the SEQUENCE of
    /// entry writes half-finished. That sequence is precisely what the
    /// "parts → manifest → purge" order claims to make safe.
    struct FileStore {
        dir: PathBuf,
        /// `abort()` immediately BEFORE the op with this 0-based index, so
        /// `crash_at = n` means ops 0..n landed and op n never happened.
        crash_at: usize,
        ops: AtomicUsize,
    }

    impl FileStore {
        fn open(dir: &Path) -> Self {
            Self {
                dir: dir.to_path_buf(),
                crash_at: usize::MAX,
                ops: AtomicUsize::new(0),
            }
        }
        fn crashing(dir: &Path, crash_at: usize) -> Self {
            Self {
                crash_at,
                ..Self::open(dir)
            }
        }
        fn path(&self, key: &str) -> PathBuf {
            let mut name = String::new();
            for byte in key.as_bytes() {
                name.push_str(&format!("{byte:02x}"));
            }
            self.dir.join(name)
        }
        /// Count this mutating op; abort the process if it is the injected one.
        fn tick(&self) {
            if self.ops.fetch_add(1, Ordering::SeqCst) == self.crash_at {
                // Hard kill. No unwinding, no Drop (so the write lock is left
                // exactly as a crashed holder leaves it), no buffer flush — the
                // closest thing to a power cut a test can produce.
                std::process::abort();
            }
        }
        fn entry_names(dir: &Path) -> Vec<String> {
            let mut out: Vec<String> = std::fs::read_dir(dir)
                .unwrap()
                .filter_map(|entry| {
                    let name = entry.ok()?.file_name().to_string_lossy().into_owned();
                    let bytes: Vec<u8> = (0..name.len())
                        .step_by(2)
                        .filter_map(|i| u8::from_str_radix(name.get(i..i + 2)?, 16).ok())
                        .collect();
                    String::from_utf8(bytes).ok()
                })
                .collect();
            out.sort();
            out
        }
    }

    impl CredentialsStore for FileStore {
        fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
            match std::fs::read_to_string(self.path(key)) {
                Ok(value) => Ok(Some(value)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(CredentialsError::Io(e)),
            }
        }
        fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
            self.tick();
            let final_path = self.path(key);
            let tmp = final_path.with_extension("tmp");
            std::fs::write(&tmp, value)?;
            std::fs::rename(&tmp, &final_path)?;
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), CredentialsError> {
            self.tick();
            match std::fs::remove_file(self.path(key)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
    }

    const KEY: &str = "oauth.chatgpt.tokens";
    const UNITS: usize = 1000;

    /// The lock a crashed holder leaves behind must not wedge the survivor. A
    /// short staleness window keeps the sweep quick; only ONE writer is ever live
    /// here, so nothing is being raced.
    fn sweep_locks(dir: &Path) -> ChunkWriteLockSite {
        ChunkWriteLockSite::in_dir(
            dir,
            LockPolicy {
                stale_after: std::time::Duration::from_millis(20),
                wait_ceiling: std::time::Duration::from_secs(10),
                heartbeat: None,
            },
        )
    }

    fn value_of(tag: char, len: usize) -> String {
        String::from(tag).repeat(len)
    }

    // -- child mode -------------------------------------------------------
    // Re-entered as a subprocess by the sweeps below. Not a test in its own
    // right; it early-returns when the harness env is absent.
    #[test]
    fn crash_child() {
        let Ok(dir) = std::env::var("WLV_DIR") else {
            return;
        };
        let lock_dir = std::env::var("WLV_LOCK_DIR").unwrap();
        let crash_at: usize = std::env::var("WLV_CRASH_AT").unwrap().parse().unwrap();
        let new_tag = std::env::var("WLV_NEW_TAG")
            .unwrap()
            .chars()
            .next()
            .unwrap();
        let new_len: usize = std::env::var("WLV_NEW_LEN").unwrap().parse().unwrap();

        let store = FileStore::crashing(Path::new(&dir), crash_at);
        let locks = sweep_locks(Path::new(&lock_dir));
        let _ = chunked_put(&store, KEY, &value_of(new_tag, new_len), UNITS, &locks);
        // Reaching here means the injected index was past the end of the write.
        std::process::exit(0);
    }

    fn run_child(dir: &Path, lock_dir: &Path, crash_at: usize, new_tag: char, new_len: usize) {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "credentials::chunk_crash_injection::crash_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("WLV_DIR", dir)
            .env("WLV_LOCK_DIR", lock_dir)
            .env("WLV_CRASH_AT", crash_at.to_string())
            .env("WLV_NEW_TAG", new_tag.to_string())
            .env("WLV_NEW_LEN", new_len.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("spawn crash child");
        let _ = status;
    }

    /// Kill a spanned rewrite at EVERY step and demand the store still reads as
    /// exactly one whole value (or refuses).
    ///
    /// `old_len`/`new_len` pick the shape: growing, shrinking and
    /// literal<->spanned transitions each move the part count differently, and the
    /// shrink case is the one where reusing a generation would tear.
    fn sweep(label: &str, old_len: usize, new_len: usize) -> Vec<String> {
        let old = value_of('O', old_len);
        let new = value_of('N', new_len);
        let mut verdicts = Vec::new();

        for crash_at in 0..24usize {
            let tmp = tempfile::tempdir().unwrap();
            let lock_tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path();
            let lock_dir = lock_tmp.path();
            let locks = sweep_locks(lock_dir);

            // Seed the OLD value with a clean, uninterrupted write.
            chunked_put(&FileStore::open(dir), KEY, &old, UNITS, &locks).unwrap();
            assert_eq!(
                chunked_get(&FileStore::open(dir), KEY, &locks)
                    .unwrap()
                    .as_deref(),
                Some(old.as_str()),
                "{label}: seeding is broken"
            );

            run_child(dir, lock_dir, crash_at, 'N', new_len);

            let read = chunked_get(&FileStore::open(dir), KEY, &locks);
            let verdict = match &read {
                Ok(Some(v)) if *v == old => "OLD".to_string(),
                Ok(Some(v)) if *v == new => "NEW".to_string(),
                Ok(Some(v)) => {
                    let tags: String = {
                        let mut t: Vec<char> = v.chars().collect();
                        t.dedup();
                        t.into_iter().collect()
                    };
                    format!(
                        "CORRUPT(len={} of old={old_len}/new={new_len}, tags=[{tags}])",
                        v.len()
                    )
                }
                Ok(None) => "ABSENT".to_string(),
                Err(_) => "ERR".to_string(),
            };
            verdicts.push(format!("{crash_at}:{verdict}"));

            assert!(
                !verdict.starts_with("CORRUPT"),
                "{label}: killing the writer before mutating op {crash_at} produced a value \
                 that is neither the old secret nor the new one — {verdict}"
            );
            assert_ne!(
                verdict, "ABSENT",
                "{label}: killing the writer before mutating op {crash_at} DESTROYED the \
                 credential (the store now reports no value at all)"
            );
        }
        verdicts
    }

    #[test]
    fn spanned_rewrite_survives_a_kill_at_every_step_same_size() {
        println!(
            "SWEEP same-size(4000->4000): {:?}",
            sweep("same-size", 4000, 4000)
        );
    }

    #[test]
    fn spanned_rewrite_survives_a_kill_at_every_step_growing() {
        println!(
            "SWEEP growing(2500->9000): {:?}",
            sweep("growing", 2500, 9000)
        );
    }

    /// The sharp one. The new value needs FEWER parts than the live manifest
    /// counts, so any in-place reuse of the live generation leaves the reader
    /// splicing new parts onto an old tail.
    #[test]
    fn spanned_rewrite_survives_a_kill_at_every_step_shrinking() {
        println!(
            "SWEEP shrinking(9000->2500): {:?}",
            sweep("shrinking", 9000, 2500)
        );
    }

    #[test]
    fn spanned_to_literal_survives_a_kill_at_every_step() {
        println!(
            "SWEEP spanned->literal(9000->40): {:?}",
            sweep("spanned->literal", 9000, 40)
        );
    }

    #[test]
    fn literal_to_spanned_survives_a_kill_at_every_step() {
        println!(
            "SWEEP literal->spanned(40->9000): {:?}",
            sweep("literal->spanned", 40, 9000)
        );
    }

    /// Do interrupted writes leak entries without bound across many rotations,
    /// and can an orphan ever be spliced into a later generation?
    #[test]
    fn interrupted_rotations_do_not_leak_entries_without_bound() {
        let tmp = tempfile::tempdir().unwrap();
        let lock_tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let lock_dir = lock_tmp.path();
        let locks = sweep_locks(lock_dir);
        chunked_put(
            &FileStore::open(dir),
            KEY,
            &value_of('O', 2000),
            UNITS,
            &locks,
        )
        .unwrap();

        let mut census = Vec::new();
        for round in 0..40usize {
            // Alternate long/short so part counts move around, and inject a kill
            // partway through the part-writing phase every round.
            let len = if round % 2 == 0 { 12000 } else { 2600 };
            let tag = char::from(b'A' + (round % 20) as u8);
            run_child(dir, lock_dir, 2, tag, len);

            // Then a clean write of the same value, so the store settles. It must
            // not be wedged by the lockfile the aborted child left behind.
            let settled = value_of(tag, len);
            chunked_put(&FileStore::open(dir), KEY, &settled, UNITS, &locks).unwrap();
            assert_eq!(
                chunked_get(&FileStore::open(dir), KEY, &locks)
                    .unwrap()
                    .as_deref(),
                Some(settled.as_str()),
                "round {round}: a settled write after an interrupted one did not read back \
                 as itself — an orphan was spliced in"
            );
            census.push(FileStore::entry_names(dir).len());
        }
        println!("CENSUS entries-after-each-round: {census:?}");
        println!("CENSUS final entries: {:?}", FileStore::entry_names(dir));

        let first_half = census[..20].iter().max().copied().unwrap();
        let second_half = census[20..].iter().max().copied().unwrap();
        assert_eq!(
            first_half, second_half,
            "entry count is still growing after 40 rotations ({first_half} -> {second_half}); \
             orphans leak without bound"
        );
    }
}
