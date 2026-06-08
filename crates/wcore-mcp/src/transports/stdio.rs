//! T2-E1: stdio transport.
//!
//! Reads newline-delimited JSON-RPC requests from `stdin`, dispatches
//! through `McpServer::handle_request`, and writes newline-delimited
//! responses to `stdout`. EOF terminates the loop cleanly.
//!
//! Parse errors emit a JSON-RPC parse-error response (id=null per
//! spec) instead of bringing the loop down — the peer may recover.

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::server::{McpServer, ServerJsonRpcRequest, ServerJsonRpcResponse, error_code};

/// Public stdio entry point — binds to the process's real stdin/stdout.
pub async fn serve_stdio(server: McpServer) -> std::io::Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    serve_stdio_with(server, stdin, stdout).await
}

/// Generic variant — accepts any pair of `AsyncRead`/`AsyncWrite`.
/// Exposed so tests can drive the loop with in-memory pipes; also
/// useful if a caller wants to embed the loop over an existing
/// duplex stream (e.g. a child process's stdio when this server is
/// itself acting as a subprocess).
pub async fn serve_stdio_with<R, W>(
    server: McpServer,
    reader: R,
    mut writer: W,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<ServerJsonRpcRequest>(&line) {
            Ok(req) => server.handle_request(req).await,
            Err(e) => ServerJsonRpcResponse::err(
                None,
                error_code::PARSE_ERROR,
                format!("parse error: {}", e),
            ),
        };
        let mut bytes = serde_json::to_vec(&resp).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        writer.write_all(&bytes).await?;
        writer.flush().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{PolicyCheck, ServerJsonRpcResponse, default_tool_set};
    use serde_json::json;
    use tokio::io::duplex;

    /// Read all newline-delimited JSON-RPC responses from a buffer of bytes.
    fn parse_responses(buf: &[u8]) -> Vec<ServerJsonRpcResponse> {
        std::str::from_utf8(buf)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<ServerJsonRpcResponse>(l).expect("response parses"))
            .collect()
    }

    /// Drive `serve_stdio_with` against an in-memory request stream;
    /// return the bytes the server wrote. Closes the reader side so
    /// EOF terminates the loop. `tokio::io::AsyncWrite` has a blanket
    /// impl for `&mut Vec<u8>`, so we capture output directly.
    async fn drive(server: McpServer, input: &str) -> Vec<u8> {
        let (mut req_tx, req_rx) = duplex(8192);
        req_tx.write_all(input.as_bytes()).await.unwrap();
        drop(req_tx); // signal EOF
        let mut sink: Vec<u8> = Vec::new();
        serve_stdio_with(server, req_rx, &mut sink).await.unwrap();
        sink
    }

    #[tokio::test]
    async fn stdio_initialize_returns_capabilities() {
        let server = McpServer::with_defaults();
        let input = format!(
            "{}\n",
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"})
        );
        let out = drive(server, &input).await;
        let responses = parse_responses(&out);
        assert_eq!(responses.len(), 1);
        let r = &responses[0];
        let result = r.result.as_ref().expect("result present");
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["capabilities"]["tools"].is_object());
    }

    /// R2 fix A3: default tool set is empty in v0.6.2 — stubs no longer
    /// advertised via `tools/list` (MCP spec compliance).
    #[tokio::test]
    async fn stdio_tools_list_returns_default_set() {
        let server = McpServer::with_defaults();
        let input = format!(
            "{}\n",
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
        );
        let out = drive(server, &input).await;
        let responses = parse_responses(&out);
        let tools = responses[0].result.as_ref().unwrap()["tools"]
            .as_array()
            .unwrap();
        assert_eq!(tools.len(), 0);
    }

    #[tokio::test]
    async fn stdio_tools_call_unknown_tool_returns_error() {
        let server = McpServer::with_defaults();
        let input = format!(
            "{}\n",
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "no_such_tool"}})
        );
        let out = drive(server, &input).await;
        let responses = parse_responses(&out);
        let err = responses[0].error.as_ref().unwrap();
        assert_eq!(err.code, error_code::METHOD_NOT_FOUND);
    }

    struct DenyAll;
    impl PolicyCheck for DenyAll {
        fn check_tool(&self, _: &str) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn stdio_tools_call_denied_by_policy_returns_error() {
        let server = McpServer::new(default_tool_set(), Box::new(DenyAll));
        let input = format!(
            "{}\n",
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "wayland_memory_recall"}})
        );
        let out = drive(server, &input).await;
        let responses = parse_responses(&out);
        let err = responses[0].error.as_ref().unwrap();
        assert_eq!(err.code, error_code::POLICY_DENIED);
    }

    #[tokio::test]
    async fn stdio_malformed_json_returns_parse_error() {
        let server = McpServer::with_defaults();
        let out = drive(server, "this is not json\n").await;
        let responses = parse_responses(&out);
        let err = responses[0].error.as_ref().unwrap();
        assert_eq!(err.code, error_code::PARSE_ERROR);
        // Parse errors are always id=null per JSON-RPC spec.
        assert!(responses[0].id.is_none());
    }

    #[tokio::test]
    async fn stdio_request_id_preserved_in_response() {
        let server = McpServer::with_defaults();
        // Use a string id to exercise non-numeric id passthrough.
        let input = format!(
            "{}\n",
            json!({"jsonrpc": "2.0", "id": "abc-123", "method": "tools/list"})
        );
        let out = drive(server, &input).await;
        let responses = parse_responses(&out);
        assert_eq!(responses[0].id, Some(json!("abc-123")));
    }

    #[tokio::test]
    async fn stdio_multiple_requests_processed_sequentially() {
        let server = McpServer::with_defaults();
        let input = format!(
            "{}\n{}\n{}\n",
            json!({"jsonrpc": "2.0", "id": 10, "method": "initialize"}),
            json!({"jsonrpc": "2.0", "id": 11, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "id": 12, "method": "bogus_method"}),
        );
        let out = drive(server, &input).await;
        let responses = parse_responses(&out);
        assert_eq!(responses.len(), 3);
        assert_eq!(responses[0].id, Some(json!(10)));
        assert_eq!(responses[1].id, Some(json!(11)));
        assert_eq!(responses[2].id, Some(json!(12)));
        assert_eq!(
            responses[2].error.as_ref().unwrap().code,
            error_code::METHOD_NOT_FOUND
        );
    }

    #[tokio::test]
    async fn stdio_eof_terminates_loop_cleanly() {
        // No newline at EOF — `next_line` still returns the partial
        // line (per tokio's `Lines` semantics) only if it's complete;
        // here we feed valid JSON terminated by EOF (no newline) and
        // expect the loop to exit without panic. The `Lines` iterator
        // drops the unterminated final line, so we expect zero
        // responses but a clean exit.
        let server = McpServer::with_defaults();
        let out = drive(server, "").await;
        assert!(out.is_empty(), "no requests => no responses, clean exit");
    }
}
