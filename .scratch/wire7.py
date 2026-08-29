p = 'crates/wcore-agent/src/engine.rs'
s = open(p).read()
def sub1(old, new, label):
    global s
    assert s.count(old) == 1, f"{label}: expected 1, found {s.count(old)}"
    s = s.replace(old, new, 1)

sub1("""    pub async fn run_with_content(
        &mut self,
        user_input: &str,
        additional_content: Vec<ContentBlock>,
        msg_id: &str,
    ) -> Result<AgentResult, AgentError> {
        if self.session_manager.is_some()""",
"""    pub async fn run_with_content(
        &mut self,
        user_input: &str,
        additional_content: Vec<ContentBlock>,
        msg_id: &str,
    ) -> Result<AgentResult, AgentError> {
        // #174 c2 — ONE task, ONE spend audit record. The body below has a
        // dozen `return` paths (answered, budget-stopped, cancelled, journal
        // refusal, `?` on an authority error); wrapping it here is the only
        // shape where every one of them emits, and where adding a thirteenth
        // cannot forget to.
        let outcome = self
            .run_with_content_audited(user_input, additional_content, msg_id)
            .await;
        self.emit_task_spend_audit();
        outcome
    }

    async fn run_with_content_audited(
        &mut self,
        user_input: &str,
        additional_content: Vec<ContentBlock>,
        msg_id: &str,
    ) -> Result<AgentResult, AgentError> {
        if self.session_manager.is_some()""", "run-wrapper")

open(p,'w').write(s)
print('ok')
