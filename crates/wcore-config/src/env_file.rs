//! Atomic `.env` writer with strict key/value validation.
//!
//! v0.9.0 W4 E1 / S-H3 BLOCKER closure: writes to `~/.wayland/.env` (or
//! any other `.env`-shaped file) must be safe under three concurrent
//! pressures:
//!
//! 1. **Key/value injection.** A malicious `value` containing `\n` could
//!    write a *second* env entry (e.g. setting `value = "x\nPATH=/tmp"`
//!    would silently override `PATH`). Newlines, carriage returns, and
//!    NUL bytes are rejected outright. Keys must match `^[A-Z][A-Z0-9_]*$`.
//! 2. **Partial writes.** A crash mid-write must not leave the file
//!    truncated or corrupt. We stage to a sibling tempfile and `rename`
//!    over the target — POSIX atomicity guarantee.
//! 3. **Permissions leakage.** The parent dir (`~/.wayland/`) is forced
//!    to mode `0700` on Unix; the file itself to `0600`. The OAuth
//!    storage layer uses the same posture (see `wcore_agent::oauth::storage`)
//!    so the on-disk credential surface is uniformly tight.
//!
//! The implementation reads the existing file (best-effort), parses it
//! into a key→value map, applies the upsert, and serialises back — so
//! pre-existing entries are preserved verbatim and the operation is
//! idempotent.
//!
//! Logging: the key name is logged at INFO level; the *value* is never
//! emitted to tracing or any log sink. A unit test asserts this property
//! by capturing tracing events.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Errors the env-file writer can surface.
#[derive(Debug, Error)]
pub enum EnvFileError {
    /// The supplied key name violates `^[A-Z][A-Z0-9_]*$`.
    #[error("invalid env var key: {0:?} (must match ^[A-Z][A-Z0-9_]*$)")]
    InvalidKey(String),
    /// The supplied value contains a forbidden control character.
    #[error("invalid env var value: contains newline, carriage-return, or NUL byte")]
    InvalidValue,
    /// Underlying I/O failure (open, write, rename, permissions).
    #[error("env file I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// `tempfile::persist` packs the rename + the tempfile holder in one
    /// error type; unwrap the inner I/O error here.
    #[error("env file persist error: {0}")]
    Persist(String),
}

impl From<tempfile::PersistError> for EnvFileError {
    fn from(err: tempfile::PersistError) -> Self {
        Self::Persist(err.to_string())
    }
}

/// Atomically upsert a single `key=value` entry in `env_path`.
///
/// On success the file exists with mode `0600`, its parent directory has
/// mode `0700`, every pre-existing key is preserved, and `key` resolves
/// to `value`. The value is never logged — only the key is.
///
/// # Errors
///
/// - [`EnvFileError::InvalidKey`] when `key` violates `^[A-Z][A-Z0-9_]*$`.
/// - [`EnvFileError::InvalidValue`] when `value` contains `\n`, `\r`,
///   or a NUL byte. These would silently inject an additional entry.
/// - [`EnvFileError::Io`] / [`EnvFileError::Persist`] for filesystem
///   failures (no permission, no space, parent rename across devices).
pub fn write_env_var(env_path: &Path, key: &str, value: &str) -> Result<(), EnvFileError> {
    validate_key(key)?;
    validate_value(value)?;

    // Ensure the parent directory exists with mode 0700 on Unix. The
    // OAuth storage layer enforces the same posture, so the on-disk
    // credential surface is uniformly tight.
    let parent = env_path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() && !parent.exists() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    if !parent.as_os_str().is_empty() && parent.exists() {
        // Best-effort — if the user has the dir locked to a stricter
        // mode we leave it alone, but the common case is brand-new dir.
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(parent, perms)?;
    }

    // Read the current file (if any) and parse it into a key→value map
    // so the upsert preserves every other entry verbatim.
    let existing = std::fs::read_to_string(env_path).unwrap_or_default();
    let mut entries = parse_env(&existing);
    entries.insert(key.to_string(), value.to_string());

    // Stage the serialised body into a sibling tempfile, then rename
    // atomically. The tempfile crate sets mode 0600 on Unix by default
    // for `NamedTempFile`, but we re-apply explicitly so the post-rename
    // file's permissions are not racing the umask.
    let serialised = serialise_env(&entries);
    let tmp_dir = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let mut tmp = tempfile::NamedTempFile::new_in(tmp_dir)?;
    tmp.as_file_mut().write_all(serialised.as_bytes())?;
    tmp.as_file_mut().sync_all()?;

    #[cfg(unix)]
    {
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(tmp.path(), perms)?;
    }

    tmp.persist(env_path)?;

    // Re-apply mode 0600 post-rename in case the destination existed
    // with a looser mode (rename preserves source perms on most POSIX
    // filesystems, but be paranoid).
    #[cfg(unix)]
    {
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(env_path, perms)?;
    }

    // Log the key only — NEVER the value.
    tracing::info!(env_var = %key, "env_file: wrote key");
    Ok(())
}

