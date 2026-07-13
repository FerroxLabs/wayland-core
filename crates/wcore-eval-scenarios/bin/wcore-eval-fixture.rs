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

    emit(&serde_json::json!({
        "type": "ready",
        "capabilities": {"cost_attribution": true}
    }));

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(command) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match command.get("type").and_then(serde_json::Value::as_str) {
            Some("message") => {
                let msg_id = command
                    .get("msg_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("fixture");
                emit(&serde_json::json!({
                    "type": "text_delta",
                    "msg_id": msg_id,
                    "text": "READY"
                }));
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
                emit(&serde_json::json!({
                    "type": "session_cost",
                    "session_id": "fixture",
                    "total_cost_usd": 0.0,
                    "per_turn": []
                }));
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
