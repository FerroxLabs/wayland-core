p = 'crates/wcore-agent/src/engine.rs'
s = open(p).read()
def sub1(old, new, label):
    global s
    assert s.count(old) == 1, f"{label}: expected 1, found {s.count(old)}"
    s = s.replace(old, new, 1)

# ── field on AgentEngine ────────────────────────────────────────────────────
sub1("""pub struct AgentEngine {
    provider: Arc<dyn LlmProvider>,
    /// Immutable outbound authority for this session.""",
"""pub struct AgentEngine {
    provider: Arc<dyn LlmProvider>,
    /// #174 c2-c5 — the run's spend guard. Always present, even in the default
    /// unrestricted mode, because the per-task spend AUDIT is unconditional
    /// while the modes are opt-in.
    ///
    /// `self.provider` is a [`crate::spend_guard::SpendGuardProvider`] wrapping
    /// this guard, so every dispatch made through the engine's provider handle
    /// — the conversation turn, its compaction call, the online-evolution
    /// paraphrase — is admitted by it without each of those sites having to
    /// remember to ask.
    spend_guard: Arc<crate::spend_guard::SpendGuard>,
    /// Immutable outbound authority for this session.""", "field")

# ── installer + audit emission helpers ──────────────────────────────────────
sub1("""impl AgentEngine {
    pub fn new(config: Config, tools: ToolRegistry, output: Arc<dyn OutputSink>) -> Self {""",
"""/// #174 c2 — durable per-task spend audit log for this profile.
///
/// Beside the daily spend ledger, not inside the prunable diagnostics cost
/// ledger: an audit trail that a prune can silently truncate is not one.
pub fn spend_audit_log_path() -> std::path::PathBuf {
    wcore_config::config::wayland_config_dir()
        .join("budget")
        .join("spend-audit.jsonl")
}

/// Build the run's spend guard from `config` and wrap `provider` in it.
///
/// The ONE place the engine installs a provider handle. Every constructor and
/// `rebind_provider` route through it, so there is no code path that leaves the
/// engine holding an unguarded provider.
fn install_spend_guard(
    provider: Arc<dyn LlmProvider>,
    provider_key: &str,
    model: &str,
    compat: &wcore_config::compat::ProviderCompat,
    mode: wcore_budget::SpendMode,
    session_id: &str,
) -> (Arc<dyn LlmProvider>, Arc<crate::spend_guard::SpendGuard>) {
    let baseline = crate::spend_guard::classify_model(provider_key, model, compat);
    let sink: Arc<dyn wcore_budget::SpendAuditSink> = Arc::new(
        wcore_budget::JsonlSpendAuditSink::new(spend_audit_log_path()),
    );
    let guard = Arc::new(crate::spend_guard::SpendGuard::new(
        mode, session_id, baseline, sink,
    ));
    let wrapped: Arc<dyn LlmProvider> = Arc::new(crate::spend_guard::SpendGuardProvider::new(
        provider,
        Arc::clone(&guard),
        provider_key,
        compat.clone(),
    ));
    (wrapped, guard)
}

impl AgentEngine {
    pub fn new(config: Config, tools: ToolRegistry, output: Arc<dyn OutputSink>) -> Self {""",
     "installer")

# ── constructor: new_with_provider ──────────────────────────────────────────
sub1("""        let workflow_live_mode = config.observability.workflow_live_mode;
        let retained_config = config.clone();
        let system_prompt = config.system_prompt.clone().unwrap_or_default();""",
"""        let workflow_live_mode = config.observability.workflow_live_mode;
        let retained_config = config.clone();
        // #174 — install the spend guard BEFORE the struct literal partially
        // moves `config`.
        let (provider, spend_guard) = install_spend_guard(
            provider,
            config.compat.provider_type(),
            &config.model,
            &config.compat,
            config.budget.spend_mode(),
            &uuid::Uuid::new_v4().to_string(),
        );
        let system_prompt = config.system_prompt.clone().unwrap_or_default();""",
     "ctor-new")

sub1("""        Self {
            flux_loop_intent: None,
            provider,
            egress_policy: wcore_egress::default_policy(),
            tools: Arc::new(tools),
            messages: Vec::new(),""",
"""        Self {
            flux_loop_intent: None,
            provider,
            spend_guard,
            egress_policy: wcore_egress::default_policy(),
            tools: Arc::new(tools),
            messages: Vec::new(),""", "ctor-new-literal")

# ── constructor: resume_with_provider_parts ─────────────────────────────────
sub1("""        let workflow_live_mode = config.observability.workflow_live_mode;
        let retained_config = config.clone();
        // #1161 — read the persisted conversation id BEFORE `session` is moved""",
"""        let workflow_live_mode = config.observability.workflow_live_mode;
        let retained_config = config.clone();
        // #174 — see `new_with_provider`: guard installed before the literal
        // partially moves `config`.
        let (provider, spend_guard) = install_spend_guard(
            provider,
            config.compat.provider_type(),
            &config.model,
            &config.compat,
            config.budget.spend_mode(),
            &uuid::Uuid::new_v4().to_string(),
        );
        // #1161 — read the persisted conversation id BEFORE `session` is moved""",
     "ctor-resume")

