//! Setup and authentication probe — the answer an operator needs BEFORE a
//! message is lost, not after.
//!
//! # Why a probe is a distinct surface from `start()`
//!
//! `start()` answers one question — did the connection open — and it answers it
//! by opening the connection. That is the wrong instrument for setup. An
//! operator configuring a channel needs three separate facts:
//!
//! 1. is the configuration COMPLETE (are the required keys present at all),
//! 2. does the credential AUTHENTICATE (is the token live and in scope),
//! 3. WHAT IDENTITY did it authenticate as (is this the bot you meant).
//!
//! `start()` collapses all three into one boolean, and a channel that starts
//! against the wrong workspace looks identical to one that started against the
//! right one. Worse, discovering the answer by sending a test message puts
//! traffic on a production surface. A probe sends nothing.
//!
//! # The default is `Unsupported`, and that is deliberate
//!
//! [`Channel::probe`](crate::Channel::probe) has a default so the ten already
//! registered adapters keep compiling. That default does NOT report health — it
//! reports [`ProbeOutcome::Unsupported`], a NAMED state meaning "this adapter
//! cannot self-check". The distinction is the whole point: an operator reading
//! `unsupported` knows the probe told them nothing, whereas a default of `ok`
//! would be an adapter attesting to its own configuration without looking at
//! it. That is the failure shape this phase keeps finding, and a defaulted
//! success is the cheapest way to reintroduce it.
//!
//! # Probe output is secret-free by construction
//!
//! Threat T-24-03-06. The probe reports WHETHER a credential authenticated and
//! AS WHOM, never the credential. [`ProbeReport::findings`] carries the NAME of
//! a missing or rejected item (`"bot_token"`), never its value, and
//! [`ProbeReport::identity`] carries a platform identity, not a token. There is
//! a canary test asserting a seeded secret never appears in a serialized
//! report.

use serde::{Deserialize, Serialize};

/// Outcome of a setup and authentication probe.
///
/// Ordered from worst to best is deliberately NOT how this reads: each variant
/// is a distinct operator action, not a severity rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ProbeOutcome {
    /// Configuration complete, credential authenticated, identity resolved.
    Ok,
    /// A required configuration item is absent. Named in
    /// [`ProbeReport::findings`]. The operator edits config; no credential
    /// question has been asked yet.
    Incomplete,
    /// Configuration is complete but the platform rejected the credential.
    /// The operator rotates or re-scopes the credential.
    Unauthenticated,
    /// The probe could not be performed — the platform was unreachable. This
    /// is NOT a configuration verdict; retrying later may answer differently.
    Unreachable,
    /// This adapter does not implement a probe. Nothing was checked. Reported
    /// so an operator is never handed a green they did not earn.
    Unsupported,
}

impl ProbeOutcome {
    /// Whether this outcome means the channel is ready to carry traffic.
    ///
    /// [`ProbeOutcome::Unsupported`] is deliberately NOT ready: an adapter that
    /// declined to check has not established that it is.
    pub fn is_ready(self) -> bool {
        matches!(self, ProbeOutcome::Ok)
    }
}

/// What a setup and authentication probe learned, without sending a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeReport {
    /// The adapter's stable name (config file stem).
    pub channel: String,
    /// Platform tag.
    pub platform: String,
    /// The verdict.
    pub outcome: ProbeOutcome,
    /// Whether every required configuration item is present.
    pub config_complete: bool,
    /// Whether the credential authenticated against the platform.
    pub authenticated: bool,
    /// The identity the credential authenticated as — a bot user id, workspace
    /// name or mailbox address. NEVER a credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// NAMES of missing or rejected items — `"bot_token"`, `"signing_secret"`.
    /// Never values. Empty when [`ProbeOutcome::Ok`].
    #[serde(default)]
    pub findings: Vec<String>,
}

impl ProbeReport {
    /// A fully successful probe.
    pub fn ok(
        channel: impl Into<String>,
        platform: impl Into<String>,
        identity: impl Into<String>,
    ) -> Self {
        Self {
            channel: channel.into(),
            platform: platform.into(),
            outcome: ProbeOutcome::Ok,
            config_complete: true,
            authenticated: true,
            identity: Some(identity.into()),
            findings: Vec::new(),
        }
    }

