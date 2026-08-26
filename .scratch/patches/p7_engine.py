import io

# ------------------------------------------------------------------- engine
p = "crates/wcore-agent/src/engine.rs"
s = io.open(p, encoding="utf-8").read()

old = """        self.admit_interrupted_tool_starts(&turn_id).await?;
        self.reconcile_authoritative_filesystem_effects("resume_after_interruption")
            .await?;"""
assert s.count(old) == 1, s.count(old)
s = s.replace(
    old,
    """        self.admit_interrupted_tool_starts(&turn_id).await?;
        self.reconcile_authoritative_tool_effects("resume_after_interruption")
            .await?;""",
    1,
)

old = """        self.reconcile_authoritative_filesystem_effects("engine_startup")
            .await?;"""
assert s.count(old) == 1, s.count(old)
s = s.replace(
    old,
    """        self.reconcile_authoritative_tool_effects("engine_startup")
            .await?;""",
    1,
)

old = """    /// Run the one authoritative reconciler Core registers — the filesystem
    /// compare-exchange receipt — over every tool effect that still requires
    /// reconciliation."""
assert s.count(old) == 1, s.count(old)
new = '''    /// Run every reconciler Core registers over the tool effects that still
    /// require reconciliation.
    ///
    /// Dispatch is by the NAME the tool declared, never by the kind alone.
    /// `ToolEffectContract::reconciler` documents `None` as "no automatic
    /// reconciler is available", and a name this process does not recognise is
    /// the same thing: nothing is resolved and the effect stays in front of an
    /// operator. That is what stops a tool — a plugin, an MCP proxy, a future
    /// built-in — from minting recovery authority for itself by pairing a
    /// repeat-safe kind with a reconciler identifier of its own invention.
    async fn reconcile_authoritative_tool_effects(
        &self,
        recovery: &'static str,
    ) -> Result<(), AgentError> {
        self.reconcile_repeat_safe_effects(recovery)?;
        self.reconcile_authoritative_filesystem_effects(recovery)
            .await
    }

    /// Settle every interrupted effect whose tool declared a REGISTERED
    /// repeat-safe reconciler.
    ///
    /// Nothing about the world is consulted, because there is nothing to
    /// consult: the certified class is "this invocation could not have changed
    /// anything". The receipt is therefore `NotStarted`, which is the exact
    /// disposition `wayland-core session cancel` has always written for this
    /// class — the two surfaces must not disagree about the same effect.
    ///
    /// The engine used to run only the filesystem receipt here, so a crash
    /// during a plain `Read` left an unresolved effect that blocked the
    /// session from ever resuming, while the CLI would have cleared the same
    /// effect without asking anyone. A correct reconciler nothing calls is not
    /// a reconciler.
    fn reconcile_repeat_safe_effects(&self, recovery: &'static str) -> Result<(), AgentError> {
        let journal = self.session_journal.as_ref().ok_or_else(|| {
            AgentError::SessionAuthority("session journal is not initialized".to_string())
        })?;
        let state = journal
            .state()
            .map_err(|error| AgentError::SessionAuthority(error.to_string()))?;
        let certified = state
            .tools
            .iter()
            .filter_map(|(tool_execution_id, tool)| {
                let reconciler = tool.effect_contract.reconciler.as_deref()?;
                if !matches!(
                    tool.effect_contract.kind,
                    wcore_types::tool::ToolEffectKind::RepeatSafe
                ) || !wcore_types::tool::repeat_safe_reconciler_is_registered(reconciler)
                    || !matches!(tool.effect, ToolEffectState::Unknown { .. })
                {
                    return None;
                }
                Some((tool_execution_id.clone(), reconciler.to_owned()))
            })
            .collect::<Vec<_>>();
        for (tool_execution_id, reconciler) in certified {
            self.resolve_unknown_tool_effect(
                tool_execution_id,
                ToolResolution::NotStarted {
                    reason: ToolNotStartedReason::Cancelled {
                        reason: format!(
                            "no external effect is possible for this invocation ({reconciler})"
                        ),
                    },
                },
                ToolResolutionSource::Reconciler {
                    reconciler: reconciler.clone(),
                },
                serde_json::json!({
                    "recovery": recovery,
                    "certified_by": reconciler,
                }),
            )?;
        }
        Ok(())
    }

    /// Run the one authoritative filesystem reconciler Core registers — the
    /// compare-exchange receipt — over every tool effect that still requires
    /// reconciliation.'''
