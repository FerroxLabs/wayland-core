//! Shared execution-policy vocabulary.
//!
//! Effective policy is output-only. Lower-trust inputs deserialize into
//! [`ExecutionPolicyRequest`] and must pass through the policy resolver; they
//! cannot deserialize a sandbox-bypass grant.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use thiserror::Error;

pub const DEFAULT_DANGEROUS_SESSION_TTL_SECS: u64 = 15 * 60;
pub const MAX_DANGEROUS_SESSION_TTL_SECS: u64 = 60 * 60;

/// User-facing posture after applying an optional managed floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPosture {
    Smart,
    Managed,
    Dangerous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    Prompt,
    AutoEdit,
    Bypass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPolicy {
    Required,
    Bypass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicySource {
    Default,
    Managed,
    UserConfig,
    Project,
    Environment,
    LocalCliLaunch,
    DesktopLocalLaunch,
    Protocol,
    Acp,
    Tui,
    Resume,
    Child,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedDangerousPolicy {
    Allow,
    Deny,
}

/// The only policy shape accepted from lower-trust serialized inputs.
///
/// Dangerous and Managed are intentionally absent. A wire peer can request a
/// Smart approval posture, but it cannot mint a managed floor or sandbox lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPolicyRequest {
    #[serde(default)]
    pub approvals: Option<ApprovalPolicyRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicyRequest {
    Prompt,
    AutoEdit,
    Bypass,
}

/// Baseline policy for a session. Managed is represented as an overlay so a
/// managed organization can allow or deny a temporary Dangerous launch without
/// losing the fact that its floor remains active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BaselineExecutionPolicy {
    posture: ExecutionPosture,
    approvals: ApprovalPolicy,
    sandbox: SandboxPolicy,
    source: PolicySource,
    managed_dangerous: Option<ManagedDangerousPolicy>,
}

impl BaselineExecutionPolicy {
    pub const fn smart(approvals: ApprovalPolicy, source: PolicySource) -> Self {
        Self {
            posture: ExecutionPosture::Smart,
            approvals,
            sandbox: SandboxPolicy::Required,
            source,
            managed_dangerous: None,
        }
    }

    pub const fn managed(approvals: ApprovalPolicy, dangerous: ManagedDangerousPolicy) -> Self {
        Self {
            posture: ExecutionPosture::Managed,
            approvals,
            sandbox: SandboxPolicy::Required,
            source: PolicySource::Managed,
            managed_dangerous: Some(dangerous),
        }
    }

    pub const fn posture(&self) -> ExecutionPosture {
        self.posture
    }

    pub const fn approvals(&self) -> ApprovalPolicy {
        self.approvals
    }

    pub const fn sandbox(&self) -> SandboxPolicy {
        self.sandbox
    }

    pub const fn source(&self) -> PolicySource {
        self.source
    }

    pub const fn is_managed(&self) -> bool {
        self.managed_dangerous.is_some()
    }
}

/// Explicit local process-launch request. This is never deserialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DangerousLaunchRequest {
    source: PolicySource,
    ttl_secs: u64,
    activation_id: String,
}

impl DangerousLaunchRequest {
    pub fn cli(ttl_secs: u64, activation_id: impl Into<String>) -> Self {
        Self {
            source: PolicySource::LocalCliLaunch,
            ttl_secs,
            activation_id: activation_id.into(),
        }
    }

    pub fn desktop(ttl_secs: u64, activation_id: impl Into<String>) -> Self {
        Self {
            source: PolicySource::DesktopLocalLaunch,
            ttl_secs,
            activation_id: activation_id.into(),
        }
    }

    #[cfg(test)]
    fn from_untrusted_source(
        source: PolicySource,
        ttl_secs: u64,
        activation_id: impl Into<String>,
    ) -> Self {
        Self {
            source,
            ttl_secs,
            activation_id: activation_id.into(),
        }
    }
}

