//! FerroxLabs/wayland#174 items 2-5 — spend MODES and the model-escalation gate.
//!
//! The rest of this crate answers "how much may this session spend?". This
//! module answers two different questions that a number cannot:
//!
//! * **What may it spend it ON?** — [`SpendMode::NoPaidModels`] and
//!   [`SpendMode::LocalOnly`]. A dollar cap bounds the bill; it does not stop a
//!   run reaching a hosted API at all. An air-gapped or
//!   metered-egress deployment needs the second guarantee, and needs it
//!   ENFORCED at dispatch rather than advertised in a config file.
//!
//! * **May it change WHICH model it spends on?** — [`EscalationGate`]. Several
//!   surfaces can move the live model mid-run (a routing tier swap, a
//!   skill/hook `switch_model`, a configured provider fallback, a compaction
//!   model). Moving DOWN the price ladder is free. Moving UP it — or onto a
//!   model whose price is unknown, which cannot be assumed cheaper — is an
//!   escalation, and an escalation nobody named a reason for is exactly the
//!   runaway-spend shape this issue was filed about.
//!
//! Both are deliberately dependency-free: the caller resolves prices (the
//! agent has `wcore-pricing`, this crate must not) and hands over a
//! [`ModelSpendProfile`]. That keeps the decision table testable in isolation
//! AND keeps `wcore-budget` at the bottom of the graph.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// How one provider/model pair is billed.
///
/// This is the axis both modes discriminate on, so it is a closed enum rather
/// than a price threshold: "costs $0.00 per million tokens" and "runs on this
/// machine" are different guarantees, and `Unpriced` is neither of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelBilling {
    /// Served by a local inference runtime. No money leaves and no request
    /// leaves the machine.
    Local,
    /// Hosted, but the published price is a real, catalogued $0.00.
    Free,
    /// Hosted and metered at a known rate.
    Metered,
    /// Hosted with NO known price. Never treated as free: an unknown price is
    /// the one case where a guard that guesses is worse than one that refuses.
    Unpriced,
}

impl ModelBilling {
    /// Does using this model move money?
    ///
    /// `Unpriced` counts as paid. The alternative — treating "we could not
    /// find a price" as "there is no price" — is how an unmetered router alias
    /// walks through a no-paid-models guarantee.
    #[must_use]
    pub fn is_paid(self) -> bool {
        matches!(self, Self::Metered | Self::Unpriced)
    }

    /// Does using this model send the conversation off the machine?
    #[must_use]
    pub fn is_remote(self) -> bool {
        !matches!(self, Self::Local)
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Free => "free",
            Self::Metered => "metered",
            Self::Unpriced => "unpriced",
        }
    }
}

/// What a session is permitted to spend on, independent of how much.
///
/// Spelled in `config.toml` as `[budget] mode = "no-paid" | "local-only"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpendMode {
    /// The historical behaviour: any model the credentials reach.
    #[default]
    Unrestricted,
    /// No model that moves money. Catalogued-free hosted models and local
    /// runtimes both still run — this forbids SPEND, not network access.
    NoPaid,
    /// Nothing but local inference. Strictly stronger than [`Self::NoPaid`]:
    /// a free hosted model still ships the conversation to somebody else's
    /// machine, which is the thing this mode exists to prevent.
    LocalOnly,
}

impl SpendMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unrestricted => "unrestricted",
            Self::NoPaid => "no-paid",
            Self::LocalOnly => "local-only",
        }
    }

    /// Whether this mode constrains anything at all. Used by call sites that
    /// skip guard construction entirely on the default path.
    #[must_use]
    pub fn is_restrictive(self) -> bool {
        !matches!(self, Self::Unrestricted)
    }

    /// Total order from most permissive to most restrictive. Everything
    /// [`Self::LocalOnly`] admits, [`Self::NoPaid`] admits too, and everything
    /// `NoPaid` admits, `Unrestricted` admits — so the modes really do form a
    /// ladder and "stricter" is well defined.
    fn strictness(self) -> u8 {
        match self {
            Self::Unrestricted => 0,
            Self::NoPaid => 1,
            Self::LocalOnly => 2,
        }
    }

    /// The stricter of two modes.
    ///
    /// This is the merge rule wherever two configuration layers both name a
    /// mode: a repo-local file must never be able to WIDEN a machine-owner's
    /// `local-only` back to `unrestricted`, the same asymmetry
    /// `max_daily_cost_usd` already uses.
    #[must_use]
    pub fn strictest(self, other: Self) -> Self {
        if other.strictness() > self.strictness() {
            other
        } else {
            self
        }
    }
}

