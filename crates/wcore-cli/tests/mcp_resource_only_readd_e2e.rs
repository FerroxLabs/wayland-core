//! wayland#605 gap 1 — a RESOURCE-ONLY MCP server (zero tools) must be
//! detected as already-connected on re-add, so `/mcp add` never dials it a
//! second time (and, for stdio, never spawns a duplicate child).
//!
//! The #135 probe (`AgentEngine::mcp_server_connected`) keys on tool
//! provenance, which a zero-tool server can never satisfy. This test drives
//! the REAL packaged binary over the json-stream protocol and counts dials at
//! the wire, so it grades the wiring, not the helper.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{body_string_contains, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

fn write_home(home: &Path) {
    std::fs::write(
        home.join("config.toml"),
        "[default]\nprovider = \"anthropic\"\nmodel = \"fixture\"\n\
         [providers.anthropic]\napi_key = \"fixture-only\"\n\
         base_url = \"http://127.0.0.1:9/unused\"\n",
    )
    .expect("write config");
}

/// A streamable-http MCP server that advertises ONLY resources.
async fn resource_only_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("\"method\":\"initialize\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {"resources": {}},
                "serverInfo": {"name": "resource-only", "version": "1.0.0"}
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains(
            "\"method\":\"notifications/initialized\"",
        ))
        .respond_with(ResponseTemplate::new(202))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("\"method\":\"resources/list\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": 10,
            "result": {"resources": []}
        })))
        .mount(&server)
        .await;
    server
}

struct CoreSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    lines: std::sync::mpsc::Receiver<String>,
}

impl CoreSession {
    fn start(home: &Path) -> Self {
        let mut command = std::process::Command::new(binary());
        command
            .args([
                "--json-stream",
                "--provider",
                "anthropic",
                "--assistant",
                "f605-assistant",
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

    fn send(&mut self, value: Value) {
        writeln!(self.stdin, "{value}").expect("write host command");
        self.stdin.flush().expect("flush host command");
    }

    fn wait_for(&mut self, predicate: impl Fn(&Value) -> bool, label: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(line) = self.lines.recv_timeout(Duration::from_millis(200))
                && let Ok(value) = serde_json::from_str::<Value>(&line)
                && predicate(&value)
            {
                return value;
            }
        }
        panic!("timed out waiting for {label}");
    }

    fn add(&mut self, url: &str) -> Value {
        self.send(json!({
            "type": "add_mcp_server",
            "name": "resource-only",
            "transport": "streamable-http",
            "url": url,
            "allow_local": true,
        }));
        self.wait_for(
            |value| value["type"] == "mcp_ready" && value["name"] == "resource-only",
            "mcp_ready",
        )
    }

    fn stop(mut self) {
        self.send(json!({"type":"stop"}));
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

async fn methods(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("request journal")
        .into_iter()
        .filter_map(|request| serde_json::from_slice::<Value>(&request.body).ok())
        .filter_map(|body| body["method"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn resource_only_server_is_not_redialled_on_re_add() {
    let server = resource_only_server().await;
    let home = TempDir::new().expect("home");
    write_home(home.path());

    let mut session = CoreSession::start(home.path());
    let first = session.add(&server.uri());
    // CONTROL: this really is the resource-only path. Zero tools means the
    // #135 tool-provenance probe cannot possibly detect it, so a pass here is
    // attributable to the name-keyed lifecycle catalog and nothing else.
    assert_eq!(
        first["tools"].as_array().map(Vec::len),
        Some(0),
        "fixture must expose zero tools, else this test grades the #135 path: {first}"
    );

    let second = session.add(&server.uri());
    assert_eq!(second["tools"].as_array().map(Vec::len), Some(0));
    session.stop();

    let seen = methods(&server).await;
    let initializes = seen.iter().filter(|m| *m == "initialize").count();
    assert_eq!(
        initializes, 1,
        "re-adding a live resource-only server must not dial it again; wire methods = {seen:?}"
    );
    assert!(
        !seen.iter().any(|m| m == "tools/list"),
        "a resources-only server must never be asked for tools: {seen:?}"
    );
}

/// The literal wording of wayland#605 gap 1 is "duplicate stdio **child**".
/// The HTTP arm above counts dials; this one counts real processes. A
/// resource-only stdio server records its own pid on every launch, so a
/// second launch is directly observable — and the test also proves the child
/// is reaped, so a duplicate can never be hidden by a leak.
///
/// Unix-only: the fixture server is a POSIX shell script. The HTTP arm above
/// carries the same property on every platform.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn resource_only_stdio_server_spawns_exactly_one_child_across_re_add() {
    let home = TempDir::new().expect("home");
    write_home(home.path());
    let pid_log = home.path().join("mcp-child-pids.log");
    let script = home.path().join("resource-only-mcp.sh");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
echo "$$" >> "{pid_log}"
while IFS= read -r line; do
  id=`printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p'`
  case "$line" in
    *'"method":"initialize"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-03-26","capabilities":{{"resources":{{}}}},"serverInfo":{{"name":"resource-only","version":"1.0.0"}}}}}}\n' "$id"
      ;;
    *'"method":"notifications/initialized"'*)
      ;;
    *'"method":"resources/list"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"resources":[]}}}}\n' "$id"
      ;;
    *)
      if [ -n "$id" ]; then
        printf '{{"jsonrpc":"2.0","id":%s,"error":{{"code":-32601,"message":"method not found"}}}}\n' "$id"
      fi
      ;;
  esac
done
"#,
            pid_log = pid_log.display()
        ),
    )
    .expect("write fixture server");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fixture server");

    let mut session = CoreSession::start(home.path());
    let add = |session: &mut CoreSession| -> Value {
        session.send(json!({
            "type": "add_mcp_server",
            "name": "resource-only-stdio",
            "transport": "stdio",
            "command": script.to_str().expect("utf-8 script path"),
            "allow_local": true,
        }));
        session.wait_for(
            |value| value["type"] == "mcp_ready" && value["name"] == "resource-only-stdio",
            "mcp_ready (stdio)",
        )
    };

    let first = add(&mut session);
    assert_eq!(
        first["tools"].as_array().map(Vec::len),
        Some(0),
        "CONTROL: the fixture must expose zero tools, else this grades the #135 \
         tool-provenance path instead of #605 gap 1: {first}"
    );
    let pids_after_first = read_pids(&pid_log);
    assert_eq!(
        pids_after_first.len(),
        1,
        "CONTROL: the first add must actually launch one child, else a zero \
         count below would prove nothing: {pids_after_first:?}"
    );

    add(&mut session);
    let pids_after_second = read_pids(&pid_log);
    assert_eq!(
        pids_after_second, pids_after_first,
        "re-adding a live resource-only stdio server must not spawn a second child"
    );

    session.stop();
    for pid in &pids_after_second {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && process_alive(*pid) {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            !process_alive(*pid),
            "MCP stdio child {pid} outlived its Core session"
        );
    }
}

#[cfg(unix)]
fn read_pids(path: &Path) -> Vec<i32> {
    // The child writes its pid at launch; give a just-completed handshake a
    // moment to flush rather than racing the filesystem.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut pids = Vec::new();
    while Instant::now() < deadline {
        pids = std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().parse::<i32>().ok())
            .collect();
        if !pids.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    pids
}

#[cfg(unix)]
fn process_alive(pid: i32) -> bool {
    // signal 0 probes for existence without delivering anything.
    unsafe { libc::kill(pid, 0) == 0 }
}
