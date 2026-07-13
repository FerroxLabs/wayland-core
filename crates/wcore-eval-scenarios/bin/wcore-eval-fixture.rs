//! Deterministic cross-platform process fixture for the evaluation driver.

use std::io::{BufRead, Write};
use std::time::Duration;

const SOURCE_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn main() {
    if std::env::args_os().any(|arg| arg == "--build-info") {
        println!(
            "wayland-core {} (source {SOURCE_COMMIT})",
            env!("CARGO_PKG_VERSION")
        );
        return;
    }

    let model = argument_value("--model").unwrap_or_default();
    let fail_canary = std::env::var_os("WCORE_EVAL_FIXTURE_FAIL_CANARY").is_some();

    emit(&serde_json::json!({
        "type": "ready",
        "capabilities": {"cost_attribution": true}
    }));

    let mut completed_turns = 0u32;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(command) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match command.get("type").and_then(serde_json::Value::as_str) {
            Some("message") => {
                completed_turns += 1;
                let msg_id = command
                    .get("msg_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("fixture");
                let is_canary = command
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|prompt| prompt.contains("single word READY"));
                let text = if model == "fixture-hermetic" {
                    hermetic_probe_text()
                } else if fail_canary && is_canary {
                    "WRONG".to_string()
                } else {
                    "READY".to_string()
                };
                emit(&serde_json::json!({
                    "type": "text_delta",
                    "msg_id": msg_id,
                    "text": text
                }));
                if model == "fixture-hermetic" {
                    let secret = fixture_secret();
                    eprintln!("fixture attempted stderr leak: {secret}");
                    emit(&serde_json::json!({
                        "type": "tool_result",
                        "call_id": "fixture-secret",
                        "tool_name": "Read",
                        "output": format!("fixture attempted trace leak: {secret}"),
                        "status": "success"
                    }));
                }
                if model == "fixture-slow" {
                    std::thread::sleep(Duration::from_millis(250));
                }
                if model == "fixture-steps" {
                    for step in 0..2 {
                        emit(&serde_json::json!({
                            "type": "tool_result",
                            "call_id": format!("fixture-{step}"),
                            "tool_name": "Read",
                            "output": "fixture",
                            "status": "success"
                        }));
                    }
                }
                if model != "fixture-no-cost" {
                    let total_cost_usd = if model == "fixture-cost" {
                        f64::from(completed_turns) * 0.01
                    } else {
                        0.0
                    };
                    emit(&serde_json::json!({
                        "type": "session_cost",
                        "session_id": "fixture",
                        "total_cost_usd": total_cost_usd,
                        "per_turn": []
                    }));
                }
                emit(&serde_json::json!({
                    "type": "stream_end",
                    "msg_id": msg_id,
                    "finish_reason": "stop",
                    "turns": 1,
                    "usage": {
                        "input_tokens": 1,
                        "output_tokens": 1,
                        "cache_creation_tokens": 0,
                        "cache_read_tokens": 0
                    }
                }));
            }
            Some("set_config" | "set_mode") => emit(&serde_json::json!({
                "type": "info",
                "message": "fixture: no changes"
            })),
            Some("stop") => break,
            _ => {}
        }
    }
}

fn hermetic_probe_text() -> String {
    let secret = fixture_secret();
    let args: Vec<String> = std::env::args().collect();
    let config = std::fs::read_to_string(".wayland-core/config.toml").unwrap_or_default();
    let provider_env_has_secret = [
        "API_KEY",
        "DEEPSEEK_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
    ]
    .iter()
    .any(|name| std::env::var(name).ok().as_deref() == Some(secret.as_str()));
    let poison_inherited = [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "GIT_CONFIG_GLOBAL",
        "SSH_AUTH_SOCK",
        "HTTPS_PROXY",
    ]
    .iter()
    .any(|name| std::env::var(name).ok().as_deref() == Some("wcore-poison"));
    let budget_seeded = config.contains("max_cost_usd = 0.031");
    format!(
        "READY arg_secret={} config_secret={} key_env={} poison={} budget={} leak={secret}",
        args.iter().any(|arg| arg.contains(&secret)),
        config.contains(&secret),
        provider_env_has_secret,
        poison_inherited,
        budget_seeded,
    )
}

fn fixture_secret() -> String {
    for name in [
        "API_KEY",
        "DEEPSEEK_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
    ] {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            return value;
        }
    }
    argument_value("--api-key").unwrap_or_default()
}

fn argument_value(name: &str) -> Option<String> {
    let mut args = std::env::args();
    while let Some(argument) = args.next() {
        if argument == name {
            return args.next();
        }
    }
    None
}

fn emit(value: &serde_json::Value) {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, value).expect("serialize fixture event");
    stdout.write_all(b"\n").expect("write fixture event");
    stdout.flush().expect("flush fixture event");
}