/// Everything the guard needs to know about one provider/model pair.
///
/// `blended_usd_per_mtok` exists only to ORDER two models against each other
/// for [`EscalationGate`]; it is never billed and never reported as a price.
/// For `Unpriced` it is meaningless and the gate never reads it — an unpriced
/// target escalates on its billing class alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSpendProfile {
    pub provider: String,
    pub model: String,
    pub billing: ModelBilling,
    pub blended_usd_per_mtok: f64,
}

impl ModelSpendProfile {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        billing: ModelBilling,
        blended_usd_per_mtok: f64,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            billing,
            // A non-finite or negative rate would make every comparison below
            // meaningless in a direction that FAVOURS the escalation. Clamp it
            // to the most expensive reading instead of trusting it.
            blended_usd_per_mtok: if blended_usd_per_mtok.is_finite()
                && blended_usd_per_mtok >= 0.0
            {
                blended_usd_per_mtok
            } else {
                f64::MAX
            },
        }
    }

    /// Stable `provider/model` label used in refusals and audit records.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

/// A dispatch the guard refused. Public and matchable: hosts render these,
/// and the audit record stores them.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(tag = "refusal", rename_all = "snake_case")]
pub enum SpendRefusal {
    /// A paid (metered or unpriced) model under a mode that forbids spend.
    #[error(
        "Provider call not started: [budget] mode is '{mode}', and {target} is billed \
         '{billing}'. No paid model may be dispatched in this mode. Choose a local model \
         (an `ollama:` id) or a catalogued-free one, or change the mode."
    )]
    PaidModel {
        mode: String,
        target: String,
        billing: String,
    },
    /// A hosted model under `local-only`, free or not.
    #[error(
        "Provider call not started: [budget] mode is 'local-only', and {target} is a hosted \
         model. Local-only admits local inference only. Choose a local model (an `ollama:` id), \
         or change the mode."
    )]
    RemoteModel { target: String },
    /// A model change up the price ladder that nobody authorized.
    #[error(
        "Provider call not started: this run is authorized for {authorized} and something \
         moved it to {requested}, which is not cheaper. A model escalation must carry a \
         recorded reason; this one carried none. Authorize the escalation explicitly, or \
         keep the run on its authorized model."
    )]
    SilentEscalation {
        authorized: String,
        requested: String,
    },
}

impl SpendRefusal {
    /// Short machine tag, for the `budget_exceeded`-style event surfaces that
    /// take a `kind` string.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::PaidModel { .. } => "paid_model_refused",
            Self::RemoteModel { .. } => "remote_model_refused",
            Self::SilentEscalation { .. } => "silent_model_escalation",
        }
    }
}

/// The mode half of the guard: does this model belong in this run at all?
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpendPolicy {
    mode: SpendMode,
}

impl SpendPolicy {
    #[must_use]
    pub fn new(mode: SpendMode) -> Self {
        Self { mode }
    }

    #[must_use]
    pub fn mode(&self) -> SpendMode {
        self.mode
    }

