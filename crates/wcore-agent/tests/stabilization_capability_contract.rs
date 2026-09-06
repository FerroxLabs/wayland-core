//! Per-turn capability boundary, including the real engine tool-dispatch caller.
mod common;
use wcore_agent::orchestration::intent::Mode;
use wcore_agent::orchestration::template_routing::{
    decision_is_unwired_template, select_graph_config,
};

#[test]
fn independent_modes_cannot_bypass_the_single_dispatch_boundary() {
    for mode in [Mode::Parallel, Mode::Sequential, Mode::SelfCritique] {
        let decision = select_graph_config("do the task", None, Some(mode));
        assert!(!decision.config.is_direct());
        assert!(decision_is_unwired_template(&decision), "mode {mode:?}");
    }
}

#[test]
fn explicit_independent_templates_require_visible_downgrade() {
    for task in [
        "@@template=consensus compare approaches",
        "@@template=hierarchical coordinate work",
        "@@template=self_critique review output",
        "@@template=adaptive compare approaches",
    ] {
        assert!(decision_is_unwired_template(&select_graph_config(
            task, None, None
        )));
    }
    assert!(!decision_is_unwired_template(&select_graph_config(
        "@@template=direct answer",
        None,
        None,
    )));
}

#[test]
fn online_evolution_is_default_off() {
    assert!(!wcore_config::config::ObservabilityConfig::default().online_evolution);
}

#[derive(Default)]
struct Capture(std::sync::Mutex<Vec<String>>);
impl wcore_agent::output::OutputSink for Capture {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(
        &self,
        _: &str,
        _: usize,
        _: u64,
        _: u64,
        _: u64,
        _: u64,
        _: wcore_types::message::FinishReason,
    ) {
    }
    fn emit_error(&self, message: &str, _: bool, _: wcore_protocol::events::FailureCategory) {
        panic!("unexpected engine error: {message}");
    }
    fn emit_info(&self, message: &str) {
        self.0.lock().unwrap().push(message.into());
    }
}

#[tokio::test]
async fn engine_emits_visible_downgrade_only_for_unsupported_shape() {
    use std::sync::Arc;
    use wcore_agent::engine::AgentEngine;
    use wcore_tools::registry::ToolRegistry;
    for (template, downgraded) in [("consensus", true), ("direct", false)] {
        let provider = Arc::new(CountedProvider {
            inner: common::MockLlmProvider::with_tool_use("call-1", "probe", serde_json::json!({})),
            final_response: common::MockLlmProvider::with_text_response("Probe completed."),
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(common::MockTool::new("probe", "observed", false)));
        let capture = Arc::new(Capture::default());
        let mut config = common::test_config();
        // This must not trigger a session-end paraphrase/provider call.
        config.observability.online_evolution = true;
        let mut engine =
            AgentEngine::new_with_provider(provider.clone(), config, registry, capture.clone());
        engine
            .run(&format!("@@template={template} inspect"), "capability-test")
            .await
            .unwrap();
        assert_eq!(
            provider.calls.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "online evolution must not issue an auxiliary paraphrase call"
        );
        let messages = capture.0.lock().unwrap();
        assert_eq!(
            messages
                .iter()
                .any(|m| m.contains("No independent agents were started")),
            downgraded
        );
    }
}

struct CountedProvider {
    inner: common::MockLlmProvider,
    final_response: common::MockLlmProvider,
    calls: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl wcore_providers::LlmProvider for CountedProvider {
    async fn stream(
        &self,
        request: &wcore_types::llm::LlmRequest,
    ) -> Result<
        tokio::sync::mpsc::Receiver<wcore_types::llm::LlmEvent>,
        wcore_providers::ProviderError,
    > {
        match self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
            0 => self.inner.stream(request).await,
            1 => self.final_response.stream(request).await,
            _ => panic!("unexpected auxiliary provider call"),
        }
    }
}
