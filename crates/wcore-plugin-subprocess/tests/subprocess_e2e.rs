//! v0.6.5 Task 3.2 — RPC state-machine integration tests.
//!
//! These tests drive [`SubprocessPluginRunner::load_with_transport`] over a
//! `tokio::io::duplex()` pair. The "fixture" half plays the role of the
//! plugin: reads request lines, replies with appropriate response lines.
//! This exercises the framing + state-machine + lifecycle without spawning
//! a real binary — keeping CI fast and cross-platform-safe.
//!
//! Real subprocess spawning is covered by Wave 4 Task 4.5's example plugin
//! (`examples/plugin-subprocess-mcp/`).

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex};
use wcore_plugin_api::access_gate::PluginAccessGate;
use wcore_plugin_subprocess::error::SubprocessPluginError;
use wcore_plugin_subprocess::rpc::{
    SubprocessRequest, SubprocessResponse, SubprocessResponseBody, SubprocessVerb, ToolDescriptor,
};
use wcore_plugin_subprocess::runner::{
    LoadedSubprocessPlugin, SubprocessPluginRunner, TransportFactory, TransportSpawn,
};

/// Read one request line; return None on EOF.
async fn read_request<R>(reader: &mut R) -> Option<SubprocessRequest>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut line = String::new();
    let n = reader.read_line(&mut line).await.ok()?;
    if n == 0 {
        return None;
    }
    serde_json::from_str(line.trim_end()).ok()
}

async fn write_response<W>(writer: &mut W, resp: &SubprocessResponse)
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut line = serde_json::to_string(resp).expect("serialize response");
    line.push('\n');
    writer.write_all(line.as_bytes()).await.expect("write line");
    writer.flush().await.expect("flush");
}

#[tokio::test]
async fn init_then_list_tools_then_call_then_shutdown() {
    // Runner side: stdin_w (host writes requests), stdout_r (host reads responses).
    // Fixture side: stdin_r (plugin reads requests), stdout_w (plugin writes responses).
    let (host_to_plugin_w, host_to_plugin_r) = duplex(8192);
    let (plugin_to_host_w, plugin_to_host_r) = duplex(8192);

    let fixture = tokio::spawn(async move {
        let mut reader = BufReader::new(host_to_plugin_r);
        let mut writer = plugin_to_host_w;

        // 1. Expect Init → reply InitResult.
        let req = read_request(&mut reader).await.expect("init request");
        assert!(matches!(req.verb, SubprocessVerb::Init));
        write_response(
            &mut writer,
            &SubprocessResponse::new(
                req.id,
                SubprocessResponseBody::InitResult {
                    manifest_version: "0.1.0".into(),
                    capabilities: vec!["tools".into()],
                },
            ),
        )
        .await;

        // 2. Expect ListTools → reply ToolsList.
        let req = read_request(&mut reader).await.expect("list_tools request");
        assert!(matches!(req.verb, SubprocessVerb::ListTools));
        write_response(
            &mut writer,
            &SubprocessResponse::new(
                req.id,
                SubprocessResponseBody::ToolsList {
                    tools: vec![ToolDescriptor {
                        name: "echo".into(),
                        description: Some("Echoes input".into()),
                        input_schema: json!({"type": "object"}),
                    }],
                },
            ),
        )
        .await;

        // 3. Expect CallTool → reply CallToolResult.
        let req = read_request(&mut reader).await.expect("call_tool request");
        match &req.verb {
            SubprocessVerb::CallTool { name, input } => {
                assert_eq!(name, "echo");
                assert_eq!(input, &json!({"msg": "hi"}));
            }
            other => panic!("expected CallTool, got {other:?}"),
        }
        write_response(
            &mut writer,
            &SubprocessResponse::new(
                req.id,
                SubprocessResponseBody::CallToolResult {
                    stdout: "hi".into(),
                    structured: Some(json!({"echoed": "hi"})),
                    is_error: false,
                },
            ),
        )
        .await;

        // 4. Expect Shutdown → reply Ack, then drop writer (close stdout).
        let req = read_request(&mut reader).await.expect("shutdown request");
        assert!(matches!(req.verb, SubprocessVerb::Shutdown));
        write_response(
            &mut writer,
            &SubprocessResponse::new(req.id, SubprocessResponseBody::Ack),
        )
        .await;
        drop(writer);
    });

    let gate = Arc::new(PluginAccessGate);
    let loaded =
        SubprocessPluginRunner::load_with_transport(host_to_plugin_w, plugin_to_host_r, gate)
            .await
            .expect("load");
    assert_eq!(loaded.manifest_version, "0.1.0");
    assert_eq!(loaded.capabilities, vec!["tools".to_string()]);
    assert_eq!(loaded.tools.len(), 1);
    assert_eq!(loaded.tools[0].name, "echo");

    let out = loaded
        .runner
        .call_tool("echo", json!({"msg": "hi"}))
        .await
        .expect("call_tool");
    assert_eq!(out.stdout, "hi");
    assert_eq!(out.structured, Some(json!({"echoed": "hi"})));
    assert!(!out.is_error);

    loaded.runner.shutdown().await.expect("shutdown");
    fixture.await.expect("fixture join");
}

