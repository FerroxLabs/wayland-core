//! Deterministic cross-platform process fixture for the evaluation driver.

use std::io::{BufRead, Write};
use std::time::Duration;

use sha2::{Digest, Sha256};
use wcore_protocol::events::{
    CapabilityActivation, CapabilityId, CapabilityReasonCode, ProtocolEvent,
};

const SOURCE_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn main() {
    if std::env::args_os().any(|arg| arg == "--mcp-stdio") {
        run_mcp_stdio();
        return;
    }

    if let Some(control_path) = argument_value("--orphan-listener") {
        run_orphan_listener(std::path::Path::new(&control_path));
    }

    #[cfg(target_os = "linux")]
    if let Some(target) = argument_value("--cgroup-migration-target") {
        let control = argument_value("--cgroup-migration-control")
            .expect("cgroup migration fixture requires a control path");
        run_cgroup_migration_listener(
            std::path::Path::new(&target),
            std::path::Path::new(&control),
        );
    }

    if std::env::args_os().any(|arg| arg == "--build-info") {
        println!(
            "wayland-core {} (source {SOURCE_COMMIT})",
            env!("CARGO_PKG_VERSION")
        );
        return;
    }

    let model = argument_value("--model").unwrap_or_default();
    let secret = load_fixture_secret();
    let fail_canary = std::env::var_os("WCORE_EVAL_FIXTURE_FAIL_CANARY").is_some();

    emit(&serde_json::json!({
        "type": "ready",
        "capabilities": {"cost_attribution": true}
    }));
    emit_capability_startup(&model);

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
                #[cfg(unix)]
                if let Some(control_path) = model.strip_prefix("fixture-orphan:") {
                    spawn_detached_orphan(std::path::Path::new(control_path));
                }
                if let Some(control_path) = model.strip_prefix("fixture-owned-orphan-exit:") {
                    spawn_owned_orphan(std::path::Path::new(control_path));
                    std::process::exit(0);
                }
                if let Some(control_path) = model.strip_prefix("fixture-owned-orphan-timeout:") {
                    spawn_owned_orphan(std::path::Path::new(control_path));
                    loop {
                        std::thread::sleep(Duration::from_secs(60));
                    }
                }
                if let Some(control_path) = model.strip_prefix("fixture-owned-orphan-cancel:") {
                    spawn_owned_orphan(std::path::Path::new(control_path));
                }
                if let Some(control_path) = model.strip_prefix("fixture-owned-orphan:") {
                    spawn_owned_orphan(std::path::Path::new(control_path));
                }
                #[cfg(target_os = "linux")]
                if let Some(spec) = model.strip_prefix("fixture-cgroup-migration:") {
                    spawn_cgroup_migration_descendants(spec);
                }
                if model == "fixture-oversized-stdout" {
                    let mut stdout = std::io::stdout().lock();
                    stdout
                        .write_all(&vec![b'x'; 256 * 1024])
                        .expect("write oversized unterminated stdout event");
                    stdout.flush().expect("flush oversized stdout event");
                    continue;
                }
                let is_canary = command
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|prompt| prompt.contains("single word READY"));
                let text = if model == "fixture-hermetic" {
                    hermetic_probe_text(&secret)
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
                if model.starts_with("fixture-owned-orphan-cancel:") {
                    // Keep the turn open so the driver must observe activity,
                    // send `stop`, and take the real mid-turn cancellation path.
                    continue;
                }
                if model == "fixture-hermetic" {
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
                            "type": "tool_request",
                            "call_id": format!("fixture-{step}"),
                            "tool": {
                                "name": "Read",
                                "args": {"path": format!("fixture-{step}.txt")}
                            }
                        }));
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
            Some("stop") => {
                if let Some(control_path) = model.strip_prefix("fixture-owned-orphan-cancel:") {
                    let marker = std::path::Path::new(control_path).with_extension("stop-observed");
                    std::fs::write(marker, b"stop observed\n")
                        .expect("publish cancellation observation");
                }
                break;
            }
            _ => {}
        }
    }
}

