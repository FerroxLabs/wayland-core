//! W02: real provider/approval/journal/tool cleanup through ordinary ACP.
//!
//! The runner supplies an isolated WAYLAND_HOME and confidential-store fixture.
//! Every case owns a separate workspace and session directory; no test mutates
//! process-global environment. Select `test(w02_)` so support self-tests cannot
//! substitute for these acceptance cases.

#[path = "support/mock_llm.rs"]
mod mock_llm;

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};
use mock_llm::MockLlm;
use tokio::time::timeout;
use wcore_acp::protocol::{MessageEvent, MessageSendRequest, SessionCreateRequest};
use wcore_acp::server::AcpServer;
use wcore_acp::transport::http::HttpHandler;
use wcore_acp::turn::{ApprovalDecision, ApprovalScopeWire};
use wcore_agent::session::SessionManager;
use wcore_agent::session_journal::{JournalError, SessionJournal};
use wcore_cli::acp_engine::EngineTurnEngine;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::Config;
use wcore_config::debug::DebugConfig;
use wcore_providers::anthropic::AnthropicProvider;

const DEADLINE: Duration = Duration::from_secs(20);
type Events = Pin<Box<dyn Stream<Item = MessageEvent> + Send>>;

fn config(workspace: &Path, durable: bool) -> Config {
    let mut config = Config {
        model: "claude-mock".into(),
        api_key: "fixture-no-real-key".into(),
        ..Default::default()
    };
    config.session.enabled = durable;
    config.session.require_durability = durable;
    config.session.directory = workspace.join("sessions").to_string_lossy().into_owned();
    config.memory.enabled = false;
    config.builtin_tools.defer_cold.enabled = false;
    config
}

fn server(config: Config, workspace: &Path, provider_url: &str, force: bool) -> AcpServer {
    let provider = Arc::new(
        AnthropicProvider::new(
            "fixture-no-real-key",
            provider_url,
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        )
        .with_cache(false),
    );
    AcpServer::new().with_turn_engine(Arc::new(
        EngineTurnEngine::with_provider(config, workspace.to_string_lossy().into_owned(), provider)
            .force_tools(force),
    ))
}

async fn start(server: &AcpServer) -> (String, Events) {
    let id = server
        .create_session(SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: None,
            mcp_servers: Vec::new(),
        })
        .await
        .expect("create session")
        .session_id;
    let stream = timeout(
        DEADLINE,
        server.send_message(MessageSendRequest {
            session_id: id.clone(),
            text: "Execute the requested fixture step.".into(),
            tools: Vec::new(),
        }),
    )
    .await
    .expect("bootstrap deadline")
    .expect("admit turn");
    (id, stream)
}

async fn approval(stream: &mut Events) -> (String, String, Vec<MessageEvent>) {
    timeout(DEADLINE, async {
        let mut seen = Vec::new();
        while let Some(event) = stream.next().await {
            if let MessageEvent::ApprovalRequired {
                call, resume_token, ..
            } = &event
            {
                assert_eq!(call.name, "Write", "fixture must reach the real Write gate");
                let gate = (call.id.clone(), resume_token.clone());
                seen.push(event);
                return (gate.0, gate.1, seen);
            }
            let terminal = matches!(
                event,
                MessageEvent::Done { .. } | MessageEvent::Error { .. }
            );
            seen.push(event);
            assert!(!terminal, "turn terminated before approval: {seen:?}");
        }
        panic!("approval stream ended: {seen:?}");
    })
    .await
    .expect("real approval deadline")
}

fn decision(token: String) -> ApprovalDecision {
    ApprovalDecision {
        approved: true,
        scope: ApprovalScopeWire::Once,
        answer: None,
        resume_token: (!token.is_empty()).then_some(token),
    }
}

fn one_terminal(frames: &[MessageEvent]) {
    assert_eq!(
        frames
            .iter()
            .filter(|event| matches!(
                event,
                MessageEvent::Done { .. } | MessageEvent::Error { .. }
            ))
            .count(),
        1,
        "{frames:?}"
    );
    assert!(
        matches!(
            frames.last(),
            Some(MessageEvent::Done { .. } | MessageEvent::Error { .. })
        ),
        "{frames:?}"
    );
}

