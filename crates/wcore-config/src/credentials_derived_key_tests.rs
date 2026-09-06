use super::*;

#[test]
#[serial_test::serial(vault_passphrase_env)]
fn encrypted_derived_key_is_reused_across_real_operations() {
    let _passphrase = EnvPassphraseGuard::set("derived-key-fixture-password");
    let _fd = EnvVarGuard::remove("WAYLAND_VAULT_PASSPHRASE_FD");
    let root = tempdir().unwrap();
    let cipher = root.path().join("credentials.enc");
    let params = root.path().join("credentials.params.json");
    EncryptedFileCredentialsStore::new(cipher.clone(), params.clone())
        .put("test.token", "first")
        .unwrap();
    let store = EncryptedFileCredentialsStore::new(cipher, params);
    let before = encrypted_file::test_derivation_count();
    assert_eq!(store.get("test.token").unwrap().as_deref(), Some("first"));
    assert_eq!(
        encrypted_file::test_derivation_count() - before,
        1,
        "cold existing-vault read must derive once, not once to unlock and again to read"
    );
    store.put("test.second", "second").unwrap();
    assert_eq!(
        store.get_many(&["test.token", "test.second"]).unwrap(),
        vec![Some("first".into()), Some("second".into())]
    );
    store.delete("test.token").unwrap();
    assert_eq!(store.get("test.token").unwrap(), None);
    assert_eq!(
        encrypted_file::test_derivation_count() - before,
        1,
        "unchanged validated parameters must reuse only the derived key across operations"
    );
}

#[test]
#[serial_test::serial(vault_passphrase_env)]
fn encrypted_derived_key_reads_external_updates_and_deletion() {
    let _passphrase = EnvPassphraseGuard::set("derived-key-update-fixture");
    let _fd = EnvVarGuard::remove("WAYLAND_VAULT_PASSPHRASE_FD");
    let root = tempdir().unwrap();
    let cipher = root.path().join("vault.enc");
    let params = root.path().join("vault.params.json");
    let first = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
    first.put("token", "before").unwrap();
    let other = EncryptedFileCredentialsStore::new(cipher, params);
    other.put("token", "after").unwrap();
    let before = encrypted_file::test_derivation_count();
    assert_eq!(first.get("token").unwrap().as_deref(), Some("after"));
    other.delete("token").unwrap();
    assert_eq!(
        first.get("token").unwrap(),
        None,
        "credentials themselves must never be cached"
    );
    assert_eq!(encrypted_file::test_derivation_count(), before);
}

#[test]
#[serial_test::serial(vault_passphrase_env)]
fn encrypted_derived_key_rebinds_to_complete_changed_params() {
    const PASSWORD: &str = "derived-key-rotation-fixture";
    let _passphrase = EnvPassphraseGuard::set(PASSWORD);
    let _fd = EnvVarGuard::remove("WAYLAND_VAULT_PASSPHRASE_FD");
    let root = tempdir().unwrap();
    let cipher = root.path().join("vault.enc");
    let params_path = root.path().join("vault.params.json");
    let store = EncryptedFileCredentialsStore::new(cipher.clone(), params_path.clone());
    store.put("token", "old").unwrap();
    let old_params = encrypted_file::load_key_params(&params_path).unwrap();
    let mut changed = old_params.clone();
    changed.t_cost += 1; // Same salt, different validated KDF tuning.
    let key = zeroize::Zeroizing::new(encrypted_file::derive_key(PASSWORD, &changed).unwrap());
    let blob = encrypted_file::encrypt_with_key(b"[secrets]\ntoken = 'new'\n", &key).unwrap();
    std::fs::write(&cipher, blob).unwrap();
    encrypted_file::save_key_params(&changed, &params_path).unwrap();
    secure_credential_file(&cipher).unwrap();
    secure_credential_file(&params_path).unwrap();
    let before = encrypted_file::test_derivation_count();
    assert_eq!(store.get("token").unwrap().as_deref(), Some("new"));
    assert_eq!(encrypted_file::test_derivation_count() - before, 1);
    assert_eq!(store.get("token").unwrap().as_deref(), Some("new"));
    assert_eq!(encrypted_file::test_derivation_count() - before, 1);

    // A newly salted valid vault also requires a fresh key, never the previous one.
    let (blob, salted) =
        encrypted_file::encrypt(b"[secrets]\ntoken = 'salted'\n", PASSWORD).unwrap();
    assert_ne!(salted.salt_b64, old_params.salt_b64);
    std::fs::write(&cipher, blob).unwrap();
    encrypted_file::save_key_params(&salted, &params_path).unwrap();
    secure_credential_file(&params_path).unwrap();
    assert_eq!(store.get("token").unwrap().as_deref(), Some("salted"));
}

