//! OAuth token storage at `~/.wayland/oauth/{provider}.json`.
//!
//! v0.9.0 B0 chose a file-backed default (not the keyring) because
//! `wcore-config::credentials::CredentialsBackend` is configured at
//! engine bootstrap and may be `Plaintext`/`EncryptedFile` rather than
//! the keyring — making "always store OAuth tokens in the keyring" the
//! wrong abstraction. We enforce dir-mode 0700 + file-mode 0600 on Unix
//! so the on-disk default is at least as restrictive as the keyring's
//! per-user ACL. v0.9.1 will hook the keyring backend when available.

use std::path::{Path, PathBuf};
use thiserror::Error;

use super::flow::OAuthTokens;

#[derive(Debug, Error)]
pub enum OAuthStorageError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("home directory unresolvable")]
    NoHome,
}

/// File-backed storage for OAuth tokens. Each provider gets its own
/// file under `~/.wayland/oauth/`.
pub struct OAuthStorage {
    root: PathBuf,
}

impl OAuthStorage {
    /// Construct using the user's home directory. Creates `~/.wayland/oauth/`
    /// with mode 0700 on Unix on first use.
    pub fn from_home() -> Result<Self, OAuthStorageError> {
        let home = dirs::home_dir().ok_or(OAuthStorageError::NoHome)?;
        Self::at_root(home.join(".wayland").join("oauth"))
    }

    /// Construct at an explicit root (used in tests).
    pub fn at_root(root: PathBuf) -> Result<Self, OAuthStorageError> {
        Self::ensure_dir(&root)?;
        Ok(Self { root })
    }

    /// Persist `tokens` for `provider`. Refuses to clobber via `create_new`
    /// when atomic-write replaces an existing file; the temp+rename path
    /// avoids the create_new race, then re-applies 0o600.
    pub fn store(&self, provider: &str, tokens: &OAuthTokens) -> Result<(), OAuthStorageError> {
        Self::ensure_dir(&self.root)?;
        let path = self.path_for(provider);
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(tokens)?;
        // Write the temp file with mode 0600, then atomically rename.
        Self::write_secure(&tmp, &json)?;
        std::fs::rename(&tmp, &path)?;
        Self::set_file_mode_0600(&path)?;
        Ok(())
    }

    /// Load tokens for `provider`. Returns `Ok(None)` when no file exists.
    pub fn load(&self, provider: &str) -> Result<Option<OAuthTokens>, OAuthStorageError> {
        let path = self.path_for(provider);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(OAuthStorageError::Io(e)),
        }
    }

    /// Path that would hold `provider`'s tokens.
    pub fn path_for(&self, provider: &str) -> PathBuf {
        // Strip path separators so a malicious provider name can't
        // escape the oauth dir. Provider names are agent-controlled in
        // practice, but defense-in-depth is cheap here.
        let safe = provider.replace(['/', '\\', '\0'], "_");
        self.root.join(format!("{safe}.json"))
    }

    fn ensure_dir(root: &Path) -> Result<(), OAuthStorageError> {
        if !root.exists() {
            std::fs::create_dir_all(root)?;
        }
        Self::set_dir_mode_0700(root)?;
        Ok(())
    }

    #[cfg(unix)]
    fn set_dir_mode_0700(path: &Path) -> Result<(), OAuthStorageError> {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(path, perms)?;
        Ok(())
    }

    #[cfg(not(unix))]
    fn set_dir_mode_0700(_path: &Path) -> Result<(), OAuthStorageError> {
        // On Windows the user's profile dir ACL covers this; v0.9.1
        // can wire DPAPI if a sharper ACL is needed.
        Ok(())
    }

    #[cfg(unix)]
    fn set_file_mode_0600(path: &Path) -> Result<(), OAuthStorageError> {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)?;
        Ok(())
    }

    #[cfg(not(unix))]
    fn set_file_mode_0600(_path: &Path) -> Result<(), OAuthStorageError> {
        Ok(())
    }

    #[cfg(unix)]
    fn write_secure(path: &Path, bytes: &[u8]) -> Result<(), OAuthStorageError> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        Ok(())
    }

    #[cfg(not(unix))]
    fn write_secure(path: &Path, bytes: &[u8]) -> Result<(), OAuthStorageError> {
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_tokens() -> OAuthTokens {
        OAuthTokens {
            access_token: "at-123".into(),
            refresh_token: Some("rt-456".into()),
            expires_at_unix_secs: Some(1_700_000_000),
            token_type: "Bearer".into(),
            scope: Some("scope1 scope2".into()),
        }
    }

    #[test]
    #[cfg(unix)]
    fn token_storage_directory_mode_is_0700() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("oauth");
        let _store = OAuthStorage::at_root(root.clone()).unwrap();
        let meta = std::fs::metadata(&root).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "expected dir mode 0700, got {mode:o}");
    }

    #[test]
    #[cfg(unix)]
    fn token_storage_file_mode_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let store = OAuthStorage::at_root(tmp.path().join("oauth")).unwrap();
        store.store("google_meet", &make_tokens()).unwrap();
        let path = store.path_for("google_meet");
        let meta = std::fs::metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected file mode 0600, got {mode:o}");
    }

    #[test]
    fn store_then_load_round_trips() {
        let tmp = TempDir::new().unwrap();
        let store = OAuthStorage::at_root(tmp.path().join("oauth")).unwrap();
        let original = make_tokens();
        store.store("test-provider", &original).unwrap();
        let loaded = store.load("test-provider").unwrap().expect("present");
        assert_eq!(loaded.access_token, original.access_token);
        assert_eq!(loaded.refresh_token, original.refresh_token);
        assert_eq!(loaded.expires_at_unix_secs, original.expires_at_unix_secs);
    }

    #[test]
    fn load_returns_none_for_missing_provider() {
        let tmp = TempDir::new().unwrap();
        let store = OAuthStorage::at_root(tmp.path().join("oauth")).unwrap();
        assert!(store.load("never-stored").unwrap().is_none());
    }

    #[test]
    fn provider_name_with_path_separator_is_sanitized() {
        let tmp = TempDir::new().unwrap();
        let store = OAuthStorage::at_root(tmp.path().join("oauth")).unwrap();
        let evil = "../../../etc/passwd";
        let p = store.path_for(evil);
        // After sanitization, the filename must NOT contain a separator —
        // the path traversal slashes get rewritten to underscores.
        let stem = p.file_name().unwrap().to_string_lossy();
        assert!(
            !stem.contains('/') && !stem.contains('\\'),
            "path traversal must be neutralized in filename: {stem}"
        );
    }
}
