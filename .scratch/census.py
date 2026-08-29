p = 'crates/wcore-agent/tests/spend_governance_test.rs'
s = open(p).read()
old_start = s.index("/// Every production `LlmProvider::stream` call site")
old_end = s.index("];", s.index("const EXPECTED_DISPATCH_SITES")) + 2
new = '''/// Every production `LlmProvider::stream` call site in the workspace, as
/// `<crate>/<crate-relative path>::<receiver expression>`, grouped by the guard
/// that covers it. A site that appears here without being in one of these three
/// groups is a hole, and a site that appears in the tree without being here at
/// all fails this test.
///
/// **Group 1 — covered by the `SpendGuardProvider` decorator.** All three
/// dispatch through a handle cloned from `AgentEngine::provider`, which
/// `install_spend_guard` makes a `SpendGuardProvider`: the conversation turn,
/// the autocompact summarization call, and the online-evolution paraphrase.
///
/// **Group 2 — decorators and transports that cannot change provider OR
/// model.** A journal wrapper, the guard's own pass-through, and the sixteen
/// concrete providers that delegate their wire work to
/// `OpenAICompatibleProvider`. Each forwards the request it was handed, so it
/// inherits whatever admitted that request. Nothing here needs its own gate,
/// and if one of them ever starts rewriting `request.model` this list is where
/// that becomes visible.
///
/// **Group 3 — the sites that DO change provider and model, inside their own
/// `stream()`, below the engine's guarded handle.** `ResilientProvider`'s
/// configured fallback and `ProviderChain`'s next slot. The decorator cannot
/// see either, so both are gated where they both funnel:
/// `retry::admit_configured_fallback`, whose admitter the engine installs with
/// a `SpendGuard::admit` call.
const EXPECTED_DISPATCH_SITES: &[&str] = &[
    // Group 1
    "wcore-agent/src/compact/auto.rs::provider",
    "wcore-agent/src/engine.rs::attempt_provider",
    "wcore-agent/src/engine.rs::provider",
    "wcore-evolve/src/mutator/llm_paraphrase_provider.rs::self.provider",
    // Group 2
    "wcore-agent/src/journal_provider.rs::self.inner",
    "wcore-agent/src/spend_guard.rs::self.inner",
    "wcore-providers/src/cerebras.rs::self.inner",
    "wcore-providers/src/deepseek.rs::self.inner",
    "wcore-providers/src/fireworks.rs::self.inner",
    "wcore-providers/src/flux_router.rs::self.inner",
    "wcore-providers/src/groq.rs::self.inner",
    "wcore-providers/src/mistral.rs::self.inner",
    "wcore-providers/src/moonshot.rs::self.inner",
    "wcore-providers/src/nvidia.rs::self.inner",
    "wcore-providers/src/openai_compatible.rs::self.inner",
    "wcore-providers/src/openrouter.rs::self.inner",
    "wcore-providers/src/perplexity.rs::self.inner",
    "wcore-providers/src/qwen.rs::self.inner",
    "wcore-providers/src/sakana.rs::self.inner",
    "wcore-providers/src/together.rs::self.inner",
    "wcore-providers/src/xai.rs::self.inner",
    "wcore-providers/src/resilient.rs::self.primary",
    // Group 3
    "wcore-providers/src/chain.rs::slot.provider",
    "wcore-providers/src/resilient.rs::fallback.provider",
];'''
s = s[:old_start] + new + s[old_end:]
open(p,'w').write(s)
print('ok')
