use std::collections::HashMap;

use serde_json::{Value, json};
use wcore_mcp::config::{McpServerConfig, TransportType};
use wcore_mcp::manager::{McpManager, McpServerHealth};
use wiremock::matchers::{body_string_contains, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn resource_only_server_connects_without_tools_list_or_duplicate_dial() {
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
            "result": {
                "resources": [{
                    "uri": "skill://resource-only/demo",
                    "name": "Demo resource",
                    "mimeType": "text/plain"
                }]
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("\"method\":\"resources/read\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": 11,
            "result": {
                "contents": [{
                    "uri": "skill://resource-only/demo",
                    "mimeType": "text/plain",
                    "text": "resource-only payload"
                }]
            }
        })))
        .mount(&server)
        .await;

    let config = McpServerConfig {
        transport: TransportType::StreamableHttp,
        command: None,
        args: None,
        env: None,
        url: Some(server.uri()),
        headers: None,
        deferred: Some(true),
        allow_local: true,
        only_for_assistant: None,
    };
    let mut manager = McpManager::connect_all(&HashMap::new())
        .await
        .expect("empty manager");

    let first_tools = manager
        .connect_one("resources".to_string(), &config)
        .await
        .expect("resource-only server connects");
    assert!(first_tools.is_empty());
    assert!(manager.all_tools().is_empty());
    assert!(manager.server_supports_resources("resources"));
    assert!(matches!(
        manager.health().get("resources"),
        Some(McpServerHealth::Ready { tool_count: 0 })
    ));

    let requests_after_first_connect = server
        .received_requests()
        .await
        .expect("request journal")
        .len();
    let second_tools = manager
        .connect_one("resources".to_string(), &config)
        .await
        .expect("re-adding a live server is idempotent");
    assert!(second_tools.is_empty());
    assert_eq!(
        server
            .received_requests()
            .await
            .expect("request journal")
            .len(),
        requests_after_first_connect,
        "re-adding a live server must not dial it again"
    );

    let resources = manager
        .list_resources("resources")
        .await
        .expect("resources/list");
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].uri, "skill://resource-only/demo");
    assert_eq!(
        manager
            .read_resource("resources", "skill://resource-only/demo")
            .await
            .expect("resources/read"),
        "resource-only payload"
    );

    let methods: Vec<String> = server
        .received_requests()
        .await
        .expect("request journal")
        .into_iter()
        .filter_map(|request| serde_json::from_slice::<Value>(&request.body).ok())
        .filter_map(|body| body["method"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        methods,
        vec![
            "initialize",
            "notifications/initialized",
            "resources/list",
            "resources/read"
        ]
    );
    assert!(!methods.iter().any(|method| method == "tools/list"));

    manager.shutdown().await;
}