sub1("""        Self {
            flux_loop_intent: None,
            provider,
            egress_policy: wcore_egress::default_policy(),
            tools: Arc::new(tools),
            messages: session.messages.clone(),""",
"""        Self {
            flux_loop_intent: None,
            provider,
            spend_guard,
            egress_policy: wcore_egress::default_policy(),
            tools: Arc::new(tools),
            messages: session.messages.clone(),""", "ctor-resume-literal")

# ── rebind_provider ─────────────────────────────────────────────────────────
sub1("""        self.provider = provider;
        self.compat = compat;
        self.model = model;""",
"""        // #174 — a rebind installs a deliberately chosen provider+model, so it
        // is the new authorized baseline. Routed through the same installer as
        // the constructors: there must be no way to reach `self.provider` with
        // an unguarded handle.
        let (wrapped, spend_guard) = install_spend_guard(
            provider,
            compat.provider_type(),
            &model,
            &compat,
            self.spend_guard.mode(),
            &self.budget_session_id(),
        );
        self.provider = wrapped;
        self.spend_guard = spend_guard;
        self.compat = compat;
        self.model = model;""", "rebind")

# ── set_model: an explicit operator pick is recorded, not silent ────────────
sub1("""    pub fn set_model(&mut self, model: impl Into<String>) {
        let model = model.into();
        self.user_model_pin = Some(model.clone());
        self.model = model;
    }""",
"""    pub fn set_model(&mut self, model: impl Into<String>) {
        let model = model.into();
        // #174 c5 — an explicit operator pick is not a SILENT escalation, but
        // it is still an escalation and must be recorded with its reason.
        // A spend MODE still binds: `/model` cannot buy through `local-only`.
        let profile =
            crate::spend_guard::classify_model(self.compat.provider_type(), &model, &self.compat);
        if let Err(refusal) = self.spend_guard.authorize(
            profile,
            crate::spend_guard::EscalationSource::Operator,
            "explicit operator model selection",
        ) {
            tracing::warn!(
                target: "wcore_agent::spend_guard",
                requested = %model,
                %refusal,
                "model selection refused by the spend guard"
            );
            self.output.emit_info(&refusal.to_string());
            return;
        }
        self.user_model_pin = Some(model.clone());
        self.model = model;
    }""", "set_model")

# ── apply_switch_model: the silent path ─────────────────────────────────────
sub1("""            return;
        }
        self.model = new_model;
    }

    /// The active model identifier (used by the TUI status bar + tests).""",
"""            return;
        }
        // #174 c5 — this IS the silent-escalation path: a skill or hook moving
        // the live model with nobody asked. `admit` (not `authorize`) is
        // deliberate — a hook cannot supply an operator's reason, so an upward
        // move here is refused rather than recorded and allowed.
        let profile = crate::spend_guard::classify_model(
            self.compat.provider_type(),
            &new_model,
            &self.compat,
        );
        if let Err(refusal) = self.spend_guard.admit(&profile) {
            tracing::warn!(
                target: "wcore_agent::spend_guard",
                requested = %new_model,
                %refusal,
                "skill/hook switch_model refused by the spend guard"
            );
            self.output.emit_info(&refusal.to_string());
            return;
        }
        self.model = new_model;
    }

    /// #174 c2 — the run's spend guard, for hosts and tests that need to read
    /// the audit or authorize an escalation.
    pub fn spend_guard(&self) -> &Arc<crate::spend_guard::SpendGuard> {
        &self.spend_guard
    }

    /// #174 c2 — close the current task's spend audit and persist its record.
    ///
    /// Called from exactly one place ([`Self::run_with_content`]), on EVERY
    /// terminal path including the error ones, which is what makes "a record
    /// after every task" true rather than "a record after a task that ended
    /// the way we expected".
    fn emit_task_spend_audit(&self) {
        let Some(record) = self.spend_guard.finish_task() else {
            return;
        };
        tracing::info!(
            target: "wcore_agent::spend_audit",
            task = %record.task_id,
            summary = %record.summary(),
            "per-task spend audit record"
        );
        // Only surface it to the user when something was refused or escalated.
        // A line on every turn would train the reader to skip the one turn
        // where it mattered.
        if !record.refusals.is_empty() || !record.escalations.is_empty() {
            self.output.emit_info(&record.summary());
        }
    }

    /// The active model identifier (used by the TUI status bar + tests).""",
     "apply_switch_model")

open(p,'w').write(s)
print('ok')