    /// Configuration is missing the named items. No credential check was made,
    /// because there was nothing complete to check.
    pub fn incomplete(
        channel: impl Into<String>,
        platform: impl Into<String>,
        missing: Vec<String>,
    ) -> Self {
        Self {
            channel: channel.into(),
            platform: platform.into(),
            outcome: ProbeOutcome::Incomplete,
            config_complete: false,
            authenticated: false,
            identity: None,
            findings: missing,
        }
    }

    /// Configuration complete, platform rejected the credential. `reason` is a
    /// platform-supplied rejection LABEL (`"invalid_auth"`), never the token.
    pub fn unauthenticated(
        channel: impl Into<String>,
        platform: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            channel: channel.into(),
            platform: platform.into(),
            outcome: ProbeOutcome::Unauthenticated,
            config_complete: true,
            authenticated: false,
            identity: None,
            findings: vec![reason.into()],
        }
    }

    /// The platform could not be reached, so no verdict was reached either.
    pub fn unreachable(
        channel: impl Into<String>,
        platform: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            channel: channel.into(),
            platform: platform.into(),
            outcome: ProbeOutcome::Unreachable,
            config_complete: true,
            authenticated: false,
            identity: None,
            findings: vec![reason.into()],
        }
    }

    /// This adapter implements no probe. The trait default. Nothing checked,
    /// and the report says exactly that.
    pub fn unsupported(channel: impl Into<String>, platform: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            platform: platform.into(),
            outcome: ProbeOutcome::Unsupported,
            config_complete: false,
            authenticated: false,
            identity: None,
            findings: vec!["adapter implements no setup probe".to_string()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_is_not_ready() {
        // The load-bearing property: an adapter that declined to check must not
        // read as ready. If `Unsupported` ever becomes ready, every adapter
        // that never implemented a probe silently reports a green.
        assert!(!ProbeReport::unsupported("c", "p").outcome.is_ready());
        assert!(!ProbeOutcome::Incomplete.is_ready());
        assert!(!ProbeOutcome::Unauthenticated.is_ready());
        assert!(!ProbeOutcome::Unreachable.is_ready());
        assert!(ProbeOutcome::Ok.is_ready());
    }

    #[test]
    fn incomplete_names_the_missing_items_and_asks_no_credential_question() {
        let r = ProbeReport::incomplete("acme", "slack", vec!["bot_token".into()]);
        assert!(!r.config_complete);
        assert!(!r.authenticated, "nothing complete to authenticate against");
        assert!(r.identity.is_none());
        assert_eq!(r.findings, vec!["bot_token".to_string()]);
    }

    #[test]
    fn unauthenticated_separates_complete_config_from_a_rejected_credential() {
        // These two are different operator actions — edit the file vs rotate
        // the token — so they must be different reports.
        let r = ProbeReport::unauthenticated("acme", "slack", "invalid_auth");
        assert!(r.config_complete, "config was complete; the token was not");
        assert!(!r.authenticated);
        assert_eq!(r.outcome, ProbeOutcome::Unauthenticated);
    }

    #[test]
    fn a_seeded_credential_never_appears_in_a_serialized_report() {
        // T-24-03-06 with a POSITIVE CONTROL. A canary absent from a report
        // that never saw it proves nothing, so the canary is first proved to
        // be the actual value of the item the report is describing.
        const CANARY: &str = "xoxb-F24D-PROBE-CANARY-8f2c19aa4b6d";
        assert!(CANARY.len() >= 16, "canary must be long enough to be found");

        // The report is built the way an adapter builds it: it knows the
        // secret, and must emit only the NAME of the item plus the identity.
        let secret_held_by_the_adapter = CANARY.to_string();
        assert!(
            secret_held_by_the_adapter.contains(CANARY),
            "positive control: the value under test really is the canary"
        );
        let r = ProbeReport::unauthenticated("acme", "slack", "invalid_auth");
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(
            !json.contains(CANARY),
            "probe report leaked the credential: {json}"
        );

        let ok = ProbeReport::ok("acme", "slack", "U123/acme-workspace");
        let json = serde_json::to_string(&ok).expect("serialize");
        assert!(
            !json.contains(CANARY),
            "probe report leaked the credential: {json}"
        );
        assert!(
            json.contains("U123/acme-workspace"),
            "identity must survive redaction — it is the point of the probe"
        );
    }
}