/// Resolver-produced lease metadata. Runtime code must enforce `ttl_millis`
/// with a monotonic clock. Unix time is display/audit metadata only.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct DangerousSessionGrant {
    source: PolicySource,
    activation_id: String,
    ttl_millis: u64,
    audit_expires_at_unix_ms: u64,
    managed_floor_active: bool,
    #[serde(skip)]
    monotonic_deadline: Instant,
}

impl DangerousSessionGrant {
    pub const fn source(&self) -> PolicySource {
        self.source
    }

    pub fn activation_id(&self) -> &str {
        &self.activation_id
    }

    pub const fn ttl_millis(&self) -> u64 {
        self.ttl_millis
    }

    pub const fn audit_expires_at_unix_ms(&self) -> u64 {
        self.audit_expires_at_unix_ms
    }

    pub const fn managed_floor_active(&self) -> bool {
        self.managed_floor_active
    }

    /// Remaining authority according to the monotonic clock captured when the
    /// trusted resolver created this one-shot grant.
    pub fn remaining_ttl(&self) -> Option<Duration> {
        self.monotonic_deadline
            .checked_duration_since(Instant::now())
    }
}

/// Canonical output snapshot consumed by hosts, audit sinks and enforcement.
///
/// It deliberately does not implement `Deserialize`; callers can only obtain
/// Dangerous through a resolver-produced [`DangerousSessionGrant`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectiveExecutionPolicy {
    posture: ExecutionPosture,
    approvals: ApprovalPolicy,
    sandbox: SandboxPolicy,
    source: PolicySource,
    managed_floor_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    dangerous_activation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dangerous_expires_at_unix_ms: Option<u64>,
}

impl EffectiveExecutionPolicy {
    pub fn baseline(policy: &BaselineExecutionPolicy) -> Self {
        Self {
            posture: policy.posture,
            approvals: policy.approvals,
            sandbox: SandboxPolicy::Required,
            source: policy.source,
            managed_floor_active: policy.is_managed(),
            dangerous_activation_id: None,
            dangerous_expires_at_unix_ms: None,
        }
    }

    pub fn dangerous(grant: &DangerousSessionGrant) -> Self {
        Self {
            posture: ExecutionPosture::Dangerous,
            approvals: ApprovalPolicy::Bypass,
            sandbox: SandboxPolicy::Bypass,
            source: grant.source,
            managed_floor_active: grant.managed_floor_active,
            dangerous_activation_id: Some(grant.activation_id.clone()),
            dangerous_expires_at_unix_ms: Some(grant.audit_expires_at_unix_ms),
        }
    }

    pub const fn posture(&self) -> ExecutionPosture {
        self.posture
    }

    pub const fn approvals(&self) -> ApprovalPolicy {
        self.approvals
    }

    pub const fn sandbox(&self) -> SandboxPolicy {
        self.sandbox
    }

    pub const fn source(&self) -> PolicySource {
        self.source
    }

