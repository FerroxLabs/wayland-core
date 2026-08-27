//! Prompt cache break detection.
//!
//! Pairs request-side prompt state (hashes) with response-side cache tokens
//! to detect and diagnose prompt cache breaks across turns.

use std::hash::{DefaultHasher, Hash, Hasher};

use wcore_types::{message::TokenUsage, tool::ToolDef};

/// Snapshot of prompt state taken before each API call.
#[derive(Debug, Clone)]
struct PromptSnapshot {
    system_hash: u64,
    tools_hash: u64,
}

/// Cache token statistics from a single API response.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Uncached input tokens (cache misses).
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

/// Diagnostic result after comparing two consecutive turns.
#[derive(Debug, Clone)]
pub enum CacheDiagnostic {
    Healthy {
        hit_rate: f64,
    },
    PartialMiss {
        hit_rate: f64,
        cause: CacheBreakCause,
    },
    FullMiss {
        cause: CacheBreakCause,
    },
}

/// What caused a cache break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheBreakCause {
    SystemPromptChanged,
    ToolsChanged,
    TtlExpiry,
    FirstRequest,
}

/// Layer E1 — warm-session cache-health warn threshold: a warm round-trip
/// whose `cache_read / total_input` ratio falls below this fires a
/// `cache_health_warn` telemetry event.
pub const CACHE_HEALTH_WARN_RATIO: f64 = 0.3;

/// Layer E1 — a session counts as "warm" strictly AFTER this many
/// round-trips have completed (the prefix has had two chances to be
/// written to the provider cache).
pub const CACHE_HEALTH_WARM_AFTER_ROUND_TRIPS: u64 = 2;

/// #559 — the smallest per-turn input for which a TOTALLY dead prompt cache
/// (zero cache tokens for the whole session) is reported.
///
/// Every provider that caches has a minimum cacheable prompt size, around
/// 1k-2k tokens; below it, zero cached tokens is correct behaviour and a
/// warning would be a false alarm. This floor sits an order of magnitude
/// above the largest of those minima, so nothing under it can be mistaken
/// for a break. The session that motivated it ran 61k-4.9M input tokens per
/// turn.
pub const CACHE_DEAD_MIN_INPUT_TOKENS: u64 = 20_000;

/// Layer E1 — a warm round-trip whose cache hit-ratio fell below
/// [`CACHE_HEALTH_WARN_RATIO`]. Detection-side fields only; the engine
/// wraps this in the wire-shaped
/// `wcore_providers::cache_observation::CacheHealthWarn` (adding
/// conversation_id + routed model) before emitting.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheHealthAlert {
    /// 1-based round-trip index within the conversation.
    pub round_trip: u64,
    /// Total input processed across uncached, cache-read, and cache-write
    /// categories.
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    /// `cache_read_tokens / total_input_tokens`.
    pub ratio: f64,
    /// #559 — no response in this session has reported a single cache token,
    /// read or written. Distinct from an ordinary low ratio: the prefix was
    /// never served from cache even once, so the whole context is being
    /// re-billed on every round trip. The engine reports this one to the
    /// user; an ordinary low ratio stays telemetry-only.
    pub dead_cache: bool,
}

/// Detects prompt cache breaks by comparing consecutive turns.
pub struct CacheBreakDetector {
    /// Snapshot from the PREVIOUS turn (used for attribution on cache break).
    prev_snapshot: Option<PromptSnapshot>,
    /// Snapshot from the CURRENT turn (just recorded by record_request).
    current_snapshot: Option<PromptSnapshot>,
    /// Cache stats from the previous API response.
    prev_stats: Option<CacheStats>,
    /// Layer E1 — completed round-trips (responses seen via
    /// [`check_response`]). Drives the warm-session gate for
    /// [`check_cache_health`].
    round_trips: u64,
    /// Layer E1 — whether ANY response in this session ever reported cache
    /// tokens (read or creation). Providers with no prompt-cache support
    /// report all-zeros forever; without this gate they would fire a
    /// `cache_health_warn` on every warm turn (mirrors the
    /// `openai_no_false_alarm` guard in [`Self::compute_diagnostic`]).
    seen_cache_tokens: bool,
}

