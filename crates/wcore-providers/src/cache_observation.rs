//! Cache retention forensics — ported from openclaw MIT (c) Peter Steinberger 2025.
//!
//! Tracks WHY a prompt cache hit / miss / invalidation happened so observability
//! can correlate cost spikes with workload patterns. Sits adjacent to cache_tier
//! (T1-C1) which decides the 5m vs 1h TTL — this module captures what actually
//! happened on the wire.

use serde::{Deserialize, Serialize};

/// Retention policy for cached prompt prefixes (matches openclaw's
/// ContextEnginePromptCacheRetention shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheRetention {
    /// Anthropic 5-minute ephemeral cache.
    Ephemeral5m,
    /// Anthropic 1-hour ephemeral cache.
    Ephemeral1h,
    /// No retention requested (cache disabled or message didn't qualify).
    None,
}

impl CacheRetention {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ephemeral5m => "5m",
            Self::Ephemeral1h => "1h",
            Self::None => "none",
        }
    }

    pub fn ttl_seconds(&self) -> Option<u64> {
        match self {
            Self::Ephemeral5m => Some(5 * 60),
            Self::Ephemeral1h => Some(60 * 60),
            Self::None => None,
        }
    }
}

impl std::fmt::Display for CacheRetention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why an invalidation happened — captured at adapter level for observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidationCause {
    /// New system prompt didn't match the cached prefix.
    SystemPromptDrift,
    /// Tool definitions changed (new tool, removed tool, schema delta).
    ToolDefinitionsChanged,
    /// Message history was rewritten (e.g. compaction).
    HistoryRewritten,
    /// Cache TTL expired before next call.
    Expired,
    /// Provider rejected the cache control marker (e.g. token count too low).
    ProviderRejected,
    /// No explicit cache marker was emitted on this turn.
    NoMarker,
    /// Cause could not be determined.
    Unknown,
}

impl InvalidationCause {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SystemPromptDrift => "system_prompt_drift",
            Self::ToolDefinitionsChanged => "tool_definitions_changed",
            Self::HistoryRewritten => "history_rewritten",
            Self::Expired => "expired",
            Self::ProviderRejected => "provider_rejected",
            Self::NoMarker => "no_marker",
            Self::Unknown => "unknown",
        }
    }
}

/// Observation of a prompt cache event — one per turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptCacheObservation {
    /// Retention policy requested for this turn.
    pub retention: CacheRetention,
    /// Cache-read input tokens (provider-reported, 0 if no read).
    pub read_input_tokens: u64,
    /// Cache-write input tokens (provider-reported, 0 if no write).
    pub write_input_tokens: u64,
    /// If the cache MISS-ed, why?
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidation_cause: Option<InvalidationCause>,
    /// Provider id (e.g. "anthropic", "openai") for cross-provider correlation.
    pub provider: String,
    /// Model id for cross-model correlation.
    pub model: String,
}

impl PromptCacheObservation {
    /// Construct a HIT observation (read tokens > 0).
    pub fn hit(
        retention: CacheRetention,
        provider: impl Into<String>,
        model: impl Into<String>,
        read_tokens: u64,
        write_tokens: u64,
    ) -> Self {
        Self {
            retention,
            read_input_tokens: read_tokens,
            write_input_tokens: write_tokens,
            invalidation_cause: None,
            provider: provider.into(),
            model: model.into(),
        }
    }

    /// Construct a MISS observation with a stated cause.
    pub fn miss(
        retention: CacheRetention,
        provider: impl Into<String>,
        model: impl Into<String>,
        cause: InvalidationCause,
    ) -> Self {
        Self {
            retention,
            read_input_tokens: 0,
            write_input_tokens: 0,
            invalidation_cause: Some(cause),
            provider: provider.into(),
            model: model.into(),
        }
    }

    pub fn is_hit(&self) -> bool {
        self.read_input_tokens > 0
    }
}

