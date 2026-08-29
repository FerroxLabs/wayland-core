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
    /// Total input the turn BEFORE the one just recorded processed.
    ///
    /// [`Self::check_response`] overwrites `prev_stats` with the turn it is
    /// checking, so by the time [`Self::check_cache_health`] runs for the same
    /// turn there is no longer a handle on the preceding one. The coverage test
    /// in [`Self::prefix_was_read_back`] needs exactly that number, so it is
    /// kept separately rather than reconstructed.
    prior_total_input_tokens: u64,
    /// #1166 Defect 5 — has the user already been told, in this session, that
    /// the prompt cache is not being reused? See [`Self::claim_health_notice`].
    health_notice_emitted: bool,
}

impl CacheBreakDetector {
    pub fn new() -> Self {
        Self {
            prev_snapshot: None,
            current_snapshot: None,
            prev_stats: None,
            round_trips: 0,
            seen_cache_tokens: false,
            prior_total_input_tokens: 0,
            health_notice_emitted: false,
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
        self.prior_total_input_tokens = self
            .prev_stats
            .as_ref()
            .map_or(0, CacheStats::total_input_tokens);
        self.prev_stats = Some(stats);
        Some(diagnostic)
    }

    /// Was the prefix the provider had ALREADY processed read back out of the
    /// cache on this turn?
    ///
    /// #1166 — the hit ratio `cache_read / total_input` charges this turn's
    /// brand-new input against the cache, but new content was never written to
    /// the cache and so cannot be read from it. A warm session where the user
    /// pastes 150k of fresh text therefore scores `40_000 / 190_000 = 0.21`
    /// and trips the absolute floor, even though the entire cached prefix came
    /// back verbatim — a false `PartialMiss` that the engine then writes into
    /// the durable ledger as an `expired` invalidation, blaming the server's
    /// TTL for a turn on which nothing was invalidated at all.
    ///
    /// Measure coverage against what the PREVIOUS turn processed instead: that
    /// is the part which could have been cached. This does not blunt the floor
    /// the #559 leader session motivated — a `cache_read` pinned at 192 while
    /// the conversation grows covers none of the previous turn either, so it
    /// still fires.
    fn prefix_was_read_back(&self, cache_read_tokens: u64, prior_total_input: u64) -> bool {
        prior_total_input > 0
            && cache_read_tokens as f64 >= CACHE_HEALTH_WARN_RATIO * prior_total_input as f64
    }

    /// `true` the FIRST time it is called in a session, `false` for ever after.
    ///
    /// #1166 Defect 5 — "even a correct verdict is silent unless explicitly
    /// enabled". The three per-turn `emit_info` lines the engine has are gated
    /// on `compact.cache_diagnostics`, which defaults to `false`; the
    /// `cache_health_warn` that is NOT gated is a `tracing::warn!`, and with
    /// `RUST_LOG` unset the CLI routes everything below ERROR to a log file
    /// (and in TUI mode to the file only), so it never reaches the person
    /// running the session. The ledger records it, but only an operator who
    /// already knows to run `wayland-core cache report` will ever see that.
    ///
    /// So the verdict is surfaced without an opt-in — ONCE. Repeating it every
    /// turn of a broken session is the noise that made `cache_diagnostics`
    /// opt-in in the first place, and flipping that flag on by default would
    /// print a hit-rate line on every healthy turn too.
    pub fn claim_health_notice(&mut self) -> bool {
        !std::mem::replace(&mut self.health_notice_emitted, true)
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
        // #1166 — same discriminator as the absolute floor, so the detecting
        // half and the recording half cannot disagree about whether this turn
        // was a break. See [`Self::prefix_was_read_back`].
        if self.prefix_was_read_back(stats.cache_read_tokens, self.prior_total_input_tokens) {
            return None;
        }
        // #1166 — attribute, do not just report. `current_snapshot` is always
        // set on the engine path (`check_response` returns `None` without it);
        // an unrecorded request has no previous turn to diverge from, which is
        // exactly what `FirstRequest` means.
        let cause = match self.current_snapshot.as_ref() {
            Some(current) => self.attribute_cause(current),
            None => CacheBreakCause::FirstRequest,
        };
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
        if prev.cache_read_tokens > 0 {
            let drop_pct = 1.0 - (stats.cache_read_tokens as f64 / prev.cache_read_tokens as f64);
            if drop_pct > 0.05 {
                let cause = self.attribute_cause(current);
                return CacheDiagnostic::PartialMiss { hit_rate, cause };
            }
        }

        // #1166 — the absolute floor. Every test above measures this turn
        // against the PREVIOUS turn, so a cache that never worked at all is
        // invisible to them: total breakage is perfectly STABLE turn over turn.
        // The measured #559 leader session sat at `cache_read = 192` flat for
        // its whole life and was classified `Healthy` on every turn. Measure
        // the ratio against this turn's own input as well, behind the same
        // warm-session and provider-supports-caching gates
        // `check_cache_health` uses, so the two halves cannot disagree.
        //
        // #1166 (follow-up) — but a low ratio is only a FINDING when the
        // already-processed prefix went unread. `prefix_was_read_back` is that
        // discriminator; without it a warm turn carrying a large new paste is
        // reported as `PartialMiss { cause: TtlExpiry }` and recorded in the
        // ledger as a durable `expired` invalidation on a turn where the whole
        // cached prefix was served.
        if self.round_trips >= CACHE_HEALTH_WARM_AFTER_ROUND_TRIPS
            && self.seen_cache_tokens
            && total_input_tokens > 0
            && hit_rate < CACHE_HEALTH_WARN_RATIO
            && !self.prefix_was_read_back(stats.cache_read_tokens, prev.total_input_tokens())
        {
            let cause = self.attribute_cause(current);
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

    /// #1166 follow-up — the absolute floor must not manufacture an
    /// `expired` finding on a turn where the cache worked perfectly.
    ///
    /// Three healthy warm turns read a 40k prefix back in full. On turn 4 the
    /// user pastes 150k of new text. The SAME 40k prefix is still served — the
    /// provider read back everything it had — but `cache_read / total_input`
    /// falls to 0.21, under the 0.3 floor. Before this fix the detector
    /// returned `PartialMiss { cause: TtlExpiry }`, `check_cache_health` fired
    /// an alert with the same cause, and `cause_of_diagnostic` turned that into
    /// a DURABLE `InvalidationCause::Expired` row in the on-disk cache ledger —
    /// blaming the server's TTL for an invalidation that never happened.
    ///
    /// The positive control for this discriminator is
    /// `flat_cache_read_on_warm_session_is_not_healthy` above: a `cache_read`
    /// pinned at 192 covers none of the previous turn either, and must keep
    /// firing.
    #[test]
    fn a_large_new_paste_does_not_fake_a_ttl_expiry() {
        let mut detector = CacheBreakDetector::new();

        // Turns 1-3: warm and healthy, 40k prefix read back against a small
        // new input each turn.
        for _ in 0..3 {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            let stats = CacheStats {
                input_tokens: 500,
                cache_read_tokens: 40_000,
                cache_creation_tokens: 0,
            };
            detector.check_response(stats.clone());
            assert!(
                detector.check_cache_health(&stats).is_none(),
                "a warm turn at a 0.99 ratio must not warn"
            );
        }

        // Turn 4: the whole 40k prefix is STILL read back — `cache_read` did
        // not move — but 150k of brand-new input arrives with it.
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let stats = CacheStats {
            input_tokens: 150_000,
            cache_read_tokens: 40_000,
            cache_creation_tokens: 0,
        };
        let diag = detector.check_response(stats.clone()).expect("diagnostic");

        assert!(
            matches!(diag, CacheDiagnostic::Healthy { .. }),
            "the full cached prefix was served; got {diag:?}"
        );
        assert_eq!(
            crate::cache_ledger::cause_of_diagnostic(&diag),
            None,
            "#1166: nothing was invalidated, so nothing may be written to the \
             ledger — least of all `expired`"
        );
        assert!(
            detector.check_cache_health(&stats).is_none(),
            "the detecting half must agree with the recording half"
        );
    }

    /// The same shape one level up: whatever the diagnostic half decides, the
    /// health probe must decide too. A turn that is graded `Healthy` may not
    /// simultaneously fire a `cache_health_warn`, and a turn graded a miss must
    /// fire one. Before #1166'"'"'s follow-up the two disagreed on exactly the
    /// large-new-input shape.
    #[test]
    fn the_floor_and_the_health_probe_never_disagree() {
        // (input_tokens, cache_read_tokens) for four warm turns, covering both
        // polarities: a served prefix swamped by new input, and a dead cache.
        for trace in [
            [
                (500u64, 40_000u64),
                (500, 40_000),
                (500, 40_000),
                (150_000, 40_000),
            ],
            [(3_225, 192), (6_028, 192), (6_256, 192), (7_001, 192)],
        ] {
            let mut detector = CacheBreakDetector::new();
            let mut last: Option<(CacheDiagnostic, Option<CacheHealthAlert>)> = None;
            for (input_tokens, cache_read_tokens) in trace {
                detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
                let stats = CacheStats {
                    input_tokens,
                    cache_read_tokens,
                    cache_creation_tokens: 0,
                };
                let diag = detector.check_response(stats.clone()).expect("diagnostic");
                let alert = detector.check_cache_health(&stats);
                last = Some((diag, alert));
            }
            let (diag, alert) = last.expect("four turns were driven");
            let diagnostic_says_broken = !matches!(diag, CacheDiagnostic::Healthy { .. });
            assert_eq!(
                diagnostic_says_broken,
                alert.is_some(),
                "diagnostic {diag:?} and health alert {alert:?} disagree for trace {trace:?}"
            );
        }
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
