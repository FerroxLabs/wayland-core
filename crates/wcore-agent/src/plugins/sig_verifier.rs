//! Sec6: ed25519 plugin signature verification (unified, v0.6.5 Task 1.3 fixup).
//!
//! ONE verification path: [`verify_path_plugin_signature`] reads
//! `<plugin_dir>/wayland-plugin.sig` (raw 64-byte ed25519 signature over the
//! entry binary bytes) and verifies against the UNION of:
//!
//! * filesystem-side trusted keys — every `*.pub` file (raw 32-byte ed25519
//!   public key) in the trust-anchor directory (`$WAYLAND_TRUSTED_KEYS_DIR`
//!   or `~/.wayland/trusted-keys`), and
//! * config-side trusted keys — every base64-encoded entry in
//!   `PluginsConfig.trusted_plugin_keys` (parsed by the loader before this
//!   function is called).
//!
//! Acceptance on first match against either source. Static plugins
//! (`plugin_path() == None`) always skip — the engine binary is their trust
//! anchor.
//!
//! Dev escape: `WAYLAND_PLUGIN_TRUST_UNSIGNED=1` allows unsigned path-based
//! plugins to load. The loader logs a warning when set; NEVER the default.

use ed25519_dalek::{Signature, VerifyingKey, ed25519::signature::Verifier};
use std::path::{Path, PathBuf};
use wcore_plugin_api::{PluginError, PluginResult};

/// Filename of the detached signature inside a plugin directory (v0.6.5 Task 1.3).
pub const PLUGIN_SIG_FILENAME: &str = "wayland-plugin.sig";

/// Env var that opts a path-based plugin out of signature verification (dev/CI only).
pub const ENV_TRUST_UNSIGNED: &str = "WAYLAND_PLUGIN_TRUST_UNSIGNED";

/// Env var that overrides the trust-anchor directory (tests / sandboxed deployments).
pub const ENV_TRUSTED_KEYS_DIR: &str = "WAYLAND_TRUSTED_KEYS_DIR";

/// Origin of a trusted key, kept alongside the key for log clarity.
#[derive(Debug, Clone)]
pub enum KeySource {
    /// `*.pub` file inside the trust-anchor directory.
    Filesystem(PathBuf),
    /// Entry in `PluginsConfig.trusted_plugin_keys` (zero-based index).
    Config(usize),
}

impl std::fmt::Display for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeySource::Filesystem(p) => write!(f, "fs:{}", p.display()),
            KeySource::Config(i) => write!(f, "config:{i}"),
        }
    }
}

/// Resolve the trust-anchor directory.
///
/// 1. `$WAYLAND_TRUSTED_KEYS_DIR` if set.
/// 2. `<WAYLAND_HOME or ~/.wayland>/trusted-keys` otherwise.
///
/// Isolation: the fallback routes through `wcore_config::config::profile_home()`
/// so each profile has its OWN trust-anchor set (a key trusted in profile A
/// must not validate plugins in profile B). Byte-identical to
/// `~/.wayland/trusted-keys` when `WAYLAND_HOME` is unset.
pub fn trusted_keys_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var(ENV_TRUSTED_KEYS_DIR)
        && !d.is_empty()
    {
        return Some(PathBuf::from(d));
    }
    Some(wcore_config::config::profile_home().join("trusted-keys"))
}

/// Load every `*.pub` file in `dir` as a raw 32-byte ed25519 public key,
/// tagged with its filesystem origin. Malformed / wrong-size files are
/// logged and skipped. Returns empty vec when the directory is absent or
/// empty.
pub fn load_filesystem_keys(dir: &Path) -> Vec<(VerifyingKey, KeySource)> {
    let mut keys = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return keys,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("pub") {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "trusted key file unreadable — skipping");
                continue;
            }
        };
        let arr: [u8; 32] = match bytes.as_slice().try_into() {
            Ok(a) => a,
            Err(_) => {
                tracing::warn!(
                    file = %path.display(),
                    len = bytes.len(),
                    "trusted key file is not 32 raw ed25519 bytes — skipping"
                );
                continue;
            }
        };
        match VerifyingKey::from_bytes(&arr) {
            Ok(k) => keys.push((k, KeySource::Filesystem(path.clone()))),
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "trusted key parse failed — skipping")
            }
        }
    }
    keys
}