fn emit_capability_startup(model: &str) {
    let capabilities = [
        (
            CapabilityId::PricingRefresher,
            CapabilityReasonCode::NoProductionConstructor,
        ),
        (
            CapabilityId::MidFlightMonitor,
            CapabilityReasonCode::RuntimePathUnwired,
        ),
        (
            CapabilityId::CooldownTracker,
            CapabilityReasonCode::NoProductionConstructor,
        ),
        (
            CapabilityId::LearnedPolicy,
            CapabilityReasonCode::RuntimePathUnwired,
        ),
        (
            CapabilityId::SmartHandoff,
            CapabilityReasonCode::DisabledByConfig,
        ),
        (
            CapabilityId::DelegateIsolation,
            CapabilityReasonCode::IsolationNotEnforced,
        ),
        (
            CapabilityId::ProcedureSkillDrafting,
            CapabilityReasonCode::DisabledByConfig,
        ),
        (
            CapabilityId::LegacyAutoSkillDrafting,
            CapabilityReasonCode::DisabledByConfig,
        ),
    ];

    for (capability, reason) in capabilities {
        if model == "fixture-missing-capability" && capability == CapabilityId::DelegateIsolation {
            continue;
        }
        if model == "fixture-unconstructed-capability" && capability == CapabilityId::SmartHandoff {
            for activation in [
                CapabilityActivation::stage(
                    capability,
                    wcore_protocol::events::CapabilityStage::Declared,
                ),
                CapabilityActivation::stage(
                    capability,
                    wcore_protocol::events::CapabilityStage::Configured,
                ),
                CapabilityActivation::stage(
                    capability,
                    wcore_protocol::events::CapabilityStage::Ready,
                ),
            ] {
                emit(
                    &serde_json::to_value(ProtocolEvent::CapabilityActivation { activation })
                        .expect("serialize capability fixture event"),
                );
            }
            continue;
        }
        for activation in [
            CapabilityActivation::stage(
                capability,
                wcore_protocol::events::CapabilityStage::Declared,
            ),
            CapabilityActivation::unavailable(capability, reason),
        ] {
            emit(
                &serde_json::to_value(ProtocolEvent::CapabilityActivation { activation })
                    .expect("serialize capability fixture event"),
            );
        }
    }
}

fn run_mcp_stdio() {
    let journal_path = argument_value("--mcp-journal").expect("--mcp-journal is required");
    let mut journal = std::fs::File::create(journal_path).expect("create MCP fixture journal");
    let mut sequence = 0_u64;

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(method) = request.get("method").and_then(serde_json::Value::as_str) else {
            continue;
        };
        sequence = sequence.saturating_add(1);
        serde_json::to_writer(
            &mut journal,
            &serde_json::json!({
                "sequence": sequence,
                "method": method,
                "body_sha256": format!("{:x}", Sha256::digest(line.as_bytes()))
            }),
        )
        .expect("write MCP fixture journal row");
        journal.write_all(b"\n").expect("finish MCP journal row");
        journal.flush().expect("flush MCP fixture journal");

        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        emit(&mcp_response(id, method, &request));
    }
}

fn mcp_response(
    id: serde_json::Value,
    method: &str,
    request: &serde_json::Value,
) -> serde_json::Value {
    let result = match method {
        "initialize" => serde_json::json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "wcore-eval-fixture", "version": "1"}
        }),
        "tools/list" => serde_json::json!({
            "tools": [{
                "name": "fixture_echo",
                "description": "Return the supplied text",
                "inputSchema": {
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"]
                }
            }]
        }),
        "tools/call" => {
            let text = request
                .pointer("/params/arguments/text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            serde_json::json!({
                "content": [{"type": "text", "text": text}],
                "isError": false
            })
        }
        _ => {
            return serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "method not found"}
            });
        }
    };
    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
}

