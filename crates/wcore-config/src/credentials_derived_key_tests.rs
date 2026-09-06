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
