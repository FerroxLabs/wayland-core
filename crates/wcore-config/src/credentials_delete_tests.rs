//! W06 backend-edge regressions. Fixtures never open the host keyring.

use super::*;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::path::Path;
use std::process::{Output, Stdio};
use std::sync::Mutex;
use std::time::Duration;

const KEY: &str = "oauth.chatgpt.tokens";
const SENTINEL: &str = "w06-private-fixture-sentinel";
const OTHER_KEY: &str = "providers.other.api_key";
const PASSPHRASE: &str = "w06-isolated-fixture-passphrase";

fn assert_redacted(error: CredentialsError) {
    let display = error.to_string();
    assert!(!display.is_empty(), "failure must retain an explanation");
    assert!(
        !display.contains(SENTINEL),
        "Display exposed credential input"
    );
    assert!(
        !format!("{error:?}").contains(SENTINEL),
        "Debug exposed credential input"
    );
    let mut source = error.source();
    while let Some(cause) = source {
        assert!(
            !cause.to_string().contains(SENTINEL) && !format!("{cause:?}").contains(SENTINEL),
            "the source chain retained credential input"
        );
        source = cause.source();
    }
    let wrapped = anyhow::Error::new(error).context("removing stored credential");
    assert!(
        !format!("{wrapped:#}").contains(SENTINEL) && !format!("{wrapped:?}").contains(SENTINEL),
        "the full application error chain exposed credential input"
    );
}

fn malformed_secret() -> String {
    format!("[secrets]\nkey = \"{SENTINEL}\" trailing-invalid\n")
}

#[test]
fn credential_parse_error_redacts_display_debug_and_sources() {
    let raw = malformed_secret().parse::<toml::Table>().unwrap_err();
    assert!(
        raw.to_string().contains(SENTINEL),
        "positive control: parser diagnostic must contain the secret-bearing line"
    );
    assert_redacted(raw.into());
}

#[test]
fn plaintext_delete_parse_error_redacts_the_secret_bearing_line() {
    let root = tempfile::tempdir().unwrap();
    let store = PlaintextCredentialsStore::new(root.path().join("credentials.toml"));
    std::fs::write(store.path(), malformed_secret()).unwrap();
    assert_redacted(store.delete(KEY).expect_err("malformed store must fail"));
}

fn vault_config(home: &Path, explicit: bool) -> CredentialsStorageConfig {
    if explicit {
        CredentialsStorageConfig {
            backend: CredentialsBackend::EncryptedFile {
                cipher_path: home.join("custom").join("tokens.cipher"),
                key_params_path: home.join("custom").join("tokens.kdf"),
            },
            service_name: None,
        }
    } else {
        CredentialsStorageConfig::default()
    }
}

fn vault_paths(home: &Path, cfg: &CredentialsStorageConfig) -> (PathBuf, PathBuf) {
    match &cfg.backend {
        CredentialsBackend::EncryptedFile {
            cipher_path,
            key_params_path,
        } => (cipher_path.clone(), key_params_path.clone()),
        _ => default_vault_paths(&home.join("credentials.toml")),
    }
}

fn write_encrypted_fixture(cipher: &Path, params_path: &Path, plaintext: &str) {
    secure_credential_dir(cipher.parent().unwrap()).unwrap();
    let (blob, params) = encrypted_file::encrypt(plaintext.as_bytes(), PASSPHRASE).unwrap();
    std::fs::write(cipher, blob).unwrap();
    secure_credential_file(cipher).unwrap();
    encrypted_file::save_key_params(&params, params_path).unwrap();
    secure_credential_file(params_path).unwrap();
}