#[cfg(unix)]
#[allow(clippy::zombie_processes)] // Intentional: exercises reparented descendant containment.
fn spawn_detached_orphan(control_path: &std::path::Path) {
    use std::os::unix::process::CommandExt;

    let executable = fixture_executable();
    let mut command = std::process::Command::new(executable);
    command
        .arg("--orphan-listener")
        .arg(control_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command.process_group(0);
    command.spawn().expect("spawn detached orphan fixture");

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while !control_path.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(control_path.exists(), "orphan fixture did not become ready");

    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[allow(clippy::zombie_processes)] // The evaluator must reap this inherited descendant tree.
fn spawn_owned_orphan(control_path: &std::path::Path) {
    let executable = fixture_executable();
    let mut command = std::process::Command::new(executable);
    command
        .arg("--orphan-listener")
        .arg(control_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command.spawn().expect("spawn owned orphan fixture");

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while !control_path.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        control_path.exists(),
        "owned orphan fixture did not become ready"
    );
}

fn run_orphan_listener(control_path: &std::path::Path) -> ! {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("bind orphan fixture loopback listener");
    let port = listener.local_addr().expect("read listener address").port();
    let pid = std::process::id();
    let mut heartbeat = 0u64;

    loop {
        let state = format!("pid={pid}\nport={port}\nheartbeat={heartbeat}\n");
        std::fs::write(control_path, state).expect("write orphan fixture control state");
        heartbeat = heartbeat.saturating_add(1);
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn fixture_executable() -> std::path::PathBuf {
    std::env::var_os("WCORE_EVAL_PINNED_EXECUTABLE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().expect("resolve fixture executable"))
}

#[cfg(target_os = "linux")]
#[allow(clippy::zombie_processes)] // The evaluator owns and reaps both hostile descendants.
fn spawn_cgroup_migration_descendants(spec: &str) {
    let (sibling, control_dir) = spec
        .split_once('|')
        .expect("cgroup migration fixture requires sibling|control-dir");
    let sibling = std::path::Path::new(sibling);
    let parent = sibling.parent().expect("sibling cgroup must have a parent");
    let executable = fixture_executable();

    for (name, target) in [("parent", parent), ("sibling", sibling)] {
        let control = std::path::Path::new(control_dir).join(format!("{name}-state"));
        std::process::Command::new(&executable)
            .arg("--cgroup-migration-target")
            .arg(target)
            .arg("--cgroup-migration-control")
            .arg(control)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn cgroup migration fixture");
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while std::time::Instant::now() < deadline {
        let parent_ready = std::path::Path::new(control_dir)
            .join("parent-state")
            .exists();
        let sibling_ready = std::path::Path::new(control_dir)
            .join("sibling-state")
            .exists();
        if parent_ready && sibling_ready {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("cgroup migration fixtures did not become ready");
}

#[cfg(target_os = "linux")]
fn run_cgroup_migration_listener(target: &std::path::Path, control: &std::path::Path) -> ! {
    let migration = match std::fs::write(target.join("cgroup.procs"), b"0") {
        Ok(()) => "allowed".to_string(),
        Err(error) => format!("denied:{:?}", error.kind()),
    };
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("bind migration fixture loopback listener");
    let port = listener.local_addr().expect("read listener address").port();
    let pid = std::process::id();
    let mut heartbeat = 0u64;

    loop {
        let state =
            format!("pid={pid}\nport={port}\nheartbeat={heartbeat}\nmigration={migration}\n");
        std::fs::write(control, state).expect("write migration fixture control state");
        heartbeat = heartbeat.saturating_add(1);
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn hermetic_probe_text(secret: &str) -> String {
    let args: Vec<String> = std::env::args().collect();
    let config = std::fs::read_to_string(".wayland-core/config.toml").unwrap_or_default();
    let provider_env_has_secret = [
        "API_KEY",
        "DEEPSEEK_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
    ]
    .iter()
    .any(|name| std::env::var(name).ok().as_deref() == Some(secret));
    let poison_inherited = [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "GIT_CONFIG_GLOBAL",
        "SSH_AUTH_SOCK",
        "HTTPS_PROXY",
        "PATH",
    ]
    .iter()
    .any(|name| std::env::var(name).ok().as_deref() == Some("wcore-poison"));
    let budget_seeded = config.contains("max_cost_usd = 0.031");
    format!(
        "READY arg_secret={} config_secret={} key_env={} poison={} budget={} leak={secret}",
        args.iter().any(|arg| arg.contains(secret)),
        config.contains(secret),
        provider_env_has_secret,
        poison_inherited,
        budget_seeded,
    )
}

fn load_fixture_secret() -> String {
    if let Some(path) = argument_value("--api-key-file") {
        let value = std::fs::read_to_string(&path).expect("read one-use credential file");
        std::fs::remove_file(&path).expect("remove one-use credential file");
        return value;
    }
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
