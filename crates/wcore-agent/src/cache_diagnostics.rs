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
    /// wayland#1206 — the cache did not serve this turn, nothing we can
    /// observe about the request explains it, and the numbers positively
    /// REFUTE the server: `cache_read` did not FALL from the previous turn's,
    /// so the provider dropped nothing between the two.
    ///
    /// [`CacheBreakCause::TtlExpiry`] is a positive claim about the server's
    /// TTL, and an expiry DESTROYS cached tokens — it always shows up as a
    /// drop. What happened instead is that the cache never covered the context
    /// this turn needed: the #559 leader session read 192 tokens back, flat,
    /// for its whole life while its input climbed. This is the honest
    /// fall-through the floor needs, and it maps to
    /// `InvalidationCause::PrefixNotCached` rather than to `expired`.
    ///
    /// Named `PrefixNotCached` rather than `Unattributed` (the name the
    /// dur-ledgers lane reached for the same state) because the branch that
    /// produces it carries positive evidence, not a shrug: the prefix was not
    /// carrying the session on the PREVIOUS turn either, and it did not fall
    /// on this one. `prefix_not_cached` is already the published vocabulary
    /// for that; `unknown` would discard what the detector actually knows.
    PrefixNotCached,
    FirstRequest,
}

/// #1206 — why the [`CacheBreakCause::TtlExpiry`] fall-through was rejected on
/// the turn just checked. Carried from the diagnostic half to the alert half so
/// the two cannot disagree about the same turn — the measured #1206 turn
/// produced `PartialMiss { cause: TtlExpiry }` AND an alert carrying
/// `TtlExpiry`, and a fix that silenced only one of them would leave the other
/// certifying the same falsehood.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefutedTtl {
    /// The prefix came back token-for-token, was already carrying the turn on
    /// its own last turn, AND this turn's total input grew. The low ratio is
    /// this turn's NEW input, not a break: nothing was invalidated, so there is
    /// nothing to report.
    PrefixReused,
    /// `cache_read` did not fall, so no invalidation can be inferred — but the
    /// prefix was not serving the session last turn either, so this is still a
    /// finding. It is reported as a coverage gap on our side rather than as a
    /// server-side expiry (the #559 dead-flat `cache_read = 192` signature
    /// lives here).
    PrefixNotCached,
}

/// #1206 — the verdict the DIAGNOSTIC half reached about one turn, carried
/// to the alert half so the two can never disagree about it.
///
/// This exists because enumerating the sites that attribute was not enough. The
/// lane closed two of them inside [`CacheBreakDetector::compute_diagnostic`]
/// and a third survived in [`CacheBreakDetector::check_cache_health`], which
/// re-derived a cause of its own whenever the diagnostic half had reached
/// none. A cause is now decided in exactly ONE function and read everywhere
/// else, so "the alert names something the diagnostic did not" is not a bug to
/// be found again, it is a state that cannot be constructed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TurnAttribution {
    /// The diagnostic half did not attribute this turn at all — it was the
    /// first request, the provider has never demonstrated prompt-cache
    /// support, or nothing about the turn was a finding. There is no verdict
    /// to report and the alert half must not invent one.
    NotAssessed,
    /// #1206 [`RefutedTtl::PrefixReused`] — the prefix came back
    /// token-for-token and was already carrying the session. Not a finding at
    /// either half.
    PrefixReused,
    /// The cause the diagnostic half reached for this turn. The alert half
    /// reports this value verbatim or reports nothing.
    Attributed(CacheBreakCause),
}

/// Fold one attribution into the diagnostic it implies, so the pair is built
/// in one step and cannot drift. `make` builds the finding when a cause was
/// actually named; a turn whose `TtlExpiry` was refuted as prefix reuse is
/// `Healthy` at BOTH halves.
fn finding(
    attribution: TurnAttribution,
    hit_rate: f64,
    make: impl FnOnce(CacheBreakCause) -> CacheDiagnostic,
) -> (CacheDiagnostic, TurnAttribution) {
    match attribution {
        TurnAttribution::Attributed(cause) => {
            (make(cause.clone()), TurnAttribution::Attributed(cause))
        }
        other => (CacheDiagnostic::Healthy { hit_rate }, other),
    }
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
    /// #1206 — the verdict the DIAGNOSTIC half reached for the turn most
    /// recently passed to [`check_response`]. `check_cache_health` runs AFTER
    /// `prev_stats` has already rotated to this turn's numbers, so it could not
    /// re-derive the comparison honestly even if it tried. It reads this, and
    /// it has no other path to a cause at all.
    last_attribution: TurnAttribution,
}

