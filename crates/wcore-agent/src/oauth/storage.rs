//! OAuth token storage, routed through the credential ladder.
//!
//! An OAuth token set is not a cache: its refresh token is a long-lived bearer
//! credential for the user's whole account. v0.9.0 B0 wrote it as pretty-printed
//! JSON at `~/.wayland/oauth/{provider}.json` with dir-mode 0700 + file-mode
//! 0600, on the argument that the configured `CredentialsBackend` might be
//! `Plaintext` and so "always store OAuth tokens in the keyring" was the wrong
//! abstraction. That argument no longer holds — `wcore_config::credentials`
//! grew an ordered ladder (keyring → encrypted vault → REFUSE) whose write path
//! has no cleartext rung at all, so the token can be stored securely without
//! caring what the operator configured for ordinary API keys.
//!
//! What this module does now:
//!
//! * **Writes** go to [`wcore_config::credentials::open_secure_ladder_store`]
//!   under [`wcore_config::credentials::oauth_tokens_key`], and are read back
//!   from the store before the write is reported as successful.
//! * **Fails closed.** A host with no keyring and a locked vault gets an error
//!   naming the remedy, not a cleartext file.
//! * **Migrates up once.** A token file written by an older build stays
//!   readable; the first `load` that finds one re-stores it through the ladder
//!   and removes the cleartext copy — but only AFTER the secure write has been
//!   verified by readback. A failed migration leaves the file in place and the
//!   user signed in.
//!
//! The `{provider}.json` path is retained as the legacy read/delete tier only.

use std::path::{Path, PathBuf};
use thiserror::Error;

use wcore_config::credentials::{CredentialsStore, oauth_tokens_key};

use super::flow::OAuthTokens;

#[derive(Debug, Error)]
pub enum OAuthStorageError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// No secure tier accepted the write. Carries the ladder's own message,
    /// which names the remedy (unlock the vault, or start a Secret Service).
    #[error("{0}")]
    NoSecureBackend(String),
    /// The write was accepted but the value did not read back. Never downgrade
    /// this to a warning: the caller is about to tell the user they are signed
    /// in, and the legacy cleartext copy is about to be removed.
    #[error(
        "the OAuth token for '{provider}' was written to the secure credential store but did \
         not read back from it; refusing to report the login as stored"
    )]
    ReadbackFailed { provider: String },
}

/// Secure storage for OAuth tokens, one entry per provider.
pub struct OAuthStorage {
    /// Legacy cleartext directory — read and delete only. No code path in this
    /// module writes a token into it.
    root: PathBuf,
    /// The keyring → vault → REFUSE ladder. Every token write lands here.
    secure: Box<dyn CredentialsStore>,
}

impl OAuthStorage {
    /// Construct for the active profile: the ladder over this profile's
    /// credential storage, plus the legacy `~/.wayland/oauth/` (or
    /// `$WAYLAND_HOME/oauth/`) directory as the read-only migration source.
    ///
    /// Resolves via [`wcore_config::config::profile_home`] so the legacy tier
    /// honours `WAYLAND_HOME` like the rest of Wayland's state: a hermetic
    /// sandbox (e.g. an auditor subprocess that sets `WAYLAND_HOME`) keeps its
    /// tokens inside the sandbox instead of reading/writing the real
    /// `~/.wayland` — the F-019 leak class.
    pub fn from_home() -> Result<Self, OAuthStorageError> {
        let secure = wcore_config::credentials::open_secure_ladder_store(
            &wcore_config::credentials::CredentialsStorageConfig::default(),
            &wcore_config::config::credentials_storage_path(),
        );
        Self::at_root(wcore_config::config::profile_home().join("oauth"), secure)
    }

    /// Construct at an explicit legacy root over an explicit secure store.
    ///
    /// The secure store is a parameter rather than a default because there is
    /// no hermetic default: the OS keyring is host-global and the vault needs
    /// unlock material. Tests pass
    /// [`wcore_config::credentials::InMemoryCredentialsStore`]; production goes
    /// through [`Self::from_home`].
    pub fn at_root(
        root: PathBuf,
        secure: Box<dyn CredentialsStore>,
    ) -> Result<Self, OAuthStorageError> {
        Self::ensure_dir(&root)?;
        Ok(Self { root, secure })
    }

    /// Persist `tokens` for `provider` in the credential ladder.
    ///
    /// Order, and it is not negotiable: **write-secure → verify-readback →
    /// remove-cleartext.** Removing the legacy file before the secure write is
    /// verified can lose the login outright, and a lost OAuth token is not a
    /// visible failure — it silently signs the user out mid-session.
    pub fn store(&self, provider: &str, tokens: &OAuthTokens) -> Result<(), OAuthStorageError> {
        let json = serde_json::to_string(tokens)?;
        self.store_serialized(provider, &json)
    }

