//! Shutdown must join the real transport reader before reporting completion.
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader, DuplexStream, ReadBuf};
use tokio::sync::Notify;
use wcore_plugin_api::access_gate::PluginAccessGate;
use wcore_plugin_subprocess::mcp_bridge::McpBridgePluginRunner;
use wcore_plugin_subprocess::rpc::{
    SubprocessRequest, SubprocessResponse, SubprocessResponseBody, SubprocessVerb,
};
use wcore_plugin_subprocess::runner::SubprocessPluginRunner;

struct TrackedRead {
    reader: DuplexStream,
    dropped: Arc<AtomicBool>,
}
impl AsyncRead for TrackedRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.reader).poll_read(cx, buf)
    }
}
impl Drop for TrackedRead {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

fn transport(
    mcp: bool,
) -> (
    DuplexStream,
    TrackedRead,
    Arc<Notify>,
    tokio::task::JoinHandle<()>,
    Arc<AtomicBool>,
) {
    let (stdin, peer_in) = tokio::io::duplex(8192);
    let (peer_out, stdout) = tokio::io::duplex(8192);
    let release = Arc::new(Notify::new());
    let gate = release.clone();
    let dropped = Arc::new(AtomicBool::new(false));
    let fixture = tokio::spawn(async move {
        let mut lines = BufReader::new(peer_in).lines();
        let mut writer = peer_out;
        while let Some(line) = lines.next_line().await.unwrap() {
            let response = if mcp {
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let Some(id) = request.get("id") else {
                    continue;
                };
                let result = match request["method"].as_str().unwrap() {
                    "initialize" => {
                        serde_json::json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}})
                    }
                    "tools/list" => serde_json::json!({"tools":[]}),
                    other => panic!("unexpected fixture request: {other}"),
                };
                serde_json::json!({"jsonrpc":"2.0","id":id,"result":result})
            } else {
                let request: SubprocessRequest = serde_json::from_str(&line).unwrap();
                let body = match request.verb {
                    SubprocessVerb::Init => SubprocessResponseBody::InitResult {
                        manifest_version: "1".into(),
                        capabilities: vec![],
                    },
                    SubprocessVerb::ListTools => {
                        SubprocessResponseBody::ToolsList { tools: vec![] }
                    }
                    SubprocessVerb::Shutdown => SubprocessResponseBody::Ack,
                    other => panic!("unexpected fixture request: {other:?}"),
                };
                serde_json::to_value(SubprocessResponse::new(request.id, body)).unwrap()
            };
            let mut encoded = serde_json::to_vec(&response).unwrap();
            encoded.push(b'\n');
            if writer.write_all(&encoded).await.is_err() {
                break;
            }
            writer.flush().await.unwrap();
            // Hold stdout even after the completed handshake/shutdown. Only a
            // joined/aborted host reader can drop TrackedRead before release.
            let done = if mcp {
                serde_json::from_str::<serde_json::Value>(&line).unwrap()["method"] == "tools/list"
            } else {
                matches!(
                    serde_json::from_str::<SubprocessRequest>(&line)
                        .unwrap()
                        .verb,
                    SubprocessVerb::Shutdown
                )
            };
            if done {
                gate.notified().await;
                break;
            }
        }
    });
    (
        stdin,
        TrackedRead {
            reader: stdout,
            dropped: dropped.clone(),
        },
        release,
        fixture,
        dropped,
    )
}

#[tokio::test]
async fn sdk_shutdown_joins_reader_before_acknowledging_cleanup() {
    let (stdin, stdout, release, fixture, dropped) = transport(false);
    let loaded =
        SubprocessPluginRunner::load_with_transport(stdin, stdout, Arc::new(PluginAccessGate))
            .await
            .unwrap();
    assert!(
        !dropped.load(Ordering::SeqCst),
        "positive control: live reader owns stdout"
    );
    let result = tokio::time::timeout(Duration::from_secs(4), loaded.runner.shutdown()).await;
    let dropped_at_ack = dropped.load(Ordering::SeqCst);
    release.notify_one();
    fixture.await.unwrap();
    result.expect("bounded shutdown").expect("shutdown cleanup");
    assert!(
        dropped_at_ack,
        "shutdown acknowledged while its reader still owned stdout"
    );
}

#[tokio::test]
async fn mcp_bridge_shutdown_joins_reader_before_acknowledging_cleanup() {
    let (stdin, stdout, release, fixture, dropped) = transport(true);
    let loaded =
        McpBridgePluginRunner::load_with_transport(stdin, stdout, Arc::new(PluginAccessGate))
            .await
            .unwrap();
    let runner = loaded.runner();
    assert!(
        !dropped.load(Ordering::SeqCst),
        "positive control: live reader owns stdout"
    );
    let result = tokio::time::timeout(Duration::from_secs(4), runner.shutdown()).await;
    let dropped_at_ack = dropped.load(Ordering::SeqCst);
    release.notify_one();
    fixture.await.unwrap();
    result.expect("bounded shutdown").expect("shutdown cleanup");
    assert!(
        dropped_at_ack,
        "shutdown acknowledged while its reader still owned stdout"
    );
}

#[tokio::test]
async fn sdk_interrupted_shutdown_can_retry_without_another_ack() {
    let (stdin, stdout, release, fixture, dropped) = transport(false);
    let loaded =
        SubprocessPluginRunner::load_with_transport(stdin, stdout, Arc::new(PluginAccessGate))
            .await
            .unwrap();
    let first = tokio::time::timeout(Duration::from_millis(50), loaded.runner.shutdown()).await;
    assert!(
        first.is_err(),
        "fixture reader must hold the first close attempt"
    );
    assert!(!dropped.load(Ordering::SeqCst));
    let retry = tokio::time::timeout(Duration::from_secs(3), loaded.runner.shutdown()).await;
    let dropped_at_ack = dropped.load(Ordering::SeqCst);
    release.notify_one();
    fixture.await.unwrap();
    retry
        .expect("retry must not request a second shutdown ACK")
        .expect("retry cleanup");
    assert!(dropped_at_ack, "retry lost the reader ownership");
    tokio::time::timeout(Duration::from_millis(100), loaded.runner.shutdown())
        .await
        .expect("completed shutdown is idempotent")
        .expect("already shut down");
}

#[tokio::test]
async fn mcp_interrupted_shutdown_retains_reader_for_retry() {
    let (stdin, stdout, release, fixture, dropped) = transport(true);
    let loaded =
        McpBridgePluginRunner::load_with_transport(stdin, stdout, Arc::new(PluginAccessGate))
            .await
            .unwrap();
    let runner = loaded.runner();
    let first = tokio::time::timeout(Duration::from_millis(50), runner.shutdown()).await;
    assert!(
        first.is_err(),
        "fixture reader must hold the first close attempt"
    );
    assert!(!dropped.load(Ordering::SeqCst));
    let retry = tokio::time::timeout(Duration::from_secs(3), runner.shutdown()).await;
    let dropped_at_ack = dropped.load(Ordering::SeqCst);
    release.notify_one();
    fixture.await.unwrap();
    retry
        .expect("bounded cleanup retry")
        .expect("retry cleanup");
    assert!(dropped_at_ack, "retry lost the reader ownership");
}
