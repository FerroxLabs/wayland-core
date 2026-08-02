//! Regression gate for the Windows "the command produced no output" defect.
//!
//! Every tool result the engine returns is rewritten by
//! `wcore_compact::compact_output` at the session's compaction level (default
//! `Safe`) before the model and the host ever see it. That rewrite treated the
//! `\r` of a CRLF line terminator as a carriage-return overwrite and kept only
//! the text after it, so on Windows — where every child process writes CRLF —
//! the ENTIRE visible output of EVERY tool was deleted. A `Bash` command was
//! reported to the model as `Exit code: 0` with an empty `STDOUT:` while its
//! side effects had really happened.
//!
//! The bytes are spelled with explicit `\r\n` here, so this test exercises the
//! Windows shape on every platform and fails against the unfixed pipeline
//! everywhere, not only on the host that shipped the bug.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use wcore_agent::confirm::ToolConfirmer;
use wcore_agent::orchestration::execute_tool_calls_with_streaming;
use wcore_tools::Tool;
use wcore_types::message::ContentBlock;
use wcore_types::tool::{JsonSchema, ToolResult};

const NEEDLE: &str = "child-stdout-must-survive";

/// A tool whose result is byte-shaped exactly like a `BashTool` result captured
/// from a Windows child: CRLF-terminated payload inside the `STDOUT:` section.
struct CrlfResultTool;

#[async_trait]
impl Tool for CrlfResultTool {
    fn name(&self) -> &str {
        "CrlfResult"
    }

    fn description(&self) -> &str {
        "Test double that returns a CRLF-terminated shell-shaped result."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({ "type": "object", "properties": {} })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn category(&self) -> wcore_protocol::events::ToolCategory {
        wcore_protocol::events::ToolCategory::Exec
    }

    async fn execute(&self, _input: Value) -> ToolResult {
        ToolResult {
            content: format!("Exit code: 0\nSTDOUT:\n{NEEDLE}\r\nsecond-line\r\n\nSTDERR:\n"),
            is_error: false,
        }
    }
}

fn registry() -> wcore_tools::registry::ToolRegistry {
    let mut registry = wcore_tools::registry::ToolRegistry::new();
    registry.set_sandbox_runtime(Arc::new(wcore_sandbox::SandboxRegistry::new(Arc::new(
        wcore_sandbox::backends::no_sandbox::NoSandboxBackend::new(),
    ))));
    registry.register(Box::new(CrlfResultTool));
    registry
}

async fn dispatch_at(level: wcore_compact::CompactionLevel) -> String {
    let registry = registry();
    let confirmer = Arc::new(Mutex::new(ToolConfirmer::new(true, vec![])));
    let calls = vec![ContentBlock::ToolUse {
        id: "call-1".into(),
        name: "CrlfResult".into(),
        input: json!({}),
        extra: None,
    }];

    let outcome = execute_tool_calls_with_streaming(
        &registry,
        &calls,
        &confirmer,
        None,
        level,
        false,
        None,
        &tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .expect("dispatch must produce a tool result");

    outcome
        .results
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } if tool_use_id == "call-1" => Some(content.clone()),
            _ => None,
        })
        .expect("the dispatched call must have a result")
}

/// The default level is the one every shipped session runs at.
#[tokio::test]
async fn crlf_tool_output_survives_the_default_pipeline() {
    let content = dispatch_at(wcore_compact::CompactionLevel::default()).await;
    assert!(
        content.contains(NEEDLE),
        "the engine deleted a CRLF-terminated tool result; \
         the model would be told the command produced no output. content = {content:?}"
    );
    assert!(
        content.contains("second-line"),
        "only the first CRLF line survived; content = {content:?}"
    );
}

/// `Full` layers folding and JSON compaction on top of the same sanitizer, so
/// it must not reintroduce the loss.
#[tokio::test]
async fn crlf_tool_output_survives_full_compaction() {
    let content = dispatch_at(wcore_compact::CompactionLevel::Full).await;
    assert!(
        content.contains(NEEDLE),
        "Full compaction deleted a CRLF-terminated tool result; content = {content:?}"
    );
}