    fn store_serialized(&self, provider: &str, json: &str) -> Result<(), OAuthStorageError> {
        let key = oauth_tokens_key(provider);

        // 1. write-secure. The ladder descends keyring → vault and REFUSES
        //    rather than writing cleartext, so an error here is fail-closed.
        self.secure
            .put(&key, json)
            .map_err(|e| OAuthStorageError::NoSecureBackend(e.to_string()))?;

        // 2. verify-readback, from the store itself — not inferred from `put`
        //    returning Ok. A keyring that silently truncates an oversized blob
        //    returns Ok and then hands back something else.
        let read_back = self.secure.get(&key).ok().flatten();
        if read_back.as_deref() != Some(json) {
            return Err(OAuthStorageError::ReadbackFailed {
                provider: provider.to_string(),
            });
        }

        // 3. remove-cleartext. Only now is the legacy copy redundant.
        self.remove_legacy(provider)?;
        Ok(())
    }

    /// Load tokens for `provider`. Returns `Ok(None)` when the provider has no
    /// stored login in either tier.
    ///
    /// A token found only in the legacy cleartext file is migrated up on the
    /// spot. Migration is best-effort: a host with no secure tier keeps reading
    /// its existing file rather than being logged out, because refusing the
    /// READ would turn a storage-hardening change into an authentication
    /// regression for every already-signed-in user.
    pub fn load(&self, provider: &str) -> Result<Option<OAuthTokens>, OAuthStorageError> {
        if let Ok(Some(json)) = self.secure.get(&oauth_tokens_key(provider)) {
            return Ok(Some(serde_json::from_str(&json)?));
        }

        let Some(bytes) = self.read_legacy(provider)? else {
            return Ok(None);
        };
        let tokens: OAuthTokens = serde_json::from_slice(&bytes)?;

        // Re-serialize through the same encoder the writer uses so the stored
        // value and its readback are byte-comparable.
        match serde_json::to_string(&tokens) {
            Ok(json) => {
                if let Err(error) = self.store_serialized(provider, &json) {
                    tracing::warn!(
                        target: "wcore_oauth",
                        provider,
                        error = %error,
                        "could not migrate a cleartext OAuth token into the secure credential \
                         store; the existing file stays in place and readable"
                    );
                }
            }
            Err(error) => tracing::warn!(
                target: "wcore_oauth",
                provider,
                error = %error,
                "could not re-encode a cleartext OAuth token for migration"
            ),
        }
        Ok(Some(tokens))
    }

    /// Remove `provider`'s login from BOTH tiers.
    ///
    /// The secure entry and the legacy file are independent; a logout that
    /// removed only one of them would leave the user signed in through the
    /// other. Reports whether anything was actually removed, so a caller can
    /// distinguish "signed out" from "already signed out".
    pub fn delete(&self, provider: &str) -> Result<bool, OAuthStorageError> {
        let key = oauth_tokens_key(provider);
        let had_secure = matches!(self.secure.get(&key), Ok(Some(_)));
        let secure_error = self.secure.delete(&key).err();
        let had_legacy = self.remove_legacy(provider)?;
        if let Some(error) = secure_error {
            return Err(OAuthStorageError::NoSecureBackend(error.to_string()));
        }
        Ok(had_secure || had_legacy)
    }

    /// Path of the LEGACY cleartext token file for `provider`.
    ///
    /// Retained so migration, logout and tests can address the pre-ladder file.
    /// It is not a write target: after a successful [`Self::store`] this path
    /// does not exist.
    pub fn path_for(&self, provider: &str) -> PathBuf {
        // Strip path separators so a malicious provider name can't
        // escape the oauth dir. Provider names are agent-controlled in
        // practice, but defense-in-depth is cheap here.
        let safe = provider.replace(['/', '\\', '\0'], "_");
        self.root.join(format!("{safe}.json"))
    }

    fn read_legacy(&self, provider: &str) -> Result<Option<Vec<u8>>, OAuthStorageError> {
        match std::fs::read(self.path_for(provider)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(OAuthStorageError::Io(e)),
        }
    }