    pub const fn managed_floor_active(&self) -> bool {
        self.managed_floor_active
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExecutionPolicyError {
    #[error("dangerous sessions are disabled by managed policy")]
    DangerousDeniedByManagedPolicy,
    #[error("dangerous sessions require an explicit local process launch")]
    DangerousRequiresLocalLaunch,
    #[error("dangerous session activation ID must not be empty")]
    DangerousActivationIdEmpty,
    #[error("dangerous session TTL must be between 1 and {MAX_DANGEROUS_SESSION_TTL_SECS} seconds")]
    DangerousTtlOutOfRange,
    #[error("dangerous session expiry overflowed")]
    DangerousExpiryOverflow,
    #[error("dangerous session grant has expired")]
    DangerousGrantExpired,
}

/// Resolve a local Dangerous request against the immutable session baseline.
///
/// Resolution binds the grant to a monotonic deadline. Runtime enforcement
/// must arm the remaining duration and terminalize the session at expiry; the
/// Unix timestamp is audit metadata only.
pub fn resolve_dangerous_launch(
    baseline: &BaselineExecutionPolicy,
    request: DangerousLaunchRequest,
    audit_now_unix_ms: u64,
) -> Result<DangerousSessionGrant, ExecutionPolicyError> {
    resolve_dangerous_launch_at(baseline, request, audit_now_unix_ms, Instant::now())
}

fn resolve_dangerous_launch_at(
    baseline: &BaselineExecutionPolicy,
    request: DangerousLaunchRequest,
    audit_now_unix_ms: u64,
    monotonic_now: Instant,
) -> Result<DangerousSessionGrant, ExecutionPolicyError> {
    if matches!(
        baseline.managed_dangerous,
        Some(ManagedDangerousPolicy::Deny)
    ) {
        return Err(ExecutionPolicyError::DangerousDeniedByManagedPolicy);
    }
    if !matches!(
        request.source,
        PolicySource::LocalCliLaunch | PolicySource::DesktopLocalLaunch
    ) {
        return Err(ExecutionPolicyError::DangerousRequiresLocalLaunch);
    }
    if request.activation_id.trim().is_empty() {
        return Err(ExecutionPolicyError::DangerousActivationIdEmpty);
    }
    if request.ttl_secs == 0 || request.ttl_secs > MAX_DANGEROUS_SESSION_TTL_SECS {
        return Err(ExecutionPolicyError::DangerousTtlOutOfRange);
    }

    let ttl_millis = request
        .ttl_secs
        .checked_mul(1_000)
        .ok_or(ExecutionPolicyError::DangerousExpiryOverflow)?;
    let audit_expires_at_unix_ms = audit_now_unix_ms
        .checked_add(ttl_millis)
        .ok_or(ExecutionPolicyError::DangerousExpiryOverflow)?;
    let monotonic_deadline = monotonic_now
        .checked_add(Duration::from_millis(ttl_millis))
        .ok_or(ExecutionPolicyError::DangerousExpiryOverflow)?;

    Ok(DangerousSessionGrant {
        source: request.source,
        activation_id: request.activation_id,
        ttl_millis,
        audit_expires_at_unix_ms,
        managed_floor_active: baseline.is_managed(),
        monotonic_deadline,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smart() -> BaselineExecutionPolicy {
        BaselineExecutionPolicy::smart(ApprovalPolicy::Prompt, PolicySource::Default)
    }

    #[test]
    fn force_equivalent_bypasses_approval_but_retains_sandbox() {
        let policy =
            BaselineExecutionPolicy::smart(ApprovalPolicy::Bypass, PolicySource::LocalCliLaunch);
        let effective = EffectiveExecutionPolicy::baseline(&policy);

        assert_eq!(effective.posture(), ExecutionPosture::Smart);
        assert_eq!(effective.approvals(), ApprovalPolicy::Bypass);
        assert_eq!(effective.sandbox(), SandboxPolicy::Required);
    }

    #[test]
    fn serialized_input_cannot_request_dangerous_or_managed() {
        for value in [
            serde_json::json!({"posture": "dangerous"}),
            serde_json::json!({"posture": "managed"}),
            serde_json::json!({"sandbox": "bypass"}),
        ] {
            assert!(serde_json::from_value::<ExecutionPolicyRequest>(value).is_err());
        }
    }

    #[test]
    fn only_explicit_local_launch_can_resolve_dangerous() {
        for source in [
            PolicySource::Default,
            PolicySource::Managed,
            PolicySource::UserConfig,
            PolicySource::Project,
            PolicySource::Environment,
            PolicySource::Protocol,
            PolicySource::Acp,
            PolicySource::Tui,
            PolicySource::Resume,
            PolicySource::Child,
        ] {
            let request = DangerousLaunchRequest::from_untrusted_source(source, 60, "attack");
            assert_eq!(
                resolve_dangerous_launch(&smart(), request, 10_000),
                Err(ExecutionPolicyError::DangerousRequiresLocalLaunch),
                "source {source:?} activated dangerous mode"
            );
        }
    }

    #[test]
    fn managed_deny_wins_before_source_validation() {
        let baseline =
            BaselineExecutionPolicy::managed(ApprovalPolicy::Prompt, ManagedDangerousPolicy::Deny);
        let request =
            DangerousLaunchRequest::from_untrusted_source(PolicySource::Protocol, 60, "attack");

        assert_eq!(
            resolve_dangerous_launch(&baseline, request, 10_000),
            Err(ExecutionPolicyError::DangerousDeniedByManagedPolicy)
        );
    }

    #[test]
    fn managed_allow_does_not_activate_without_an_explicit_request() {
        let baseline = BaselineExecutionPolicy::managed(
            ApprovalPolicy::AutoEdit,
            ManagedDangerousPolicy::Allow,
        );
        let effective = EffectiveExecutionPolicy::baseline(&baseline);

        assert_eq!(effective.posture(), ExecutionPosture::Managed);
        assert_eq!(effective.sandbox(), SandboxPolicy::Required);
        assert!(effective.managed_floor_active());
    }

    #[test]
    fn local_dangerous_grant_retains_managed_floor_provenance() {
        let baseline =
            BaselineExecutionPolicy::managed(ApprovalPolicy::Prompt, ManagedDangerousPolicy::Allow);
        let grant = resolve_dangerous_launch(
            &baseline,
            DangerousLaunchRequest::desktop(60, "desktop-123"),
            10_000,
        )
        .unwrap();
        let effective = EffectiveExecutionPolicy::dangerous(&grant);

        assert_eq!(effective.posture(), ExecutionPosture::Dangerous);
        assert_eq!(effective.approvals(), ApprovalPolicy::Bypass);
        assert_eq!(effective.sandbox(), SandboxPolicy::Bypass);
        assert!(effective.managed_floor_active());
    }

    #[test]
    fn dangerous_ttl_and_activation_id_are_checked() {
        for ttl_secs in [0, MAX_DANGEROUS_SESSION_TTL_SECS + 1] {
            assert_eq!(
                resolve_dangerous_launch(
                    &smart(),
                    DangerousLaunchRequest::cli(ttl_secs, "cli-123"),
                    0,
                ),
                Err(ExecutionPolicyError::DangerousTtlOutOfRange)
            );
        }
        assert_eq!(
            resolve_dangerous_launch(&smart(), DangerousLaunchRequest::cli(1, "  "), 0,),
            Err(ExecutionPolicyError::DangerousActivationIdEmpty)
        );
        assert_eq!(
            resolve_dangerous_launch(
                &smart(),
                DangerousLaunchRequest::cli(1, "cli-123"),
                u64::MAX,
            ),
            Err(ExecutionPolicyError::DangerousExpiryOverflow)
        );
    }

    #[test]
    fn stale_dangerous_grant_has_no_remaining_authority() {
        let now = Instant::now();
        let issued_at = now
            .checked_sub(Duration::from_secs(2))
            .expect("test instant must support a two-second subtraction");
        let grant = resolve_dangerous_launch_at(
            &smart(),
            DangerousLaunchRequest::cli(1, "one-shot"),
            0,
            issued_at,
        )
        .expect("historical resolution itself is valid");

        assert_eq!(grant.remaining_ttl(), None);
    }

    #[test]
    fn policy_snapshot_wire_names_are_stable() {
        let baseline = BaselineExecutionPolicy::managed(
            ApprovalPolicy::AutoEdit,
            ManagedDangerousPolicy::Deny,
        );
        let policy = EffectiveExecutionPolicy::baseline(&baseline);

        assert_eq!(
            serde_json::to_value(policy).unwrap(),
            serde_json::json!({
                "posture": "managed",
                "approvals": "auto_edit",
                "sandbox": "required",
                "source": "managed",
                "managed_floor_active": true
            })
        );
    }
}