#[test]
#[serial_test::serial(vault_passphrase_env)]
fn encrypted_derived_key_never_accepts_corrupt_cipher_or_params() {
    let _passphrase = EnvPassphraseGuard::set("derived-key-corruption-fixture");
    let _fd = EnvVarGuard::remove("WAYLAND_VAULT_PASSPHRASE_FD");
    let root = tempdir().unwrap();
    let cipher = root.path().join("vault.enc");
    let params = root.path().join("vault.params.json");
    let store = EncryptedFileCredentialsStore::new(cipher.clone(), params.clone());
    store.put("token", "valid").unwrap();
    let valid_cipher = std::fs::read(&cipher).unwrap();
    let valid_params = std::fs::read(&params).unwrap();
    let mut corrupted = valid_cipher.clone();
    *corrupted.last_mut().unwrap() ^= 1;
    std::fs::write(&cipher, &corrupted).unwrap();
    assert!(store.get("token").is_err());
    assert!(
        store
            .unlocked
            .lock()
            .as_ref()
            .unwrap()
            .derived_key
            .is_none()
    );
    assert_eq!(std::fs::read(&cipher).unwrap(), corrupted);
    std::fs::write(&cipher, &valid_cipher).unwrap();
    assert_eq!(store.get("token").unwrap().as_deref(), Some("valid"));
    let mut unsupported: encrypted_file::KdfParams = serde_json::from_slice(&valid_params).unwrap();
    unsupported.version = 2;
    let unsupported = serde_json::to_vec(&unsupported).unwrap();
    for bad in [b"not json".as_slice(), unsupported.as_slice()] {
        std::fs::write(&params, bad).unwrap();
        assert!(store.get("token").is_err());
        assert_eq!(std::fs::read(&cipher).unwrap(), valid_cipher);
    }
    std::fs::remove_file(&params).unwrap();
    assert!(
        store.get("token").is_err(),
        "missing parameters cannot fall back to warm cached parameters"
    );
    std::fs::write(&params, valid_params).unwrap();
    secure_credential_file(&params).unwrap();
    assert_eq!(store.get("token").unwrap().as_deref(), Some("valid"));
    let _wrong = EnvPassphraseGuard::set("incorrect-fixture-password");
    let wrong = EncryptedFileCredentialsStore::new(cipher.clone(), params);
    assert!(wrong.get("token").is_err());
    assert_eq!(std::fs::read(cipher).unwrap(), valid_cipher);
}

#[test]
#[cfg(unix)]
#[serial_test::serial(vault_passphrase_env)]
fn encrypted_derived_key_warm_store_rechecks_parameter_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let _passphrase = EnvPassphraseGuard::set("derived-key-permission-fixture");
    let _fd = EnvVarGuard::remove("WAYLAND_VAULT_PASSPHRASE_FD");
    let root = tempdir().unwrap();
    let cipher = root.path().join("vault.enc");
    let params = root.path().join("vault.params.json");
    let store = EncryptedFileCredentialsStore::new(cipher, params.clone());
    store.put("token", "valid").unwrap();
    std::fs::set_permissions(&params, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store.get("token").is_err());
    assert!(
        store
            .unlocked
            .lock()
            .as_ref()
            .unwrap()
            .derived_key
            .is_none()
    );
    std::fs::set_permissions(&params, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(store.get("token").unwrap().as_deref(), Some("valid"));
}

#[test]
#[serial_test::serial(vault_passphrase_env)]
fn encrypted_derived_key_preserves_recovery_key_and_aad_authority() {
    use crate::confidential_blob::{
        ConfidentialBlobAad, delete_confidential_blob_key, load_confidential_blob_key,
        load_or_create_confidential_blob_key, open_confidential_blob, seal_confidential_blob,
    };
    let _passphrase = EnvPassphraseGuard::set("derived-key-recovery-fixture");
    let _fd = EnvVarGuard::remove("WAYLAND_VAULT_PASSPHRASE_FD");
    let root = tempdir().unwrap();
    let cipher = root.path().join("vault.enc");
    let params = root.path().join("vault.params.json");
    let make_store = || {
        ConfidentialCredentialsStore::new(
            Box::new(EncryptedFileCredentialsStore::new(
                cipher.clone(),
                params.clone(),
            )),
            root.path().join("confidential-key.lock"),
            None,
        )
    };
    let first = make_store();
    let key = load_or_create_confidential_blob_key(&first, "recovery.fixture").unwrap();
    let aad = ConfidentialBlobAad::new("fixture-purpose", b"session-one".to_vec());
    let sealed = seal_confidential_blob(&key, &aad, b"exact request").unwrap();
    let encoded = first.get("recovery.fixture").unwrap();
    let second = make_store();
    let reopened = load_or_create_confidential_blob_key(&second, "recovery.fixture").unwrap();
    assert_eq!(second.get("recovery.fixture").unwrap(), encoded);
    assert_eq!(
        open_confidential_blob(&reopened, &aad, &sealed).unwrap(),
        b"exact request"
    );
    assert!(
        open_confidential_blob(
            &reopened,
            &ConfidentialBlobAad::new("fixture-purpose", b"different-session".to_vec()),
            &sealed
        )
        .is_err()
    );
    second
        .put("recovery.fixture", "corrupted key representation")
        .unwrap();
    let corrupted = std::fs::read(&cipher).unwrap();
    assert!(load_or_create_confidential_blob_key(&first, "recovery.fixture").is_err());
    assert_eq!(
        std::fs::read(&cipher).unwrap(),
        corrupted,
        "corrupt recovery keys are refused, never replaced"
    );
    delete_confidential_blob_key(&second, "recovery.fixture").unwrap();
    assert!(load_confidential_blob_key(&first, "recovery.fixture").is_err());
    assert_eq!(first.get("recovery.fixture").unwrap(), None);
}