    /// Remove the legacy cleartext file and any orphaned temp file beside it.
    /// Returns whether the token file was there.
    fn remove_legacy(&self, provider: &str) -> Result<bool, OAuthStorageError> {
        let path = self.path_for(provider);
        let removed = match std::fs::remove_file(&path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(OAuthStorageError::Io(e)),
        };
        // A half-written `.json.tmp` from an interrupted pre-ladder write is
        // still a cleartext token on disk.
        let tmp = path.with_extension("json.tmp");
        if let Err(e) = std::fs::remove_file(&tmp)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                target: "wcore_oauth",
                path = %tmp.display(),
                error = %e,
                "could not remove an orphaned cleartext OAuth temp file"
            );
        }
        Ok(removed)
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
        // On Windows the user's profile dir ACL covers this.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use wcore_config::credentials::{CredentialsError, InMemoryCredentialsStore};

    fn make_tokens() -> OAuthTokens {
        OAuthTokens {
            access_token: "at-123".into(),
            refresh_token: Some("rt-456".into()),
            expires_at_unix_secs: Some(1_700_000_000),
            token_type: "Bearer".into(),
            scope: Some("scope1 scope2".into()),
            id_token: None,
        }
    }

    fn store_at(root: PathBuf) -> (OAuthStorage, InMemoryCredentialsStore) {
        let secure = InMemoryCredentialsStore::new();
        let storage = OAuthStorage::at_root(root, Box::new(secure.clone())).unwrap();
        (storage, secure)
    }

    /// A tier that ACCEPTS every write and then denies ever having seen it.
    /// This is the only shape that can tell a real readback check apart from a
    /// decorative one: with any honest store, deleting the check changes
    /// nothing observable.
    struct AcceptsThenForgets;

    impl CredentialsStore for AcceptsThenForgets {
        fn get(&self, _key: &str) -> Result<Option<String>, CredentialsError> {
            Ok(None)
        }
        fn put(&self, _key: &str, _value: &str) -> Result<(), CredentialsError> {
            Ok(())
        }
        fn delete(&self, _key: &str) -> Result<(), CredentialsError> {
            Ok(())
        }
    }

    /// A tier with no secure rung: every write is refused, exactly as the
    /// ladder refuses on a host with no keyring and a locked vault.
    struct RefusesWrites;

    impl CredentialsStore for RefusesWrites {
        fn get(&self, _key: &str) -> Result<Option<String>, CredentialsError> {
            Ok(None)
        }
        fn put(&self, _key: &str, _value: &str) -> Result<(), CredentialsError> {
            Err(CredentialsError::BackendUnavailable(
                "no secure credential backend is available on this host".to_string(),
            ))
        }
        fn delete(&self, _key: &str) -> Result<(), CredentialsError> {
            Ok(())
        }
    }

    #[test]
    #[cfg(unix)]
    fn legacy_directory_mode_is_0700() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("oauth");
        let (_store, _secure) = store_at(root.clone());
        let meta = std::fs::metadata(&root).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "expected dir mode 0700, got {mode:o}");
    }

    #[test]
    fn store_then_load_round_trips() {
        let tmp = TempDir::new().unwrap();
        let (store, _secure) = store_at(tmp.path().join("oauth"));
        let original = make_tokens();
        store.store("test-provider", &original).unwrap();
        let loaded = store.load("test-provider").unwrap().expect("present");
        assert_eq!(loaded.access_token, original.access_token);
        assert_eq!(loaded.refresh_token, original.refresh_token);
        assert_eq!(loaded.expires_at_unix_secs, original.expires_at_unix_secs);
    }

    /// The security property itself: a stored token leaves NO cleartext behind.
    #[test]
    fn store_writes_no_cleartext_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("oauth");
        let (store, secure) = store_at(root.clone());
        store.store("chatgpt", &make_tokens()).unwrap();

        assert!(
            !store.path_for("chatgpt").exists(),
            "a cleartext token file must not exist after a secure store"
        );
        // And nothing anywhere under the oauth dir contains the refresh token.
        for entry in std::fs::read_dir(&root).unwrap() {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap_or_default();
            assert!(
                !String::from_utf8_lossy(&bytes).contains("rt-456"),
                "{} holds the refresh token in cleartext",
                path.display()
            );
        }
        // The value is in the ladder, under the shared key spelling.
        let stored = secure.get(&oauth_tokens_key("chatgpt")).unwrap();
        assert!(stored.is_some_and(|v| v.contains("rt-456")));
    }

    #[test]
    fn load_returns_none_for_missing_provider() {
        let tmp = TempDir::new().unwrap();
        let (store, _secure) = store_at(tmp.path().join("oauth"));
        assert!(store.load("never-stored").unwrap().is_none());
    }

    /// A token file written by a pre-ladder build stays readable, is promoted
    /// into the secure tier, and only then loses its cleartext copy.
    #[test]
    fn legacy_cleartext_token_is_readable_and_migrates_up() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("oauth");
        let (store, secure) = store_at(root.clone());
        let path = store.path_for("chatgpt");
        std::fs::write(&path, serde_json::to_vec_pretty(&make_tokens()).unwrap()).unwrap();

        let loaded = store
            .load("chatgpt")
            .unwrap()
            .expect("legacy token readable");
        assert_eq!(loaded.refresh_token.as_deref(), Some("rt-456"));

        assert!(
            secure.get(&oauth_tokens_key("chatgpt")).unwrap().is_some(),
            "the legacy token must be promoted into the secure tier"
        );
        assert!(
            !path.exists(),
            "the cleartext copy must be removed after a verified secure write"
        );
    }

    /// The other half of the migration contract: with NO secure tier the user
    /// must stay signed in and the file must survive. A fix that logged
    /// everyone out would be worse than the defect it closes.
    #[test]
    fn legacy_token_survives_when_no_secure_tier_accepts_it() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("oauth");
        let store = OAuthStorage::at_root(root, Box::new(RefusesWrites)).unwrap();
        let path = store.path_for("chatgpt");
        std::fs::write(&path, serde_json::to_vec_pretty(&make_tokens()).unwrap()).unwrap();

        let loaded = store.load("chatgpt").unwrap().expect("still signed in");
        assert_eq!(loaded.refresh_token.as_deref(), Some("rt-456"));
        assert!(
            path.exists(),
            "a failed migration must not delete the token"
        );
    }

    /// Fail closed: with no secure tier, a NEW login is refused rather than
    /// written in cleartext.
    #[test]
    fn store_fails_closed_with_no_secure_tier() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("oauth");
        let store = OAuthStorage::at_root(root, Box::new(RefusesWrites)).unwrap();
        let error = store.store("chatgpt", &make_tokens()).unwrap_err();
        assert!(
            matches!(error, OAuthStorageError::NoSecureBackend(_)),
            "expected a fail-closed refusal, got {error:?}"
        );
        assert!(
            !store.path_for("chatgpt").exists(),
            "a refused store must not leave a cleartext token behind"
        );
    }

    /// MUTATION TARGET. Deleting the readback comparison in
    /// `store_serialized` makes this test fail on BOTH assertions: `store`
    /// starts returning `Ok`, and the legacy cleartext file gets removed on the
    /// strength of a write that did not land.
    #[test]
    fn readback_failure_is_reported_and_keeps_the_cleartext_copy() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("oauth");
        let store = OAuthStorage::at_root(root, Box::new(AcceptsThenForgets)).unwrap();
        let path = store.path_for("chatgpt");
        std::fs::write(&path, serde_json::to_vec_pretty(&make_tokens()).unwrap()).unwrap();

        let error = store.store("chatgpt", &make_tokens()).unwrap_err();
        assert!(
            matches!(error, OAuthStorageError::ReadbackFailed { .. }),
            "a write that does not read back must be reported, got {error:?}"
        );
        assert!(
            path.exists(),
            "the cleartext copy must survive a write that did not read back"
        );
    }

    /// Logout must clear both tiers. A delete that missed the secure entry
    /// would leave the user signed in through it.
    #[test]
    fn delete_clears_both_tiers() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("oauth");
        let (store, secure) = store_at(root);
        store.store("chatgpt", &make_tokens()).unwrap();
        // Re-create a legacy file to prove both tiers are addressed.
        std::fs::write(store.path_for("chatgpt"), b"{}").unwrap();

        assert!(store.delete("chatgpt").unwrap(), "something was removed");
        assert!(secure.get(&oauth_tokens_key("chatgpt")).unwrap().is_none());
        assert!(!store.path_for("chatgpt").exists());
        assert!(store.load("chatgpt").unwrap().is_none());
        assert!(!store.delete("chatgpt").unwrap(), "already signed out");
    }

    #[test]
    fn provider_name_with_path_separator_is_sanitized() {
        let tmp = TempDir::new().unwrap();
        let (store, _secure) = store_at(tmp.path().join("oauth"));
        let evil = "../../../etc/passwd";
        let p = store.path_for(evil);
        // After sanitization, the filename must NOT contain a separator —
        // the path traversal slashes get rewritten to underscores.
        let stem = p.file_name().unwrap().to_string_lossy();
        assert!(
            !stem.contains('/') && !stem.contains('\\'),
            "path traversal must be neutralized in filename: {stem}"
        );
        // The store key is namespaced the same way, so a hostile provider name
        // cannot address another key or escape the `oauth.` namespace.
        let key = oauth_tokens_key(evil);
        assert!(key.starts_with("oauth."), "key={key}");
        assert!(key.ends_with(".tokens"), "key={key}");
        assert_eq!(
            key.matches('.').count(),
            2,
            "the provider segment must not introduce dots: {key}"
        );
    }
}
