p='crates/wcore-budget/src/spend.rs'
s=open(p).read()
old = """    /// Authorize an escalation, raising the ceiling and minting the durable
    /// record that must be persisted."""
new = """    /// Undo the most recent [`Self::authorize`], restoring the ceiling it
    /// raised.
    ///
    /// Exists for exactly one caller: an authorization whose durable record
    /// could not be written. An escalation that is not recorded must not be in
    /// force, and rebuilding the gate from scratch would silently drop the
    /// escalations that WERE recorded before it.
    pub fn revert_last_authorization(&mut self) -> Option<EscalationRecord> {
        let record = self.history.pop()?;
        self.authorized = record.from.clone();
        Some(record)
    }

    /// Authorize an escalation, raising the ceiling and minting the durable
    /// record that must be persisted."""
assert old in s
s = s.replace(old, new, 1)

old_t = """    #[test]
    fn authorizing_a_downgrade_records_nothing() {"""
new_t = """    #[test]
    fn reverting_an_authorization_restores_the_previous_ceiling_only() {
        let mut gate = EscalationGate::new("s1", metered("haiku", 1.0));
        gate.authorize(metered("sonnet", 6.0), "operator", "first", 1)
            .unwrap()
            .unwrap();
        gate.authorize(metered("opus", 30.0), "operator", "second", 2)
            .unwrap()
            .unwrap();
        let reverted = gate.revert_last_authorization().expect("a record to revert");
        assert_eq!(reverted.to.model, "opus");
        // The FIRST escalation survives — a failed write of the second must
        // not silently un-authorize a model that was properly recorded.
        assert_eq!(gate.authorized().model, "sonnet");
        assert_eq!(gate.history().len(), 1);
        assert!(gate.admit(&metered("sonnet", 6.0)).is_ok());
        assert!(gate.admit(&metered("opus", 30.0)).is_err());
    }

    #[test]
    fn authorizing_a_downgrade_records_nothing() {"""
assert old_t in s
s = s.replace(old_t, new_t, 1)
open(p,'w').write(s)


g='crates/wcore-agent/src/spend_guard.rs'
c=open(g).read()
old_auth = """        let reason = reason.into();
        let mut gate = self.gate.lock();
        if !gate.is_escalation(&profile) {
            return Ok(None);
        }
        let candidate = EscalationRecord {
            schema_version: wcore_budget::SPEND_SCHEMA_VERSION,
            session_id: String::new(),
            from: gate.authorized().clone(),
            to: profile.clone(),
            source: source.as_str().to_owned(),
            reason: reason.clone(),
            at_unix_ms: now_unix_ms(),
        };
        let _ = candidate;
        match gate.authorize(profile.clone(), source.as_str(), reason, now_unix_ms()) {
            Ok(Some(record)) => {
                if let Err(error) = self.sink.escalation(&record) {
                    // Roll the ceiling back: an escalation that could not be
                    // recorded must not take effect.
                    *gate = EscalationGate::new(record.session_id.clone(), record.from.clone());
                    tracing::error!(
                        target: "wcore_agent::spend_guard",
                        %error,
                        "model escalation refused: its record could not be persisted"
                    );
                    let refusal = SpendRefusal::SilentEscalation {
                        authorized: record.from.label(),
                        requested: record.to.label(),
                    };
                    self.auditor.lock().refused(&refusal);
                    return Err(refusal);
                }
                self.auditor.lock().escalated(record.clone());
                Ok(Some(record))
            }"""
new_auth = """        let reason = reason.into();
        let mut gate = self.gate.lock();
        if !gate.is_escalation(&profile) {
            return Ok(None);
        }
        match gate.authorize(profile.clone(), source.as_str(), reason, now_unix_ms()) {
            Ok(Some(record)) => {
                if let Err(error) = self.sink.escalation(&record) {
                    // An escalation that could not be recorded must not take
                    // effect. Revert only THIS authorization, so escalations
                    // already written durably stay in force.
                    gate.revert_last_authorization();
                    drop(gate);
                    tracing::error!(
                        target: "wcore_agent::spend_guard",
                        %error,
                        "model escalation refused: its record could not be persisted"
                    );
                    let refusal = SpendRefusal::SilentEscalation {
                        authorized: record.from.label(),
                        requested: record.to.label(),
                    };
                    self.auditor.lock().refused(&refusal);
                    return Err(refusal);
                }
                drop(gate);
                self.auditor.lock().escalated(record.clone());
                Ok(Some(record))
            }"""
assert old_auth in c
c = c.replace(old_auth, new_auth, 1)
c = c.replace("""            Ok(None) => Ok(None),""", """            Ok(None) => Ok(None),""",1)
open(g,'w').write(c)
print('ok')