    /// Admit one dispatch under the mode.
    ///
    /// `local-only` is checked BEFORE `no-paid` on a remote model so a free
    /// hosted model under `local-only` is refused for the reason that actually
    /// applies to it, rather than passing the paid check and confusing the
    /// operator about which rule bit.
    pub fn admit(&self, profile: &ModelSpendProfile) -> Result<(), SpendRefusal> {
        match self.mode {
            SpendMode::Unrestricted => Ok(()),
            SpendMode::LocalOnly => {
                if profile.billing.is_remote() {
                    Err(SpendRefusal::RemoteModel {
                        target: profile.label(),
                    })
                } else {
                    Ok(())
                }
            }
            SpendMode::NoPaid => {
                if profile.billing.is_paid() {
                    Err(SpendRefusal::PaidModel {
                        mode: self.mode.as_str().to_owned(),
                        target: profile.label(),
                        billing: profile.billing.as_str().to_owned(),
                    })
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// One authorized model escalation, durably recorded.
///
/// Every field is here because a reader six months later needs it to answer
/// "who moved this run onto the expensive model, and why": both endpoints of
/// the move, the reason text, the surface that asked, and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EscalationRecord {
    pub schema_version: u32,
    /// Session the escalation happened in.
    pub session_id: String,
    /// The model the run was authorized for before this record.
    pub from: ModelSpendProfile,
    /// The model it moved to.
    pub to: ModelSpendProfile,
    /// The surface that requested it (`tier_swap`, `switch_model`,
    /// `configured_fallback`, `compaction_model`, `operator`).
    pub source: String,
    /// Free text supplied by that surface. Never empty — an empty reason is
    /// refused at [`EscalationGate::authorize`], because "recorded with no
    /// reason" is the silent escalation this gate exists to stop, wearing a
    /// record as a disguise.
    pub reason: String,
    pub at_unix_ms: u64,
}

/// Current schema of [`EscalationRecord`] and
/// [`crate::spend_audit::SpendAuditRecord`].
pub const SPEND_SCHEMA_VERSION: u32 = 1;

/// Why an authorization was refused before it was ever recorded.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EscalationError {
    /// An escalation whose reason is empty or whitespace. Refused rather than
    /// stored: a blank reason satisfies the letter of "recorded" and none of
    /// its purpose.
    #[error("a model escalation must carry a non-empty reason")]
    EmptyReason,
    /// An escalation whose source label is empty.
    #[error("a model escalation must name the surface that requested it")]
    EmptySource,
}

/// The escalation half of the guard: may this run move to THIS model?
///
/// The gate holds the run's authorized price ceiling. It starts at the
/// baseline model the operator configured, and only [`Self::authorize`] —
/// which demands a reason and emits a durable record — ever raises it.
///
/// Cheaper models are always admitted and never recorded. That is deliberate:
/// the routing tier swap and the cheap compaction model exist precisely to
/// move DOWN the ladder, and a gate that made them ask permission would be
/// turned off within a week.
#[derive(Debug, Clone)]
pub struct EscalationGate {
    session_id: String,
    authorized: ModelSpendProfile,
    /// Every escalation authorized in this run, in order.
    history: Vec<EscalationRecord>,
}

impl EscalationGate {
    #[must_use]
    pub fn new(session_id: impl Into<String>, baseline: ModelSpendProfile) -> Self {
        Self {
            session_id: session_id.into(),
            authorized: baseline,
            history: Vec::new(),
        }
    }

    /// The model this run is currently authorized up to.
    #[must_use]
    pub fn authorized(&self) -> &ModelSpendProfile {
        &self.authorized
    }

    /// Every escalation authorized so far, oldest first.
    #[must_use]
    pub fn history(&self) -> &[EscalationRecord] {
        &self.history
    }

    /// Is moving to `requested` an escalation relative to the authorized
    /// ceiling?
    ///
    /// Four rules, in order:
    /// 1. the same model is never an escalation;
    /// 2. an `Unpriced` target always is — an unknown price cannot be shown to
    ///    be cheaper, and assuming it is, is the exact mistake the pricing
    ///    layer's `priced` flag exists to prevent;
    /// 3. moving from an unpaid baseline (local or catalogued-free) onto ANY
    ///    paid model always is, whatever the rate: $0 to $0.25/Mtok is an
    ///    infinite proportional increase and the run was authorized to spend
    ///    nothing;
    /// 4. otherwise it is an escalation exactly when the blended rate rises.
    #[must_use]
    pub fn is_escalation(&self, requested: &ModelSpendProfile) -> bool {
        if requested.model == self.authorized.model && requested.provider == self.authorized.provider
        {
            return false;
        }
        if requested.billing == ModelBilling::Unpriced {
            return self.authorized.billing != ModelBilling::Unpriced;
        }
        if !self.authorized.billing.is_paid() && requested.billing.is_paid() {
            return true;
        }
        requested.blended_usd_per_mtok > self.authorized.blended_usd_per_mtok
    }

    /// Admit a dispatch on `requested` without authorizing anything.
    ///
    /// This is the call every dispatch site makes. An un-authorized
    /// escalation is refused here; that refusal IS the "silent escalation is
    /// blocked" guarantee.
    pub fn admit(&self, requested: &ModelSpendProfile) -> Result<(), SpendRefusal> {
        if self.is_escalation(requested) {
            return Err(SpendRefusal::SilentEscalation {
                authorized: self.authorized.label(),
                requested: requested.label(),
            });
        }
        Ok(())
    }

    /// Undo the most recent [`Self::authorize`], restoring the ceiling it
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
    /// record that must be persisted.
    ///
    /// Returns the record so the caller can hand it to a
    /// [`crate::spend_audit::SpendAuditSink`]; the gate does no I/O of its own
    /// so it stays usable inside a lock.
    ///
    /// Authorizing a NON-escalation (a downgrade, or the same model) is a
    /// no-op that still returns `Ok(None)`: a caller must not have to
    /// re-implement [`Self::is_escalation`] to know whether to ask.
    pub fn authorize(
        &mut self,
        requested: ModelSpendProfile,
        source: impl Into<String>,
        reason: impl Into<String>,
        at_unix_ms: u64,
    ) -> Result<Option<EscalationRecord>, EscalationError> {
        let source = source.into();
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(EscalationError::EmptyReason);
        }
        if source.trim().is_empty() {
            return Err(EscalationError::EmptySource);
        }
        if !self.is_escalation(&requested) {
            return Ok(None);
        }
        let record = EscalationRecord {
            schema_version: SPEND_SCHEMA_VERSION,
            session_id: self.session_id.clone(),
            from: self.authorized.clone(),
            to: requested.clone(),
            source,
            reason,
            at_unix_ms,
        };
        self.authorized = requested;
        self.history.push(record.clone());
        Ok(Some(record))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metered(model: &str, rate: f64) -> ModelSpendProfile {
        ModelSpendProfile::new("anthropic", model, ModelBilling::Metered, rate)
    }

    fn local(model: &str) -> ModelSpendProfile {
        ModelSpendProfile::new("ollama", model, ModelBilling::Local, 0.0)
    }

    fn free(model: &str) -> ModelSpendProfile {
        ModelSpendProfile::new("flux", model, ModelBilling::Free, 0.0)
    }

    fn unpriced(model: &str) -> ModelSpendProfile {
        ModelSpendProfile::new("flux", model, ModelBilling::Unpriced, 0.0)
    }

    #[test]
    fn no_paid_refuses_metered_and_unpriced_but_admits_free_and_local() {
        let policy = SpendPolicy::new(SpendMode::NoPaid);
        assert!(policy.admit(&metered("sonnet", 6.0)).is_err());
        // The load-bearing half: an UNPRICED model is refused too. Treating an
        // unknown price as $0 is how a router alias walks through this mode.
        assert!(matches!(
            policy.admit(&unpriced("flux-auto")),
            Err(SpendRefusal::PaidModel { .. })
        ));
        assert!(policy.admit(&free("flux-free")).is_ok());
        assert!(policy.admit(&local("qwen3")).is_ok());
    }

    #[test]
    fn local_only_refuses_even_a_free_hosted_model() {
        let policy = SpendPolicy::new(SpendMode::LocalOnly);
        assert!(matches!(
            policy.admit(&free("flux-free")),
            Err(SpendRefusal::RemoteModel { .. })
        ));
        assert!(matches!(
            policy.admit(&metered("sonnet", 6.0)),
            Err(SpendRefusal::RemoteModel { .. })
        ));
        assert!(policy.admit(&local("qwen3")).is_ok());
    }

    #[test]
    fn unrestricted_admits_everything() {
        let policy = SpendPolicy::default();
        assert_eq!(policy.mode(), SpendMode::Unrestricted);
        for profile in [metered("opus", 30.0), unpriced("x"), free("y"), local("z")] {
            assert!(policy.admit(&profile).is_ok());
        }
    }

    #[test]
    fn the_stricter_mode_wins_a_merge_in_both_orders() {
        use SpendMode::{LocalOnly, NoPaid, Unrestricted};
        assert_eq!(Unrestricted.strictest(LocalOnly), LocalOnly);
        assert_eq!(LocalOnly.strictest(Unrestricted), LocalOnly);
        assert_eq!(NoPaid.strictest(LocalOnly), LocalOnly);
        assert_eq!(LocalOnly.strictest(NoPaid), LocalOnly);
        assert_eq!(Unrestricted.strictest(NoPaid), NoPaid);
        assert_eq!(NoPaid.strictest(Unrestricted), NoPaid);
    }

    #[test]
    fn a_cheaper_model_is_never_an_escalation() {
        let gate = EscalationGate::new("s1", metered("opus", 30.0));
        assert!(!gate.is_escalation(&metered("haiku", 1.0)));
        assert!(gate.admit(&metered("haiku", 1.0)).is_ok());
    }

    #[test]
    fn an_unauthorized_price_increase_is_refused() {
        let gate = EscalationGate::new("s1", metered("haiku", 1.0));
        let err = gate.admit(&metered("opus", 30.0)).unwrap_err();
        assert!(matches!(err, SpendRefusal::SilentEscalation { .. }));
        assert_eq!(err.kind(), "silent_model_escalation");
    }

    #[test]
    fn an_unpriced_target_escalates_even_though_its_rate_reads_zero() {
        // `blended_usd_per_mtok` is 0.0 for an unpriced row, so a purely
        // numeric comparison would call this a DOWNGRADE from a $1/Mtok
        // baseline and wave it through. Rule 2 is what stops that.
        let gate = EscalationGate::new("s1", metered("haiku", 1.0));
        assert!(gate.is_escalation(&unpriced("flux-auto")));
        assert!(gate.admit(&unpriced("flux-auto")).is_err());
    }

    #[test]
    fn any_paid_model_escalates_from_a_free_or_local_baseline() {
        for baseline in [local("qwen3"), free("flux-free")] {
            let gate = EscalationGate::new("s1", baseline);
            // Even the cheapest metered model in the catalog: the run was
            // authorized to spend nothing at all.
            assert!(gate.is_escalation(&metered("haiku", 0.25)));
        }
    }

    #[test]
    fn authorizing_records_the_reason_and_raises_the_ceiling() {
        let mut gate = EscalationGate::new("s1", metered("haiku", 1.0));
        let record = gate
            .authorize(metered("opus", 30.0), "operator", "user approved opus", 42)
            .expect("authorization accepted")
            .expect("an escalation produces a record");
        assert_eq!(record.from.model, "haiku");
        assert_eq!(record.to.model, "opus");
        assert_eq!(record.reason, "user approved opus");
        assert_eq!(record.source, "operator");
        assert_eq!(record.at_unix_ms, 42);
        assert_eq!(gate.authorized().model, "opus");
        assert_eq!(gate.history().len(), 1);
        // The same dispatch now passes.
        assert!(gate.admit(&metered("opus", 30.0)).is_ok());
    }

    #[test]
    fn a_blank_reason_is_refused_rather_than_recorded() {
        let mut gate = EscalationGate::new("s1", metered("haiku", 1.0));
        assert_eq!(
            gate.authorize(metered("opus", 30.0), "operator", "   ", 1),
            Err(EscalationError::EmptyReason)
        );
        assert_eq!(
            gate.authorize(metered("opus", 30.0), "  ", "because", 1),
            Err(EscalationError::EmptySource)
        );
        // The refusal must not have moved the ceiling.
        assert_eq!(gate.authorized().model, "haiku");
        assert!(gate.history().is_empty());
        assert!(gate.admit(&metered("opus", 30.0)).is_err());
    }

    #[test]
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
    fn authorizing_a_downgrade_records_nothing() {
        let mut gate = EscalationGate::new("s1", metered("opus", 30.0));
        assert_eq!(
            gate.authorize(metered("haiku", 1.0), "tier_swap", "cheap tier", 1),
            Ok(None)
        );
        assert_eq!(gate.authorized().model, "opus");
        assert!(gate.history().is_empty());
    }

    #[test]
    fn a_nonfinite_rate_is_clamped_to_the_most_expensive_reading() {
        // A NaN rate compares false against everything, so an unclamped NaN
        // would make `requested > authorized` false and admit the swap.
        let gate = EscalationGate::new("s1", metered("haiku", 1.0));
        assert!(gate.is_escalation(&metered("nan-model", f64::NAN)));
        assert!(gate.is_escalation(&metered("negative-model", -5.0)));
    }
}
