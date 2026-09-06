//! W10: persisted restrictions are validated by actual bootstrap before dispatch.
use std::{path::Path, sync::Arc, time::Duration};

use serde_json::{Value, json};
use wcore_agent::{
    bootstrap::AgentBootstrap, bootstrap_cleanup::BootstrapCleanup, output::null_sink::NullSink,
};
use wcore_config::{config::Config, credentials::CredentialsBackend};
use wcore_permissions::{CallActor, LearnedDecision, LearnedPolicy};

#[path = "../../wcore-cli/tests/support/mock_llm.rs"]
mod mock_llm;

#[test]
fn actual_bootstrap_preserves_learned_restrictions() {
    const CHILD: &str = "W10_LEARNED_POLICY_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(exercise(Path::new(&root)));
        return;
    }
    // Set environment before the child's runtime starts: no process-global
    // environment mutation races, and no access to the operator's profile.
    let root = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "actual_bootstrap_preserves_learned_restrictions",
            "--nocapture",
        ])
        .env(CHILD, root.path())
        .env("WAYLAND_HOME", root.path())
        .env("WAYLAND_VAULT_PASSPHRASE", "w10-fixture-only-passphrase")
        .env_remove("WAYLAND_VAULT_PASSPHRASE_FD")
        .env("HOME", root.path())
        .env("USERPROFILE", root.path())
        .env("XDG_CONFIG_HOME", root.path())
        .env("XDG_DATA_HOME", root.path())
        .env("WAYLAND_PLUGINS_DIR", root.path().join("plugins"))
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn exercise(root: &Path) {
    let path = LearnedPolicy::default_path().unwrap();
    assert_eq!(path, root.join("permissions.toml"));
    std::fs::write(root.join("sentinel.txt"), "w10-sentinel-payload").unwrap();
    probe(root, "absent", false, false).await;

    let mut deny = LearnedPolicy::new();
    deny.record("Read", Some("*".into()), LearnedDecision::DenyAlways);
    deny.save_to(&path).unwrap();
    probe(root, "valid", false, true).await;

    std::fs::write(&path, "rules = [this is corrupt").unwrap();
    probe(root, "corrupt", true, false).await;
    deny.save_to(&path).unwrap();
    probe(root, "repaired-corrupt", false, true).await;

    // A directory produces a real read error even under privileged Linux
    // runners, where chmod(000) would still allow the file to be read.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(std::fs::read_to_string(&path).is_err());
    probe(root, "unreadable", true, false).await;
    std::fs::remove_dir(&path).unwrap();
    deny.save_to(&path).unwrap();
    probe(root, "repaired-unreadable", false, true).await;
}

async fn probe(root: &Path, case: &str, startup_error: bool, denied: bool) {
    let mock = mock_llm::MockLlm::new()
        .tool_use("Read", json!({"file_path": root.join("sentinel.txt")}))
        .text("finished")
        .start()
        .await;
    let mut config = Config {
        model: "claude-mock".into(),
        api_key: "fixture-not-real".into(),
        base_url: mock.uri(),
        max_turns: Some(3),
        ..Default::default()
    };
    config.compat = toml::from_str("cost_is_known_free = true").unwrap();
    config.memory.enabled = false;
    config.observability.skills_lifecycle = false;
    config.storage.credentials.backend = CredentialsBackend::EncryptedFile {
        cipher_path: root.join("credentials.enc"),
        key_params_path: root.join("credentials.kdf.json"),
    };
    config.session.enabled = true;
    config.session.directory = root.join(case).to_string_lossy().into_owned();
    let cleanup = Arc::new(BootstrapCleanup::default());
    let mut result = match AgentBootstrap::new(config, root.to_string_lossy(), Arc::new(NullSink))
        .with_cleanup(cleanup.clone())
        .without_channels(true)
        .defer_config_mcp(true)
        .build()
        .await
    {
        Ok(result) => {
            assert!(
                !startup_error,
                "{case}: broken restrictions allowed startup"
            );
            result
        }
        Err(error) => {
            assert!(startup_error, "{case}: unexpected startup error: {error:#}");
            assert!(
                error.to_string().contains("failed to load learned policy"),
                "{error:#}"
            );
            assert!(mock.received_requests().await.unwrap().is_empty());
            cleanup.close().await.unwrap_or_else(|error| {
                panic!("{case}: failed startup could not retire: {error:#}")
            });
            return;
        }
    };
    // Use bootstrap's installed policy, never install a test copy. Actor
    // selection mirrors the sub-agent dispatch boundary used by the spawner.
    result.engine.set_call_actor(CallActor::SubAgent {
        id: case.into(),
        parent_id: Some("parent".into()),
    });
    result
        .engine
        .init_session("anthropic", &root.to_string_lossy(), None)
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(20),
        result.engine.run("read sentinel", case),
    )
    .await
    .unwrap()
    .unwrap();
    let requests = mock.received_requests().await.unwrap();
    let body: Value = requests
        .last()
        .expect("provider continuation")
        .body_json()
        .unwrap();
    let receipt = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .filter_map(|message| message["content"].as_array())
        .flat_map(|blocks| blocks.iter().rev())
        .find(|block| block["type"] == "tool_result")
        .expect("sentinel dispatch receipt");
    if denied {
        assert_eq!(receipt["is_error"], true, "{case}: {receipt}");
        assert!(
            receipt["content"]
                .to_string()
                .contains("Denied by sub-agent learned policy"),
            "{case}: {receipt}"
        );
        assert!(
            !receipt["content"]
                .to_string()
                .contains("w10-sentinel-payload")
        );
    } else {
        assert_ne!(receipt["is_error"], true, "{case}: {receipt}");
        assert!(
            receipt["content"]
                .to_string()
                .contains("w10-sentinel-payload"),
            "{case}: {receipt}"
        );
    }
}