/// Structured `cache_health_warn` event (Layer E1) — emitted when a warm
/// session's cache hit-ratio (`cache_read / input`) falls below the warn
/// threshold on a round-trip where the prefix should already be cached.
/// Warning-only telemetry: never alters the request. Detection lives in
/// `wcore-agent::cache_diagnostics` (which tracks per-conversation
/// round-trips); this is the wire/observability shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheHealthWarn {
    /// Stable per-session conversation id (Flux sticky-routing id).
    pub conversation_id: String,
    /// 1-based round-trip index within the conversation.
    pub round_trip: u64,
    /// Provider-reported input tokens for the turn.
    pub input_tokens: u64,
    /// Provider-reported cache-read tokens for the turn.
    pub cache_read_tokens: u64,
    /// `cache_read_tokens / input_tokens`.
    pub ratio: f64,
    /// Model that served the turn (Flux `ProviderMeta.routed_model` when
    /// signaled back, else the dispatched model id).
    pub routed_model: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_as_str_round_trip() {
        assert_eq!(CacheRetention::Ephemeral5m.as_str(), "5m");
        assert_eq!(CacheRetention::Ephemeral1h.as_str(), "1h");
        assert_eq!(CacheRetention::None.as_str(), "none");
    }

    #[test]
    fn retention_ttl_seconds() {
        assert_eq!(CacheRetention::Ephemeral5m.ttl_seconds(), Some(300));
        assert_eq!(CacheRetention::Ephemeral1h.ttl_seconds(), Some(3600));
        assert_eq!(CacheRetention::None.ttl_seconds(), None);
    }

    #[test]
    fn retention_serde_snake_case() {
        let j = serde_json::to_string(&CacheRetention::Ephemeral1h).unwrap();
        assert_eq!(j, "\"ephemeral1h\"");
        let back: CacheRetention = serde_json::from_str(&j).unwrap();
        assert_eq!(back, CacheRetention::Ephemeral1h);
    }

    #[test]
    fn invalidation_cause_strings() {
        assert_eq!(
            InvalidationCause::SystemPromptDrift.as_str(),
            "system_prompt_drift"
        );
        assert_eq!(
            InvalidationCause::ToolDefinitionsChanged.as_str(),
            "tool_definitions_changed"
        );
        assert_eq!(
            InvalidationCause::HistoryRewritten.as_str(),
            "history_rewritten"
        );
        assert_eq!(InvalidationCause::Expired.as_str(), "expired");
        assert_eq!(
            InvalidationCause::ProviderRejected.as_str(),
            "provider_rejected"
        );
        assert_eq!(InvalidationCause::NoMarker.as_str(), "no_marker");
        assert_eq!(InvalidationCause::Unknown.as_str(), "unknown");
    }

    #[test]
    fn observation_hit_construction() {
        let obs = PromptCacheObservation::hit(
            CacheRetention::Ephemeral5m,
            "anthropic",
            "claude-opus-4-7",
            1000,
            0,
        );
        assert!(obs.is_hit());
        assert_eq!(obs.invalidation_cause, None);
        assert_eq!(obs.read_input_tokens, 1000);
    }

    #[test]
    fn observation_miss_construction() {
        let obs = PromptCacheObservation::miss(
            CacheRetention::Ephemeral1h,
            "anthropic",
            "claude-opus-4-7",
            InvalidationCause::SystemPromptDrift,
        );
        assert!(!obs.is_hit());
        assert_eq!(
            obs.invalidation_cause,
            Some(InvalidationCause::SystemPromptDrift)
        );
        assert_eq!(obs.read_input_tokens, 0);
        assert_eq!(obs.write_input_tokens, 0);
    }

    #[test]
    fn observation_serde_round_trip() {
        let obs =
            PromptCacheObservation::hit(CacheRetention::Ephemeral5m, "openai", "gpt-5", 500, 100);
        let j = serde_json::to_string(&obs).unwrap();
        let back: PromptCacheObservation = serde_json::from_str(&j).unwrap();
        assert_eq!(obs, back);
    }

    #[test]
    fn cache_health_warn_serde_round_trip() {
        let warn = CacheHealthWarn {
            conversation_id: "conv-123".into(),
            round_trip: 3,
            input_tokens: 15_000,
            cache_read_tokens: 128,
            ratio: 128.0 / 15_000.0,
            routed_model: "gpt-5.4".into(),
        };
        let j = serde_json::to_string(&warn).unwrap();
        let back: CacheHealthWarn = serde_json::from_str(&j).unwrap();
        assert_eq!(warn, back);
    }

    #[test]
    fn observation_serde_skips_none_cause() {
        let obs =
            PromptCacheObservation::hit(CacheRetention::Ephemeral5m, "openai", "gpt-5", 500, 0);
        let j = serde_json::to_string(&obs).unwrap();
        assert!(
            !j.contains("invalidation_cause"),
            "None should be skipped: {}",
            j
        );
    }
}