/// First 8 bytes of the key, lowercase-hex, for log identification.
pub fn key_fingerprint(key: &VerifyingKey) -> String {
    let bytes = key.as_bytes();
    let mut s = String::with_capacity(16);
    for b in &bytes[..8] {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// v0.6.5 Task 1.3 (fixup): verify a path-based plugin against the UNION of
/// filesystem-side and config-side trusted keys.
///
/// Reads `<plugin_path.parent()>/wayland-plugin.sig` (raw 64-byte ed25519
/// signature over the entry binary bytes). Verifies against every key in
/// `union_keys`; accepts on first match. Callers MUST have already checked
/// the `WAYLAND_PLUGIN_TRUST_UNSIGNED` escape — this function does NOT
/// consult it.
///
/// Errors:
/// - [`PluginError::SignatureMissing`] when `wayland-plugin.sig` is absent.
/// - [`PluginError::ConfigError`] when `union_keys` is empty (no anchor to
///   verify against — refuse rather than silently accept).
/// - [`PluginError::SignatureVerificationFailed`] when no key accepts the sig.
pub fn verify_path_plugin_signature(
    plugin_name: &str,
    plugin_path: &Path,
    union_keys: &[(VerifyingKey, KeySource)],
) -> PluginResult<()> {
    let plugin_dir = plugin_path.parent().ok_or_else(|| {
        PluginError::ConfigError(format!(
            "plugin {plugin_name}: plugin_path {plugin_path:?} has no parent directory"
        ))
    })?;

    let binary_bytes =
        std::fs::read(plugin_path).map_err(|_| PluginError::SignatureVerificationFailed {
            plugin: plugin_name.to_string(),
        })?;

    verify_plugin_signature_bytes(plugin_name, plugin_dir, &binary_bytes, union_keys)
}

/// Aud-14/Aud-18 (verify-vs-execute TOCTOU): verify a signature over
/// already-read binary bytes, rather than re-reading the path.
///
/// This is the bytes-first verification core. Callers that will subsequently
/// EXECUTE a plugin artifact MUST read the bytes once, verify them here, and
/// then execute those SAME bytes (e.g. via `WasmPluginRunner::load_from_bytes`)
/// — never re-open the path between verify and execute, or an attacker who
/// swaps the file in the gap gets unverified code run.
///
/// `plugin_dir` is the directory containing both the artifact and its detached
/// `wayland-plugin.sig`. `binary_bytes` are the exact artifact bytes the caller
/// will run.
pub fn verify_plugin_signature_bytes(
    plugin_name: &str,
    plugin_dir: &Path,
    binary_bytes: &[u8],
    union_keys: &[(VerifyingKey, KeySource)],
) -> PluginResult<()> {
    let sig_path = plugin_dir.join(PLUGIN_SIG_FILENAME);

    let sig_bytes = std::fs::read(&sig_path).map_err(|_| PluginError::SignatureMissing {
        plugin: plugin_name.to_string(),
        sig_path: sig_path.display().to_string(),
    })?;
    let signature = parse_signature(plugin_name, &sig_bytes)?;

    if union_keys.is_empty() {
        return Err(PluginError::ConfigError(format!(
            "plugin {plugin_name}: no trusted keys available — add ed25519 *.pub \
             files to the trust-anchor directory (default ~/.wayland/trusted-keys, \
             override via WAYLAND_TRUSTED_KEYS_DIR) or set trusted_plugin_keys in \
             plugins.toml, or set WAYLAND_PLUGIN_TRUST_UNSIGNED=1 (DEV ONLY)"
        )));
    }

    for (key, source) in union_keys {
        if key.verify(binary_bytes, &signature).is_ok() {
            tracing::info!(
                plugin = plugin_name,
                key_fp = %key_fingerprint(key),
                key_source = %source,
                "plugin signature verified"
            );
            return Ok(());
        }
    }

    let attempted: Vec<String> = union_keys
        .iter()
        .map(|(k, s)| format!("{} ({s})", key_fingerprint(k)))
        .collect();
    tracing::error!(
        plugin = plugin_name,
        sig_path = %sig_path.display(),
        tried_keys = ?attempted,
        "plugin signature did not verify against any trusted key"
    );
    Err(PluginError::SignatureVerificationFailed {
        plugin: plugin_name.to_string(),
    })
}

fn parse_signature(plugin_name: &str, bytes: &[u8]) -> PluginResult<Signature> {
    let arr: [u8; 64] = bytes
        .try_into()
        .map_err(|_| PluginError::SignatureVerificationFailed {
            plugin: plugin_name.to_string(),
        })?;
    Ok(Signature::from_bytes(&arr))
}

/// Parse a base64-encoded verifying key (base64 of 32 raw bytes).
/// Returns `None` for malformed input; callers log and skip bad entries.
pub fn parse_verifying_key_b64(b64: &str) -> Option<VerifyingKey> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    VerifyingKey::from_bytes(&arr).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{SigningKey, ed25519::signature::Signer};
    use rand::rngs::OsRng;
    use tempfile::TempDir;

    /// Isolation (Phase 0): the trust-anchor dir follows `WAYLAND_HOME` so each
    /// profile has its OWN trust set, and the explicit `$WAYLAND_TRUSTED_KEYS_DIR`
    /// override still wins over it.
    #[test]
    #[serial_test::serial(wayland_home_env)]
    fn trusted_keys_dir_follows_wayland_home_and_env_override() {
        let wh = "WAYLAND_HOME";
        let tk = ENV_TRUSTED_KEYS_DIR;
        let prev_wh = std::env::var_os(wh);
        let prev_tk = std::env::var_os(tk);
        // 1. Fallback roots under WAYLAND_HOME (per-profile trust set).
        unsafe {
            std::env::remove_var(tk);
            std::env::set_var(wh, "/tmp/wl-trust-test");
        }
        assert_eq!(
            trusted_keys_dir(),
            Some(PathBuf::from("/tmp/wl-trust-test").join("trusted-keys")),
            "fallback must follow WAYLAND_HOME"
        );
        // 2. Explicit override still wins over WAYLAND_HOME.
        unsafe {
            std::env::set_var(tk, "/tmp/explicit-trust");
        }
        assert_eq!(
            trusted_keys_dir(),
            Some(PathBuf::from("/tmp/explicit-trust")),
            "WAYLAND_TRUSTED_KEYS_DIR must win over WAYLAND_HOME"
        );
        // restore
        unsafe {
            match prev_wh {
                Some(v) => std::env::set_var(wh, v),
                None => std::env::remove_var(wh),
            }
            match prev_tk {
                Some(v) => std::env::set_var(tk, v),
                None => std::env::remove_var(tk),
            }
        }
    }

    fn make_key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    /// Build a path-based plugin layout: `<tmp>/plugin.bin` + (optional) `wayland-plugin.sig`
    /// and `<tmp>/keys/<name>.pub`. Returns (plugin_path, keys_dir, tmp).
    fn setup_path_plugin(
        content: &[u8],
        sign_with: Option<&SigningKey>,
        fs_trusted: &[&SigningKey],
    ) -> (PathBuf, PathBuf, TempDir) {
        let tmp = TempDir::new().unwrap();
        let plugin_path = tmp.path().join("plugin.bin");
        std::fs::write(&plugin_path, content).unwrap();
        if let Some(sk) = sign_with {
            let sig: Signature = sk.sign(content);
            std::fs::write(tmp.path().join(PLUGIN_SIG_FILENAME), sig.to_bytes()).unwrap();
        }
        let keys_dir = tmp.path().join("keys");
        std::fs::create_dir_all(&keys_dir).unwrap();
        for (i, sk) in fs_trusted.iter().enumerate() {
            std::fs::write(
                keys_dir.join(format!("key-{i}.pub")),
                sk.verifying_key().as_bytes(),
            )
            .unwrap();
        }
        (plugin_path, keys_dir, tmp)
    }

    /// Build a union vector with `config_keys` tagged Config and (optional)
    /// filesystem keys loaded from `keys_dir`.
    fn union(config_keys: &[&SigningKey], fs_dir: Option<&Path>) -> Vec<(VerifyingKey, KeySource)> {
        let mut out: Vec<(VerifyingKey, KeySource)> = config_keys
            .iter()
            .enumerate()
            .map(|(i, sk)| (sk.verifying_key(), KeySource::Config(i)))
            .collect();
        if let Some(d) = fs_dir {
            out.extend(load_filesystem_keys(d));
        }
        out
    }

    #[test]
    fn path_plugin_valid_signature_accepted() {
        let key = make_key();
        let (plugin_path, keys_dir, _tmp) = setup_path_plugin(b"plugin body", Some(&key), &[&key]);
        let keys = union(&[], Some(&keys_dir));
        assert!(verify_path_plugin_signature("p", &plugin_path, &keys).is_ok());
    }

    /// Positive-verify audit trail: `verify_path_plugin_signature` emits
    /// `tracing::info!(plugin, key_fp, key_source, "plugin signature verified")`
    /// on the OK arm (sig_verifier.rs, the `for (key, source)` loop). This
    /// test asserts the function returns `Ok(())` for a valid signature so
    /// the info log path is exercised on every test run.
    #[test]
    fn positive_verify_emits_ok_and_info_log_path_exercised() {
        let key = make_key();
        let (plugin_path, keys_dir, _tmp) =
            setup_path_plugin(b"audit-trail-body", Some(&key), &[&key]);
        let keys = union(&[], Some(&keys_dir));
        // Ok(()) means the tracing::info! line was reached.
        let result = verify_path_plugin_signature("audit-plugin", &plugin_path, &keys);
        assert!(
            result.is_ok(),
            "expected Ok(()) for valid sig — info log was not reached: {result:?}"
        );
    }

    #[test]
    fn path_plugin_missing_sig_rejected() {
        let key = make_key();
        let (plugin_path, keys_dir, _tmp) = setup_path_plugin(b"plugin body", None, &[&key]);
        let keys = union(&[], Some(&keys_dir));
        let result = verify_path_plugin_signature("p", &plugin_path, &keys);
        assert!(
            matches!(result, Err(PluginError::SignatureMissing { .. })),
            "expected SignatureMissing, got {result:?}"
        );
    }

    #[test]
    fn path_plugin_wrong_key_rejected() {
        let signer = make_key();
        let other = make_key();
        let (plugin_path, keys_dir, _tmp) =
            setup_path_plugin(b"plugin body", Some(&signer), &[&other]);
        let keys = union(&[], Some(&keys_dir));
        let result = verify_path_plugin_signature("p", &plugin_path, &keys);
        assert!(
            matches!(result, Err(PluginError::SignatureVerificationFailed { .. })),
            "expected SignatureVerificationFailed, got {result:?}"
        );
    }

    #[test]
    fn path_plugin_empty_union_rejected_as_config_error() {
        let key = make_key();
        let (plugin_path, _keys_dir, _tmp) = setup_path_plugin(b"plugin body", Some(&key), &[]);
        let keys: Vec<(VerifyingKey, KeySource)> = vec![];
        let result = verify_path_plugin_signature("p", &plugin_path, &keys);
        assert!(
            matches!(result, Err(PluginError::ConfigError(_))),
            "expected ConfigError when union is empty, got {result:?}"
        );
    }

    #[test]
    fn path_plugin_one_of_many_trusted_keys_accepts() {
        let signer = make_key();
        let other1 = make_key();
        let other2 = make_key();
        let (plugin_path, keys_dir, _tmp) =
            setup_path_plugin(b"body", Some(&signer), &[&other1, &signer, &other2]);
        let keys = union(&[], Some(&keys_dir));
        assert!(verify_path_plugin_signature("p", &plugin_path, &keys).is_ok());
    }

    #[test]
    fn load_filesystem_keys_skips_non_pub_and_malformed() {
        let tmp = TempDir::new().unwrap();
        let key = make_key();
        std::fs::write(tmp.path().join("a.pub"), key.verifying_key().as_bytes()).unwrap();
        std::fs::write(tmp.path().join("note.txt"), b"ignore me").unwrap();
        std::fs::write(tmp.path().join("bad.pub"), b"short").unwrap();
        let keys = load_filesystem_keys(tmp.path());
        assert_eq!(keys.len(), 1);
        assert!(matches!(keys[0].1, KeySource::Filesystem(_)));
    }

    #[test]
    fn load_filesystem_keys_missing_dir_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist");
        assert!(load_filesystem_keys(&missing).is_empty());
    }

    #[test]
    fn key_fingerprint_is_16_hex_chars() {
        let key = make_key();
        let fp = key_fingerprint(&key.verifying_key());
        assert_eq!(fp.len(), 16);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn parse_verifying_key_b64_round_trip() {
        let key = make_key();
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(key.verifying_key().as_bytes());
        assert!(parse_verifying_key_b64(&b64).is_some());
        assert!(parse_verifying_key_b64("not-base64!!!").is_none());
        assert!(parse_verifying_key_b64("AAAA").is_none());
    }

    // Union-coverage tests (v0.6.5 Task 1.3 fixup).

    #[test]
    fn union_config_only_signer_accepted() {
        let signer = make_key();
        let (plugin_path, _empty_fs, _tmp) = setup_path_plugin(b"plugin body", Some(&signer), &[]);
        // Config holds the signing key; filesystem is empty.
        let keys = union(&[&signer], None);
        assert!(verify_path_plugin_signature("p", &plugin_path, &keys).is_ok());
    }

    #[test]
    fn union_filesystem_only_signer_accepted() {
        let signer = make_key();
        let (plugin_path, keys_dir, _tmp) =
            setup_path_plugin(b"plugin body", Some(&signer), &[&signer]);
        // Config is empty; filesystem holds the signing key.
        let keys = union(&[], Some(&keys_dir));
        assert!(verify_path_plugin_signature("p", &plugin_path, &keys).is_ok());
    }

    #[test]
    fn union_both_populated_signer_in_fs_only_accepted() {
        let signer = make_key();
        let other = make_key();
        let (plugin_path, keys_dir, _tmp) =
            setup_path_plugin(b"plugin body", Some(&signer), &[&signer]);
        // Config has `other`, filesystem has `signer` — must accept via fs.
        let keys = union(&[&other], Some(&keys_dir));
        assert!(verify_path_plugin_signature("p", &plugin_path, &keys).is_ok());
    }

    #[test]
    fn union_both_populated_signer_in_config_only_accepted() {
        let signer = make_key();
        let other = make_key();
        let (plugin_path, keys_dir, _tmp) =
            setup_path_plugin(b"plugin body", Some(&signer), &[&other]);
        // Filesystem has `other`, config has `signer` — must accept via config.
        let keys = union(&[&signer], Some(&keys_dir));
        assert!(verify_path_plugin_signature("p", &plugin_path, &keys).is_ok());
    }

    #[test]
    fn union_both_populated_signer_in_neither_rejected() {
        let signer = make_key();
        let other1 = make_key();
        let other2 = make_key();
        let (plugin_path, keys_dir, _tmp) =
            setup_path_plugin(b"plugin body", Some(&signer), &[&other1]);
        // Neither source carries `signer` — must fail.
        let keys = union(&[&other2], Some(&keys_dir));
        let result = verify_path_plugin_signature("p", &plugin_path, &keys);
        assert!(
            matches!(result, Err(PluginError::SignatureVerificationFailed { .. })),
            "expected SignatureVerificationFailed, got {result:?}"
        );
    }
}