impl CacheBreakDetector {
    pub fn new() -> Self {
        Self {
            prev_snapshot: None,
            current_snapshot: None,
            prev_stats: None,
            round_trips: 0,
            seen_cache_tokens: false,
        }
    }

    /// Record the prompt state before an API call.
    pub fn record_request(&mut self, system: &str, tools: &[ToolDef]) {
        let mut system_hasher = DefaultHasher::new();
        system.hash(&mut system_hasher);
        let system_hash = system_hasher.finish();

        let mut tools_hasher = DefaultHasher::new();
        for t in tools {
            t.name.hash(&mut tools_hasher);
            t.description.hash(&mut tools_hasher);
            let schema_str = serde_json::to_string(&t.input_schema).unwrap_or_default();
            schema_str.hash(&mut tools_hasher);
            t.deferred.hash(&mut tools_hasher);
        }
        let tools_hash = tools_hasher.finish();

        // Rotate: current becomes prev, new snapshot becomes current
        self.prev_snapshot = self.current_snapshot.take();
        self.current_snapshot = Some(PromptSnapshot {
            system_hash,
            tools_hash,
        });
    }

    /// Check the response cache tokens against the previous turn.
    ///
    /// Returns `None` if no snapshot was recorded before the call.
    pub fn check_response(&mut self, stats: CacheStats) -> Option<CacheDiagnostic> {
        let current = self.current_snapshot.as_ref()?;
        let diagnostic = self.compute_diagnostic(current, &stats);
        // Layer E1 — track warmth for check_cache_health.
        self.round_trips += 1;
        if stats.cache_read_tokens > 0 || stats.cache_creation_tokens > 0 {
            self.seen_cache_tokens = true;
        }
        self.prev_stats = Some(stats);
        Some(diagnostic)
    }

    /// Layer E1 — warm-session cache-health probe. Call AFTER
    /// [`check_response`] for the same turn (so `round_trips` includes the
    /// turn being probed). Returns `Some` when the session is warm (more
    /// than [`CACHE_HEALTH_WARM_AFTER_ROUND_TRIPS`] round-trips), the
    /// provider has demonstrated prompt-cache support at least once OR is
    /// declared to have one via `cache_expected`, and
    /// this turn's `cache_read / total_input` ratio fell below
    /// [`CACHE_HEALTH_WARN_RATIO`]. Warning-only telemetry — callers must
    /// never alter the request based on it.
    ///
    /// `cache_expected` is the caller's resolved
    /// `ProviderCompat::prompt_cache_expected()`. It is what lets a session
    /// that has NEVER been served a cached token be reported (#559) without
    /// firing on every turn of an endpoint that simply has no prompt cache.
    /// It is passed per call rather than held on the detector because the
    /// engine's compat can be replaced mid-session.
    pub fn check_cache_health(
        &self,
        stats: &CacheStats,
        cache_expected: bool,
    ) -> Option<CacheHealthAlert> {
        if self.round_trips <= CACHE_HEALTH_WARM_AFTER_ROUND_TRIPS {
            return None;
        }
        let total_input_tokens = stats.total_input_tokens();
        if total_input_tokens == 0 {
            return None;
        }
        // #559 — a session that has never seen a single cache token is either
        // an endpoint with no prompt cache (nothing to report) or an endpoint
        // with one that is serving us nothing (the whole context re-billed
        // every round trip). The response cannot tell them apart; only the
        // resolved provider capability can, which is why `cache_expected`
        // comes from the caller instead of being inferred here.
        //
        // Without this branch the probe was blind to the total-failure case
        // and reported only a cache that worked and then broke — so the
        // reported session ran 26 turns at `cache_read = 0` in silence.
        let dead_cache = !self.seen_cache_tokens;
        if dead_cache {
            if !cache_expected {
                return None;
            }
            // Below every provider's minimum cacheable prompt size, zero
            // cached tokens is correct, not a break.
            if total_input_tokens < CACHE_DEAD_MIN_INPUT_TOKENS {
                return None;
            }
        }
        let ratio = stats.cache_read_tokens as f64 / total_input_tokens as f64;
        if ratio >= CACHE_HEALTH_WARN_RATIO {
            return None;
        }
        Some(CacheHealthAlert {
            round_trip: self.round_trips,
            input_tokens: total_input_tokens,
            cache_read_tokens: stats.cache_read_tokens,
            ratio,
            dead_cache,
        })
    }