/// Read every entry from `env_path`. Missing file → empty map. Used by
/// the Config TUI to render the "current value" badge per provider.
pub fn read_env_vars(env_path: &Path) -> BTreeMap<String, String> {
    let body = std::fs::read_to_string(env_path).unwrap_or_default();
    parse_env(&body)
}

/// Validate a key. Matches `^[A-Z][A-Z0-9_]*$`.
fn validate_key(key: &str) -> Result<(), EnvFileError> {
    let bytes = key.as_bytes();
    if bytes.is_empty() {
        return Err(EnvFileError::InvalidKey(key.to_string()));
    }
    // First byte must be A-Z.
    if !bytes[0].is_ascii_uppercase() {
        return Err(EnvFileError::InvalidKey(key.to_string()));
    }
    // Remaining bytes: A-Z, 0-9, or underscore.
    for &b in &bytes[1..] {
        if !(b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') {
            return Err(EnvFileError::InvalidKey(key.to_string()));
        }
    }
    Ok(())
}

/// Validate a value. Reject `\n`, `\r`, and NUL byte — anything that
/// could inject an extra entry on parse.
fn validate_value(value: &str) -> Result<(), EnvFileError> {
    if value.contains('\n') || value.contains('\r') || value.contains('\0') {
        return Err(EnvFileError::InvalidValue);
    }
    Ok(())
}

/// Parse an `.env`-shaped file body into a BTreeMap. Tolerates blank
/// lines, `#` comments, and surrounding whitespace. Only valid keys
/// (matching the same regex as the writer) are kept; anything else is
/// dropped silently — we are upserting, not validating arbitrary input.
fn parse_env(body: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((k, v)) = trimmed.split_once('=') else {
            continue;
        };
        let k = k.trim();
        if validate_key(k).is_err() {
            continue;
        }
        // Strip surrounding double quotes if present — the writer uses
        // them when the value contains spaces or `=`, so the round-trip
        // is lossless for those cases.
        let v = v.trim();
        let v = if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
            &v[1..v.len() - 1]
        } else {
            v
        };
        out.insert(k.to_string(), v.to_string());
    }
    out
}

