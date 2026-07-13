use std::collections::HashMap;

use serde_json::json;
use wcore_eval_scenarios::fixtures::mcp::{McpHttpFixture, McpHttpMode};
use wcore_mcp::config::{McpServerConfig, TransportType};
use wcore_mcp::manager::McpManager;

async fn assert_http_round_trip(mode: McpHttpMode, transport: TransportType) {
    let fixture = McpHttpFixture::start(mode)
        .await
        .expect("start MCP HTTP fixture");
    let mut configs = HashMap::new();
    configs.insert(
        "fixture".to_string(),
        McpServerConfig {
            transport,
            command: None,
            args: None,
            env: None,
            url: Some(fixture.url().to_string()),
            headers: None,
            deferred: Some(false),
            allow_local: true,
            only_for_assistant: None,
        },
    );

    let manager = McpManager::connect_all(&configs)
        .await
        .expect("connect MCP fixture");
    assert_eq!(manager.server_names(), ["fixture"]);
    let outcome = manager
        .call_tool("fixture", "fixture_echo", json!({"text": "ROUNDTRIP"}))
        .await
        .expect("call fixture tool");
    assert_eq!(outcome.text, "ROUNDTRIP");
    assert!(!outcome.is_error);
    manager.shutdown().await;

    let observation = fixture.shutdown().await.expect("stop MCP fixture");
    assert!(observation.complete(), "observation: {observation:?}");
    assert_eq!(
        observation.methods(),
        [
            "initialize",
            "notifications/initialized",
            "tools/list",
            "tools/call"
        ]
    );
    assert!(
        observation
            .requests
            .iter()
            .all(|request| request.body_sha256.len() == 64)
    );
    let debug = format!("{observation:?}");
    assert!(!debug.contains("ROUNDTRIP"));
}

#[tokio::test]
async fn streamable_http_direct_json_round_trip() {
    assert_http_round_trip(McpHttpMode::DirectJson, TransportType::StreamableHttp).await;
}

#[tokio::test]
async fn streamable_http_sse_round_trip() {
    assert_http_round_trip(McpHttpMode::SseResponse, TransportType::StreamableHttp).await;
}

#[tokio::test]
async fn legacy_sse_round_trip() {
    assert_http_round_trip(McpHttpMode::LegacySse, TransportType::Sse).await;
}

#[tokio::test]
async fn stdio_round_trip_uses_portable_fixture_binary() {
    let tempdir = tempfile::tempdir().expect("MCP stdio fixture tempdir");
    let journal = tempdir.path().join("mcp-stdio-journal.jsonl");
    let mut configs = HashMap::new();
    configs.insert(
        "fixture".to_string(),
        McpServerConfig {
            transport: TransportType::Stdio,
            command: Some(env!("CARGO_BIN_EXE_wcore-eval-fixture").to_string()),
            args: Some(vec![
                "--mcp-stdio".to_string(),
                "--mcp-journal".to_string(),
                journal.to_string_lossy().into_owned(),
            ]),
            env: None,
            url: None,
            headers: None,
            deferred: Some(false),
            allow_local: false,
            only_for_assistant: None,
        },
    );

    let manager = McpManager::connect_all(&configs)
        .await
        .expect("connect stdio fixture");
    assert_eq!(manager.server_names(), ["fixture"]);
    let outcome = manager
        .call_tool("fixture", "fixture_echo", json!({"text": "STDIO"}))
        .await
        .expect("call stdio fixture tool");
    assert_eq!(outcome.text, "STDIO");
    assert!(!outcome.is_error);
    manager.shutdown().await;

    let rows = std::fs::read_to_string(journal).expect("read MCP stdio journal");
    let records = rows
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("journal JSON"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 4);
    assert_eq!(
        records
            .iter()
            .map(|record| record["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "initialize",
            "notifications/initialized",
            "tools/list",
            "tools/call"
        ]
    );
    assert!(records.iter().all(|record| {
        record["body_sha256"]
            .as_str()
            .is_some_and(|digest| digest.len() == 64)
    }));
    assert!(!rows.contains("STDIO"));
}