    fn compute_diagnostic(&self, current: &PromptSnapshot, stats: &CacheStats) -> CacheDiagnostic {
        let Some(prev) = &self.prev_stats else {
            // First request — no previous data to compare
            return CacheDiagnostic::Healthy { hit_rate: 0.0 };
        };

        // If provider doesn't support caching (both turns have 0 cache tokens),
        // report healthy to avoid false alarms (e.g., OpenAI).
        if prev.cache_read_tokens == 0
            && prev.cache_creation_tokens == 0
            && stats.cache_read_tokens == 0
            && stats.cache_creation_tokens == 0
        {
            return CacheDiagnostic::Healthy { hit_rate: 0.0 };
        }

        let prev_had_cache = prev.cache_read_tokens > 0 || prev.cache_creation_tokens > 0;

        // Full miss: had cache before, now read tokens dropped to 0
        if prev_had_cache && stats.cache_read_tokens == 0 {
            let cause = self.attribute_cause(current);
            return CacheDiagnostic::FullMiss { cause };
        }

        // Calculate hit rate
        let total_input_tokens = stats.total_input_tokens();
        let hit_rate = if total_input_tokens > 0 {
            stats.cache_read_tokens as f64 / total_input_tokens as f64
        } else {
            0.0
        };

        // Partial miss: cache_read dropped >5% compared to previous
        if prev.cache_read_tokens > 0 {
            let drop_pct = 1.0 - (stats.cache_read_tokens as f64 / prev.cache_read_tokens as f64);
            if drop_pct > 0.05 {
                let cause = self.attribute_cause(current);
                return CacheDiagnostic::PartialMiss { hit_rate, cause };
            }
        }

        CacheDiagnostic::Healthy { hit_rate }
    }

    /// Determine what caused the cache break by comparing prev vs current snapshots.
    fn attribute_cause(&self, current: &PromptSnapshot) -> CacheBreakCause {
        let Some(prev) = &self.prev_snapshot else {
            return CacheBreakCause::FirstRequest;
        };

        if prev.system_hash != current.system_hash {
            return CacheBreakCause::SystemPromptChanged;
        }
        if prev.tools_hash != current.tools_hash {
            return CacheBreakCause::ToolsChanged;
        }

        // Hashes match but cache was lost — server-side TTL expiry
        CacheBreakCause::TtlExpiry
    }
}

impl CacheStats {
    fn total_input_tokens(&self) -> u64 {
        TokenUsage::total_input_from(
            self.input_tokens,
            self.cache_read_tokens,
            self.cache_creation_tokens,
        )
    }
}