#[tokio::test]
async fn broken_pipe_during_call_yields_typed_error() {
    let (host_to_plugin_w, host_to_plugin_r) = duplex(8192);
    let (plugin_to_host_w, plugin_to_host_r) = duplex(8192);

    let fixture = tokio::spawn(async move {
        let mut reader = BufReader::new(host_to_plugin_r);
        let mut writer = plugin_to_host_w;

        // Honor Init + ListTools, then drop the writer mid-call.
        let req = read_request(&mut reader).await.unwrap();
        write_response(
            &mut writer,
            &SubprocessResponse::new(
                req.id,
                SubprocessResponseBody::InitResult {
                    manifest_version: "0.1.0".into(),
                    capabilities: vec![],
                },
            ),
        )
        .await;
        let req = read_request(&mut reader).await.unwrap();
        write_response(
            &mut writer,
            &SubprocessResponse::new(req.id, SubprocessResponseBody::ToolsList { tools: vec![] }),
        )
        .await;

        // Read the CallTool request but DROP the writer instead of replying.
        let _ = read_request(&mut reader).await;
        drop(writer);
    });

    let gate = Arc::new(PluginAccessGate);
    let loaded =
        SubprocessPluginRunner::load_with_transport(host_to_plugin_w, plugin_to_host_r, gate)
            .await
            .expect("load");

    let result = loaded.runner.call_tool("missing", json!({})).await;
    // The reader task drained pending senders on EOF → WorkerTerminated.
    assert!(
        matches!(result, Err(SubprocessPluginError::WorkerTerminated)),
        "expected WorkerTerminated, got {result:?}"
    );

    fixture.await.expect("fixture join");
}

// -----------------------------------------------------------------------
// v0.6.5 Task 3.3 — crash budget + restart policy
// -----------------------------------------------------------------------
//
// These tests drive `load_with_factory`, which lets the test supply a
// `TransportFactory` that returns a fresh duplex pair on each call. The
// "fixture behavior" for instance N is encoded in `FixtureScript` so each
// restart can deliberately behave differently (e.g. crash first, succeed
// second).

#[derive(Clone, Debug)]
enum FixtureScript {
    /// Honor Init + ListTools, then on CallTool drop the writer (broken pipe).
    CrashOnCall,
    /// Honor Init + ListTools + CallTool with a successful echo response.
    EchoOk,
}

/// Build a `TransportFactory` whose successive calls return spawns driven
/// by the i'th entry of `scripts` (saturating at the last entry).
fn make_factory(scripts: Vec<FixtureScript>) -> TransportFactory {
    let scripts = Arc::new(scripts);
    let next = Arc::new(StdMutex::new(0usize));
    Arc::new(move || {
        let scripts = scripts.clone();
        let next = next.clone();
        Box::pin(async move {
            let idx = {
                let mut g = next.lock().expect("script counter mutex");
                let i = *g;
                *g = (i + 1).min(scripts.len() - 1);
                i.min(scripts.len() - 1)
            };
            let script = scripts[idx].clone();
            Ok(spawn_for_script(script).await)
        })
    })
}

