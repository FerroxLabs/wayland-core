//! Packaged acceptance for the host-owned runtime MCP lifecycle.
//!
//! Two real Core processes use the same server name against different local
//! fixtures. Each removes, repeats removal idempotently, re-adds, and restarts.
//! Profile OAuth sentinels are byte-checked throughout: MCP bearer/header
//! lifecycle authority must never mutate provider OAuth state.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use wcore_eval_scenarios::fixtures::mcp::{McpHttpFixture, McpHttpMode};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

fn write_home(home: &Path, oauth_sentinel: &[u8]) {
    std::fs::create_dir_all(home.join("oauth")).expect("create oauth dir");
    std::fs::write(home.join("oauth/chatgpt.json"), oauth_sentinel).expect("write OAuth sentinel");
    std::fs::write(
        home.join("config.toml"),
        "[default]\nprovider = \"anthropic\"\nmodel = \"fixture\"\n\
         [providers.anthropic]\napi_key = \"fixture-only\"\n\
         base_url = \"http://127.0.0.1:9/unused\"\n",
    )
    .expect("write config");
}

struct CoreSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    lines: std::sync::mpsc::Receiver<String>,
}

impl CoreSession {
    fn start(home: &Path, assistant: &str) -> Self {
        let mut command = std::process::Command::new(binary());
        command
            .args([
                "--json-stream",
                "--provider",
                "anthropic",
                "--assistant",
                assistant,
            ])
            .current_dir(home)
            .env("WAYLAND_HOME", home)
            .env("HOME", home)
            .env("TERM", "dumb")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .env_remove("GEMINI_API_KEY")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().expect("spawn packaged Core");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut session = Self {
            child,
            stdin,
            lines,
        };
        session.wait_for(|value| value["type"] == "ready", "ready");
        session
    }

    fn send(&mut self, value: serde_json::Value) {
        writeln!(self.stdin, "{value}").expect("write host command");
        self.stdin.flush().expect("flush host command");
    }

    fn wait_for(
        &mut self,
        predicate: impl Fn(&serde_json::Value) -> bool,
        label: &str,
    ) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(line) = self.lines.recv_timeout(Duration::from_millis(200))
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(&line)
                && predicate(&value)
            {
                return value;
            }
        }
        panic!("timed out waiting for {label}");
    }

    fn add(&mut self, url: &str) {
        self.send(serde_json::json!({
            "type": "add_mcp_server",
            "name": "shared-name",
            "transport": "streamable-http",
            "url": url,
            "allow_local": true,
        }));
        self.wait_for(
            |value| value["type"] == "mcp_ready" && value["name"] == "shared-name",
            "mcp_ready",
        );
    }

    fn add_conflict(&mut self, url: &str) {
        self.send(serde_json::json!({
            "type": "add_mcp_server",
            "name": "shared-name",
            "transport": "streamable-http",
            "url": url,
            "allow_local": true,
        }));
        self.wait_for(
            |value| {
                value["type"] == "mcp_failed"
                    && value["name"] == "shared-name"
                    && value["reason"]
                        .as_str()
                        .is_some_and(|reason| reason.contains("different configuration"))
            },
            "mcp_failed configuration conflict",
        );
    }

    fn remove_named(&mut self, request_id: &str, name: &str, expected: &str) -> serde_json::Value {
        self.send(serde_json::json!({
            "type": "remove_mcp_server",
            "lifecycle_version": 1,
            "request_id": request_id,
            "name": name,
        }));
        self.wait_for(
            |value| {
                value["type"] == "mcp_removal_result"
                    && value["request_id"] == request_id
                    && value["outcome"] == expected
            },
            "mcp_removal_result",
        )
    }

    fn remove(&mut self, request_id: &str, expected: &str) -> serde_json::Value {
        self.remove_named(request_id, "shared-name", expected)
    }

    fn stop(mut self) {
        self.send(serde_json::json!({"type":"stop"}));
        drop(self.stdin);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.child.try_wait().expect("poll child").is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn parallel_sessions_remove_readd_restart_and_preserve_oauth_state() {
    let fixture_a = McpHttpFixture::start(McpHttpMode::DirectJson)
        .await
        .expect("fixture A");
    let fixture_b = McpHttpFixture::start(McpHttpMode::DirectJson)
        .await
        .expect("fixture B");
    let home_a = TempDir::new().expect("home A");
    let home_b = TempDir::new().expect("home B");
    let oauth_a = br#"{"access_token":"profile-a-sentinel"}"#;
    let oauth_b = br#"{"access_token":"profile-b-sentinel"}"#;
    write_home(home_a.path(), oauth_a);
    write_home(home_b.path(), oauth_b);

    let mut session_a = CoreSession::start(home_a.path(), "profile-a-assistant");
    let mut session_b = CoreSession::start(home_b.path(), "profile-b-assistant");
    session_a.add(fixture_a.url());
    session_b.add(fixture_b.url());
    // Same config is an idempotent replay; a different config is a typed
    // conflict and must not dial or claim the proposed config ready.
    session_a.add(fixture_a.url());
    session_a.add_conflict(fixture_b.url());

    let removed_a = session_a.remove("a-remove-1", "removed");
    let removed_b = session_b.remove("b-remove-1", "removed");
    assert_eq!(removed_a["name"], "shared-name");
    assert_eq!(removed_b["name"], "shared-name");
    let replayed_a = session_a.remove("a-remove-1", "removed");
    assert_eq!(replayed_a, removed_a);
    session_a.remove_named("a-remove-1", "another-name", "request_id_conflict");
    session_a.remove("a-remove-2", "already_absent");
    session_b.remove("b-remove-2", "already_absent");

    session_a.add(fixture_a.url());
    session_b.add(fixture_b.url());
    session_a.stop();
    session_b.stop();

    assert_eq!(
        std::fs::read(home_a.path().join("oauth/chatgpt.json")).unwrap(),
        oauth_a
    );
    assert_eq!(
        std::fs::read(home_b.path().join("oauth/chatgpt.json")).unwrap(),
        oauth_b
    );

    let mut restarted_a = CoreSession::start(home_a.path(), "profile-a-assistant");
    let mut restarted_b = CoreSession::start(home_b.path(), "profile-b-assistant");
    restarted_a.remove("a-after-restart", "already_absent");
    restarted_b.remove("b-after-restart", "already_absent");
    restarted_a.stop();
    restarted_b.stop();

    let observation_a = fixture_a.shutdown().await.expect("shutdown fixture A");
    let observation_b = fixture_b.shutdown().await.expect("shutdown fixture B");
    for observation in [observation_a, observation_b] {
        let methods = observation.methods();
        assert_eq!(
            methods
                .iter()
                .filter(|method| **method == "initialize")
                .count(),
            2
        );
        assert_eq!(
            methods
                .iter()
                .filter(|method| **method == "tools/list")
                .count(),
            2
        );
        assert!(observation.violations.is_empty(), "{observation:?}");
    }
    assert_eq!(
        std::fs::read(home_a.path().join("oauth/chatgpt.json")).unwrap(),
        oauth_a
    );
    assert_eq!(
        std::fs::read(home_b.path().join("oauth/chatgpt.json")).unwrap(),
        oauth_b
    );
}