s = s.replace(old, new, 1)
io.open(p, "w", encoding="utf-8").write(s)

# --------------------------------------------------------- session_lifecycle
p = "crates/wcore-agent/src/session_lifecycle.rs"
s = io.open(p, encoding="utf-8").read()

old = """    /// The tool itself declared [`ToolEffectKind::RepeatSafe`]: by its own
    /// contract the invocation cannot have created an external effect, so
    /// there is no landed effect for anyone to have an opinion about.
    RepeatSafeContract,"""
assert s.count(old) == 1
s = s.replace(
    old,
    """    /// The tool declared [`ToolEffectKind::RepeatSafe`] AND named a
    /// reconciler this build registers: by a contract the product recognises,
    /// the invocation cannot have created an external effect, so there is no
    /// landed effect for anyone to have an opinion about.
    RepeatSafeContract,""",
    1,
)

old = """        ReconcileKind::ToolExecution => {
            let tool = state.tools.get(&item.tool_execution_id)?;
            (tool.effect_contract.kind == ToolEffectKind::RepeatSafe)
                .then_some(DeterminedBy::RepeatSafeContract)
        }"""
assert s.count(old) == 1, s.count(old)
s = s.replace(
    old,
    """        ReconcileKind::ToolExecution => {
            let tool = state.tools.get(&item.tool_execution_id)?;
            // The NAME is load-bearing, not the kind. `reconciler: None` is
            // documented on the field as "no automatic reconciler is
            // available", and an unregistered name says the same thing — so a
            // tool cannot obtain a receipt written on the operator's behalf by
            // declaring a repeat-safe kind and inventing an identifier. This
            // is the same gate the engine's own recovery applies, and the two
            // must not disagree about the same effect.
            let reconciler = tool.effect_contract.reconciler.as_deref()?;
            (tool.effect_contract.kind == ToolEffectKind::RepeatSafe
                && wcore_types::tool::repeat_safe_reconciler_is_registered(reconciler))
            .then_some(DeterminedBy::RepeatSafeContract)
        }""",
    1,
)

old = """/// `None` means it genuinely cannot, and the honest response is to say so and
/// ask — never to pick a default. The one class that reaches `None` in
/// practice is a tool whose effect contract is `Opaque` (`Bash`, `Write`,
/// `Edit`): the journal records that the tool STARTED and nothing after it,
/// there is no receipt to compare, and repeating it is not safe. No amount of
/// reading the journal turns that into knowledge."""
assert s.count(old) == 1
s = s.replace(
    old,
    """/// `None` means it genuinely cannot, and the honest response is to say so and
/// ask — never to pick a default. The class that reaches `None` in practice is
/// a tool whose effect contract is `Opaque` (`Write`, `Edit`, and every `Bash`
/// command outside the read-only classifier): the journal records that the
/// tool STARTED and nothing after it, there is no receipt to compare, and
/// repeating it is not safe. No amount of reading the journal turns that into
/// knowledge.""",
    1,
)
io.open(p, "w", encoding="utf-8").write(s)

# ------------------------------------------------- the CLI repeat-safe test
p = "crates/wcore-cli/tests/session_crash_running_tool.rs"
s = io.open(p, encoding="utf-8").read()
old = """        wcore_types::tool::ToolEffectContract {
            kind: wcore_types::tool::ToolEffectKind::RepeatSafe,
            reconciler: None,
        },"""
assert s.count(old) == 1, s.count(old)
s = s.replace(
    old,
    """        // Exactly what the real `Grep` declares: the repeat-safe kind AND the
        // registered reconciler that certifies it. The kind alone buys
        // nothing — see `determined_disposition`.
        wcore_types::tool::repeat_safe_contract(
            wcore_types::tool::READ_ONLY_FILESYSTEM_RECONCILER,
        ),""",
    1,
)
io.open(p, "w", encoding="utf-8").write(s)
print("p7 ok")
