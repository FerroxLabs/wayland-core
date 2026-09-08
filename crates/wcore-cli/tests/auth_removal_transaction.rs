//! W06 / wayland#1324: real CLI removal must report incomplete cleanup.

#[path = "support/owned_tree.rs"]
mod owned_tree;

use owned_tree::OwnedTree;
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
async fn malformed_config_does_not_echo_its_secret_bearing_line() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        format!("[providers.openai]\napi_key = \"{CONFIG_VALUE}\" trailing-invalid\n"),
    )
    .unwrap();

    let output = remove(home.path()).await;
    assert_incomplete(&output);
}

#[tokio::test]
async fn invalid_credential_setting_does_not_echo_its_value() {
    let home = tempfile::tempdir().unwrap();
    config(home.path(), CONFIG_VALUE);

    let output = remove(home.path()).await;
    assert_incomplete(&output);
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

fn oauth_storage(home: &Path) -> wcore_agent::oauth::OAuthStorage {
    wcore_agent::oauth::OAuthStorage::at_root(
        home.join("oauth"),
        Box::new(wcore_config::credentials::InMemoryCredentialsStore::new()),
    )
    .unwrap()
}

fn codex_login(home: &Path, account: &str) {
    use base64::Engine;
    let codex = home.join("codex");
    std::fs::create_dir_all(&codex).unwrap();
    let claims = serde_json::json!({"https://api.openai.com/auth": {"chatgpt_account_id": account, "chatgpt_plan_type": account}, "exp": 4_000_000_000u64});
    let jwt = format!(
        "hdr.{}.sig",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let file = codex.join("auth.json");
    std::fs::write(
        &file,
        serde_json::json!({"tokens": {"access_token": jwt, "refresh_token": "fixture-refresh"}})
            .to_string(),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn oauth_command(home: &Path, args: &[&str]) -> tokio::process::Command {
    let mut command = shell_command_argv(env!("CARGO_BIN_EXE_wayland-core"), args);
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
        .env("CODEX_HOME", home.join("codex"))
        .env(
            "WAYLAND_VAULT_PASSPHRASE",
            "w07-isolated-fixture-passphrase",
        )
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn oauth_run(home: &Path, args: &[&str]) -> Output {
    tokio::time::timeout(Duration::from_secs(55), oauth_command(home, args).output())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn oauth_logout_waits_for_writer_then_removes_its_login() {
    let home = tempfile::tempdir().unwrap();
    let storage = oauth_storage(home.path());
    let writer =
        wcore_agent::oauth::refresh_lock::hold_for_writer(storage.refresh_lock_path("chatgpt"))
            .await
            .unwrap();
    let mut child = OwnedTree::new(
        oauth_command(home.path(), &["auth", "logout", "chatgpt"])
            .spawn()
            .unwrap(),
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(500), child.wait())
            .await
            .is_err(),
        "CLI logout must contend on the provider writer lock"
    );
    // Simulate the earlier writer landing a credential while logout waits.
    let token = wcore_agent::oauth::OAuthTokens {
        access_token: "fixture-access".into(),
        refresh_token: Some("fixture-refresh".into()),
        expires_at_unix_secs: Some(0),
        token_type: "Bearer".into(),
        scope: None,
        id_token: None,
    };
    std::fs::write(
        storage.path_for("chatgpt"),
        serde_json::to_vec(&token).unwrap(),
    )
    .unwrap();
    drop(writer);
    let output = child.wait_with_output().await.unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("Signed out"));
    assert!(!storage.path_for("chatgpt").exists());
    let fresh = oauth_run(home.path(), &["auth", "status"]).await;
    assert!(fresh.status.success(), "{fresh:?}");
    assert!(String::from_utf8_lossy(&fresh.stdout).contains("not signed in"));
}

#[tokio::test]
async fn oauth_import_and_status_reread_external_source_after_writer() {
    for args in [
        &["auth", "login", "chatgpt", "--import-codex"][..],
        &["auth", "status"][..],
    ] {
        let home = tempfile::tempdir().unwrap();
        codex_login(home.path(), "before");
        let storage = oauth_storage(home.path());
        let writer =
            wcore_agent::oauth::refresh_lock::hold_for_writer(storage.refresh_lock_path("chatgpt"))
                .await
                .unwrap();
        let mut child = OwnedTree::new(oauth_command(home.path(), args).spawn().unwrap());
        assert!(
            tokio::time::timeout(Duration::from_millis(500), child.wait())
                .await
                .is_err(),
            "import must wait for writer authority"
        );
        codex_login(home.path(), "after");
        drop(writer);
        let output = child.wait_with_output().await.unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("after"),
            "must read source under lock: {output:?}"
        );
        let status = oauth_run(home.path(), &["auth", "status"]).await;
        assert!(status.status.success(), "{status:?}");
        assert!(String::from_utf8_lossy(&status.stdout).contains("after"));
    }
}

#[tokio::test]
async fn oauth_logout_reports_external_codex_reauthentication_source() {
    let home = tempfile::tempdir().unwrap();
    codex_login(home.path(), "external");
    let imported = oauth_run(home.path(), &["auth", "login", "chatgpt", "--import-codex"]).await;
    assert!(imported.status.success(), "{imported:?}");
    let logout = oauth_run(home.path(), &["auth", "logout", "chatgpt"]).await;
    assert!(logout.status.success(), "{logout:?}");
    assert!(String::from_utf8_lossy(&logout.stdout).contains("Codex CLI login can authenticate"));
    let status = oauth_run(home.path(), &["auth", "status"]).await;
    assert!(status.status.success(), "{status:?}");
    let text = String::from_utf8_lossy(&status.stdout);
    assert!(
        text.contains("imported an existing ChatGPT login from the Codex CLI"),
        "{text}"
    );
    assert!(text.contains("signed in"));
}

#[tokio::test]
async fn oauth_busy_writer_refuses_logout_without_success_text() {
    let home = tempfile::tempdir().unwrap();
    let storage = oauth_storage(home.path());
    let _writer =
        wcore_agent::oauth::refresh_lock::hold_for_writer(storage.refresh_lock_path("chatgpt"))
            .await
            .unwrap();
    let output = oauth_run(home.path(), &["auth", "logout", "chatgpt"]).await;
    assert!(!output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("Signed out"));
    assert!(!stdout.contains("Already signed out"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Nothing was changed"));
}

#[tokio::test]
async fn explicit_codex_import_replaces_malformed_prior_oauth_json() {
    let home = tempfile::tempdir().unwrap();
    codex_login(home.path(), "replacement");
    let storage = oauth_storage(home.path());
    std::fs::write(storage.path_for("chatgpt"), "{malformed-old-login").unwrap();
    let output = oauth_run(home.path(), &["auth", "login", "chatgpt", "--import-codex"]).await;
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("replacement"));
    assert!(!storage.path_for("chatgpt").exists());
    let fresh = oauth_run(home.path(), &["auth", "status"]).await;
    assert!(fresh.status.success(), "{fresh:?}");
    assert!(String::from_utf8_lossy(&fresh.stdout).contains("replacement"));
}