#[tokio::test]
async fn w02_real_write_approval_positive_control_executes_after_resolution() {
    let workspace = tempfile::tempdir().expect("workspace");
    let output = workspace.path().join("approved.txt");
    let provider = MockLlm::new()
        .tool_use(
            "Write",
            serde_json::json!({
                "file_path": output, "content": "approved fixture",
            }),
        )
        .text("approved completion")
        .start()
        .await;
    let server = server(
        config(workspace.path(), false),
        workspace.path(),
        &provider.uri(),
        false,
    );
    let (id, mut stream) = start(&server).await;
    let (call, token, mut frames) = approval(&mut stream).await;
    assert!(!output.exists(), "gated Write must not execute yet");
    server
        .resolve_approval(id.clone(), call, decision(token))
        .await
        .expect("resolve emitted gate");
    frames.extend(
        timeout(DEADLINE, stream.collect::<Vec<_>>())
            .await
            .expect("completion"),
    );
    let deleted = server.delete_session(id).await;
    one_terminal(&frames);
    assert_eq!(
        std::fs::read_to_string(output).expect("approved Write effect"),
        "approved fixture"
    );
    assert!(
        frames
            .iter()
            .any(|e| matches!(e, MessageEvent::ToolResult { result } if !result.is_error)),
        "{frames:?}"
    );
    deleted.expect("control cleanup");
}

#[tokio::test]
async fn w02_delete_cancels_real_pending_write_approval_without_effect() {
    let workspace = tempfile::tempdir().expect("workspace");
    let output = workspace.path().join("must-not-write.txt");
    let provider = MockLlm::new()
        .tool_use(
            "Write",
            serde_json::json!({
                "file_path": output, "content": "forbidden after delete",
            }),
        )
        .text("must not continue")
        .start()
        .await;
    let server = server(
        config(workspace.path(), false),
        workspace.path(),
        &provider.uri(),
        false,
    );
    let (id, mut stream) = start(&server).await;
    let (call, token, mut frames) = approval(&mut stream).await;
    assert!(!output.exists());
    let deleted = timeout(Duration::from_secs(10), server.delete_session(id.clone())).await;
    let late = server.resolve_approval(id, call, decision(token)).await;
    frames.extend(
        timeout(DEADLINE, stream.collect::<Vec<_>>())
            .await
            .expect("cancel terminal"),
    );
    deleted
        .expect("delete deadline")
        .expect("close pending approval");
    assert!(late.is_err(), "deleted gate must not be resolvable");
    one_terminal(&frames);
    assert!(
        !output.exists(),
        "pending Write executed after cancellation"
    );
    assert_eq!(
        provider
            .received_requests()
            .await
            .expect("provider trace")
            .len(),
        1,
        "cancellation must not start another provider round"
    );
}

#[tokio::test]
async fn w02_delete_releases_real_durable_writer_and_preserves_history() {
    let workspace = tempfile::tempdir().expect("workspace");
    let provider = MockLlm::new()
        .text("durable lifecycle history sentinel")
        .start()
        .await;
    let config = config(workspace.path(), true);
    let sessions = std::path::PathBuf::from(&config.session.directory);
    let server = server(config, workspace.path(), &provider.uri(), true);
    let (id, stream) = start(&server).await;
    let frames = timeout(DEADLINE, stream.collect::<Vec<_>>())
        .await
        .expect("durable turn");
    one_terminal(&frames);
    assert!(frames.iter().any(|e| matches!(e, MessageEvent::TextDelta { text } if text.contains("durable lifecycle history sentinel"))), "{frames:?}");
    let path = sessions.join(format!("{id}.journal"));
    assert!(
        matches!(
            SessionJournal::open(&path, id.clone()),
            Err(JournalError::AlreadyOwned { .. })
        ),
        "positive lease control: the live engine must hold this journal"
    );
    server
        .delete_session(id.clone())
        .await
        .expect("durable close");
    let journal =
        SessionJournal::open(&path, id.clone()).expect("204 must release writer authority");
    assert!(!journal.state().expect("replay state").turns.is_empty());
    drop(journal);
    let loaded = SessionManager::new(sessions, 64)
        .load_for_run_if_exists(&id)
        .expect("authoritative reload after delete")
        .expect("history still exists");
    assert!(
        serde_json::to_string(&loaded.session)
            .expect("session snapshot")
            .contains("durable lifecycle history sentinel")
    );
}

#[cfg(unix)]
mod process_cleanup {
    use super::*;
    use wcore_agent::session_journal::ToolEffectState;
    use wcore_config::config::{McpServerConfig, TransportType};

    struct FixtureCleanup(std::path::PathBuf);

    impl Drop for FixtureCleanup {
        fn drop(&mut self) {
            for name in ["mcp-child.pid", "mcp-parent.pid"] {
                if let Ok(text) = std::fs::read_to_string(self.0.join(name))
                    && let Ok(pid) = text.parse::<i32>()
                    && pid > 1
                {
                    // SAFETY: these files are written only by our owned MCP
                    // fixture inside its private temp directory. This backstop
                    // contains a failing candidate; it is not a passing probe.
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
            }
        }
    }