/// Wire up a duplex pair, spawn the fixture task for `script`, return the
/// host's view as a `TransportSpawn`.
async fn spawn_for_script(script: FixtureScript) -> TransportSpawn {
    let (host_to_plugin_w, host_to_plugin_r) = duplex(8192);
    let (plugin_to_host_w, plugin_to_host_r) = duplex(8192);

    tokio::spawn(async move {
        let mut reader = BufReader::new(host_to_plugin_r);
        let mut writer = plugin_to_host_w;

        // 1. Init.
        let Some(req) = read_request(&mut reader).await else {
            return;
        };
        write_response(
            &mut writer,
            &SubprocessResponse::new(
                req.id,
                SubprocessResponseBody::InitResult {
                    manifest_version: "0.1.0".into(),
                    capabilities: vec![],
                },
            ),
        )
        .await;

        // 2. ListTools.
        let Some(req) = read_request(&mut reader).await else {
            return;
        };
        write_response(
            &mut writer,
            &SubprocessResponse::new(
                req.id,
                SubprocessResponseBody::ToolsList {
                    tools: vec![ToolDescriptor {
                        name: "echo".into(),
                        description: None,
                        input_schema: json!({"type": "object"}),
                    }],
                },
            ),
        )
        .await;

        match script {
            FixtureScript::CrashOnCall => {
                // Read the CallTool request, then drop the writer so the
                // host sees a broken pipe / EOF.
                let _ = read_request(&mut reader).await;
                drop(writer);
            }
            FixtureScript::EchoOk => loop {
                let Some(req) = read_request(&mut reader).await else {
                    break;
                };
                match req.verb {
                    SubprocessVerb::CallTool { name, input } => {
                        write_response(
                            &mut writer,
                            &SubprocessResponse::new(
                                req.id,
                                SubprocessResponseBody::CallToolResult {
                                    stdout: format!("{name}:{input}"),
                                    structured: Some(input),
                                    is_error: false,
                                },
                            ),
                        )
                        .await;
                    }
                    SubprocessVerb::Shutdown => {
                        write_response(
                            &mut writer,
                            &SubprocessResponse::new(req.id, SubprocessResponseBody::Ack),
                        )
                        .await;
                        break;
                    }
                    _ => break,
                }
            },
        }
    });

    TransportSpawn {
        stdin: Box::new(host_to_plugin_w),
        stdout: Box::new(plugin_to_host_r),
        child: None,
    }
}

#[tokio::test]
async fn crash_during_call_triggers_restart() {
    // First instance crashes on the first CallTool. Second instance echoes
    // back. Runner restarts in-place and the host sees a successful
    // ToolOutput on the (same) `call_tool` invocation.
    let factory = make_factory(vec![FixtureScript::CrashOnCall, FixtureScript::EchoOk]);
    let gate = Arc::new(PluginAccessGate);
    let LoadedSubprocessPlugin { runner, tools, .. } =
        SubprocessPluginRunner::load_with_factory(factory, gate, "p")
            .await
            .expect("initial load");
    assert_eq!(tools.len(), 1, "engine-side tools registered at load time");

    let out = runner
        .call_tool("echo", json!({"k": "v"}))
        .await
        .expect("call_tool succeeds after transparent restart");
    assert!(out.stdout.starts_with("echo:"));
    // Successful retry resets the crash counter (consecutive-only semantics).
    assert_eq!(runner.crash_count(), 0);
}

