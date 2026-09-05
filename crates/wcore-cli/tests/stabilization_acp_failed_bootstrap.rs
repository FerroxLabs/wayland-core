//! W02: a failed ordinary ACP bootstrap must retain cleanup authority.
#![cfg(unix)]
use futures::StreamExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use wcore_acp::protocol::{MessageEvent, MessageSendRequest, SessionCreateRequest};
use wcore_acp::server::AcpServer;
use wcore_acp::transport::http::HttpHandler;
use wcore_cli::acp_engine::EngineTurnEngine;
use wcore_config::config::Config;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

struct FixtureProvider;
#[async_trait::async_trait]
impl LlmProvider for FixtureProvider {
    async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let (tx, rx) = mpsc::channel(2);
        tx.try_send(LlmEvent::TextDelta("bootstrap control".into()))
            .unwrap();
        tx.try_send(LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
        })
        .unwrap();
        Ok(rx)
    }
}

async fn run_child(mode: &str) {
    let root = tempfile::tempdir().unwrap();
    let mut command = wcore_config::shell::shell_command_argv(
        std::env::current_exe().unwrap().to_str().unwrap(),
        &["--exact", "bootstrap_child", "--nocapture"],
    );
    command
        .env("W02_BOOTSTRAP_CHILD", mode)
        .env("W02_BOOTSTRAP_ROOT", root.path())
        .env("WAYLAND_HOME", root.path().join("home"))
        .env("WAYLAND_PLUGINS_DIR", root.path().join("plugins"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(Duration::from_secs(35), command.output())
        .await
        .expect("bootstrap fixture subprocess deadline")
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn w02_failed_bootstrap_can_delete_loaded_plugin_resources() {
    run_child("failure").await;
}

#[tokio::test]
async fn w02_successful_bootstrap_delete_reaps_loaded_plugin() {
    run_child("control").await;
}

#[tokio::test]
async fn bootstrap_child() {
    let Ok(mode) = std::env::var("W02_BOOTSTRAP_CHILD") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("W02_BOOTSTRAP_ROOT").unwrap());
    assert!(wcore_config::config::profile_home().starts_with(&root));
    let plugin_dir = root.join("plugins").join("bootstrap-fixture");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let config_path = wcore_config::plugins_config::plugins_config_path();
    assert!(config_path.starts_with(&root));
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    // Trust is granted only inside this child process's private fixture home.
    std::fs::write(config_path, "plugin_signature_verification = false\n").unwrap();
    let script = plugin_dir.join("fixture");
    std::fs::write(
        &script,
        r#"#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path(sys.argv[1])
(root / 'plugin.pid').write_text(str(os.getpid()))
for line in sys.stdin:
    request = json.loads(line)
    verb = request['verb']
    if verb == 'init':
        body = {'kind':'init_result', 'manifest_version':'1', 'capabilities':[]}
    elif verb == 'list_tools':
        (root / 'plugin-initialized').write_text('handshake completed')
        body = {'kind':'tools_list', 'tools':[]}
    elif verb == 'shutdown':
        body = {'kind':'ack'}
    else:
        raise RuntimeError('unexpected fixture verb')
    print(json.dumps({'id':request['id'], **body}), flush=True)
    if verb == 'shutdown': break
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let manifest = format!(
        r#"[plugin]
name = "bootstrap-fixture"
version = "0.1.0"
description = "owned bootstrap lifecycle fixture"
entry = "fixture"
license = "MIT"
[permissions]
register_tools = true
tool_namespace = "BootstrapFixture"
[runtime]
kind = "subprocess"
[runtime.subprocess]
binary_path = "fixture"
args = [{}]
"#,
        toml::Value::String(root.to_string_lossy().into_owned())
    );
    std::fs::write(plugin_dir.join("plugin.toml"), manifest).unwrap();
    let mut config = Config {
        model: "fixture".into(),
        ..Default::default()
    };
    config.session.enabled = false;
    config.memory.enabled = false;
    if mode == "failure" {
        // Actual fallible bootstrap step AFTER on-disk runtime initialization.
        config.provider_chain.enabled = true;
        config.provider_chain.fallback_models = vec!["unresolved-fixture-model".into()];
        config.resolved_fallbacks.clear();
    }
    let server = AcpServer::new().with_turn_engine(Arc::new(EngineTurnEngine::with_provider(
        config,
        root.to_string_lossy().into_owned(),
        Arc::new(FixtureProvider),
    )));
    let id = server
        .create_session(SessionCreateRequest {
            model: None,
            tools: vec![],
            system_prompt: None,
            agent: None,
            mcp_servers: vec![],
        })
        .await
        .unwrap()
        .session_id;
    let send = tokio::time::timeout(
        Duration::from_secs(20),
        server.send_message(MessageSendRequest {
            session_id: id.clone(),
            text: "fixture".into(),
            tools: vec![],
        }),
    )
    .await
    .expect("bootstrap deadline");
    if mode == "failure" {
        let error = match send {
            Ok(_) => panic!("invalid fallback configuration was accepted"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("fallback configuration resolution mismatch"),
            "{error}"
        );
    } else {
        let frames =
            tokio::time::timeout(Duration::from_secs(5), send.unwrap().collect::<Vec<_>>())
                .await
                .unwrap();
        assert!(frames.iter().any(|event| matches!(event, MessageEvent::TextDelta { text } if text == "bootstrap control")), "{frames:?}");
    }
    assert!(
        root.join("plugin-initialized").exists(),
        "positive control: plugin must actually initialize before the tested failure/close"
    );
    let pid: i32 = std::fs::read_to_string(root.join("plugin.pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(pid > 1);
    tokio::time::timeout(Duration::from_secs(10), server.delete_session(id.clone()))
        .await
        .expect("close deadline")
        .expect("failed or successful bootstrap resources must be closable");
    // SAFETY: signal0 only probes the PID recorded by this private fixture.
    let exists = unsafe { libc::kill(pid, 0) } == 0;
    let error = std::io::Error::last_os_error();
    assert!(
        !exists && error.raw_os_error() == Some(libc::ESRCH),
        "plugin PID remains after close acknowledgment: {pid}"
    );
    assert!(server.get_session(id).await.is_err());
}
