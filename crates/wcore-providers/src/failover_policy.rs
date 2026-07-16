//! Provider-neutral admission policy for semantic failover candidates.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use wcore_types::llm::LlmRequest;
use wcore_types::message::ContentBlock;

use crate::FailoverReason;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CandidateCapabilities {
    pub tools: bool,
    pub vision: bool,
    pub structured_output: bool,
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricingEvidence {
    pub source: String,
    pub age_seconds: Option<u64>,
    pub stale: bool,
    pub priced: bool,
    pub estimated_microcents: Option<u64>,
}

impl Default for PricingEvidence {
    fn default() -> Self {
        Self {
            source: "unknown".into(),
            age_seconds: None,
            stale: true,
            priced: false,
            estimated_microcents: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailoverCandidateMetadata {
    pub label: String,
    pub provider: String,
    pub model: String,
    pub organization: Option<String>,
    pub region: Option<String>,
    pub capabilities: CandidateCapabilities,
    pub pricing: PricingEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FailoverRoutingPolicy {
    pub allowed_providers: BTreeSet<String>,
    pub denied_providers: BTreeSet<String>,
    pub allowed_regions: BTreeSet<String>,
    pub organization: Option<String>,
    pub require_fresh_pricing: bool,
    pub require_priced: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestRequirements {
    pub context_tokens: Option<u64>,
    pub tools: bool,
    pub vision: bool,
    pub structured_output: bool,
}

impl RequestRequirements {
    pub fn from_request(request: &LlmRequest) -> Self {
        let vision = request.messages.iter().any(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Image { .. }))
        });
        Self {
            context_tokens: request
                .client_context_tokens
                .map(|input| input.saturating_add(u64::from(request.max_tokens))),
            tools: !request.tools.is_empty(),
            vision,
            // LlmRequest has no response-schema field today. This remains a
            // distinct requirement so the future field has one admission seam
            // rather than another provider-specific branch.
            structured_output: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateRejection {
    ProviderNotAllowed,
    ProviderDenied,
    RegionNotAllowed,
    OrganizationMismatch,
    ToolsUnsupported,
    VisionUnsupported,
    StructuredOutputUnsupported,
    ContextWindowUnknown,
    ContextWindowTooSmall,
    PricingStale,
    PricingUnavailable,
    CooldownActive,
    BudgetDenied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateReceipt {
    pub provider: String,
    pub model: String,
    pub region: Option<String>,
    pub disposition: Result<(), CandidateRejection>,
    /// Set only when an admitted candidate was physically attempted and the
    /// provider rejected or failed the request.
    pub failure_reason: Option<FailoverReason>,
    /// Reason an otherwise compatible candidate is currently cooling. This is
    /// distinct from `failure_reason` because no new provider call occurred.
    pub cooldown_reason: Option<FailoverReason>,
    pub retry_after_ms: Option<u64>,
    pub pricing: PricingEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailoverReceipt {
    pub reason: FailoverReason,
    pub failed_provider: String,
    pub failed_model: String,
    pub candidates: Vec<CandidateReceipt>,
    pub selected_provider: Option<String>,
    pub selected_model: Option<String>,
}

impl FailoverReceipt {
    pub fn new(
        reason: FailoverReason,
        failed_provider: impl Into<String>,
        failed_model: impl Into<String>,
    ) -> Self {
        Self {
            reason,
            failed_provider: failed_provider.into(),
            failed_model: failed_model.into(),
            candidates: Vec::new(),
            selected_provider: None,
            selected_model: None,
        }
    }
}

pub fn evaluate_candidate(
    candidate: &FailoverCandidateMetadata,
    requirements: RequestRequirements,
    policy: &FailoverRoutingPolicy,
) -> Result<(), CandidateRejection> {
    if !policy.allowed_providers.is_empty()
        && !policy.allowed_providers.contains(&candidate.provider)
    {
        return Err(CandidateRejection::ProviderNotAllowed);
    }
    if policy.denied_providers.contains(&candidate.provider) {
        return Err(CandidateRejection::ProviderDenied);
    }
    if !policy.allowed_regions.is_empty()
        && candidate
            .region
            .as_ref()
            .is_none_or(|region| !policy.allowed_regions.contains(region))
    {
        return Err(CandidateRejection::RegionNotAllowed);
    }
    if let Some(required) = policy.organization.as_deref()
        && candidate.organization.as_deref() != Some(required)
    {
        return Err(CandidateRejection::OrganizationMismatch);
    }
    if requirements.tools && !candidate.capabilities.tools {
        return Err(CandidateRejection::ToolsUnsupported);
    }
    if requirements.vision && !candidate.capabilities.vision {
        return Err(CandidateRejection::VisionUnsupported);
    }
    if requirements.structured_output && !candidate.capabilities.structured_output {
        return Err(CandidateRejection::StructuredOutputUnsupported);
    }
    if let Some(required) = requirements.context_tokens {
        let Some(window) = candidate.capabilities.context_window else {
            return Err(CandidateRejection::ContextWindowUnknown);
        };
        if window < required {
            return Err(CandidateRejection::ContextWindowTooSmall);
        }
    }
    if policy.require_fresh_pricing && candidate.pricing.stale {
        return Err(CandidateRejection::PricingStale);
    }
    if policy.require_priced && !candidate.pricing.priced {
        return Err(CandidateRejection::PricingUnavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> FailoverCandidateMetadata {
        FailoverCandidateMetadata {
            label: "openai:gpt-5".into(),
            provider: "openai".into(),
            model: "gpt-5".into(),
            organization: Some("acme".into()),
            region: Some("us-east".into()),
            capabilities: CandidateCapabilities {
                tools: true,
                vision: true,
                structured_output: true,
                context_window: Some(400_000),
            },
            pricing: PricingEvidence {
                source: "bundled".into(),
                age_seconds: Some(0),
                stale: false,
                priced: true,
                estimated_microcents: Some(10),
            },
        }
    }

    #[test]
    fn rejects_each_incompatible_requirement() {
        let policy = FailoverRoutingPolicy::default();
        let mut candidate = candidate();
        candidate.capabilities.tools = false;
        assert_eq!(
            evaluate_candidate(
                &candidate,
                RequestRequirements {
                    context_tokens: None,
                    tools: true,
                    vision: false,
                    structured_output: false,
                },
                &policy,
            ),
            Err(CandidateRejection::ToolsUnsupported)
        );

        candidate.capabilities.tools = true;
        candidate.capabilities.vision = false;
        assert_eq!(
            evaluate_candidate(
                &candidate,
                RequestRequirements {
                    context_tokens: None,
                    tools: false,
                    vision: true,
                    structured_output: false,
                },
                &policy,
            ),
            Err(CandidateRejection::VisionUnsupported)
        );

        candidate.capabilities.vision = true;
        candidate.capabilities.structured_output = false;
        assert_eq!(
            evaluate_candidate(
                &candidate,
                RequestRequirements {
                    context_tokens: None,
                    tools: false,
                    vision: false,
                    structured_output: true,
                },
                &policy,
            ),
            Err(CandidateRejection::StructuredOutputUnsupported)
        );
    }

    #[test]
    fn rejects_unknown_or_too_small_context() {
        let policy = FailoverRoutingPolicy::default();
        let mut candidate = candidate();
        candidate.capabilities.context_window = None;
        let requirements = RequestRequirements {
            context_tokens: Some(250_000),
            tools: false,
            vision: false,
            structured_output: false,
        };
        assert_eq!(
            evaluate_candidate(&candidate, requirements, &policy),
            Err(CandidateRejection::ContextWindowUnknown)
        );
        candidate.capabilities.context_window = Some(200_000);
        assert_eq!(
            evaluate_candidate(&candidate, requirements, &policy),
            Err(CandidateRejection::ContextWindowTooSmall)
        );
    }

    #[test]
    fn trusted_policy_checks_provider_region_org_and_price() {
        let candidate = candidate();
        let mut policy = FailoverRoutingPolicy {
            allowed_providers: BTreeSet::from(["anthropic".into()]),
            ..Default::default()
        };
        let requirements = RequestRequirements {
            context_tokens: None,
            tools: false,
            vision: false,
            structured_output: false,
        };
        assert_eq!(
            evaluate_candidate(&candidate, requirements, &policy),
            Err(CandidateRejection::ProviderNotAllowed)
        );
        policy.allowed_providers = BTreeSet::from(["openai".into()]);
        policy.allowed_regions = BTreeSet::from(["eu-west".into()]);
        assert_eq!(
            evaluate_candidate(&candidate, requirements, &policy),
            Err(CandidateRejection::RegionNotAllowed)
        );
        policy.allowed_regions = BTreeSet::from(["us-east".into()]);
        policy.organization = Some("other".into());
        assert_eq!(
            evaluate_candidate(&candidate, requirements, &policy),
            Err(CandidateRejection::OrganizationMismatch)
        );
        policy.organization = Some("acme".into());
        policy.require_fresh_pricing = true;
        let mut stale = candidate;
        stale.pricing.stale = true;
        assert_eq!(
            evaluate_candidate(&stale, requirements, &policy),
            Err(CandidateRejection::PricingStale)
        );
    }

    #[test]
    fn compatible_candidate_passes() {
        let candidate = candidate();
        let policy = FailoverRoutingPolicy {
            allowed_providers: BTreeSet::from(["openai".into()]),
            allowed_regions: BTreeSet::from(["us-east".into()]),
            organization: Some("acme".into()),
            require_fresh_pricing: true,
            require_priced: true,
            ..Default::default()
        };
        assert_eq!(
            evaluate_candidate(
                &candidate,
                RequestRequirements {
                    context_tokens: Some(300_000),
                    tools: true,
                    vision: true,
                    structured_output: true,
                },
                &policy,
            ),
            Ok(())
        );
    }
}
