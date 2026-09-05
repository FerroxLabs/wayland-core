//! W06 / wayland#1324: real CLI removal must report incomplete cleanup.

use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use wcore_config::credentials::{CredentialsStore, PlaintextCredentialsStore};
use wcore_config::shell::shell_command_argv;

const CONFIG_VALUE: &str = "config-fixture-value";
const STORE_VALUE: &str = "store-fixture-value";
const SLOT: &str = "providers.openai.api_key";

fn config(home: &Path, backend: &str) {
    std::fs::write(
        home.join("config.toml"),
        format!(
            "[storage.credentials]\nbackend = {backend:?}\n\
             [providers.openai]\napi_key = {CONFIG_VALUE:?}\n"
        ),
    )
    .expect("write isolated configuration");
}

async fn remove(home: &Path) -> Output {
    let mut command = shell_command_argv(
        env!("CARGO_BIN_EXE_wayland-core"),
        &["auth", "remove", "openai"],
    );
    command.env_clear();
    for key in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "TMP",
        "TEMP",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("WAYLAND_HOME", home)
        .current_dir(home)
        .stdin(Stdio::null());
    tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .expect("bounded CLI removal")
        .expect("run packaged CLI")
}

fn assert_config_copy_removed(home: &Path) {
    let text = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(
        !text.contains(CONFIG_VALUE),
        "plaintext config copy survived"
    );
}

fn assert_incomplete(output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "false removal success: {stdout}");
    assert!(!stdout.contains("Removed API key"), "{stdout}");
    assert!(
        !stderr.is_empty(),
        "incomplete removal needs an explanation"
    );
    for value in [CONFIG_VALUE, STORE_VALUE] {
        assert!(!stdout.contains(value));
        assert!(!stderr.contains(value));
    }
}

#[tokio::test]
async fn unavailable_store_does_not_report_success_after_config_removal() {
    let home = tempfile::tempdir().unwrap();
    config(home.path(), "invalid-backend-fixture");
    let store = PlaintextCredentialsStore::new(home.path().join("credentials.toml"));
    store.put(SLOT, STORE_VALUE).unwrap();

    let output = remove(home.path()).await;
    assert_incomplete(&output);
    assert_config_copy_removed(home.path());
    assert_eq!(store.get(SLOT).unwrap().as_deref(), Some(STORE_VALUE));

    // Repair only the backend selection and retry the same user operation.
    let path = home.path().join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("invalid-backend-fixture", "plaintext")).unwrap();
    let retry = remove(home.path()).await;
    assert!(retry.status.success(), "{:?}", retry);
    assert_eq!(store.get(SLOT).unwrap(), None);
}

#[tokio::test]
async fn failed_store_delete_still_removes_the_independent_config_copy() {
    let home = tempfile::tempdir().unwrap();
    config(home.path(), "plaintext");
    std::fs::write(home.path().join("credentials.toml"), "[invalid").unwrap();

    let output = remove(home.path()).await;
    assert_incomplete(&output);
    assert_config_copy_removed(home.path());
}

#[tokio::test]
async fn successful_removal_clears_both_locations_through_the_real_cli() {
    let home = tempfile::tempdir().unwrap();
    config(home.path(), "plaintext");
    let store = PlaintextCredentialsStore::new(home.path().join("credentials.toml"));
    store.put(SLOT, STORE_VALUE).unwrap();

    let output = remove(home.path()).await;
    assert!(output.status.success(), "{:?}", output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Removed API key"));
    assert_config_copy_removed(home.path());
    assert_eq!(store.get(SLOT).unwrap(), None);
}
