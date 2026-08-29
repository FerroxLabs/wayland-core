//! wayland#1165 — `/mcp add --replace`: the EXPLICIT opt-in that tears a
//! connected MCP server's connection down and re-establishes it.
//!
//! wayland#605 closed the accidental case: a duplicate add of a ready server is
//! a no-op, because a retry or two hosts racing the same add must never mutate
//! a live connection. That left the deliberate case with no route at all —
//! reconfiguring a connected server was impossible, and this test grades the
//! opt-in that gives it one.
//!
//! It drives the REAL packaged binary over the json-stream protocol and counts
//! real stdio CHILD PROCESSES, because "tears the connection down" is a claim
//! about a process, not about a HashMap entry. The fixture server names its one
//! tool after its own argv, so the two configurations are distinguishable at
//! the wire and a replace that silently kept the old connection would be caught
//! by the tool name as well as by the pid.
//!
//! Unix-only: the fixture server is a POSIX shell script. The default-path
//! guard (`a_plain_re_add_still_changes_nothing`) is the wayland#605 behaviour
//! this feature must not weaken, and it is asserted here beside the opt-in so
//! the two cannot drift.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::OwnedTree;

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

/// A stdio MCP server that records its pid at launch and advertises exactly one
/// tool, named by its first argument. Two launches with different arguments are
/// therefore distinguishable both by pid and by the tool they contribute.
fn write_fixture_server(home: &Path, pid_log: &Path) -> std::path::PathBuf {
    let script = home.join("named-tool-mcp.sh");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
echo "$$" >> "{pid_log}"
tool="$1"
while IFS= read -r line; do
  id=`printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p'`
  case "$line" in
    *'"method":"initialize"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-03-26","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"named-tool","version":"1.0.0"}}}}}}\n' "$id"
      ;;
    *'"method":"notifications/initialized"'*)
      ;;
    *'"method":"tools/list"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"%s","description":"d","inputSchema":{{"type":"object","properties":{{}}}}}}]}}}}\n' "$id" "$tool"
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
    script
}

struct CoreSession {
    child: OwnedTree<std::process::Child>,
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
                "f1165-assistant",
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
        let mut child = OwnedTree::new(command.spawn().expect("spawn packaged Core"));
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
        let deadline = Instant::now() + Duration::from_secs(30);
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

