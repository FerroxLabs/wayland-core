use crate::events::Capabilities;

pub(super) fn capabilities() -> Capabilities {
    Capabilities {
        tool_approval: true,
        thinking: true,
        effort: true,
        effort_levels: vec!["low".into(), "medium".into(), "high".into()],
        modes: vec!["default".into(), "auto_edit".into(), "force".into()],
        current_mode: "default".into(),
        mcp: true,
        streaming_tools: true,
        sub_agent_traces: true,
        cost_attribution: true,
        hitl_suspend: true,
        non_destructive_compact: true,
        structured_traces: true,
        rpc_tool_script: true,
        browser_suite: true,
        computer_use: true,
        plugins: true,
        gepa_enabled: true,
        user_model_backend: "local".into(),
        online_evolution: true,
        memory_enabled: true,
    }
}