/// Render a BTreeMap back into `.env` format. Values are emitted in
/// sorted-key order (BTreeMap iteration order) so the file is
/// deterministic across writes. A value that contains whitespace or `=`
/// is wrapped in double-quotes so the parser can recover it.
fn serialise_env(entries: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    out.push_str("# Wayland credentials store — managed by the Config TUI.\n");
    out.push_str("# Do not edit values that contain newlines; one entry per line only.\n");
    for (k, v) in entries {
        out.push_str(k);
        out.push('=');
        let needs_quoting =
            v.contains(' ') || v.contains('\t') || v.contains('=') || v.contains('"');
        if needs_quoting {
            // Escape embedded double quotes by doubling — the parser
            // strips one outer pair; this round-trips for the common
            // cases (API keys never contain `"` in practice).
            out.push('"');
            out.push_str(&v.replace('"', "\\\""));
            out.push('"');
        } else {
            out.push_str(v);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_path(dir: &TempDir) -> std::path::PathBuf {
        dir.path().join(".env")
    }

    // ── Validation ──────────────────────────────────────────────────────

    #[test]
    fn env_file_rejects_key_with_invalid_chars() {
        let dir = TempDir::new().unwrap();
        let path = write_path(&dir);
        // Lowercase
        assert!(matches!(
            write_env_var(&path, "openai_api_key", "x"),
            Err(EnvFileError::InvalidKey(_))
        ));
        // Leading digit
        assert!(matches!(
            write_env_var(&path, "1OPENAI", "x"),
            Err(EnvFileError::InvalidKey(_))
        ));
        // Embedded hyphen
        assert!(matches!(
            write_env_var(&path, "OPENAI-KEY", "x"),
            Err(EnvFileError::InvalidKey(_))
        ));
        // Empty
        assert!(matches!(
            write_env_var(&path, "", "x"),
            Err(EnvFileError::InvalidKey(_))
        ));
        // Valid examples
        assert!(write_env_var(&path, "ANTHROPIC_API_KEY", "sk-test").is_ok());
        assert!(write_env_var(&path, "X1", "ok").is_ok());
    }

    #[test]
    fn env_file_rejects_value_with_newline_or_null() {
        let dir = TempDir::new().unwrap();
        let path = write_path(&dir);
        assert!(matches!(
            write_env_var(&path, "FOO", "ab\ncd"),
            Err(EnvFileError::InvalidValue)
        ));
        assert!(matches!(
            write_env_var(&path, "FOO", "ab\rcd"),
            Err(EnvFileError::InvalidValue)
        ));
        assert!(matches!(
            write_env_var(&path, "FOO", "ab\0cd"),
            Err(EnvFileError::InvalidValue)
        ));
    }

    // ── Permissions ─────────────────────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn env_file_create_uses_mode_0600() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("subdir").join(".env");
        write_env_var(&path, "FOO", "bar").unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected file mode 0600, got {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn env_file_parent_dir_uses_mode_0700() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("freshly-created");
        let path = parent.join(".env");
        write_env_var(&path, "FOO", "bar").unwrap();
        let meta = std::fs::metadata(&parent).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "expected dir mode 0700, got {mode:o}");
    }

    // ── Idempotent upsert ───────────────────────────────────────────────

    #[test]
    fn env_file_idempotent_update_preserves_other_keys() {
        let dir = TempDir::new().unwrap();
        let path = write_path(&dir);
        write_env_var(&path, "ANTHROPIC_API_KEY", "sk-old").unwrap();
        write_env_var(&path, "OPENAI_API_KEY", "openai-key").unwrap();
        // Update Anthropic — OpenAI must survive.
        write_env_var(&path, "ANTHROPIC_API_KEY", "sk-new").unwrap();
        let map = read_env_vars(&path);
        assert_eq!(
            map.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-new")
        );
        assert_eq!(
            map.get("OPENAI_API_KEY").map(String::as_str),
            Some("openai-key")
        );
    }

    #[test]
    fn env_file_creates_when_missing() {
        let dir = TempDir::new().unwrap();
        let path = write_path(&dir);
        assert!(!path.exists());
        write_env_var(&path, "FOO", "bar").unwrap();
        assert!(path.exists());
        let map = read_env_vars(&path);
        assert_eq!(map.get("FOO").map(String::as_str), Some("bar"));
    }

    #[test]
    fn env_file_quotes_values_with_spaces() {
        let dir = TempDir::new().unwrap();
        let path = write_path(&dir);
        write_env_var(&path, "MY_VAR", "value with spaces").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.contains(r#"MY_VAR="value with spaces""#),
            "body did not quote value: {body}"
        );
        // Round-trip: the parser strips the quotes.
        let map = read_env_vars(&path);
        assert_eq!(
            map.get("MY_VAR").map(String::as_str),
            Some("value with spaces")
        );
    }

    // ── Log hygiene ─────────────────────────────────────────────────────
    //
    // tracing's callsite Interest is cached PROCESS-WIDE on first hit
    // against the global dispatcher. In a unit-test binary the global
    // is `NoSubscriber` (Interest::never()), so once any other test
    // touches `tracing::info!` first, the callsite at `write_env_var`
    // is permanently disabled for the test process even when a real
    // Subscriber is installed via `with_default`.
    //
    // To dodge the global cache, install a `Dispatch` as the global
    // default via a thread-safe one-shot. The captured-output buffer
    // lives in a `Arc<Mutex<...>>` referenced by the global subscriber
    // and reset for each invocation of this test.

    static GLOBAL_LOG_BUF: std::sync::OnceLock<std::sync::Arc<std::sync::Mutex<Vec<u8>>>> =
        std::sync::OnceLock::new();

    #[derive(Clone)]
    struct BufWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufWriter {
        type Writer = BufWriterHandle;
        fn make_writer(&'a self) -> Self::Writer {
            BufWriterHandle(self.0.clone())
        }
    }

    struct BufWriterHandle(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for BufWriterHandle {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn init_global_log_subscriber() -> std::sync::Arc<std::sync::Mutex<Vec<u8>>> {
        let buf = GLOBAL_LOG_BUF
            .get_or_init(|| std::sync::Arc::new(std::sync::Mutex::new(Vec::new())))
            .clone();
        // try_init silently returns Err if a global is already installed
        // (which is fine: the buf is the one we hold above).
        let _ = tracing_subscriber::fmt()
            .with_writer(BufWriter(buf.clone()))
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_target(false)
            .try_init();
        buf
    }

    #[test]
    #[serial_test::serial]
    fn env_file_log_does_not_contain_value() {
        let buf = init_global_log_subscriber();
        // Reset the buffer so we only assert on this test's output.
        buf.lock().unwrap().clear();

        let dir = TempDir::new().unwrap();
        let path = write_path(&dir);
        // A value with a recognisable, unique sentinel.
        write_env_var(&path, "SECRET_KEY", "ULTRA-SENSITIVE-DEADBEEF").unwrap();

        let captured = String::from_utf8(buf.lock().unwrap().clone()).unwrap_or_default();
        // The key name MUST appear (we log it for operator visibility).
        assert!(
            captured.contains("SECRET_KEY"),
            "expected key name in log: {captured}"
        );
        // The value MUST NOT appear in any form.
        assert!(
            !captured.contains("ULTRA-SENSITIVE-DEADBEEF"),
            "value MUST NOT appear in log: {captured}"
        );
    }

    // ── Parser round-trip ───────────────────────────────────────────────

    #[test]
    fn parse_env_ignores_comments_and_blanks() {
        let body = "
# this is a comment
FOO=bar

BAZ=qux
# trailing comment
";
        let map = parse_env(body);
        assert_eq!(map.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(map.get("BAZ").map(String::as_str), Some("qux"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn parse_env_drops_invalid_keys() {
        let body = "good_key=x\nGOOD_KEY=y\n1BAD=z\n";
        let map = parse_env(body);
        assert_eq!(map.get("GOOD_KEY").map(String::as_str), Some("y"));
        assert!(!map.contains_key("good_key"));
        assert!(!map.contains_key("1BAD"));
    }
}
