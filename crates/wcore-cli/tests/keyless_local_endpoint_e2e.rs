//! #1173 — the real binary, pointed at a keyless local endpoint, must reach
//! first dispatch.
//!
//! The reproduction from the issue, verbatim except for the port:
//!
//! ```text
//! wayland-core --provider openai --model qwen3:8b \
//!              --base-url http://127.0.0.1:11434/v1 "…"
//! ```
//!
//! Before the fix this exited non-zero with `Error: No API key found` — the
//! startup credential gate returned before the OpenAI provider's own
//! `SELF_HOSTED_PLACEHOLDER_KEY` path could ever run.
//!
//! Asserting only "it starts" would be too weak: it would also pass on a build
//! that started and then sent an EMPTY bearer, or a real credential harvested
//! from somewhere else. So the mock records the `Authorization` header the
//! binary actually put on the wire and the test pins it to the placeholder.
//!
//! [`a_remote_endpoint_without_a_key_is_still_refused`] is the paired negative
//! control and passes in BOTH arms: it is what proves the positive test is
//! measuring an exemption for self-hosted endpoints and not the removal of the
//! credential requirement.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

/// The bearer `OpenAIProvider::select_key` sends when no key is configured and
/// the endpoint is self-hosted (`SELF_HOSTED_PLACEHOLDER_KEY`, openai.rs).
const PLACEHOLDER_BEARER: &str = "Bearer wayland-local";

/// One scripted OpenAI chat-completions SSE turn.
struct ChatCompletion(&'static str);

impl Respond for ChatCompletion {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        let chunk = |delta: Value, finish: Value| {
            json!({
                "id": "keyless-local",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "qwen3:8b",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]
            })
        };
        let role = chunk(json!({"role": "assistant"}), Value::Null);
        let text = chunk(json!({"content": self.0}), Value::Null);
        let stop = json!({
            "id": "keyless-local",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "qwen3:8b",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10}
        });
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(format!(
                "data: {role}\n\ndata: {text}\n\ndata: {stop}\n\ndata: [DONE]\n\n"
            ))
    }
}

/// Start the OpenAI-wire mock on loopback, holding its runtime alive for the
/// caller's scope (the spawned binary POSTs to it over real loopback).
fn start_mock(reply: &'static str) -> (tokio::runtime::Runtime, MockServer) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ChatCompletion(reply))
            .mount(&server)
            .await;
        server
    });
    (rt, server)
}

/// Every `Authorization` header value the mock received, in order.
fn recorded_authorization(rt: &tokio::runtime::Runtime, server: &MockServer) -> Vec<String> {
    rt.block_on(async {
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|req| {
                req.headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("<absent>")
                    .to_string()
            })
            .collect()
    })
}

struct Capture {
    stdout: String,
    stderr: String,
    status: Option<i32>,
}

fn case_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wl-1173-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create case dir");
    dir
}

/// Run the binary headless with a trailing positional prompt against a
/// hermetic home. `WAYLAND_HOME` isolates the profile; `HOME`/`USERPROFILE` are
/// redirected too because `HOME` alone does not isolate config on Windows. Every
/// credential the key chain reads is stripped from the child env — the whole
/// point is that this run has no credential anywhere.
fn run_headless(dir: &Path, base_url: &str) -> Capture {
    let home = dir.join("home");
    let fake_home = dir.join("fakehome");
    let proj = dir.join("proj");
    for d in [&home, &fake_home, &proj] {
        std::fs::create_dir_all(d).expect("create dir");
    }
    // Only the storage posture is seeded, so the run can neither touch a real
    // keyring nor need one: `plaintext` cannot hold the confidential key that
    // durable session recovery requires, so sessions are turned off with it.
    // Provider, model and endpoint come from the CLI, exactly as the issue's
    // reproduction types them.
    std::fs::write(
        home.join("config.toml"),
        "[storage.credentials]\nbackend = \"plaintext\"\n\n[session]\nenabled = false\n",
    )
    .expect("write config.toml");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wayland-core"));
    cmd.args([
        "--no-tui",
        "--provider",
        "openai",
        "--model",
        "qwen3:8b",
        "--base-url",
        base_url,
        "--project-dir",
    ])
    .arg(&proj)
    .arg("say")
    .arg("hello")
    .current_dir(&proj)
    .env("WAYLAND_HOME", &home)
    .env("HOME", &fake_home)
    .env("USERPROFILE", &fake_home)
    .env("TERM", "dumb")
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    for key in [
        "API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "OPENROUTER_API_KEY",
        "DEEPSEEK_API_KEY",
        "GROQ_API_KEY",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AWS_PROFILE",
        "WAYLAND_VAULT_PASSPHRASE",
        "WAYLAND_VAULT_PASSPHRASE_FD",
    ] {
        cmd.env_remove(key);
    }

    let out = cmd
        .spawn()
        .expect("spawn wayland-core")
        .wait_with_output()
        .expect("wait for wayland-core");
    Capture {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        status: out.status.code(),
    }
}

/// THE DEFECT, end to end: the binary must start against a keyless local
/// endpoint AND reach first dispatch carrying the placeholder bearer.
#[test]
fn keyless_local_endpoint_starts_and_dispatches_with_the_placeholder_bearer() {
    let (rt, server) = start_mock("ok from the local model");
    let dir = case_dir("local");
    // The base URL is the server ROOT: `openai_defaults()` appends the
    // `/v1/chat/completions` api_path itself, which is where a stock Ollama
    // serves its OpenAI-compatible surface (`http://127.0.0.1:11434`).
    let cap = run_headless(&dir, &server.uri());

    assert!(
        !cap.stderr.contains("No API key found"),
        "a keyless self-hosted endpoint must not be refused for a missing \
         credential. rc={:?}\nstderr:\n{}\nstdout:\n{}",
        cap.status,
        cap.stderr,
        cap.stdout
    );
    assert_eq!(
        cap.status,
        Some(0),
        "the run must complete. stderr:\n{}\nstdout:\n{}",
        cap.stderr,
        cap.stdout
    );

    let seen = recorded_authorization(&rt, &server);
    assert!(
        !seen.is_empty(),
        "the engine must have reached first dispatch against the local \
         endpoint; the mock received nothing. stdout:\n{}\nstderr:\n{}",
        cap.stdout,
        cap.stderr
    );
    assert!(
        seen.iter().all(|v| v == PLACEHOLDER_BEARER),
        "every dispatch must carry the self-hosted placeholder bearer -- not an \
         empty credential and not one harvested elsewhere. Got: {seen:?}"
    );
    assert!(
        cap.stdout.contains("ok from the local model"),
        "the model's answer must reach the user. stdout:\n{}\nstderr:\n{}",
        cap.stdout,
        cap.stderr
    );
}

/// NEGATIVE CONTROL — passes in BOTH arms. A public endpoint with no
/// credential anywhere is still refused at startup, so the exemption cannot be
/// read as "the credential requirement was dropped". Nothing is dispatched, so
/// no request is made to the public host.
#[test]
fn a_remote_endpoint_without_a_key_is_still_refused() {
    let dir = case_dir("remote");
    let cap = run_headless(&dir, "https://api.openai.com");

    assert_ne!(
        cap.status,
        Some(0),
        "a remote endpoint with no credential must refuse to start. stdout:\n{}",
        cap.stdout
    );
    assert!(
        cap.stderr.contains("No API key found"),
        "the refusal must still name the missing credential. stderr:\n{}",
        cap.stderr
    );
}