impl CacheBreakDetector {
    pub fn new() -> Self {
        Self {
            prev_snapshot: None,
            current_snapshot: None,
            prev_stats: None,
            round_trips: 0,
            seen_cache_tokens: false,
            last_attribution: TurnAttribution::NotAssessed,
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
        // #1206 — cleared BEFORE the `?` below can return. The comment that
        // stood here claimed the field was "set on EVERY turn"; the early
        // return made that false, so a turn that recorded no request left the
        // PREVIOUS turn's verdict standing for `check_cache_health` to read.
        self.last_attribution = TurnAttribution::NotAssessed;
        let current = self.current_snapshot.as_ref()?;
        let (diagnostic, attribution) = self.compute_diagnostic(current, &stats);
        self.last_attribution = attribution;
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
        // #1166/#1206 — READ the diagnostic half's verdict, never derive one.
        // This function has no path to `attribute_cause` any more, which is
        // what makes it impossible for the alert to name a cause the
        // diagnostic half did not reach. The third instance of the #1206 class
        // was exactly such a path: a bare `attribute_cause` in the arm taken
        // whenever the diagnostic half had reached no verdict at all.
        let cause = match &self.last_attribution {
            TurnAttribution::Attributed(cause) => cause.clone(),
            // A reused prefix is not a finding, and a turn the diagnostic half
            // never assessed has no cause to report. Silence, not invention.
            TurnAttribution::PrefixReused | TurnAttribution::NotAssessed => return None,
        };
        Some(CacheHealthAlert {
            round_trip: self.round_trips,
            input_tokens: total_input_tokens,
            cache_read_tokens: stats.cache_read_tokens,
            ratio,
            cause,
        })
    }

    /// Returns the diagnostic AND the attribution that produced it. This is
    /// the ONLY function in which a [`CacheBreakCause`] is decided;
    /// `check_cache_health` reads the second element rather than deriving one.
    fn compute_diagnostic(
        &self,
        current: &PromptSnapshot,
        stats: &CacheStats,
    ) -> (CacheDiagnostic, TurnAttribution) {
        let Some(prev) = &self.prev_stats else {
            // First request — no previous data to compare
            return (
                CacheDiagnostic::Healthy { hit_rate: 0.0 },
                TurnAttribution::NotAssessed,
            );
        };

        // If provider doesn't support caching (both turns have 0 cache tokens),
        // report healthy to avoid false alarms (e.g., OpenAI).
        //
        // #1206 — gated on the session never having seen cache tokens. A
        // session that HAS seen them is not a provider without cache support;
        // it is a cache that has gone completely dead, and taking this return
        // there both hid the finding and, by recording no verdict, left the
        // alert half free to invent one. That was the third #1206 instance.
        if !self.seen_cache_tokens
            && prev.cache_read_tokens == 0
            && prev.cache_creation_tokens == 0
            && stats.cache_read_tokens == 0
            && stats.cache_creation_tokens == 0
        {
            return (
                CacheDiagnostic::Healthy { hit_rate: 0.0 },
                TurnAttribution::NotAssessed,
            );
        }

        let prev_had_cache = prev.cache_read_tokens > 0 || prev.cache_creation_tokens > 0;

        // Calculate hit rate. Computed before the full-miss arm because every
        // arm below can now resolve to `Healthy` (#1206) and therefore needs it.
        let total_input_tokens = stats.total_input_tokens();
        let hit_rate = if total_input_tokens > 0 {
            stats.cache_read_tokens as f64 / total_input_tokens as f64
        } else {
            0.0
        };

        // Full miss: had cache before, now read tokens dropped to 0
        if prev_had_cache && stats.cache_read_tokens == 0 {
            return finding(self.attribute_break(current, stats), hit_rate, |cause| {
                CacheDiagnostic::FullMiss { cause }
            });
        }

        // Partial miss: cache_read dropped >5% compared to previous
        if prev.cache_read_tokens > 0 {
            let drop_pct = 1.0 - (stats.cache_read_tokens as f64 / prev.cache_read_tokens as f64);
            if drop_pct > 0.05 {
                // The #1206 shape cannot reach this arm — it requires
                // `cache_read` to have FALLEN by more than 5%, and the shape is
                // `cache_read` unchanged. Routed through the same attribution
                // anyway so there is one place where `TtlExpiry` is decided,
                // rather than three that have to be kept in agreement.
                return finding(self.attribute_break(current, stats), hit_rate, |cause| {
                    CacheDiagnostic::PartialMiss { hit_rate, cause }
                });
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
            return finding(self.attribute_break(current, stats), hit_rate, |cause| {
                CacheDiagnostic::PartialMiss { hit_rate, cause }
            });
        }

        (
            CacheDiagnostic::Healthy { hit_rate },
            TurnAttribution::NotAssessed,
        )
    }

    /// #1206 — attribute a break, and reject the `TtlExpiry` fall-through when
    /// this turn's numbers refute it.
    ///
    /// `TtlExpiry` is not a shrug: it is a positive claim that the SERVER
    /// dropped the cached prefix. A turn whose `cache_read` is unchanged from
    /// the previous turn's lost nothing — the provider handed back exactly the
    /// same number of cached tokens it handed back before — so when the ratio
    /// fell it fell because this turn's NEW input grew. Measured on the real
    /// module: three turns at `cache_read = 40,000 / input = 500` then one at
    /// `cache_read = 40,000 / input = 150,000` printed
    /// `PartialMiss { hit_rate: 0.2105, cause: TtlExpiry }` and wrote a durable
    /// `expired` invalidation for a turn on which the whole prefix was read
    /// back.
    ///
    /// Only the fall-through is refuted. A NAMED cause (model, system prompt,
    /// tools, a mutated message) is evidence we produced ourselves and it
    /// outranks this arithmetic — the #1166 attribution stands untouched.
    fn attribute_break(&self, current: &PromptSnapshot, stats: &CacheStats) -> TurnAttribution {
        let cause = self.attribute_cause(current);
        if cause != CacheBreakCause::TtlExpiry {
            return TurnAttribution::Attributed(cause);
        }
        match self.refute_ttl(stats) {
            Some(RefutedTtl::PrefixReused) => TurnAttribution::PrefixReused,
            Some(RefutedTtl::PrefixNotCached) => {
                TurnAttribution::Attributed(CacheBreakCause::PrefixNotCached)
            }
            None => TurnAttribution::Attributed(cause),
        }
    }

    /// #1206 — is the `TtlExpiry` claim refuted by this turn's own numbers, and
    /// if so, does anything remain to report?
    ///
    /// The refutation is the ticket's discriminator with its incidental half
    /// dropped: `cache_read` did not FALL from the previous turn. #1206 c1
    /// names the growing-input case because that is the shape that was
    /// measured, but growth is not what refutes the claim — a `cache_read`
    /// that did not fall is. Requiring growth as well left the identical
    /// falsehood reachable on a turn whose input happened to hold still or
    /// shrink. What is LEFT after it is the part the ticket does not decide,
    /// and it matters: the
    /// #559 leader session sat at `cache_read = 192` flat while its input grew
    /// every turn, which is the same shape and is emphatically not healthy. So
    /// the previous turn's own hit ratio breaks the tie — a prefix that was
    /// carrying the session last turn and came back intact is reuse, and one
    /// that was already failing to carry it is still a finding, just not the
    /// server's fault.
    fn refute_ttl(&self, stats: &CacheStats) -> Option<RefutedTtl> {
        let prev = self.prev_stats.as_ref()?;
        // The refutation itself, and the whole of it: the provider did not hand
        // back materially fewer cached tokens than it handed back last turn, so
        // it dropped nothing between the two. A genuine TTL expiry always MOVES
        // this number DOWN.
        //
        // The predicate is [`cache_read_dropped`], not equality, and that is
        // the lane/f13-misc closure of the same class preserved through this
        // structural rewrite. Equality alone leaves `TtlExpiry` reachable on a
        // turn whose `cache_read` slipped by less than the module's own
        // [`CACHE_READ_DROP_TOLERANCE`] — the very tolerance under which the
        // partial-miss arm above declines to call it a drop at all. A module
        // that says "this did not fall" in one arm may not say "the server
        // expired it" in the next. It also makes the property structural: the
        // partial-miss arm returns on every drop over the tolerance, so a
        // `TtlExpiry` reaching the floor is impossible by construction rather
        // than by the floor remembering to guard itself.
        if cache_read_dropped(stats.cache_read_tokens, prev.cache_read_tokens) {
            return None;
        }
        let prev_total = prev.total_input_tokens();
        let prev_hit_rate = if prev_total > 0 {
            prev.cache_read_tokens as f64 / prev_total as f64
        } else {
            0.0
        };
        // What is LEFT is the tie-break, and reuse needs BOTH halves of it: the
        // prefix was carrying the session last turn, AND the ratio fell only
        // because this turn's new input grew. Anything else is still a finding,
        // reported as a coverage gap on our side rather than as a false expiry.
        if prev_hit_rate >= CACHE_HEALTH_WARN_RATIO && stats.total_input_tokens() > prev_total {
            Some(RefutedTtl::PrefixReused)
        } else {
            Some(RefutedTtl::PrefixNotCached)
        }
    }

    /// Determine what caused the cache break by comparing prev vs current
    /// snapshots. Unguarded, and callable from exactly ONE place —
    /// [`Self::attribute_break`], which is where the `TtlExpiry` fall-through
    /// is refuted. `attribute_cause_is_called_from_exactly_one_place` pins
    /// that count so a new caller cannot quietly reopen the class.
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

    /// wayland#1206 c1/c2, SECOND instance — the FULL-miss arm.
    ///
    /// Found by the round-2 verifier. Strictly larger and cheaper to hit than
    /// the 240k trace round 1 closed: `prev_had_cache` is satisfied by
    /// cache_CREATION alone and this arm has no warm-session gate, so it is
    /// reached on TURN 2 of any session where the provider wrote a cache entry
    /// and did not serve it back — a below-minimum prefix, a rejected marker,
    /// a moved prefix. `cache_read` is 0 on both turns while total input grows
    /// 10_000 -> 30_000, which is c1's sentence verbatim, and the arm answered
    /// `FullMiss { TtlExpiry }` / `InvalidationCause::Expired`.
    #[test]
    fn a_full_miss_with_flat_zero_cache_read_is_not_attributed_to_expiry() {
        let mut detector = CacheBreakDetector::new();

        // Turn 1: the provider WRITES a prefix and serves nothing back.
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        detector.check_response(CacheStats {
            input_tokens: 10_000,
            cache_read_tokens: 0,
            cache_creation_tokens: 9_000,
        });

        // Turn 2: byte-identical request, still nothing served back, more input.
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let stats = CacheStats {
            input_tokens: 30_000,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };

        // Premises first, so this cannot pass by drifting off c1's shape.
        assert!(
            !cache_read_dropped(0, 0),
            "premise: cache_read is UNCHANGED turn over turn (0 -> 0), so no \
             cached tokens were destroyed and no expiry is visible"
        );
        assert!(
            stats.total_input_tokens() > 10_000,
            "premise: total input grew, which is the other half of c1"
        );

        let diag = detector.check_response(stats).unwrap();
        assert!(
            matches!(diag, CacheDiagnostic::FullMiss { .. }),
            "premise: this is the FULL-miss arm — the one that shipped with no \
             floor guard. got {diag:?}"
        );
        assert!(
            matches!(
                diag,
                CacheDiagnostic::FullMiss {
                    cause: CacheBreakCause::PrefixNotCached
                }
            ),
            "c1: cache_read was unchanged at 0 while total input grew 10_000 \
             -> 30_000. Nothing expired — the prefix was written and never \
             served. got {diag:?}"
        );

        // c2 — the DURABLE consequence, read off the function engine.rs feeds
        // into the ledger write, not inferred from the enum.
        let recorded = crate::cache_ledger::cause_of_diagnostic(&diag);
        assert_ne!(
            recorded,
            Some(wcore_providers::cache_observation::InvalidationCause::Expired),
            "c2: no InvalidationCause::Expired may be written for this turn"
        );
        assert_eq!(
            recorded,
            Some(wcore_providers::cache_observation::InvalidationCause::PrefixNotCached),
            "and the honest fall-through must be what lands in the ledger"
        );
    }

    /// Narrowness control for the arm above: the FULL-miss rewrite must only
    /// touch the `TtlExpiry` fall-through. A real divergence on a 0 -> 0 turn
    /// is a positive finding about THIS request and must survive — otherwise
    /// the #1206 repair becomes the #1166 Defect 4 it exists to undo, one
    /// cause over.
    #[test]
    fn a_full_miss_with_flat_zero_cache_read_still_names_a_real_divergence() {
        let mut detector = CacheBreakDetector::new();
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        detector.check_response(CacheStats {
            input_tokens: 10_000,
            cache_read_tokens: 0,
            cache_creation_tokens: 9_000,
        });

        // Same 0 -> 0 shape, but the dispatched model moved: a different cache
        // pool, and a client-side cause the detector knows by name.
        detector.record_request("model-b", "prompt", &make_tools(), &make_messages(2));
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 30_000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            })
            .unwrap();
        assert!(
            matches!(
                diag,
                CacheDiagnostic::FullMiss {
                    cause: CacheBreakCause::ModelChanged
                }
            ),
            "the fall-through rewrite must not launder a real attribution into \
             PrefixNotCached. got {diag:?}"
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
        // #559 c6 gave this call a third argument — `&self.compat`, which
        // decides whether the transient gets a carrier message of its own — so
        // rustfmt now spreads it across lines. The marker is the call HEAD,
        // still unique in this file: the unit tests below spell it
        // `super::AgentEngine::apply_pre_prompt_contribution(`.
        const PREPROMPT: &str = "Self::apply_pre_prompt_contribution(";
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

    // --- #1206: the TtlExpiry fall-through is refuted, not widened ---

    /// The measured #1206 turn, driven verbatim: three healthy turns at
    /// `cache_read = 40,000 / input = 500`, then one at `cache_read = 40,000`
    /// (the same full prefix, still read back) `/ input = 150,000`.
    ///
    /// Against today's floor this printed
    /// `PartialMiss { hit_rate: 0.2105, cause: TtlExpiry }` plus an alert
    /// carrying the same cause, and `cause_of_diagnostic` turned that into a
    /// durable `expired` invalidation. Nothing was invalidated: `cache_read` is
    /// identical to the previous turn's, so the provider dropped nothing.
    #[test]
    fn a_prefix_read_back_whole_under_a_flood_of_new_input_is_not_ttl_expiry() {
        let mut detector = CacheBreakDetector::new();
        for _ in 0..3 {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            detector.check_response(CacheStats {
                input_tokens: 500,
                cache_read_tokens: 40_000,
                cache_creation_tokens: 0,
            });
        }

        let stats = CacheStats {
            input_tokens: 150_000,
            cache_read_tokens: 40_000,
            cache_creation_tokens: 0,
        };
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let diag = detector.check_response(stats.clone()).unwrap();
        assert!(
            matches!(diag, CacheDiagnostic::Healthy { .. }),
            "the whole prefix came back; got {diag:?}"
        );
        assert_eq!(
            detector.check_cache_health(&stats),
            None,
            "the alert half must not certify what the diagnostic half just refuted"
        );
    }

    /// The other half of the same discriminator, and the reason it is not just
    /// "return Healthy". The #559 leader session sat at `cache_read = 192` flat
    /// while its input grew every turn — the SAME shape — and it was a dead
    /// cache, which is the whole reason the #1166 floor exists. The turn stays
    /// a finding; only the false blame on the server is removed.
    #[test]
    fn a_dead_flat_cache_under_growing_input_is_a_finding_that_is_not_the_servers_fault() {
        let mut detector = CacheBreakDetector::new();
        for i in 0..3u64 {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            detector.check_response(CacheStats {
                input_tokens: 6_000 + i * 1_000,
                cache_read_tokens: 192,
                cache_creation_tokens: 0,
            });
        }

        let stats = CacheStats {
            input_tokens: 9_000,
            cache_read_tokens: 192,
            cache_creation_tokens: 0,
        };
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let diag = detector.check_response(stats.clone()).unwrap();
        match diag {
            CacheDiagnostic::PartialMiss { cause, .. } => assert_eq!(
                cause,
                CacheBreakCause::PrefixNotCached,
                "a cache that never worked is still a finding, but the server did not lose it"
            ),
            other => panic!("expected the floor to still fire, got {other:?}"),
        }
        assert_eq!(
            detector.check_cache_health(&stats).map(|a| a.cause),
            Some(CacheBreakCause::PrefixNotCached),
            "the alert must carry the same cause the diagnostic reached"
        );
    }

    /// The SECOND instance of the #1206 shape, and the one a fix that only
    /// touched the floor would leave open: the full-miss arm attributed bare
    /// too. A turn that read nothing back, on a session whose previous turn
    /// also read nothing (it only WROTE cache), has an unchanged `cache_read`
    /// and therefore cannot be evidence that the server expired anything.
    #[test]
    fn a_full_miss_whose_cache_read_never_moved_is_not_the_servers_fault() {
        let mut detector = CacheBreakDetector::new();

        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        detector.check_response(CacheStats {
            input_tokens: 10_000,
            cache_read_tokens: 0,
            cache_creation_tokens: 9_000,
        });

        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let diag = detector
            .check_response(CacheStats {
                input_tokens: 40_000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            })
            .unwrap();

        match diag {
            CacheDiagnostic::FullMiss { cause } => assert_eq!(
                cause,
                CacheBreakCause::PrefixNotCached,
                "cache_read went 0 -> 0: nothing was dropped, so nothing expired"
            ),
            other => panic!("expected FullMiss, got {other:?}"),
        }
    }

    /// #1206 — the THIRD reachable instance of the class, found by an
    /// independent verifier after two were closed inside `compute_diagnostic`.
    /// Both of those ran through `attribute_break`; this one never reaches it.
    /// `compute_diagnostic`'s "provider doesn't support caching" early return
    /// fires whenever BOTH turns report zero cache tokens and recorded no
    /// verdict at all, so `check_cache_health` fell through to a bare
    /// `attribute_cause` and re-derived a cause of its own — on a turn the
    /// diagnostic half had just called `Healthy`.
    ///
    /// Driven through the module's real public pair in the order `engine.rs`
    /// uses it (`check_response` then `check_cache_health`): three warm turns
    /// at `cache_read = 40,000 / input = 500`, then turn 4 at all-zero cache
    /// with `input = 60,000`, then turn 5 at all-zero cache with
    /// `input = 90,000`. Across turns 4 and 5 `cache_read` is unchanged 0 -> 0
    /// while total input grows — #1206 c1's discriminator verbatim. Before the
    /// fix this printed an alert carrying `cause: TtlExpiry`, which
    /// `engine.rs` publishes as `cache_health_warn ... cause = "expired"`.
    #[test]
    fn a_second_dead_turn_cannot_manufacture_an_expiry_the_diagnostic_never_found() {
        let mut detector = CacheBreakDetector::new();
        for _ in 0..3 {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            detector.check_response(CacheStats {
                input_tokens: 500,
                cache_read_tokens: 40_000,
                cache_creation_tokens: 0,
            });
        }

        // Turn 4 — the cache genuinely dies: `cache_read` 40,000 -> 0. This
        // turn IS attributable; it is the turn after it that is not.
        let turn4 = CacheStats {
            input_tokens: 60_000,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        detector.check_response(turn4.clone());
        detector.check_cache_health(&turn4);

        // Turn 5 — `cache_read` unchanged 0 -> 0, total input 60,000 -> 90,000.
        let turn5 = CacheStats {
            input_tokens: 90_000,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let diag = detector.check_response(turn5.clone());
        let alert = detector.check_cache_health(&turn5);
        println!("PROBE diag={diag:?} alert={alert:?}");

        let alert_cause = alert.as_ref().map(|a| a.cause.clone());
        assert_ne!(
            alert_cause,
            Some(CacheBreakCause::TtlExpiry),
            "cache_read went 0 -> 0: the provider dropped nothing, so nothing expired"
        );
        assert_ne!(
            alert_cause
                .as_ref()
                .map(crate::cache_ledger::invalidation_cause_of),
            Some(wcore_providers::cache_observation::InvalidationCause::Expired),
            "the published vocabulary must not carry `expired` on this turn either"
        );
        assert_eq!(
            alert_cause,
            Some(CacheBreakCause::PrefixNotCached),
            "a cache that has gone dead on a session which demonstrably DOES support \
             caching is still a finding — just not the server's fault"
        );

        // The CLASS, not the instance: the alert half may only name a cause the
        // diagnostic half actually reached for the same turn.
        let diagnostic_cause = diag.as_ref().and_then(|d| match d {
            CacheDiagnostic::Healthy { .. } => None,
            CacheDiagnostic::PartialMiss { cause, .. } | CacheDiagnostic::FullMiss { cause } => {
                Some(cause.clone())
            }
        });
        assert_eq!(
            alert_cause, diagnostic_cause,
            "the two halves must not disagree about the same turn"
        );
    }

    /// The same shape with the growth clause removed, which is why the fix is
    /// not a fourth site-specific guard. `cache_read` is unchanged 0 -> 0 and
    /// total input SHRINKS, so #1206 c1's literal wording does not cover it —
    /// but the provider still dropped nothing, so `TtlExpiry` is still a claim
    /// about the server that the numbers refuse. Before the refutation was
    /// widened, routing this turn through `attribute_break` would have produced
    /// `TtlExpiry` and a durable `expired`.
    #[test]
    fn an_unchanged_cache_read_refutes_ttl_expiry_even_when_input_shrinks() {
        let mut detector = CacheBreakDetector::new();
        for _ in 0..3 {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            detector.check_response(CacheStats {
                input_tokens: 500,
                cache_read_tokens: 40_000,
                cache_creation_tokens: 0,
            });
        }

        // Turn 4 — the cache dies. `cache_read` MOVES, so this turn is a real
        // full miss and stays attributable.
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let t4 = detector
            .check_response(CacheStats {
                input_tokens: 90_000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            })
            .unwrap();
        assert!(
            matches!(
                t4,
                CacheDiagnostic::FullMiss {
                    cause: CacheBreakCause::TtlExpiry
                }
            ),
            "a cache_read that fell 40,000 -> 0 IS evidence the server dropped it: {t4:?}"
        );

        // Turn 5 — `cache_read` unchanged 0 -> 0, total input 90,000 -> 60,000.
        let turn5 = CacheStats {
            input_tokens: 60_000,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let diag = detector.check_response(turn5.clone()).unwrap();
        match diag {
            CacheDiagnostic::PartialMiss { cause, .. } => assert_eq!(
                cause,
                CacheBreakCause::PrefixNotCached,
                "cache_read held still, so nothing expired, whichever way input moved"
            ),
            other => panic!("expected the floor to fire, got {other:?}"),
        }
        assert_eq!(
            detector.check_cache_health(&turn5).map(|a| a.cause),
            Some(CacheBreakCause::PrefixNotCached)
        );
    }

    /// The CLASS, swept rather than spot-checked. Two instances were closed by
    /// enumerating attribution sites and a third survived, so the property is
    /// asserted over a grid of traces driven through the module's real public
    /// pair: `check_cache_health` may never name a cause `check_response` did
    /// not reach for the SAME turn. The grid covers cold opens, warm-up turns,
    /// dead-flat caches, full misses, partial drops, prefix reuse under a flood
    /// of new input, providers that never cache, and zero-input turns.
    #[test]
    fn the_alert_half_never_names_a_cause_the_diagnostic_half_did_not_reach() {
        fn cause_of(diag: Option<&CacheDiagnostic>) -> Option<CacheBreakCause> {
            match diag? {
                CacheDiagnostic::Healthy { .. } => None,
                CacheDiagnostic::PartialMiss { cause, .. }
                | CacheDiagnostic::FullMiss { cause } => Some(cause.clone()),
            }
        }

        let reads = [0u64, 128, 3_000, 40_000];
        let creations = [0u64, 9_000];
        let inputs = [0u64, 500, 10_000, 90_000];
        let mut alerts_seen = 0usize;
        let mut causes: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

        for early_read in reads {
            for early_creation in creations {
                for early_input in inputs {
                    for late_read in reads {
                        for late_input in inputs {
                            let trace = [
                                (early_input, early_read, early_creation),
                                (early_input, early_read, 0),
                                (early_input, early_read, 0),
                                (late_input, late_read, 0),
                            ];
                            let mut detector = CacheBreakDetector::new();
                            for (turn, (input, read, creation)) in trace.iter().enumerate() {
                                let stats = CacheStats {
                                    input_tokens: *input,
                                    cache_read_tokens: *read,
                                    cache_creation_tokens: *creation,
                                };
                                detector.record_request(
                                    "model-a",
                                    "prompt",
                                    &make_tools(),
                                    &make_messages(2),
                                );
                                let diag = detector.check_response(stats.clone());
                                let diagnostic_cause = cause_of(diag.as_ref());
                                if let Some(alert) = detector.check_cache_health(&stats) {
                                    alerts_seen += 1;
                                    causes.insert(format!("{:?}", alert.cause));
                                    assert_eq!(
                                        Some(alert.cause),
                                        diagnostic_cause,
                                        "turn {turn} of trace {trace:?}: the alert named a cause \
                                         the diagnostic half never reached"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        // Positive control: an invariant no trace exercises is vacuous, and a
        // grid that only ever produces one verdict is not a grid.
        assert!(
            alerts_seen > 0,
            "the sweep never fired a single alert, so it proved nothing"
        );
        assert!(
            causes.len() >= 2,
            "the sweep only ever produced {causes:?}, so it is not discriminating"
        );
    }

    /// The guard on the guard, and the reason this is not a fourth
    /// site-specific patch. Two instances of the #1206 class were closed by
    /// enumerating the places that attribute, and a verifier still found a
    /// third; so the count is pinned rather than trusted. `attribute_cause` is
    /// the only function that can return [`CacheBreakCause::TtlExpiry`], and it
    /// may be CALLED from exactly one place — `attribute_break`, which is where
    /// the refutation lives. Any new caller is a new chance to name a cause the
    /// diagnostic half never reached, and reddens this test on the spot.
    #[test]
    fn attribute_cause_is_called_from_exactly_one_place() {
        let src = include_str!("cache_diagnostics.rs");
        // Built at runtime so this test's own source does not match itself, and
        // comment lines are dropped so a doc comment quoting the call cannot be
        // mistaken for one.
        let needle = format!(".{}(", "attribute_cause");
        let mut current_fn = String::new();
        let mut sites: Vec<(String, String)> = Vec::new();
        for line in src.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with('*') {
                continue;
            }
            if let Some(rest) = trimmed
                .strip_prefix("fn ")
                .or_else(|| trimmed.strip_prefix("pub fn "))
            {
                current_fn = rest.split('(').next().unwrap_or_default().to_string();
            }
            if trimmed.contains(&needle) {
                sites.push((current_fn.clone(), trimmed.to_string()));
            }
        }

        assert_eq!(
            sites.len(),
            1,
            "exactly one call site is expected; found {sites:?}"
        );
        // Positive control on the scan itself: if the filter were broken the
        // assert above would have fired at zero, and if the enclosing-function
        // tracking were broken this names the wrong owner.
        assert_eq!(
            sites[0].0, "attribute_break",
            "the single call site must live in the function that refutes, got {sites:?}"
        );
    }

    /// The remaining shape in which the diagnostic half legitimately reaches no
    /// verdict while every one of the alert half's gates is open: the turn on
    /// which cache tokens FIRST appear. `compute_diagnostic` reads
    /// `seen_cache_tokens` as it stood BEFORE this response, so its floor is
    /// still shut; `check_cache_health` reads it updated, so its
    /// provider-supports-caching gate is open. Until the alert half became a
    /// pure reader, that one-turn asymmetry produced an alert carrying a
    /// freshly derived `TtlExpiry` on a turn the diagnostic half had called
    /// `Healthy` — the same class, on the cold open of every caching session.
    #[test]
    fn the_turn_cache_tokens_first_appear_on_is_not_alerted_from_a_derived_cause() {
        let mut detector = CacheBreakDetector::new();
        for _ in 0..2 {
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            detector.check_response(CacheStats {
                input_tokens: 10_000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            });
        }

        let stats = CacheStats {
            input_tokens: 10_000,
            cache_read_tokens: 100,
            cache_creation_tokens: 0,
        };
        detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
        let diag = detector.check_response(stats.clone()).unwrap();
        assert!(
            matches!(diag, CacheDiagnostic::Healthy { .. }),
            "the prefix has only just been written back; got {diag:?}"
        );
        assert_eq!(
            detector.check_cache_health(&stats),
            None,
            "the diagnostic half reached no verdict, so the alert half has none to report"
        );
    }

    /// #1206 c4 — a case that sits ON the floor's boundary rather than far from
    /// it. The two existing healthy tests run at ratios of ~0.99 and 0.8, so
    /// neither would notice the threshold moving. These two turns differ by one
    /// token of cached prefix across `CACHE_HEALTH_WARN_RATIO`.
    ///
    /// The graded property is the BOUNDARY — one token under the line is still
    /// a finding, one token over it is `Healthy` — not the cause. The cause
    /// below is `PrefixNotCached`, and on the merged tree it could not be
    /// anything else: the partial-miss arm returns on every drop over
    /// [`CACHE_READ_DROP_TOLERANCE`], so a turn that reaches the floor at all
    /// has by definition not lost cached tokens, and [`Self::refute_ttl`]
    /// refuses `TtlExpiry` there. A one-token slip from 3_000 to 2_999 is not
    /// an expiry; the module's own drop arm already declined to call it one.
    #[test]
    fn the_floor_still_bites_one_token_below_the_threshold() {
        fn warm(read_now: u64, uncached_now: u64) -> CacheDiagnostic {
            let mut detector = CacheBreakDetector::new();
            for _ in 0..3 {
                detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
                detector.check_response(CacheStats {
                    input_tokens: 7_000,
                    cache_read_tokens: 3_000,
                    cache_creation_tokens: 0,
                });
            }
            detector.record_request("model-a", "prompt", &make_tools(), &make_messages(2));
            detector
                .check_response(CacheStats {
                    input_tokens: uncached_now,
                    cache_read_tokens: read_now,
                    cache_creation_tokens: 0,
                })
                .unwrap()
        }

        // 2_999 / 10_000 = 0.2999 — one token under the line.
        match warm(2_999, 7_001) {
            CacheDiagnostic::PartialMiss { hit_rate, cause } => {
                assert!(hit_rate < CACHE_HEALTH_WARN_RATIO, "hit_rate={hit_rate}");
                assert_eq!(
                    cause,
                    CacheBreakCause::PrefixNotCached,
                    "the floor fired, so cache_read had not dropped past the \
                     module's own tolerance -- the server expired nothing"
                );
            }
            other => panic!("one token below the threshold must still be a finding: {other:?}"),
        }

        // 3_001 / 10_000 = 0.3001 — one token over it. This arm is `Healthy`
        // because the floor RELEASED, not because anything was refuted: the
        // refutation only ever rewrites a cause, it never suppresses a finding
        // except on a reused prefix, which this is not.
        let over = warm(3_001, 6_999);
        assert!(
            matches!(over, CacheDiagnostic::Healthy { .. }),
            "one token above the threshold is healthy: {over:?}"
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