impl Default for CacheBreakDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "Read".into(),
            description: "Read a file".into(),
            input_schema: json!({"type": "object"}),
            deferred: false,
            server: None,
        }]
    }

    #[test]
    fn first_request_returns_healthy() {
        let mut detector = CacheBreakDetector::new();
        detector.record_request("system prompt", &make_tools());
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 10000,
                cache_read_tokens: 0,
                cache_creation_tokens: 5000,
            })
            .unwrap();
        assert!(matches!(diag, CacheDiagnostic::Healthy { .. }));
    }

    #[test]
    fn healthy_when_cache_read_stable() {
        let mut detector = CacheBreakDetector::new();

        // Turn 1
        detector.record_request("prompt", &make_tools());
        detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 8000,
            cache_creation_tokens: 2000,
        });

        // Turn 2 — similar cache_read
        detector.record_request("prompt", &make_tools());
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 11000,
                cache_read_tokens: 8000,
                cache_creation_tokens: 0,
            })
            .unwrap();

        assert!(matches!(diag, CacheDiagnostic::Healthy { .. }));
    }

    #[test]
    fn hit_rate_uses_all_disjoint_input_categories() {
        let mut detector = CacheBreakDetector::new();
        detector.record_request("prompt", &make_tools());
        detector.check_response(CacheStats {
            input_tokens: 20,
            cache_read_tokens: 80,
            cache_creation_tokens: 0,
        });

        detector.record_request("prompt", &make_tools());
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 20,
                cache_read_tokens: 80,
                cache_creation_tokens: 0,
            })
            .unwrap();

        match diag {
            CacheDiagnostic::Healthy { hit_rate } => {
                assert!((hit_rate - 0.8).abs() < f64::EPSILON);
            }
            other => panic!("expected Healthy, got {other:?}"),
        }
    }

    #[test]
    fn full_miss_when_cache_read_drops_to_zero() {
        let mut detector = CacheBreakDetector::new();

        // Turn 1 — cache established
        detector.record_request("prompt", &make_tools());
        detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 8000,
            cache_creation_tokens: 2000,
        });

        // Turn 2 — cache_read drops to 0
        detector.record_request("prompt", &make_tools());
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 10000,
                cache_read_tokens: 0,
                cache_creation_tokens: 10000,
            })
            .unwrap();

        assert!(matches!(diag, CacheDiagnostic::FullMiss { .. }));
    }

    #[test]
    fn full_miss_system_prompt_changed() {
        let mut detector = CacheBreakDetector::new();

        // Turn 1
        detector.record_request("prompt v1", &make_tools());
        detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 8000,
            cache_creation_tokens: 2000,
        });

        // Turn 2 — different system prompt
        detector.record_request("prompt v2", &make_tools());
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 10000,
                cache_read_tokens: 0,
                cache_creation_tokens: 10000,
            })
            .unwrap();

        match diag {
            CacheDiagnostic::FullMiss { cause } => {
                assert_eq!(cause, CacheBreakCause::SystemPromptChanged);
            }
            _ => panic!("expected FullMiss"),
        }
    }

    #[test]
    fn full_miss_tools_changed() {
        let mut detector = CacheBreakDetector::new();

        // Turn 1
        detector.record_request("prompt", &make_tools());
        detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 8000,
            cache_creation_tokens: 2000,
        });

        // Turn 2 — different tools
        let new_tools = vec![ToolDef {
            name: "Write".into(),
            description: "Write a file".into(),
            input_schema: json!({"type": "object"}),
            deferred: false,
            server: None,
        }];
        detector.record_request("prompt", &new_tools);
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 10000,
                cache_read_tokens: 0,
                cache_creation_tokens: 10000,
            })
            .unwrap();

        match diag {
            CacheDiagnostic::FullMiss { cause } => {
                assert_eq!(cause, CacheBreakCause::ToolsChanged);
            }
            _ => panic!("expected FullMiss"),
        }
    }

    #[test]
    fn full_miss_ttl_expiry() {
        let mut detector = CacheBreakDetector::new();

        // Turn 1
        detector.record_request("prompt", &make_tools());
        detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 8000,
            cache_creation_tokens: 2000,
        });

        // Turn 2 — same prompt and tools but cache lost (TTL expired server-side)
        detector.record_request("prompt", &make_tools());
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 10000,
                cache_read_tokens: 0,
                cache_creation_tokens: 10000,
            })
            .unwrap();

        match diag {
            CacheDiagnostic::FullMiss { cause } => {
                assert_eq!(cause, CacheBreakCause::TtlExpiry);
            }
            _ => panic!("expected FullMiss"),
        }
    }

    #[test]
    fn partial_miss_when_cache_read_drops_significantly() {
        let mut detector = CacheBreakDetector::new();

        // Turn 1
        detector.record_request("prompt", &make_tools());
        detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 8000,
            cache_creation_tokens: 2000,
        });

        // Turn 2 — 50% drop in cache_read
        detector.record_request("prompt", &make_tools());
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 10000,
                cache_read_tokens: 4000,
                cache_creation_tokens: 6000,
            })
            .unwrap();

        assert!(matches!(diag, CacheDiagnostic::PartialMiss { .. }));
    }

    #[test]
    fn openai_no_false_alarm() {
        // OpenAI never returns cache tokens — both turns have all zeros
        let mut detector = CacheBreakDetector::new();

        detector.record_request("prompt", &make_tools());
        detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        });

        detector.record_request("prompt", &make_tools());
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 10000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            })
            .unwrap();

        // Should be Healthy, not FullMiss
        assert!(matches!(diag, CacheDiagnostic::Healthy { .. }));
    }

    // --- Layer E1: check_cache_health ---

    /// Drive one round-trip through the detector and return the health probe
    /// for that same turn (mirrors the engine call order: record_request →
    /// check_response → check_cache_health).
    fn round_trip(
        detector: &mut CacheBreakDetector,
        stats: CacheStats,
    ) -> Option<CacheHealthAlert> {
        round_trip_on(detector, stats, false)
    }

    /// As [`round_trip`], on a route whose prompt-cache capability is stated
    /// by `cache_expected` (the resolved `ProviderCompat` value).
    fn round_trip_on(
        detector: &mut CacheBreakDetector,
        stats: CacheStats,
        cache_expected: bool,
    ) -> Option<CacheHealthAlert> {
        detector.record_request("prompt", &make_tools());
        detector.check_response(stats.clone());
        detector.check_cache_health(&stats, cache_expected)
    }

    #[test]
    fn warm_session_low_cache_read_fires_health_warn() {
        let mut detector = CacheBreakDetector::new();

        // Turn 1: cold — prefix written to cache. Turn 2: still inside the
        // warm-up window. Neither may warn.
        assert!(
            round_trip(
                &mut detector,
                CacheStats {
                    input_tokens: 10_000,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 9_000,
                }
            )
            .is_none(),
            "turn 1 (cold) must not warn"
        );
        assert!(
            round_trip(
                &mut detector,
                CacheStats {
                    input_tokens: 10_000,
                    cache_read_tokens: 128,
                    cache_creation_tokens: 0,
                }
            )
            .is_none(),
            "turn 2 (warm-up window) must not warn"
        );

        // Turn 3: warm session, cache_read stuck at 128 on a 15k input —
        // the exact 128-flat signature. Must warn.
        let alert = round_trip(
            &mut detector,
            CacheStats {
                input_tokens: 15_000,
                cache_read_tokens: 128,
                cache_creation_tokens: 0,
            },
        )
        .expect("warm turn with dead cache must fire cache_health_warn");
        assert_eq!(alert.round_trip, 3);
        assert_eq!(alert.input_tokens, 15_128);
        assert_eq!(alert.cache_read_tokens, 128);
        assert!(alert.ratio < CACHE_HEALTH_WARN_RATIO);
    }

    #[test]
    fn warm_session_healthy_ratio_does_not_warn() {
        let mut detector = CacheBreakDetector::new();
        for _ in 0..2 {
            round_trip(
                &mut detector,
                CacheStats {
                    input_tokens: 10_000,
                    cache_read_tokens: 8_000,
                    cache_creation_tokens: 2_000,
                },
            );
        }
        // Turn 3: 8k cache reads / 10k total input = 0.8 — healthy, no warn.
        assert!(
            round_trip(
                &mut detector,
                CacheStats {
                    input_tokens: 2_000,
                    cache_read_tokens: 8_000,
                    cache_creation_tokens: 0,
                }
            )
            .is_none()
        );
    }

    /// RED ARM (#559) — the reported signature: a long session whose every
    /// turn reports `cache_read = 0` on a route where prompt caching is
    /// expected to work. 26 warm round trips at 50k input each.
    #[test]
    fn dead_cache_from_the_first_turn_is_reported() {
        let mut detector = CacheBreakDetector::new();
        let mut alerts = Vec::new();
        for _ in 0..26 {
            if let Some(a) = round_trip_on(
                &mut detector,
                CacheStats {
                    input_tokens: 50_000,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                },
                true,
            ) {
                alerts.push(a);
            }
        }
        let first = alerts.first();
        assert!(
            first.is_some_and(|a| a.dead_cache && a.round_trip == 3),
            "the first alert must be the dead-cache class and must arrive on \
             round trip 3 (the first warm one), saw {:?}",
            first
        );
        let alerts = alerts.len();
        assert!(
            alerts > 0,
            "26 warm round trips, 50k input each, cache_read=0 on every one \
             (the #559 signature: 77.7M input tokens, cache_read=0 on 26/26 \
             turns) and the cache-health detector raised {alerts} alerts"
        );
    }

    /// The other direction of the same gate: identical traffic on a route
    /// that does NOT declare a prompt cache must stay silent. Without this,
    /// the #559 fix would accuse every OpenAI-compatible endpoint that has no
    /// cache of having a broken one.
    #[test]
    fn dead_cache_is_silent_when_no_cache_is_declared() {
        let mut detector = CacheBreakDetector::new();
        for turn in 0..26 {
            assert!(
                round_trip_on(
                    &mut detector,
                    CacheStats {
                        input_tokens: 50_000,
                        cache_read_tokens: 0,
                        cache_creation_tokens: 0,
                    },
                    false,
                )
                .is_none(),
                "turn {turn} warned on a route with no declared prompt cache"
            );
        }
    }

    /// Below every provider's minimum cacheable prompt size, zero cached
    /// tokens is correct behaviour, so a declared cache must not warn there.
    #[test]
    fn dead_cache_is_silent_below_the_minimum_cacheable_size() {
        let mut detector = CacheBreakDetector::new();
        let small = CACHE_DEAD_MIN_INPUT_TOKENS - 1;
        for turn in 0..8 {
            assert!(
                round_trip_on(
                    &mut detector,
                    CacheStats {
                        input_tokens: small,
                        cache_read_tokens: 0,
                        cache_creation_tokens: 0,
                    },
                    true,
                )
                .is_none(),
                "turn {turn} warned at {small} input tokens, under the \
                 {CACHE_DEAD_MIN_INPUT_TOKENS} floor"
            );
        }
    }

    /// A cache that WORKED and then broke is not the dead-cache class, even
    /// on a route that declares a cache. The two carry different advice, so
    /// the flag must separate them.
    #[test]
    fn a_break_after_a_working_cache_is_not_the_dead_cache_class() {
        let mut detector = CacheBreakDetector::new();
        for _ in 0..2 {
            round_trip_on(
                &mut detector,
                CacheStats {
                    input_tokens: 10_000,
                    cache_read_tokens: 40_000,
                    cache_creation_tokens: 0,
                },
                true,
            );
        }
        let alert = round_trip_on(
            &mut detector,
            CacheStats {
                input_tokens: 50_000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
            true,
        )
        .expect("a warm turn that lost its cache must still warn");
        assert!(
            !alert.dead_cache,
            "the cache was served on earlier turns, so this is a break, not a \
             cache that was never alive"
        );
    }

    #[test]
    fn provider_without_cache_support_never_warns() {
        // A provider that never reports cache tokens (all zeros forever) is
        // indistinguishable from "no prompt-cache support" — suppress, same
        // as the openai_no_false_alarm diagnostic guard.
        let mut detector = CacheBreakDetector::new();
        for turn in 0..4 {
            let alert = round_trip(
                &mut detector,
                CacheStats {
                    input_tokens: 10_000,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                },
            );
            assert!(alert.is_none(), "turn {turn} must not warn");
        }
    }

    #[test]
    fn zero_input_tokens_does_not_warn() {
        let mut detector = CacheBreakDetector::new();
        for _ in 0..3 {
            round_trip(
                &mut detector,
                CacheStats {
                    input_tokens: 10_000,
                    cache_read_tokens: 5_000,
                    cache_creation_tokens: 0,
                },
            );
        }
        assert!(
            round_trip(
                &mut detector,
                CacheStats {
                    input_tokens: 0,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                }
            )
            .is_none(),
            "zero-input turn cannot produce a meaningful ratio"
        );
    }

    #[test]
    fn no_diagnostic_without_record_request() {
        let mut detector = CacheBreakDetector::new();
        let diag = detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        });
        assert!(diag.is_none());
    }
}
