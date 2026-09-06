//! Task-owned MCP effect witness. Its journal is outside candidate-owned roots.
use axum::response::IntoResponse;
use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use std::{
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) struct EffectFixture {
    _root: tempfile::TempDir,
    state: Arc<EffectState>,
    pub url: String,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

struct EffectState {
    operation: String,
    journal: std::path::PathBuf,
    barrier: std::path::PathBuf,
    count: Mutex<u64>,
    hold: AtomicBool,
    release: tokio::sync::Notify,
}

impl EffectFixture {
    pub async fn start(operation: String, hold: bool) -> anyhow::Result<Self> {
        let root = tempfile::tempdir()?;
        let state = Arc::new(EffectState {
            operation,
            journal: root.path().join("effect.jsonl"),
            barrier: root.path().join("after-effect.ready"),
            count: Mutex::new(0),
            hold: AtomicBool::new(hold),
            release: tokio::sync::Notify::new(),
        });
        let app = Router::new()
            .route("/mcp", post(handle))
            .with_state(state.clone());
        let socket = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let url = format!("http://{}/mcp", socket.local_addr()?);
        let server = tokio::spawn(async move { axum::serve(socket, app).await });
        Ok(Self {
            _root: root,
            state,
            url,
            server,
        })
    }

    pub fn barrier(&self) -> crate::scenario::EffectBarrier {
        crate::scenario::EffectBarrier {
            path: self.state.barrier.clone(),
            expected: self.record(1),
        }
    }

    fn record(&self, count: u64) -> Vec<u8> {
        format!(
            "{}\n",
            json!({"operation":self.state.operation,"effect":count})
        )
        .into_bytes()
    }

    pub fn release(&self) {
        self.state.hold.store(false, Ordering::Release);
        self.state.release.notify_waiters();
    }

    pub fn export(&self, path: &std::path::Path) -> anyhow::Result<u64> {
        let count = *self.state.count.lock().expect("effect count lock");
        if self.state.journal.exists() {
            std::fs::copy(&self.state.journal, path)?;
        }
        Ok(count)
    }

    pub async fn stop(mut self) -> anyhow::Result<()> {
        self.release();
        self.server.abort();
        match (&mut self.server).await {
            Ok(result) => Ok(result?),
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for EffectFixture {
    fn drop(&mut self) {
        self.release();
        self.server.abort();
    }
}

async fn handle(
    State(state): State<Arc<EffectState>>,
    Json(request): Json<Value>,
) -> axum::response::Response {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let result = match request["method"].as_str().unwrap_or("") {
        "initialize" => {
            json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"paired-effect","version":"1"}})
        }
        "tools/list" => json!({"tools":[
            {"name":"record_parcel_effect","description":"Record one parcel operation durably; never repeat an operation already recorded.","inputSchema":{"type":"object","properties":{"operation":{"type":"string"}},"required":["operation"]}},
            {"name":"inspect_parcel_effect","description":"Read the authoritative recorded parcel operation without changing it.","inputSchema":{"type":"object","properties":{}}}
        ]}),
        "tools/call" => {
            let name = request["params"]["name"].as_str().unwrap_or("");
            if name == "record_parcel_effect" {
                if request["params"]["arguments"]["operation"] != state.operation {
                    return Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"operation mismatch"}})).into_response();
                }
                let written = (|| -> std::io::Result<()> {
                    let mut count = state.count.lock().expect("effect count lock");
                    let next = *count + 1;
                    let mut file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&state.journal)?;
                    writeln!(
                        file,
                        "{}",
                        json!({"operation":state.operation,"effect":next})
                    )?;
                    file.sync_all()?;
                    *count = next;
                    // Publish the cut barrier only AFTER the real journal has
                    // been synced. Observing partially written effect bytes
                    // must not masquerade as a durable-effect boundary.
                    wcore_config::atomic_write(
                        &state.barrier,
                        format!("{}\n", json!({"operation":state.operation,"effect":next}))
                            .as_bytes(),
                    )?;
                    Ok(())
                })();
                if let Err(error) = written {
                    return Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":error.to_string()}})).into_response();
                }
                loop {
                    let released = state.release.notified();
                    tokio::pin!(released);
                    released.as_mut().enable();
                    if !state.hold.load(Ordering::Acquire) {
                        break;
                    }
                    released.await;
                }
            } else if name != "inspect_parcel_effect" {
                return Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unknown tool"}})).into_response();
            }
            let count = *state.count.lock().expect("effect count lock");
            json!({"content":[{"type":"text","text":json!({"operation":state.operation,"effects":count}).to_string()}],"isError":false})
        }
        "notifications/initialized" => return axum::http::StatusCode::ACCEPTED.into_response(),
        _ => {
            return Json(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unknown method"}}),
            )
            .into_response();
        }
    };
    Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn actual_effect_is_durable_before_cut_and_duplicates_are_counted() {
        let fixture = EffectFixture::start("REC-control".into(), true)
            .await
            .unwrap();
        let url = fixture.url.clone();
        let call = tokio::spawn(async move {
            // Evaluator-owned loopback only; this never reaches a paid provider.
            reqwest::Client::new().post(url).json(&json!({"jsonrpc":"2.0","id":1,
                "method":"tools/call","params":{"name":"record_parcel_effect","arguments":{"operation":"REC-control"}}}))
                .send().await.unwrap()
        });
        let barrier = fixture.barrier();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if std::fs::read(&barrier.path).ok().as_deref() == Some(barrier.expected.as_slice())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            !call.is_finished(),
            "effect happened but result delivery must still be held"
        );
        fixture.release();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), call)
                .await
                .unwrap()
                .unwrap()
                .status()
                .is_success()
        );
        let result = reqwest::Client::new().post(&fixture.url).json(&json!({"jsonrpc":"2.0","id":2,
            "method":"tools/call","params":{"name":"record_parcel_effect","arguments":{"operation":"REC-control"}}})).send().await.unwrap();
        assert!(result.status().is_success());
        let out = tempfile::tempdir().unwrap();
        assert_eq!(
            fixture.export(&out.path().join("effects.jsonl")).unwrap(),
            2,
            "repeat effects cannot be hidden by idempotency"
        );
        assert_eq!(
            std::fs::read_to_string(out.path().join("effects.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        fixture.stop().await.unwrap();
    }
}