    fn alive(pid: i32) -> bool {
        assert!(pid > 1);
        // SAFETY: signal 0 only probes the fixture's recorded process.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        let error = std::io::Error::last_os_error();
        assert_eq!(
            error.raw_os_error(),
            Some(libc::ESRCH),
            "cannot establish fixture process absence: {error}"
        );
        false
    }

    #[tokio::test]
    async fn w02_delete_during_real_mcp_tool_reaps_child_and_retains_unknown_effect() {
        let workspace = tempfile::tempdir().expect("workspace");
        let _cleanup = FixtureCleanup(workspace.path().to_path_buf());
        let script = workspace.path().join("mcp_child.py");
        std::fs::write(&script, include_str!("fixtures/stabilization_mcp_child.py"))
            .expect("fixture script");
        let provider = MockLlm::new()
            .tool_use("w02_hold_child", serde_json::json!({}))
            .text("must not continue the cancelled tool")
            .start()
            .await;
        let mut config = config(workspace.path(), true);
        let sessions = std::path::PathBuf::from(&config.session.directory);
        config.mcp.servers.insert(
            "lifecycle-child".into(),
            McpServerConfig {
                transport: TransportType::Stdio,
                command: Some("python3".into()),
                args: Some(vec![
                    "-u".into(),
                    script.to_string_lossy().into_owned(),
                    workspace.path().to_string_lossy().into_owned(),
                ]),
                env: None,
                url: None,
                headers: None,
                deferred: Some(false),
                allow_local: false,
                only_for_assistant: None,
                allowed_tools: None,
            },
        );
        let server = server(config, workspace.path(), &provider.uri(), true);
        let (id, stream) = start(&server).await;
        let marker = timeout(DEADLINE, async {
            loop {
                match tokio::fs::read(workspace.path().join("running.json")).await {
                    Ok(bytes) => {
                        break serde_json::from_slice::<serde_json::Value>(&bytes)
                            .expect("atomic fixture marker");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        tokio::time::sleep(Duration::from_millis(10)).await
                    }
                    Err(error) => panic!("read fixture state: {error}"),
                }
            }
        })
        .await;
        let marker = match marker {
            Ok(marker) => marker,
            Err(error) => {
                let cleanup = server.delete_session(id).await;
                let frames = timeout(DEADLINE, stream.collect::<Vec<_>>()).await;
                panic!(
                    "real tool never created its child: {error}; cleanup={cleanup:?}; frames={frames:?}"
                );
            }
        };
        let parent = marker["parent"].as_i64().expect("parent PID") as i32;
        let child = marker["child"].as_i64().expect("child PID") as i32;
        assert!(
            alive(parent) && alive(child),
            "positive control must observe both owned processes live"
        );
        let journal_path = sessions.join(format!("{id}.journal"));
        assert!(matches!(
            SessionJournal::open(&journal_path, id.clone()),
            Err(JournalError::AlreadyOwned { .. })
        ));
        let deleted = timeout(Duration::from_secs(10), server.delete_session(id.clone())).await;
        let parent_alive_at_ack = alive(parent);
        let child_alive_at_ack = alive(child);
        let frames = timeout(DEADLINE, stream.collect::<Vec<_>>())
            .await
            .expect("cancelled tool stream");
        deleted
            .expect("DELETE deadline")
            .expect("owned tool/child cleanup");
        one_terminal(&frames);
        assert!(
            !parent_alive_at_ack && !child_alive_at_ack,
            "DELETE acknowledged before process reap: parent={parent_alive_at_ack}, child={child_alive_at_ack}"
        );
        assert!(
            frames.iter().any(
                |e| matches!(e, MessageEvent::ToolCall { call } if call.name == "w02_hold_child")
            ),
            "{frames:?}"
        );
        std::fs::write(workspace.path().join("release-child"), "release").expect("release marker");
        assert!(!workspace.path().join("child-effect.txt").exists());
        let journal = SessionJournal::open(&journal_path, id)
            .expect("writer lease released after tool close");
        let state = journal.state().expect("recover durable effects");
        let tool = state
            .tools
            .values()
            .find(|tool| tool.tool == "w02_hold_child")
            .expect("durable tool intent");
        assert!(
            matches!(tool.effect, ToolEffectState::Unknown { .. }),
            "unobserved physical MCP outcome must remain unknown: {:?}",
            tool.effect
        );
    }
}