#[tokio::test]
async fn three_consecutive_crashes_disables_plugin() {
    // Factory hands out four crash-on-call instances back-to-back. After
    // three strikes, the runner short-circuits with PermissionDenied
    // ("auto-disabled after 3 crashes") and does NOT consult the factory
    // for a fourth restart.
    let factory = make_factory(vec![FixtureScript::CrashOnCall; 4]);
    let gate = Arc::new(PluginAccessGate);
    let LoadedSubprocessPlugin { runner, .. } =
        SubprocessPluginRunner::load_with_factory(factory, gate, "noisy")
            .await
            .expect("initial load");

    // Strike 1: crash → restart attempt → second instance also crashes on
    // the retried call. Net effect: strike count climbs as the retry
    // re-trips. Drive call_tool until the dedicated auto-disable error
    // surfaces.
    let mut last_err: Option<SubprocessPluginError> = None;
    for _ in 0..10 {
        match runner.call_tool("echo", json!({})).await {
            Ok(_) => panic!("call_tool should keep failing on the always-crash fixture"),
            Err(e) => {
                let disabled = matches!(
                    e,
                    SubprocessPluginError::PermissionDenied(ref msg)
                        if msg.contains("auto-disabled") && msg.contains("3")
                );
                last_err = Some(e);
                if disabled {
                    break;
                }
            }
        }
    }
    let final_err = last_err.expect("at least one error");
    assert!(
        matches!(
            final_err,
            SubprocessPluginError::PermissionDenied(ref msg)
                if msg.contains("auto-disabled") && msg.contains("3")
        ),
        "expected auto-disable PermissionDenied, got {final_err:?}"
    );
    assert!(
        runner.crash_count() >= 3,
        "crash counter should be at least 3"
    );
}

#[tokio::test]
async fn restart_preserves_registered_tools() {
    // The tools captured at load() time are what the engine registered into
    // its tool registry. They must remain reachable across a restart — the
    // runner's identity (and thus the engine-side routing) is unchanged.
    let factory = make_factory(vec![FixtureScript::CrashOnCall, FixtureScript::EchoOk]);
    let gate = Arc::new(PluginAccessGate);
    let LoadedSubprocessPlugin { runner, tools, .. } =
        SubprocessPluginRunner::load_with_factory(factory, gate, "p")
            .await
            .expect("initial load");
    let tool_names_before: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
    assert_eq!(tool_names_before, vec!["echo".to_string()]);

    // Trigger restart via a crash-then-success cycle.
    let _ = runner
        .call_tool("echo", json!({"x": 1}))
        .await
        .expect("post-restart call_tool succeeds");

    // The same `tools` vector returned by load() is still valid (it's owned
    // by the host apply-pipeline, not by the subprocess). The runner
    // continues to answer for those tools — verify with a second call.
    let out = runner
        .call_tool("echo", json!({"x": 2}))
        .await
        .expect("subsequent call_tool also succeeds");
    assert!(out.stdout.contains("echo"));
    assert_eq!(tool_names_before, vec!["echo".to_string()]);
}

#[tokio::test]
async fn unexpected_exit_propagates_as_typed_error() {
    // Plugin closes both pipes immediately (before even replying to Init).
    let (host_to_plugin_w, host_to_plugin_r) = duplex(8192);
    let (plugin_to_host_w, plugin_to_host_r) = duplex(8192);

    let fixture = tokio::spawn(async move {
        // Drain the host's request line so the host's write doesn't block
        // on a full buffer, then drop both ends.
        let mut reader = BufReader::new(host_to_plugin_r);
        let mut line = String::new();
        // tolerate either EOF or one line before drop
        let _ = tokio::time::timeout(Duration::from_millis(50), reader.read_line(&mut line)).await;
        drop(reader);
        drop(plugin_to_host_w);
    });

    let gate = Arc::new(PluginAccessGate);
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        SubprocessPluginRunner::load_with_transport(host_to_plugin_w, plugin_to_host_r, gate),
    )
    .await
    .expect("load should not hang");

    // Reader task sees immediate EOF → drains pending senders → request's
    // oneshot rx closes → WorkerTerminated. (LoadedSubprocessPlugin has no
    // Debug impl, so we summarize the error arm rather than printing the Ok
    // path on failure.)
    let err_label = match &result {
        Ok(_) => "Ok(loaded)".to_string(),
        Err(e) => format!("Err({e})"),
    };
    assert!(
        matches!(result, Err(SubprocessPluginError::WorkerTerminated)),
        "expected WorkerTerminated on early plugin exit, got {err_label}"
    );

    fixture.await.expect("fixture join");
}