    /// One `add_mcp_server`. `replace` is omitted from the payload entirely
    /// when false, so the default arm is exercised on the pre-#1165 wire.
    fn add(&mut self, script: &Path, tool: &str, replace: bool) -> Value {
        let mut payload = json!({
            "type": "add_mcp_server",
            "name": "named",
            "transport": "stdio",
            "command": script.to_str().expect("utf-8 script path"),
            "args": [tool],
            "allow_local": true,
        });
        if replace {
            payload["replace"] = json!(true);
        }
        self.send(payload);
        self.wait_for(
            |value| {
                (value["type"] == "mcp_ready" || value["type"] == "mcp_failed")
                    && value["name"] == "named"
            },
            "mcp_ready/mcp_failed for 'named'",
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

fn read_pids(path: &Path, expect_at_least: usize) -> Vec<i32> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut pids = Vec::new();
    while Instant::now() < deadline {
        pids = std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().parse::<i32>().ok())
            .collect();
        if pids.len() >= expect_at_least {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    pids
}

fn process_alive(pid: i32) -> bool {
    // signal 0 probes for existence without delivering anything.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn await_exit(pid: i32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && process_alive(pid) {
        std::thread::sleep(Duration::from_millis(100));
    }
    !process_alive(pid)
}

fn tool_names(ready: &Value) -> Vec<String> {
    ready["tools"]
        .as_array()
        .expect("mcp_ready carries a tools array")
        .iter()
        .filter_map(|t| t.as_str().map(str::to_string))
        .collect()
}

/// THE criterion (wayland#1165 c1). An explicit `replace` tears the connected
/// server's connection down — the stdio child really exits — and re-establishes
/// it from the NEW configuration, whose different tool proves it is the new
/// connection and not the old one re-announced.
#[tokio::test(flavor = "multi_thread")]
async fn replace_tears_the_connection_down_and_re_establishes_it() {
    let home = TempDir::new().expect("home");
    write_home(home.path());
    let pid_log = home.path().join("mcp-child-pids.log");
    let script = write_fixture_server(home.path(), &pid_log);

    let mut session = CoreSession::start(home.path());

    let first = session.add(&script, "alpha", false);
    assert_eq!(first["type"], "mcp_ready", "first add must connect: {first}");
    assert!(
        tool_names(&first).iter().any(|t| t.contains("alpha")),
        "CONTROL: the first connection must contribute its own tool, else the \
         tool-name evidence below proves nothing: {first}"
    );
    let after_first = read_pids(&pid_log, 1);
    assert_eq!(
        after_first.len(),
        1,
        "CONTROL: the first add must launch exactly one child: {after_first:?}"
    );

    let replaced = session.add(&script, "beta", true);
    assert_eq!(
        replaced["type"], "mcp_ready",
        "the replace must connect, not refuse: {replaced}"
    );
    assert!(
        replaced["already_connected"].is_null() || replaced["already_connected"] == json!(false),
        "a replace is a REAL reconnect, never the #605 skip annotation: {replaced}"
    );
    let names = tool_names(&replaced);
    assert!(
        names.iter().any(|t| t.contains("beta")),
        "the re-established connection must serve the NEW configuration: {names:?}"
    );
    assert!(
        !names.iter().any(|t| t.contains("alpha")),
        "the old configuration's tool must be gone after a replace: {names:?}"
    );

    let after_replace = read_pids(&pid_log, 2);
    assert_eq!(
        after_replace.len(),
        2,
        "the replace must launch a second child: {after_replace:?}"
    );
    assert!(
        await_exit(after_first[0]),
        "TEAR DOWN, not leak: the replaced child {} must exit",
        after_first[0]
    );
    assert!(
        process_alive(after_replace[1]),
        "RE-ESTABLISH, not just remove: the new child {} must be serving",
        after_replace[1]
    );

    session.stop();
    for pid in &after_replace {
        assert!(
            await_exit(*pid),
            "MCP stdio child {pid} outlived its Core session"
        );
    }
}

/// wayland#1165 c2 — the guard the opt-in must not weaken. WITHOUT `replace`, a
/// re-add of a ready server whose configuration has changed leaves the live
/// connection, its tools and its child process exactly as they were.
#[tokio::test(flavor = "multi_thread")]
async fn a_plain_re_add_still_changes_nothing() {
    let home = TempDir::new().expect("home");
    write_home(home.path());
    let pid_log = home.path().join("mcp-child-pids.log");
    let script = write_fixture_server(home.path(), &pid_log);

    let mut session = CoreSession::start(home.path());

    let first = session.add(&script, "alpha", false);
    assert_eq!(first["type"], "mcp_ready");
    let after_first = read_pids(&pid_log, 1);
    assert_eq!(after_first.len(), 1, "CONTROL: one child: {after_first:?}");

    // Same name, DIFFERENT configuration, and no opt-in.
    let second = session.add(&script, "beta", false);
    assert_eq!(
        second["type"], "mcp_failed",
        "a same-name re-add carrying a different configuration is refused, not \
         silently applied: {second}"
    );

    // And the live server is untouched: same single child, still alive.
    let after_second = read_pids(&pid_log, 1);
    assert_eq!(
        after_second, after_first,
        "a refused re-add must not spawn anything: {after_second:?}"
    );
    assert!(
        process_alive(after_first[0]),
        "the connected child must survive a re-add that was not opted into"
    );

    session.stop();
    for pid in &after_second {
        assert!(
            await_exit(*pid),
            "MCP stdio child {pid} outlived its Core session"
        );
    }
}
