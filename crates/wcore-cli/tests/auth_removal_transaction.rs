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
    remove_provider(home, "openai").await
}

async fn remove_provider(home: &Path, provider: &str) -> Output {
    let mut command = shell_command_argv(
        env!("CARGO_BIN_EXE_wayland-core"),
        &["auth", "remove", provider],
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

#[tokio::test]
async fn partial_account_removal_preserves_identity_for_retry() {
    for broken_backend in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let backend = if broken_backend {
            "invalid-backend-fixture"
        } else {
            "plaintext"
        };
        std::fs::write(
            home.path().join("config.toml"),
            format!(
                "[storage.credentials]\nbackend = {backend:?}\n\
                 [providers.work-openai]\nprovider = \"openai\"\n\
                 base_url = \"https://tenant.example.invalid/v1\"\n\
                 model = \"fixture-model\"\napi_key = {CONFIG_VALUE:?}\n"
            ),
        )
        .unwrap();
        let slot = "providers.work-openai.api_key";
        let store_path = home.path().join("credentials.toml");
        let store = PlaintextCredentialsStore::new(&store_path);
        if broken_backend {
            store.put(slot, STORE_VALUE).unwrap();
        } else {
            std::fs::write(&store_path, "[invalid").unwrap();
        }

        let output = remove_provider(home.path(), "work-openai").await;
        assert_incomplete(&output);
        assert_config_copy_removed(home.path());
        let config_path = home.path().join("config.toml");
        let text = std::fs::read_to_string(&config_path).unwrap();
        let document: toml::Table = toml::from_str(&text).unwrap();
        let account = document
            .get("providers")
            .and_then(|providers| providers.get("work-openai"))
            .expect("partial removal must retain the account identity needed for retry");
        assert_eq!(account["provider"].as_str(), Some("openai"));
        assert_eq!(account["model"].as_str(), Some("fixture-model"));
        assert_eq!(
            account["base_url"].as_str(),
            Some("https://tenant.example.invalid/v1")
        );
        assert!(account.get("api_key").is_none());

        std::fs::write(
            &config_path,
            text.replace("invalid-backend-fixture", "plaintext"),
        )
        .unwrap();
        if !broken_backend {
            std::fs::write(&store_path, "[secrets]\n").unwrap();
            store.put(slot, STORE_VALUE).unwrap();
        }
        let retry = remove_provider(home.path(), "work-openai").await;
        assert!(retry.status.success(), "{:?}", retry);
        assert_eq!(store.get(slot).unwrap(), None);
        let document: toml::Table =
            toml::from_str(&std::fs::read_to_string(config_path).unwrap()).unwrap();
        assert!(
            document
                .get("providers")
                .and_then(|providers| providers.get("work-openai"))
                .is_none()
        );
    }
}