// Re-entered with only fixture-specific environment; returns in ordinary enumeration.
#[tokio::test]
async fn vault_child() {
    let Ok(action) = std::env::var("W06_VAULT_ACTION") else {
        return;
    };
    let home = PathBuf::from(std::env::var_os("WAYLAND_HOME").unwrap());
    let cfg = vault_config(
        &home,
        std::env::var("W06_VAULT_LAYOUT").unwrap() == "explicit",
    );
    let plaintext_path = home.join("credentials.toml");
    let (cipher, params) = vault_paths(&home, &cfg);
    match action.as_str() {
        "seed" => {
            let ladder = build_ladder(&cfg, &plaintext_path);
            ladder.put(KEY, SENTINEL).unwrap();
            ladder.put(OTHER_KEY, "unrelated-fixture-value").unwrap();
            assert_eq!(ladder.get(KEY).unwrap().as_deref(), Some(SENTINEL));
            assert!(cipher.exists() && params.exists());
            assert!(!plaintext_path.exists());
        }
        "locked" => {
            assert!(!vault_unlock_material_present());
            let before = std::fs::read(&cipher).unwrap();
            let ladder = build_ladder(&cfg, &plaintext_path);
            ladder
                .delete(KEY)
                .expect_err("a locked existing vault is incomplete removal");
            assert_eq!(std::fs::read(&cipher).unwrap(), before);
            assert!(!plaintext_path.exists());
        }
        "retry" => {
            let ladder = build_ladder(&cfg, &plaintext_path);
            assert_eq!(ladder.get(KEY).unwrap().as_deref(), Some(SENTINEL));
            ladder.delete(KEY).unwrap();
            ladder.delete(KEY).unwrap();
            let reopened = build_ladder(&cfg, &plaintext_path);
            assert_eq!(reopened.get(KEY).unwrap(), None);
            assert_eq!(
                reopened.get(OTHER_KEY).unwrap().as_deref(),
                Some("unrelated-fixture-value")
            );
            assert!(!plaintext_path.exists());
        }
        "absent" => {
            assert!(!vault_unlock_material_present());
            let ladder = build_ladder(&cfg, &plaintext_path);
            ladder.delete(KEY).unwrap();
            ladder.delete(KEY).unwrap();
            assert!(!cipher.exists() && !params.exists() && !plaintext_path.exists());
        }
        "late" => {
            assert!(!vault_unlock_material_present());
            let ladder = build_ladder(&cfg, &plaintext_path);
            assert!(!cipher.exists());
            write_encrypted_fixture(
                &cipher,
                &params,
                &format!("[secrets]\n\"{KEY}\" = \"{SENTINEL}\"\n"),
            );
            ladder
                .delete(KEY)
                .expect_err("delete must recheck a vault created after ladder construction");
            assert!(cipher.exists());
        }
        "malformed" => {
            write_encrypted_fixture(&cipher, &params, &malformed_secret());
            let ladder = build_ladder(&cfg, &plaintext_path);
            assert_redacted(
                ladder
                    .delete(KEY)
                    .expect_err("malformed encrypted store must fail"),
            );
        }
        _ => panic!("unknown fixture action"),
    }
}

async fn run_vault_child(home: &Path, action: &str, explicit: bool) -> Output {
    let exe = std::env::current_exe().unwrap();
    let mut command = crate::shell::shell_command_argv(
        exe.to_str().unwrap(),
        &[
            "--exact",
            "credentials::delete_backend_tests::vault_child",
            "--nocapture",
            "--test-threads=1",
        ],
    );
    command.env_clear();
    for name in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "TMP",
        "TEMP",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("WAYLAND_HOME", home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("W06_VAULT_ACTION", action)
        .env(
            "W06_VAULT_LAYOUT",
            if explicit { "explicit" } else { "default" },
        )
        .current_dir(home)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    if matches!(action, "seed" | "retry" | "malformed") {
        command.env("WAYLAND_VAULT_PASSPHRASE", PASSPHRASE);
    }
    tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .expect("bounded fixture child")
        .expect("start fixture child")
}

fn assert_child_passed(output: Output) {
    assert!(
        output.status.success(),
        "fixture failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn locked_default_vault_refuses_delete_then_recovers_in_another_process() {
    let home = tempfile::tempdir().unwrap();
    assert_child_passed(run_vault_child(home.path(), "seed", false).await);
    assert_child_passed(run_vault_child(home.path(), "locked", false).await);
    assert_child_passed(run_vault_child(home.path(), "retry", false).await);
}

#[tokio::test]
async fn locked_explicit_vault_refuses_delete_then_recovers_in_another_process() {
    let home = tempfile::tempdir().unwrap();
    assert_child_passed(run_vault_child(home.path(), "seed", true).await);
    assert_child_passed(run_vault_child(home.path(), "locked", true).await);
    assert_child_passed(run_vault_child(home.path(), "retry", true).await);
}

#[tokio::test]
async fn absent_isolated_vault_delete_is_idempotent_without_creating_stores() {
    for explicit in [false, true] {
        let home = tempfile::tempdir().unwrap();
        assert_child_passed(run_vault_child(home.path(), "absent", explicit).await);
    }
}

#[tokio::test]
async fn locked_vault_created_after_open_is_not_cached_as_absent() {
    for explicit in [false, true] {
        let home = tempfile::tempdir().unwrap();
        assert_child_passed(run_vault_child(home.path(), "late", explicit).await);
    }
}

#[tokio::test]
async fn encrypted_delete_error_redacts_the_decrypted_secret_bearing_line() {
    let home = tempfile::tempdir().unwrap();
    assert_child_passed(run_vault_child(home.path(), "malformed", false).await);
}

#[derive(Default)]
struct ChunkFaultStore {
    entries: Mutex<HashMap<String, String>>,
    failed_deletes: Mutex<HashSet<String>>,
    deletes: Mutex<Vec<String>>,
}

impl CredentialsStore for ChunkFaultStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        Ok(self.entries.lock().unwrap().get(key).cloned())
    }
    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        self.entries
            .lock()
            .unwrap()
            .insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        self.deletes.lock().unwrap().push(key.into());
        if self.failed_deletes.lock().unwrap().contains(key) {
            return Err(CredentialsError::Keyring("injected retained chunk".into()));
        }
        self.entries.lock().unwrap().remove(key);
        Ok(())
    }
}

