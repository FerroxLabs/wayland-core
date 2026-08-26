//! FerroxLabs/wayland#605 — an `mcp_ready` that announces a SKIP must not look
//! like one that announces a connection.
//!
//! `add_mcp_server` for a name already held at `Ready` dials nothing: the
//! reservation comes back `Existing`, the existing lifecycle generation is
//! kept, and Core restates the current tool set. That restatement used to be
//! byte-identical to the frame a real connect produces, so a host had no way to
//! tell "your server is up, nothing changed" from "your server just came back".
//!
//! This drives the REAL binary over the real json-stream loop, because the skip
//! branch lives inside the host command loop and no unit seam reaches it. The
//! fixture's own `initialize` count is the positive control: without it,
//! "already_connected" could be a label on a frame that really did reconnect.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use wcore_eval_scenarios::fixtures::mcp::{McpHttpFixture, McpHttpMode};

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
                "skip-probe-assistant",
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

    fn add(&mut self, url: &str) -> serde_json::Value {
        self.send(serde_json::json!({
            "type": "add_mcp_server",
            "name": "skip-probe",
            "transport": "streamable-http",
            "url": url,
            "allow_local": true,
        }));
        self.wait_for(
            |value| value["type"] == "mcp_ready" && value["name"] == "skip-probe",
            "mcp_ready",
        )
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
async fn a_repeat_add_reports_a_skip_not_a_reconnect() {
    let fixture = McpHttpFixture::start(McpHttpMode::DirectJson)
        .await
        .expect("fixture");
    let home = TempDir::new().expect("home");
    write_home(home.path());

    let mut session = CoreSession::start(home.path());
    let first = session.add(fixture.url());
    let second = session.add(fixture.url());
    session.stop();

    assert_eq!(
        first["outcome"], "connected",
        "the first add really dialed the server: {first}"
    );
    assert_eq!(
        second["outcome"], "already_connected",
        "the repeat add dialed nothing and must say so: {second}"
    );
    // Without the annotation these two frames are byte-identical. That is the
    // defect, so assert it directly rather than inferring it.
    assert_ne!(first, second, "a skip must not be indistinguishable");
    assert_eq!(
        first["tools"], second["tools"],
        "the skip restates the SAME tool set: {first} vs {second}"
    );

    // The positive control. A label is only worth what the transport did: if
    // the second add had really reconnected, the fixture would have seen a
    // second `initialize` and `already_connected` would be a lie.
    let observation = fixture.shutdown().await.expect("shutdown fixture");
    assert_eq!(
        observation
            .methods()
            .iter()
            .filter(|method| **method == "initialize")
            .count(),
        1,
        "exactly one real connect happened: {observation:?}"
    );
    assert!(observation.violations.is_empty(), "{observation:?}");
}
