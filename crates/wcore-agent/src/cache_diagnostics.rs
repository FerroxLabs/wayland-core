//! Prompt cache break detection.
//!
//! Pairs request-side prompt state (hashes) with response-side cache tokens
//! to detect and diagnose prompt cache breaks across turns.

use std::hash::{DefaultHasher, Hash, Hasher};

use wcore_types::{
    message::{Message, TokenUsage},
    tool::ToolDef,
};

/// Snapshot of prompt state taken before each API call.
#[derive(Debug, Clone)]
struct PromptSnapshot {
    /// #1166 — the dispatched model. A tier swap moves the request to a
    /// different cache pool entirely, which the detector reported as
    /// `TtlExpiry` (i.e. the server's fault).
    model_hash: u64,
    system_hash: u64,
    tools_hash: u64,
    /// #1166 — per-message content hashes, index-aligned with the message
    /// array actually dispatched. Position-sensitive, so a divergence names
    /// the first index instead of only reporting that something moved.
    message_hashes: Vec<u64>,
}

/// `std::io::Write` sink that feeds bytes straight into a hasher, so a message
/// can be hashed from its serialized form without materialising the string.
struct HashWriter<'a>(&'a mut DefaultHasher);

impl std::io::Write for HashWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Hasher::write(self.0, buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Hash one message the way the provider sees it: role + content, nothing else.
///
/// `cache_breakpoint` is deliberately excluded — `mark_cache_boundaries`
/// re-marks the tail on every turn, and a breakpoint moving is not a change to
/// the cached prefix's CONTENT. `timestamp` is excluded because it never
/// reaches the wire.
fn hash_message(message: &Message) -> u64 {
    let mut hasher = DefaultHasher::new();
    if serde_json::to_writer(HashWriter(&mut hasher), &(&message.role, &message.content)).is_err() {
        // The sink is infallible and the value is in memory, so this is
        // unreachable in practice. Fold in a marker rather than panic — a
        // diagnostic must never take down a session.
        Hasher::write_u8(&mut hasher, 0xff);
    }
    hasher.finish()
}

/// Did this turn read its PRIOR CONTEXT back out of the cache? (#1166 follow-up)
///
/// The absolute floor added for #1166 tests `cache_read / total_input`, which
/// is the right question about a cache that never worked and the wrong one
/// about a turn that simply carries a lot of NEW input. A warm turn that reads
/// its whole 40k prefix back and then adds a 150k tool result scores 0.21 on
/// that ratio, and the floor reported it as `PartialMiss { TtlExpiry }` — an
/// invalidation that did not happen, attributed to the server, and written
/// durably into the cache ledger as `expired`. That is #1166's own Defect 4
/// re-entering through #1166's fix.
///
/// The discriminator is the denominator. Measure `cache_read` against the
/// PREVIOUS turn's total input — the context a working cache would have to
/// hand back — instead of against this turn's. On the measured #559 leader
/// session that is 192 / 6_220 = 0.031 and the floor still fires; on a fully
/// reused prefix it is 40_000 / 40_500 = 0.988 and it does not.
///
/// `prior_context_tokens == 0` is not evidence of reuse (there was no prior
/// turn, or it processed nothing), so it does NOT suppress.
fn cached_prefix_covers_prior_context(cache_read_tokens: u64, prior_context_tokens: u64) -> bool {
    prior_context_tokens > 0
        && (cache_read_tokens as f64 / prior_context_tokens as f64) >= CACHE_HEALTH_WARN_RATIO
}

/// How much of the previous turn's `cache_read` may be lost before it counts as
/// a partial miss.
const CACHE_READ_DROP_TOLERANCE: f64 = 0.05;

/// Did `cache_read` fall materially against the previous turn's?
///
/// One function, because BOTH halves have to apply it or they disagree.
/// `compute_diagnostic` has always tested this before reaching the absolute
/// floor, so the floor's prior-context suppression can never swallow a real
/// drop. `check_cache_health` has no drop branch of its own — until this
/// existed, its copy of the suppression could, and the two halves would then
/// report a turn as `PartialMiss` and stay silent about it in the same breath.
/// That is ticket Defect 2 (the detecting half and the explaining half being
/// different paths) re-entering through the #1166 c5 repair.
fn cache_read_dropped(current: u64, previous: u64) -> bool {
    previous > 0 && (1.0 - (current as f64 / previous as f64)) > CACHE_READ_DROP_TOLERANCE
}

/// wayland#1206 — keep the absolute floor's fall-through off the server.
///
/// [`CacheBreakDetector::attribute_cause`] ends in [`CacheBreakCause::TtlExpiry`]
/// whenever nothing about the request diverged, so EVERY turn the #1166
/// absolute floor catches on a stable prefix was being reported as a
/// server-side expiry — and written durably into the cache ledger as
/// `expired`. But a prefix that is identical turn over turn and whose
/// `cache_read` did not fall did not expire: an expiry destroys cached tokens,
/// so it shows up as a DROP. What actually happened is that the cache never
/// covered the context in the first place — the #559 leader session read 192
/// tokens back, flat, for its whole life while its input climbed.
///
/// Only the fall-through is rewritten. A real attribution — a changed system
/// prompt, tools, model, or message prefix — is a positive finding about this
/// turn and survives untouched, and so does a `TtlExpiry` on a turn where
/// `cache_read` really did fall.
fn floor_cause(attributed: CacheBreakCause, cache_read_fell: bool) -> CacheBreakCause {
    if matches!(attributed, CacheBreakCause::TtlExpiry) && !cache_read_fell {
        CacheBreakCause::PrefixNotCached
    } else {
        attributed
    }
}

/// First index at which two turns' message arrays diverge, if any.
///
/// Only the COMMON prefix is compared. Appending a message is the shape of
/// every ordinary turn and leaves the cached prefix intact; truncating the tail
/// (context shedding) likewise leaves the surviving prefix byte-identical.
/// Neither is a cache break, and reporting one would make this detector scream
/// on every healthy turn.
fn first_divergent_message(prev: &[u64], current: &[u64]) -> Option<usize> {
    (0..prev.len().min(current.len())).find(|&i| prev[i] != current[i])
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
    /// #1166 — the dispatched model changed (e.g. the smart-routing tier swap),
    /// so the prefix was looked up in a different cache pool.
    ModelChanged,
    /// #1166 — the message array diverged from the previous turn inside its
    /// common prefix, at `first_divergent_index`. This is the class #559 lives
    /// in (a volatile `Current date:` prefix on `messages[1]`), and every
    /// in-place history rewrite: microcompact tool-result clearing, orphan
    /// repair, context shedding, crash-recovery re-injection.
    MessagesChanged {
        first_divergent_index: usize,
    },
    TtlExpiry,
    /// wayland#1206 — the absolute floor fired on a turn where NOTHING was
    /// invalidated: the request is byte-identical to the previous one and
    /// `cache_read` did not fall. The cache simply never covered the context.
    /// This is the honest fall-through the floor needs; `TtlExpiry` names a
    /// server-side event that would have shown up as a DROP.
    PrefixNotCached,
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
    /// #1166 — why the prefix was not read back. The path that DETECTS a dead
    /// cache and the path that EXPLAINS one are now the same path: before this,
    /// `check_cache_health` logged "hit ratio 0.030, this is bad" while the
    /// diagnostic half recorded `Healthy, 0 causes` for the same turn.
    pub cause: CacheBreakCause,
}

/// Detects prompt cache breaks by comparing consecutive turns.
pub struct CacheBreakDetector {
    /// Snapshot from the PREVIOUS turn (used for attribution on cache break).
    prev_snapshot: Option<PromptSnapshot>,
    /// Snapshot from the CURRENT turn (just recorded by record_request).
    current_snapshot: Option<PromptSnapshot>,
    /// Cache stats from the previous API response.
    prev_stats: Option<CacheStats>,
    /// Total input the turn BEFORE `prev_stats` processed — the context a
    /// working cache would have to read back on the turn `check_cache_health`
    /// is probing. `check_response` rotates `prev_stats` to the CURRENT turn
    /// before the engine calls `check_cache_health`, so that field cannot
    /// answer this question; `compute_diagnostic` runs before the rotation and
    /// reads its own `prev` directly.
    prior_context_tokens: Option<u64>,
    /// `cache_read` on the turn before `prev_stats`, for the same reason and
    /// with the same rotation caveat as `prior_context_tokens`. Feeds
    /// [`cache_read_dropped`] on the `check_cache_health` side.
    prior_cache_read_tokens: Option<u64>,
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
            prior_context_tokens: None,
            prior_cache_read_tokens: None,
            round_trips: 0,
            seen_cache_tokens: false,
        }
    }

    /// Record the prompt state before an API call.
    ///
    /// #1166 — takes the FULL dispatched prompt (model + system + tools +
    /// messages). The message array is what the #559 class of break mutates,
    /// and the model is what the smart-routing tier swap changes; neither was
    /// an input, so neither could ever be named as a cause.
    pub fn record_request(
        &mut self,
        model: &str,
        system: &str,
        tools: &[ToolDef],
        messages: &[Message],
    ) {
        let mut model_hasher = DefaultHasher::new();
        model.hash(&mut model_hasher);
        let model_hash = model_hasher.finish();

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

        let message_hashes: Vec<u64> = messages.iter().map(hash_message).collect();

        // Rotate: current becomes prev, new snapshot becomes current
        self.prev_snapshot = self.current_snapshot.take();
        self.current_snapshot = Some(PromptSnapshot {
            model_hash,
            system_hash,
            tools_hash,
            message_hashes,
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
        self.prior_context_tokens = self.prev_stats.as_ref().map(CacheStats::total_input_tokens);
        self.prior_cache_read_tokens = self.prev_stats.as_ref().map(|s| s.cache_read_tokens);
        self.prev_stats = Some(stats);
        Some(diagnostic)
    }

    /// Layer E1 — warm-session cache-health probe. Call AFTER
    /// [`check_response`] for the same turn (so `round_trips` includes the
    /// turn being probed). Returns `Some` when the session is warm (more
    /// than [`CACHE_HEALTH_WARM_AFTER_ROUND_TRIPS`] round-trips), the
    /// provider has demonstrated prompt-cache support at least once, and
    /// this turn's `cache_read / total_input` ratio fell below
    /// [`CACHE_HEALTH_WARN_RATIO`]. Warning-only telemetry — callers must
    /// never alter the request based on it.
    pub fn check_cache_health(&self, stats: &CacheStats) -> Option<CacheHealthAlert> {
        if self.round_trips <= CACHE_HEALTH_WARM_AFTER_ROUND_TRIPS {
            return None;
        }
        if !self.seen_cache_tokens {
            return None;
        }
        let total_input_tokens = stats.total_input_tokens();
        if total_input_tokens == 0 {
            return None;
        }
        let ratio = stats.cache_read_tokens as f64 / total_input_tokens as f64;
        if ratio >= CACHE_HEALTH_WARN_RATIO {
            return None;
        }
        // #1166 follow-up — a low ratio on a turn that read its whole prior
        // context back is a big new input, not a break. Same guard as the
        // absolute floor in `compute_diagnostic`, so the two halves cannot
        // disagree about whether this turn was healthy.
        if cached_prefix_covers_prior_context(
            stats.cache_read_tokens,
            self.prior_context_tokens.unwrap_or(0),
        ) && !cache_read_dropped(
            stats.cache_read_tokens,
            self.prior_cache_read_tokens.unwrap_or(0),
        ) {
            return None;
        }
        // #1166 — attribute, do not just report. `current_snapshot` is always
        // set on the engine path (`check_response` returns `None` without it);
        // an unrecorded request has no previous turn to diverge from, which is
        // exactly what `FirstRequest` means.
        // wayland#1206 — the same fall-through rewrite the detecting half
        // applies. `prior_cache_read_tokens` is the previous turn's figure:
        // `check_response` has already rotated `prev_stats` onto THIS turn by
        // the time the engine calls us.
        let cause = floor_cause(
            match self.current_snapshot.as_ref() {
                Some(current) => self.attribute_cause(current),
                None => CacheBreakCause::FirstRequest,
            },
            cache_read_dropped(
                stats.cache_read_tokens,
                self.prior_cache_read_tokens.unwrap_or(0),
            ),
        );
        Some(CacheHealthAlert {
            round_trip: self.round_trips,
            input_tokens: total_input_tokens,
            cache_read_tokens: stats.cache_read_tokens,
            ratio,
            cause,
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
        if cache_read_dropped(stats.cache_read_tokens, prev.cache_read_tokens) {
            let cause = self.attribute_cause(current);
            return CacheDiagnostic::PartialMiss { hit_rate, cause };
        }

        // #1166 — the absolute floor. Every test above measures this turn
        // against the PREVIOUS turn, so a cache that never worked at all is
        // invisible to them: total breakage is perfectly STABLE turn over turn.
        // The measured #559 leader session sat at `cache_read = 192` flat for
        // its whole life and was classified `Healthy` on every turn. Measure
        // the ratio against this turn's own input as well, behind the same
        // warm-session and provider-supports-caching gates
        // `check_cache_health` uses, so the two halves cannot disagree.
        // The floor must not fire on a turn that read its whole prior context
        // back: `cached_prefix_covers_prior_context` is the discriminator
        // between a cache that never worked (the #559 leader, 192 / 6_220)
        // and one that worked perfectly under a large new input.
        if self.round_trips >= CACHE_HEALTH_WARM_AFTER_ROUND_TRIPS
            && self.seen_cache_tokens
            && total_input_tokens > 0
            && hit_rate < CACHE_HEALTH_WARN_RATIO
            && !cached_prefix_covers_prior_context(
                stats.cache_read_tokens,
                prev.total_input_tokens(),
            )
        {
            // wayland#1206 — the floor may fire here, but it may not blame the
            // server's TTL for it. The drop branch above already returned, so
            // `cache_read` did not fall; the boolean is recomputed rather than
            // assumed so a future reordering cannot silently make it a lie.
            let cause = floor_cause(
                self.attribute_cause(current),
                cache_read_dropped(stats.cache_read_tokens, prev.cache_read_tokens),
            );
            return CacheDiagnostic::PartialMiss { hit_rate, cause };
        }

        CacheDiagnostic::Healthy { hit_rate }
    }

    /// Determine what caused the cache break by comparing prev vs current snapshots.
    fn attribute_cause(&self, current: &PromptSnapshot) -> CacheBreakCause {
        let Some(prev) = &self.prev_snapshot else {
            return CacheBreakCause::FirstRequest;
        };

        // Ordered by how much of the prefix each invalidates: a different model
        // is a different cache pool entirely, then the wire order
        // system -> tools -> messages.
        if prev.model_hash != current.model_hash {
            return CacheBreakCause::ModelChanged;
        }
        if prev.system_hash != current.system_hash {
            return CacheBreakCause::SystemPromptChanged;
        }
        if prev.tools_hash != current.tools_hash {
            return CacheBreakCause::ToolsChanged;
        }
        if let Some(first_divergent_index) =
            first_divergent_message(&prev.message_hashes, &current.message_hashes)
        {
            return CacheBreakCause::MessagesChanged {
                first_divergent_index,
            };
        }

        // Everything we can observe is identical but the cache was lost — the
        // only remaining explanation is server-side TTL expiry. This is the
        // fall-through, so it is only honest once the observable set is
        // complete enough to have ruled the alternatives out.
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
    use wcore_types::message::{ContentBlock, MessageCacheHint, Role};

    /// A stable message array: the shape every ordinary turn shares. Each
    /// message's text is a pure function of its INDEX, so growing `n` appends
    /// rather than rewriting the prefix — which is what an ordinary turn does.
    fn make_messages(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| {
                Message::new(
                    if i % 2 == 0 {
                        Role::User
                    } else {
                        Role::Assistant
                    },
                    vec![ContentBlock::Text {
                        text: format!("message {i}"),
                    }],
                )
            })
            .collect()
    }

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
        detector.record_request("model-a", "system prompt", &make_tools(), &make_messages(2));
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
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 8000,
            cache_creation_tokens: 2000,
        });

        // Turn 2 — similar cache_read
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
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
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        detector.check_response(CacheStats {
            input_tokens: 20,
            cache_read_tokens: 80,
            cache_creation_tokens: 0,
        });

        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
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
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 8000,
            cache_creation_tokens: 2000,
        });

        // Turn 2 — cache_read drops to 0
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
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
        detector.record_request("model-a", "prompt v1", &make_tools(), &make_messages(2));
        detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 8000,
            cache_creation_tokens: 2000,
        });

        // Turn 2 — different system prompt
        detector.record_request("model-a", "prompt v2", &make_tools(), &make_messages(2));
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
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
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
        detector.record_request("model-a", "prompt", &new_tools, &make_messages(2));
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
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 8000,
            cache_creation_tokens: 2000,
        });

        // Turn 2 — same prompt and tools but cache lost (TTL expired server-side)
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
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
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 8000,
            cache_creation_tokens: 2000,
        });

        // Turn 2 — 50% drop in cache_read
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
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

        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        detector.check_response(CacheStats {
            input_tokens: 10000,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        });

        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
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
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        detector.check_response(stats.clone());
        detector.check_cache_health(&stats)
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

    // --- #1166 red arm: a cache that never worked ---

    /// The #559 leader session, verbatim: `cache_read` is pinned flat at 192
    /// while `input_tokens` climbs, for a 3% hit ratio. A totally broken cache
    /// is perfectly STABLE turn over turn, so the turn-over-turn delta test is
    /// structurally blind to it. The detector must not call this Healthy.
    #[test]
    fn flat_cache_read_on_warm_session_is_not_healthy() {
        let mut detector = CacheBreakDetector::new();
        let trace = [(3225u64, 192u64), (6028, 192), (6256, 192), (7001, 192)];
        let mut last = None;
        for (input_tokens, cache_read_tokens) in trace {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            last = detector.check_response(CacheStats {
                input_tokens,
                cache_read_tokens,
                cache_creation_tokens: 0,
            });
        }
        let diag = last.expect("detector must produce a diagnostic");
        assert!(
            !matches!(diag, CacheDiagnostic::Healthy { .. }),
            "a 3% hit ratio held flat for four turns is not Healthy, got {diag:?}"
        );
    }

    /// Control for the red arm above: a detector that always screams is as
    /// useless as one that never does. A genuinely healthy trace — high hit
    /// ratio, stable prefix — must stay Healthy for every turn, and must never
    /// fire a health alert.
    #[test]
    fn genuinely_healthy_trace_stays_healthy() {
        let mut detector = CacheBreakDetector::new();
        for turn in 1..=6u64 {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            let stats = CacheStats {
                input_tokens: 400 + turn * 50,
                cache_read_tokens: 40_000,
                cache_creation_tokens: 0,
            };
            let diag = detector.check_response(stats.clone()).unwrap();
            assert!(
                matches!(diag, CacheDiagnostic::Healthy { .. }),
                "turn {turn} of a healthy trace must stay Healthy, got {diag:?}"
            );
            assert!(
                detector.check_cache_health(&stats).is_none(),
                "turn {turn} of a healthy trace must not fire a health alert"
            );
        }
    }

    /// #1166 follow-up — the absolute floor must not manufacture an
    /// invalidation on a turn where the cache worked perfectly.
    ///
    /// The floor tests `cache_read / total_input`, so ANY warm turn whose NEW
    /// input dwarfs the cached prefix trips it — even when the entire prefix
    /// was read back verbatim. That is the shape of a long tool result, a
    /// pasted file, or a `Read` of a large source file: nothing was
    /// invalidated, the whole prior context came back from cache, and the
    /// ratio collapses purely because the numerator did not grow with the
    /// denominator. Reporting `PartialMiss { cause: TtlExpiry }` there blames
    /// the server's TTL for an event that did not happen, and (via
    /// `cause_of_diagnostic`) writes a durable `expired` invalidation into the
    /// cache ledger.
    ///
    /// The discriminator is prior-context coverage: `cache_read` measured
    /// against the PREVIOUS turn's total input, which is what a working cache
    /// would have to read back. Here that is 40_000 / 40_500 = 0.988. In the
    /// #559 leader session it was 192 / 6_220 = 0.031, which is why
    /// `flat_cache_read_on_warm_session_is_not_healthy` still reddens.
    #[test]
    fn a_fully_reused_prefix_under_a_large_new_input_is_not_an_invalidation() {
        let mut detector = CacheBreakDetector::new();

        // Three healthy warm turns: 40k of prefix read back against 500 new.
        for turn in 1..=3u64 {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            let stats = CacheStats {
                input_tokens: 500,
                cache_read_tokens: 40_000,
                cache_creation_tokens: 0,
            };
            let diag = detector.check_response(stats.clone()).unwrap();
            assert!(
                matches!(diag, CacheDiagnostic::Healthy { .. }),
                "warm-up turn {turn} must be Healthy, got {diag:?}"
            );
            assert!(detector.check_cache_health(&stats).is_none());
        }

        // Turn 4: the SAME 40k prefix is read back — nothing was invalidated —
        // but the turn carries 150k of new input, so the hit ratio is 0.21.
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let stats = CacheStats {
            input_tokens: 150_000,
            cache_read_tokens: 40_000,
            cache_creation_tokens: 0,
        };
        let diag = detector
            .check_response(stats.clone())
            .expect("detector must produce a diagnostic");
        assert!(
            matches!(diag, CacheDiagnostic::Healthy { .. }),
            "the whole prior context was read back from cache; nothing expired. \
             got {diag:?}"
        );
        assert!(
            detector.check_cache_health(&stats).is_none(),
            "a turn that read its entire prior context back must not fire a \
             cache-health alert: {:?}",
            detector.check_cache_health(&stats)
        );
        // wayland#1205/#1206: the verdict is DURABLE. `record_cache_ledger_turn`
        // runs `cause_of_diagnostic` into the ledger and `recording_enabled` is
        // on unless the env kill-switch is set, so a wrong verdict here becomes
        // an `expired` invalidation an operator reads back weeks later out of
        // `wayland cache`. Assert the recorded consequence, not only the
        // in-memory enum.
        assert_eq!(
            crate::cache_ledger::cause_of_diagnostic(&diag),
            None,
            "nothing was invalidated on this turn, so nothing may be written \
             to the cache ledger for it -- least of all Expired, which names \
             the server's TTL for a client-side event that did not happen"
        );
    }

    /// wayland#1206 c1/c2, in the criterion's own words: "a turn whose
    /// `cache_read` is unchanged from the previous turn while total input
    /// grows is not attributed to `TtlExpiry`", and "no
    /// `InvalidationCause::Expired` is written to the cache ledger for such a
    /// turn".
    ///
    /// The prior-context suppression shipped for #1206 does NOT cover this: it
    /// is a ratio against the previous turn's total input, so it only quiets
    /// the floor when the previous turn was itself well-covered. Here the
    /// previous turn read 40_000 back out of a 240_000-token context — a ratio
    /// of 0.167, below the threshold — so the suppression is disarmed and the
    /// floor fires. The floor firing is CORRECT; 40k of coverage on a 240k
    /// context is genuinely poor. What is not correct is the cause: nothing
    /// expired, because an expiry destroys cached tokens and `cache_read` is
    /// identical turn over turn.
    #[test]
    fn flat_cache_read_under_growing_input_is_not_attributed_to_expiry() {
        let mut detector = CacheBreakDetector::new();
        let trace = [
            (200_000u64, 40_000u64),
            (200_000, 40_000),
            (300_000, 40_000),
        ];
        let mut last = None;
        let mut last_stats = None;
        for (input_tokens, cache_read_tokens) in trace {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            let stats = CacheStats {
                input_tokens,
                cache_read_tokens,
                cache_creation_tokens: 0,
            };
            last = detector.check_response(stats.clone());
            last_stats = Some(stats);
        }
        let stats = last_stats.unwrap();

        // Premises, asserted so this test cannot pass by drifting out of the
        // shape it is about.
        assert!(
            !cached_prefix_covers_prior_context(40_000, 240_000),
            "premise: the #1206 prior-context suppression must be DISARMED \
             here, or this proves nothing about the floor's fall-through"
        );
        assert!(
            !cache_read_dropped(40_000, 40_000),
            "premise: cache_read is identical turn over turn, so nothing was \
             invalidated"
        );

        let diag = last.expect("detector must produce a diagnostic");
        assert!(
            matches!(
                diag,
                CacheDiagnostic::PartialMiss {
                    cause: CacheBreakCause::PrefixNotCached,
                    ..
                }
            ),
            "c1: cache_read was unchanged while total input grew -- the cache \
             never covered this context, and blaming the server's TTL for it \
             is #1166's own Defect 4. got {diag:?}"
        );
        assert_eq!(
            crate::cache_ledger::cause_of_diagnostic(&diag),
            Some(wcore_providers::cache_observation::InvalidationCause::PrefixNotCached),
            "c2: the durable record must not say `expired` for a turn on which \
             nothing was invalidated"
        );

        // Both halves apply one rule, or the ledger and the telemetry disagree.
        let alert = detector
            .check_cache_health(&stats)
            .expect("the floor fired, so the alerting half must speak too");
        assert_eq!(
            alert.cause,
            CacheBreakCause::PrefixNotCached,
            "the detecting half stopped blaming the TTL and the alerting half \
             did not -- that is #1166 Defect 2"
        );
    }

    /// Known-positive control for the test above: a REAL expiry must still be
    /// called `TtlExpiry` and still be written to the ledger as `expired`.
    ///
    /// Rewriting the fall-through is only a fix if it is narrow. An expiry is
    /// visible precisely because it destroys cached tokens — `cache_read`
    /// FALLS while the request itself is unchanged. Here the prefix is
    /// identical and `cache_read` collapses 40_000 -> 0. If this ever reports
    /// `PrefixNotCached`, wayland#1206's fix has blinded the detector to the
    /// event it is named for.
    #[test]
    fn a_real_expiry_is_still_attributed_to_the_ttl() {
        let mut detector = CacheBreakDetector::new();
        for _ in 0..3 {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            detector.check_response(CacheStats {
                input_tokens: 500,
                cache_read_tokens: 40_000,
                cache_creation_tokens: 0,
            });
        }
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let stats = CacheStats {
            input_tokens: 40_500,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        assert!(
            cache_read_dropped(0, 40_000),
            "premise: this is a real drop, which is what an expiry looks like"
        );
        let diag = detector.check_response(stats).unwrap();
        assert!(
            matches!(
                diag,
                CacheDiagnostic::FullMiss {
                    cause: CacheBreakCause::TtlExpiry
                }
            ),
            "an unchanged request that stopped reading its cache back IS the \
             TTL case; #1206 must not swallow it. got {diag:?}"
        );
        assert_eq!(
            crate::cache_ledger::cause_of_diagnostic(&diag),
            Some(wcore_providers::cache_observation::InvalidationCause::Expired),
            "and it must still be recorded as `expired`"
        );
    }

    /// Control on the #1166 c5 suppression: it must not open a blind spot.
    ///
    /// The suppression fires when `cache_read / previous total input` is at or
    /// above the threshold. There is a window where that is true and the turn's
    /// own hit ratio is still under the floor — it needs the turn's input to
    /// more than triple — and a REAL partial invalidation can land inside it.
    /// Here the previous turn read 40_000 back and this one reads 13_000
    /// (coverage 13_000 / 40_500 = 0.32, so the suppression is armed) while the
    /// turn carries 137_000 of new input (hit ratio 0.087, under the floor).
    /// Two-thirds of the cached prefix is gone; that is a break and must be
    /// reported by BOTH halves.
    #[test]
    fn a_real_drop_inside_the_suppression_window_is_still_reported() {
        let mut detector = CacheBreakDetector::new();

        for _ in 0..3 {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            let stats = CacheStats {
                input_tokens: 500,
                cache_read_tokens: 40_000,
                cache_creation_tokens: 0,
            };
            detector.check_response(stats.clone());
            assert!(detector.check_cache_health(&stats).is_none());
        }

        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let stats = CacheStats {
            input_tokens: 137_000,
            cache_read_tokens: 13_000,
            cache_creation_tokens: 0,
        };
        assert!(
            cached_prefix_covers_prior_context(13_000, 40_500),
            "the premise of this control is that the suppression IS armed here; \
             if it is not, the test is no longer probing the window"
        );
        let diag = detector.check_response(stats.clone()).unwrap();
        assert!(
            matches!(diag, CacheDiagnostic::PartialMiss { .. }),
            "two thirds of the cached prefix was lost; that is a partial miss, \
             not a large new input. got {diag:?}"
        );
        assert!(
            detector.check_cache_health(&stats).is_some(),
            "the detecting half reported PartialMiss and the alerting half said \
             nothing -- that is #1166 Defect 2, and the two halves are supposed \
             to apply one rule"
        );
    }

    /// #1166 c4 — the snapshot must describe the request actually DISPATCHED.
    ///
    /// The criterion is positional: it is about which side of the smart-routing
    /// tier swap and the transient tail injections `record_request` sits on.
    /// It had been anchored on a bare `engine.rs` line number, which resolves
    /// against any file long enough and had already drifted onto an unrelated
    /// block by the time this was re-graded — so the criterion was carrying an
    /// anchor that could never fail. This lint is the anchor instead.
    ///
    /// Snapshotted before the swap, `request.model` is the pre-swap model and a
    /// genuinely cold pool reports `TtlExpiry` (ticket Defect 4); snapshotted
    /// before the tail injections, the #559 prefix mutation — which lived in
    /// exactly one of them — is invisible to attribution (ticket Defect 3).
    #[test]
    fn the_snapshot_is_taken_after_the_tier_swap_and_the_transient_injections() {
        const ENGINE: &str = include_str!("engine.rs");
        const SWAP: &str = "request.model = tier_model.clone();";
        const HINT: &str = "Self::attach_transient_block(last, hint);";
        // The THIRD transient injection. It was missing, so the lint said
        // nothing about a hook contribution landing after the snapshot — the
        // same defect it exists to catch, one injection over.
        const PREPROMPT: &str =
            "Self::apply_pre_prompt_contribution(&mut request.messages, &outcome);";
        const RECORD: &str = "self.cache_detector.record_request(";

        // Controls: each marker must be present exactly once, so a rename
        // reddens this test instead of making it pass vacuously on an absence.
        for (label, marker) in [
            ("swap", SWAP),
            ("hint", HINT),
            ("preprompt", PREPROMPT),
            ("record", RECORD),
        ] {
            assert_eq!(
                ENGINE.matches(marker).count(),
                1,
                "{label} marker `{marker}` is not in engine.rs exactly once; \
                 this lint can no longer say anything about the ordering"
            );
        }

        let swap = ENGINE.find(SWAP).unwrap();
        let hint = ENGINE.find(HINT).unwrap();
        let preprompt = ENGINE.find(PREPROMPT).unwrap();
        let record = ENGINE.find(RECORD).unwrap();

        // A byte offset only stands in for execution order while all four
        // markers share one function body — move `record_request` into a
        // helper defined lower in the file but CALLED earlier and the offsets
        // would still read in order while the snapshot ran first. Nothing
        // between the first marker and the last may open a new `fn`.
        let span = &ENGINE[hint.min(swap).min(preprompt)..record];
        for boundary in [
            "\n    fn ",
            "\n    pub fn ",
            "\n    async fn ",
            "\n    pub async fn ",
        ] {
            assert!(
                !span.contains(boundary),
                "a `{}` boundary now sits between the transient injections and \
                 the snapshot, so these byte offsets no longer imply execution \
                 order and this lint cannot say anything about #1166 c4",
                boundary.trim()
            );
        }

        assert!(
            record > preprompt,
            "cache_detector.record_request runs BEFORE the PrePrompt hook \
             contribution is applied, so a hook that mutates the prefix is \
             invisible to attribution (#1166 Defect 3)"
        );
        assert!(
            record > swap,
            "cache_detector.record_request runs BEFORE the smart-routing tier \
             swap, so the snapshot names the pre-swap model and a cold pool is \
             attributed to the server's TTL (#1166 Defect 4)"
        );
        assert!(
            record > hint,
            "cache_detector.record_request runs BEFORE the skill-router hint \
             is attached, so a prefix mutation in a transient tail injection is \
             invisible to attribution (#1166 Defect 3, and the #559 cause)"
        );
    }

    /// Control for the warm-session gate on the absolute floor. Turns 1 and 2
    /// are the cache WARM-UP: the prefix has only just been written and a low
    /// read ratio there is normal, not a break. Only from turn 3 may the floor
    /// fire — the same window `check_cache_health` uses. Without this gate the
    /// detector reports a break on the cold open of every session.
    #[test]
    fn cold_turns_inside_the_warmup_window_are_not_flagged() {
        let mut detector = CacheBreakDetector::new();

        // Turn 1 — cold: the prefix is written, nothing is read back.
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let t1 = detector
            .check_response(CacheStats {
                input_tokens: 10_000,
                cache_read_tokens: 0,
                cache_creation_tokens: 9_000,
            })
            .unwrap();
        assert!(
            matches!(t1, CacheDiagnostic::Healthy { .. }),
            "turn 1 (cold) got {t1:?}"
        );

        // Turn 2 — still inside the warm-up window: a low ratio here is the
        // prefix taking its second chance to land, not a break.
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let t2 = detector
            .check_response(CacheStats {
                input_tokens: 10_000,
                cache_read_tokens: 100,
                cache_creation_tokens: 0,
            })
            .unwrap();
        assert!(
            matches!(t2, CacheDiagnostic::Healthy { .. }),
            "turn 2 (warm-up window) got {t2:?}"
        );

        // Turn 3 — warm. The identical ratio is now a finding.
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let t3 = detector
            .check_response(CacheStats {
                input_tokens: 10_000,
                cache_read_tokens: 100,
                cache_creation_tokens: 0,
            })
            .unwrap();
        assert!(
            matches!(t3, CacheDiagnostic::PartialMiss { .. }),
            "turn 3 (warm) got {t3:?}"
        );
    }

    // --- #1166 attribution ---

    /// The #559 root cause, reproduced: the first user message carries a
    /// volatile `Current date:` prefix on turn 1 and loses it on turn 2. Model,
    /// system prompt and tool definitions are all identical, so before the
    /// message array was an input this was laundered into `TtlExpiry` — i.e.
    /// blamed on the server for a mutation we made ourselves.
    #[test]
    fn mutated_prefix_message_is_attributed_with_its_index() {
        let mut detector = CacheBreakDetector::new();

        let mut volatile = make_messages(3);
        volatile[1].content = vec![ContentBlock::Text {
            text: "Current date: 2026-08-28\n\nmessage 1".into(),
        }];

        detector.record_request("model-a", "prompt", &make_tools(), &volatile);
        detector.check_response(CacheStats {
            input_tokens: 3_000,
            cache_read_tokens: 8_000,
            cache_creation_tokens: 2_000,
        });

        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(3));
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 10_000,
                cache_read_tokens: 0,
                cache_creation_tokens: 10_000,
            })
            .unwrap();

        match diag {
            CacheDiagnostic::FullMiss { cause } => assert_eq!(
                cause,
                CacheBreakCause::MessagesChanged {
                    first_divergent_index: 1
                }
            ),
            other => panic!("expected FullMiss, got {other:?}"),
        }
    }

    /// A smart-routing tier swap moves the lookup to a different cache pool.
    /// That is a client-side decision; reporting it as `TtlExpiry` blames the
    /// server for our own routing.
    #[test]
    fn model_swap_is_attributed_to_the_model_not_the_server() {
        let mut detector = CacheBreakDetector::new();

        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        detector.check_response(CacheStats {
            input_tokens: 3_000,
            cache_read_tokens: 8_000,
            cache_creation_tokens: 2_000,
        });

        detector.record_request("model-cheap", "prompt", &make_tools(), &make_messages(2));
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 10_000,
                cache_read_tokens: 0,
                cache_creation_tokens: 10_000,
            })
            .unwrap();

        match diag {
            CacheDiagnostic::FullMiss { cause } => {
                assert_eq!(cause, CacheBreakCause::ModelChanged)
            }
            other => panic!("expected FullMiss, got {other:?}"),
        }
    }

    /// The discriminator that keeps `MessagesChanged` honest. APPENDING is what
    /// every ordinary turn does and it leaves the cached prefix intact, so a
    /// miss on an append-only turn must fall through to `TtlExpiry`. Without
    /// this, the message hash would name a cause on literally every turn and
    /// the detector would be as useless in the other direction.
    #[test]
    fn appending_a_message_is_not_a_message_change() {
        let mut detector = CacheBreakDetector::new();

        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(4));
        detector.check_response(CacheStats {
            input_tokens: 3_000,
            cache_read_tokens: 8_000,
            cache_creation_tokens: 2_000,
        });

        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(6));
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 10_000,
                cache_read_tokens: 0,
                cache_creation_tokens: 10_000,
            })
            .unwrap();

        match diag {
            CacheDiagnostic::FullMiss { cause } => {
                assert_eq!(cause, CacheBreakCause::TtlExpiry)
            }
            other => panic!("expected FullMiss, got {other:?}"),
        }
    }

    /// `mark_cache_boundaries` re-marks the tail every turn, so the breakpoint
    /// marker moves between messages constantly. The marker is not prompt
    /// CONTENT and must be excluded from the message hash — including it would
    /// fire `MessagesChanged` on every single turn of a healthy session.
    #[test]
    fn moving_a_cache_breakpoint_is_not_a_message_change() {
        let mut detector = CacheBreakDetector::new();

        let mut turn1 = make_messages(4);
        turn1[1].cache_breakpoint = Some(MessageCacheHint::Breakpoint);
        turn1[0].timestamp = Some(chrono::Utc::now());
        detector.record_request("model-a", "prompt", &make_tools(), &turn1);
        detector.check_response(CacheStats {
            input_tokens: 3_000,
            cache_read_tokens: 8_000,
            cache_creation_tokens: 2_000,
        });

        let mut turn2 = make_messages(4);
        turn2[3].cache_breakpoint = Some(MessageCacheHint::Breakpoint);
        detector.record_request("model-a", "prompt", &make_tools(), &turn2);
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 10_000,
                cache_read_tokens: 0,
                cache_creation_tokens: 10_000,
            })
            .unwrap();

        match diag {
            CacheDiagnostic::FullMiss { cause } => {
                assert_eq!(cause, CacheBreakCause::TtlExpiry)
            }
            other => panic!("expected FullMiss, got {other:?}"),
        }
    }

    /// The full #1166 shape: a flat, non-zero `cache_read` on a warm session is
    /// both DETECTED (not `Healthy`) and ATTRIBUTED (a named cause), where
    /// before it produced `Healthy` and `distinct_causes=0`.
    #[test]
    fn flat_cache_read_is_detected_and_attributed() {
        let mut detector = CacheBreakDetector::new();

        // Turns 1-2: warm-up. The prefix is stable and the cache is already
        // dead flat at 192 — the measured #559 signature.
        for _ in 0..2 {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            detector.check_response(CacheStats {
                input_tokens: 6_000,
                cache_read_tokens: 192,
                cache_creation_tokens: 0,
            });
        }

        // Turn 3: warm, and the prefix mutates in place at index 0.
        let mut mutated = make_messages(2);
        mutated[0].content = vec![ContentBlock::Text {
            text: "Current date: 2026-08-28\n\nmessage 0".into(),
        }];
        detector.record_request("model-a", "prompt", &make_tools(), &mutated);
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 6_256,
                cache_read_tokens: 192,
                cache_creation_tokens: 0,
            })
            .unwrap();

        match diag {
            CacheDiagnostic::PartialMiss { hit_rate, cause } => {
                assert!(hit_rate < CACHE_HEALTH_WARN_RATIO, "hit_rate={hit_rate}");
                assert_eq!(
                    cause,
                    CacheBreakCause::MessagesChanged {
                        first_divergent_index: 0
                    }
                );
            }
            other => panic!("expected PartialMiss with a named cause, got {other:?}"),
        }
    }

    /// The two halves are now one path: `check_cache_health` fired correctly on
    /// the #559 session while the diagnostic half recorded `Healthy, 0 causes`.
    /// The alert must carry the same attribution the diagnostic does.
    #[test]
    fn health_alert_carries_the_attributed_cause() {
        let mut detector = CacheBreakDetector::new();

        for _ in 0..2 {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            detector.check_response(CacheStats {
                input_tokens: 6_000,
                cache_read_tokens: 192,
                cache_creation_tokens: 0,
            });
        }

        let stats = CacheStats {
            input_tokens: 6_256,
            cache_read_tokens: 192,
            cache_creation_tokens: 0,
        };
        detector.record_request("model-cheap", "prompt", &make_tools(), &make_messages(2));
        detector.check_response(stats.clone());
        let alert = detector
            .check_cache_health(&stats)
            .expect("warm turn with a dead cache must fire cache_health_warn");
        assert_eq!(alert.cause, CacheBreakCause::ModelChanged);
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