#[test]
fn failed_chunk_deletion_keeps_identity_and_retry_removes_every_fragment() {
    for failures in [1, 2] {
        let dir = tempfile::tempdir().unwrap();
        let locks = ChunkWriteLockSite::in_dir(dir.path(), LockPolicy::CREDENTIAL_WRITE);
        let store = ChunkFaultStore::default();
        chunked_put(&store, KEY, &SENTINEL.repeat(150), 1000, &locks).unwrap();
        let manifest = read_previous_manifest(&store, KEY)
            .unwrap()
            .expect("spanned control");
        assert!(manifest.count > 2);
        let failed: Vec<String> = (0..failures)
            .map(|i| chunk_key(KEY, manifest.generation, i))
            .collect();
        store
            .failed_deletes
            .lock()
            .unwrap()
            .extend(failed.iter().cloned());
        store.deletes.lock().unwrap().clear();

        let result = chunked_delete(&store, KEY, &locks);
        assert!(
            result.is_err(),
            "retained secret chunks cannot report complete removal"
        );
        assert!(
            read_previous_manifest(&store, KEY).unwrap().is_some(),
            "retry must retain the generation identity"
        );
        for i in 0..manifest.count {
            assert!(
                store
                    .deletes
                    .lock()
                    .unwrap()
                    .contains(&chunk_key(KEY, manifest.generation, i)),
                "every required chunk must be attempted"
            );
        }
        for key in &failed {
            assert!(store.get(key).unwrap().is_some());
        }
        store.failed_deletes.lock().unwrap().clear();
        chunked_delete(&store, KEY, &locks).unwrap();
        chunked_delete(&store, KEY, &locks).unwrap();
        assert!(store.entries.lock().unwrap().is_empty());
    }
}

#[test]
fn successful_chunk_delete_removes_manifest_and_parts() {
    let dir = tempfile::tempdir().unwrap();
    let locks = ChunkWriteLockSite::in_dir(dir.path(), LockPolicy::CREDENTIAL_WRITE);
    let store = ChunkFaultStore::default();
    chunked_put(&store, KEY, &SENTINEL.repeat(150), 1000, &locks).unwrap();
    assert!(read_previous_manifest(&store, KEY).unwrap().is_some());
    chunked_delete(&store, KEY, &locks).unwrap();
    chunked_delete(&store, KEY, &locks).unwrap();
    assert!(store.entries.lock().unwrap().is_empty());
}

#[test]
fn superseded_write_chunk_cleanup_remains_best_effort() {
    let dir = tempfile::tempdir().unwrap();
    let locks = ChunkWriteLockSite::in_dir(dir.path(), LockPolicy::CREDENTIAL_WRITE);
    let store = ChunkFaultStore::default();
    chunked_put(&store, KEY, &SENTINEL.repeat(150), 1000, &locks).unwrap();
    let old = read_previous_manifest(&store, KEY).unwrap().unwrap();
    store
        .failed_deletes
        .lock()
        .unwrap()
        .insert(chunk_key(KEY, old.generation, 0));
    chunked_put(&store, KEY, "replacement", 1000, &locks).unwrap();
    assert_eq!(
        chunked_get(&store, KEY, &locks).unwrap().as_deref(),
        Some("replacement")
    );
}
