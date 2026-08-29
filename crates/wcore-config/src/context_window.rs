//! THE KERNEL — the single per-turn context-window computation.
//!
//! `% full` and the pre-flight overflow ceiling must be computed against the
//! window of the model that will ACTUALLY serve THIS request — i.e. the
//! post-swap effective model — not a stale `CompactConfig` default (the #255
//! "false context window size" bug, where a 200k default denominator survived
//! a Flux/tier swap down to a 128k model).
//!
//! There is exactly ONE division (in [`ContextWindow::fraction`]). Every other
//! consumer (the overflow guard today; the #279 gauge and #280 autocompact
//! trigger as follow-ons) derives its number from this struct and never
//! re-divides. The window is `Option<u64>` on purpose: an unknown model yields
//! `None`, which forbids fabricating a denominator and makes every downstream
//! consumer fail open rather than guard/display against a wrong number.
//!
//! Placement: this module lives in `wcore-config` next to
//! [`crate::limits::model_output_ceiling`], the only per-model window table in
//! the tree. Co-locating adds zero cross-crate edges and no dep cycle — the
//! kernel calls a sibling module. `wcore-agent` (overflow guard, autocompact
//! trigger) and `wcore-cli` (TUI) already depend on `wcore-config`.
//! `wcore-protocol` deliberately does NOT depend on `wcore-config`, so it
//! cannot call the kernel: protocol transports the computed integer percent as
//! an opaque serde number, matching the observability crate's decoupling.

use crate::limits::{flux_tier_context_window, model_output_ceiling};

/// One turn's assembled-tokens-over-active-window view.
///
/// Construct once per turn via [`ContextWindow::resolve`] immediately AFTER the
/// model swap so `model` is the post-swap effective model. Recompute-on-swap is
/// therefore structural — there is no stored state to invalidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextWindow {
    /// Assembled input tokens for this request (`estimate_request_tokens`).
    pub used_tokens: u64,
    /// The active model's REAL context window. `None` = unknown model with no
    /// usable config fallback; downstream consumers fail open on `None`.
    pub window: Option<u64>,
}

/// The model's STATIC context window: its real window if known, else the
/// conservative Flux tier-alias floor.
///
/// Extracted because three production paths need exactly this pair of lookups
/// and two of them had only the first half. `model_output_ceiling` returns
/// `None` for the four Flux tier aliases BY DESIGN — #112/#426 keep them
/// unknown to the OUTPUT lookup so `size_output_cap` stays conservative and
/// `should_omit_max_tokens` keeps omitting the wire field — so any caller that
/// wants a WINDOW must also consult [`flux_tier_context_window`]. Calling only
/// the first is silently wrong for Flux and looks completely correct.
///
/// The two tables stay SEPARATE (see `flux_tier_context_window`'s own doc for
/// why merging them would revoke both contracts); this composes them at the
/// one place callers actually need, instead of at three.
///
/// `None` means genuinely unknown — callers must fail open and never fabricate
/// a denominator.
pub fn static_context_window(provider: &str, model: &str) -> Option<u64> {
    model_output_ceiling(provider, model)
        .map(|(_, ctx)| u64::from(ctx))
        .or_else(|| flux_tier_context_window(model).map(u64::from))
}

impl ContextWindow {
    /// THE KERNEL. Resolve the active model's window for this turn.
    ///
    /// `provider` / `model` are the POST-swap effective values (the same pair
    /// fed to `size_output_cap`). A KNOWN model's real window
    /// ([`model_output_ceiling`]`.1`) ALWAYS wins — this is the #255 root-cause
    /// fix: a swapped-in gpt-4o (128k) must not be measured against a stale
    /// 200k default. A Flux tier alias (`flux-auto` / `flux-fast` /
    /// `flux-standard` / `flux-reasoning`) resolves to the conservative
    /// 128k pool-minimum floor ([`flux_tier_context_window`], CORE-4) and,
    /// like a known model, beats `config_window` — a 200k config default over
    /// a tier that can route to a 128k backend is exactly the wedge where
    /// compaction never fired and a session grew to 17M cumulative input
    /// (callers that receive the real served-model window from Flux, #282,
    /// override this struct's `window` directly and still win).
    /// `config_window` is a fail-open fallback used ONLY when the
    /// model is unknown AND the user supplied a positive override (their TOML
    /// `context_window`); when both are absent the window is `None` and no
    /// denominator is fabricated.
    pub fn resolve(used_tokens: u64, provider: &str, model: &str, config_window: u64) -> Self {
        let window = static_context_window(provider, model).or(if config_window > 0 {
            Some(config_window)
        } else {
            None
        });
        ContextWindow {
            used_tokens,
            window,
        }
    }

    /// The ONLY division. `used / window`. `None` when the window is unknown or
    /// zero (defensive — `resolve` already refuses a zero fallback). Returns
    /// `> 1.0` on overflow on purpose (not clamped): the overflow guard relies
    /// on `used >= ceiling` firing and the gauge should show the truth.
    pub fn fraction(&self) -> Option<f64> {
        let w = self.window?;
        if w == 0 {
            return None;
        }
        Some(self.used_tokens as f64 / w as f64)
    }

    /// Integer percent full. Thin wrapper over [`fraction`](Self::fraction); no
    /// re-division. `> 100` on overflow (intentionally unclamped).
    pub fn percent(&self) -> Option<u32> {
        self.fraction().map(|f| (f * 100.0).round() as u32)
    }

    /// Pre-flight input ceiling = window − output_reserve − emergency_buffer,
    /// with both reserves SCALED to this window
    /// ([`CompactConfig::scaled_reserves`]). `None` when the window is unknown —
    /// the overflow guard then SKIPS (fail open), identical to the old
    /// `window > 0` skip, with `size_output_cap`'s UNKNOWN_CAP + the provider
    /// 400 as backstops.
    ///
    /// # Why this takes the config and not two numbers (#1179)
    ///
    /// It used to take `output_reserve` and `emergency_buffer` as bare `u64`s,
    /// and every caller passed `config.output_reserve` / `config.emergency_buffer`
    /// unscaled. Those absolutes were tuned for a 200,000-token window; against
    /// the 4,096-token slot #1172 measured they saturate the ceiling to zero and
    /// the guard fires on every turn. Taking the config means there is no
    /// unscaled spelling of this left for a caller to reach for by accident —
    /// the scaling is not something a call site can forget.
    pub fn input_ceiling(&self, config: &crate::compact::CompactConfig) -> Option<u64> {
        let w = self.window?;
        Some(config.input_ceiling_for_window(w as usize) as u64)
    }
}

// ---------------------------------------------------------------------------
// FerroxLabs/wayland#1172 — the SERVED window, learned from what came back
// ---------------------------------------------------------------------------

/// The `reported / estimated` ratio below which a turn is judged to have been
/// truncated by the endpoint rather than merely over-estimated by us.
///
/// CALIBRATED FROM MEASUREMENT, not from a model of the estimator. #1172's
/// reproduction drove a real `qwen3:8b` on stock Ollama through a logging
/// reverse proxy and captured every request body next to the `usage` block it
/// came back with (`/root/w3/proxylog{1,2,3,4}` on hetzner). Across the 24
/// turns the endpoint served IN FULL the ratio never left **0.839..0.902** —
/// Core's `char/4` estimator runs consistently ~15% high, which is what holds
/// the healthy band well under 1.0. The one grossly-truncated turn in the same
/// corpus measured **0.391** (10,466 tokens estimated sent, `prompt_tokens`
/// 4,095 came back, Ollama's journal logged `truncated = 1`).
///
/// 0.60 sits between 0.391 and 0.839 with room on both sides. It is
/// deliberately NOT tight: a ratio test alone cannot separate a *marginal*
/// truncation from estimator noise — the same corpus contains a turn that was
/// truncated at 0.839, inside the healthy band — which is why
/// [`TruncationSignal::Regression`] exists and why this arm additionally
/// requires [`MIN_SHORTFALL_TOKENS`].
pub const SERVED_SHORTFALL_RATIO: f64 = 0.60;

/// Absolute corroboration for [`TruncationSignal::Shortfall`]: the endpoint
/// must have come up short by at least this many tokens.
///
/// The ratio alone can be dragged down by content whose real tokenization is
/// much denser than `char/4` (long runs of repeated whitespace, for example).
/// Requiring an absolute shortfall too means such content must ALSO be large
/// in absolute terms before it can be mistaken for truncation. The measured
/// truncated turn was 6,371 tokens short.
pub const MIN_SHORTFALL_TOKENS: u64 = 1_024;

/// Turns reporting fewer prompt tokens than this carry no usable evidence:
/// integer noise dominates, and no real served slot is this small.
pub const MIN_OBSERVABLE_INPUT_TOKENS: u64 = 512;

/// Which measured signature said the endpoint discarded part of the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationSignal {
    /// The endpoint reported processing far fewer prompt tokens than we sent
    /// — below [`SERVED_SHORTFALL_RATIO`] and short by at least
    /// [`MIN_SHORTFALL_TOKENS`]. This is the gross case: measured at 0.391 on
    /// a turn where Ollama kept 4,095 of ~10,466 tokens.
    Shortfall,
    /// The reported prompt count went DOWN while the prompt we sent grew.
    ///
    /// Prompt tokens are a monotone function of the prompt for any fixed
    /// tokenizer, so appending to the conversation cannot reduce them. When it
    /// does, the server dropped content. This is the arm that catches
    /// truncation the ratio test cannot: measured at 4,050 → 3,910 reported
    /// across a +128-token append, a ratio of 0.847 that sits INSIDE the
    /// healthy band.
    Regression,
}

/// What the endpoint told us about itself, and the window it implies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServedWindowEvidence {
    /// Which signature fired.
    pub signal: TruncationSignal,
    /// Our estimate of the prompt we sent on this turn.
    pub sent_estimate: u64,
    /// The prompt tokens the endpoint reported processing on this turn.
    pub reported_input: u64,
    /// Best LOWER BOUND on the served slot: the largest prompt the endpoint
    /// has been observed to actually process on this route.
    pub served_window: u64,
}

/// Learns an endpoint's SERVED context window from the `usage` it already
/// returns — no probe, no extra request, no endpoint sniffing.
///
/// # Why this is not a probe
///
/// The model's ADVERTISED window is not the number that binds. #1172 measured
/// `qwen3:8b` advertising 40,960 (`/api/show`) while the loaded slot was 4,096
/// (`ollama ps`, `n_ctx_slot = 4096`), and only `/api/ps` reports the slot.
/// Two earlier attempts to reach for that figure were backed out: probing the
/// endpoint means deciding WHICH endpoints to probe, and every mock server in
/// this workspace binds `127.0.0.1`, so "the endpoint is loopback" cannot
/// separate a real self-hosted server from a test fixture.
///
/// The response we already receive carries the answer. `usage.prompt_tokens`
/// is what the server actually processed; when it is materially less than what
/// we sent, the difference was discarded — and with llama.cpp's `n_keep = 4`
/// the discarded head is the system prompt and the user's task.
///
/// # Per-route
///
/// Observations are keyed to a provider/model route string and thrown away
/// when it changes: a different tokenizer shifts the reported count by a few
/// percent, which is the same order as the [`TruncationSignal::Regression`]
/// arm, so comparing across a model swap would manufacture evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServedWindowTracker {
    /// Provider/model the observations below belong to.
    route: Option<String>,
    /// Previous turn's `(sent_estimate, reported_input)` on this route.
    prev: Option<(u64, u64)>,
    /// Largest prompt this route has been observed to actually process.
    max_reported: u64,
    /// Set once truncation has been OBSERVED. `None` means "no evidence" —
    /// never "the window is fine".
    served_window: Option<u64>,
}

impl ServedWindowTracker {
    /// Record one completed turn. Returns evidence exactly on the turns where
    /// the learned served window is newly established or moves, so a caller
    /// can tell the user once per figure instead of once per turn.
    ///
    /// `route` must be the POST-swap provider/model pair that actually served
    /// the turn. `sent_estimate` is our own count of the assembled request;
    /// `reported_input` is the provider's total input for the same turn
    /// (`TokenUsage::total_input_tokens` — cached reads included, so a prompt
    /// cache cannot masquerade as a shortfall).
    pub fn observe(
        &mut self,
        route: &str,
        sent_estimate: u64,
        reported_input: u64,
    ) -> Option<ServedWindowEvidence> {
        if self.route.as_deref() != Some(route) {
            *self = Self {
                route: Some(route.to_string()),
                ..Self::default()
            };
        }
        // A turn with no reported usage is evidence in neither direction, and
        // recording it as `prev` would fabricate a regression on the next one.
        if reported_input == 0 || sent_estimate == 0 {
            return None;
        }
        let prev = self.prev.replace((sent_estimate, reported_input));
        self.max_reported = self.max_reported.max(reported_input);

        if reported_input < MIN_OBSERVABLE_INPUT_TOKENS {
            return None;
        }

        let regressed = matches!(
            prev,
            Some((prev_sent, prev_reported))
                if prev_reported >= MIN_OBSERVABLE_INPUT_TOKENS
                    && sent_estimate > prev_sent
                    && reported_input < prev_reported
        );
        let signal = if regressed {
            TruncationSignal::Regression
        } else if sent_estimate.saturating_sub(reported_input) >= MIN_SHORTFALL_TOKENS
            && (reported_input as f64) < sent_estimate as f64 * SERVED_SHORTFALL_RATIO
        {
            TruncationSignal::Shortfall
        } else {
            return None;
        };

        let served_window = self.max_reported;
        if self.served_window == Some(served_window) {
            // Already established and unchanged — the user has been told.
            return None;
        }
        self.served_window = Some(served_window);
        Some(ServedWindowEvidence {
            signal,
            sent_estimate,
            reported_input,
            served_window,
        })
    }

    /// The served window learned from observation, or `None` when this session
    /// has seen no truncation. `None` is "no evidence", never "fine".
    pub fn served_window(&self) -> Option<u64> {
        self.served_window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_known_model_uses_real_window_not_config() {
        // #255 root-cause assertion: a KNOWN model overrides the stale 200k
        // config default. gpt-4o-mini's real window is 128k.
        let ctx = ContextWindow::resolve(1_000, "openai", "gpt-4o-mini", 200_000);
        assert_eq!(ctx.window, Some(128_000));
    }

    #[test]
    fn resolve_unknown_model_falls_back_to_config() {
        // A genuinely unknown model -> fail open to the user/config value, NOT
        // a hardcoded 200k inside the kernel. (flux-auto no longer qualifies:
        // the tier aliases resolve to the CORE-4 floor, tested below.)
        let ctx = ContextWindow::resolve(1_000, "some-provider", "mystery-model", 200_000);
        assert_eq!(ctx.window, Some(200_000));
    }

    #[test]
    fn resolve_unknown_model_and_zero_config_is_none() {
        // No real window and no positive config -> no fabricated denominator.
        let ctx = ContextWindow::resolve(1_000, "some-provider", "mystery-model", 0);
        assert_eq!(ctx.window, None);
    }

    #[test]
    fn resolve_flux_tier_aliases_get_conservative_floor() {
        // CORE-4: every Flux tier alias resolves to the 128k pool-minimum
        // floor even with NO config fallback — this is the denominator the
        // smart-compact trigger divides by, so compaction now fires
        // proactively instead of the session growing until
        // `finish_reason: length` (customer hit 17M cumulative input).
        for alias in ["flux-auto", "flux-fast", "flux-standard", "flux-reasoning"] {
            let ctx = ContextWindow::resolve(1_000, "flux-router", alias, 0);
            assert_eq!(
                ctx.window,
                Some(128_000),
                "{alias} must resolve the conservative 128k floor"
            );
        }
        // Provider-independent: the customer-log path reaches Flux through the
        // plain `openai` provider key.
        let ctx = ContextWindow::resolve(1_000, "openai", "flux-auto", 0);
        assert_eq!(ctx.window, Some(128_000));
        // Case-insensitive, like every other model match in limits.rs.
        let ctx = ContextWindow::resolve(1_000, "flux-router", "Flux-Auto", 0);
        assert_eq!(ctx.window, Some(128_000));
    }

    #[test]
    fn flux_tier_floor_beats_larger_config_window() {
        // The wedge scenario: a 200k config default over a tier alias that can
        // route to a 128k backend meant `used/200k` never crossed the trigger.
        // The conservative floor must win over config, exactly like a known
        // model's real window does (#255 doctrine).
        let ctx = ContextWindow::resolve(96_000, "openai", "flux-auto", 200_000);
        assert_eq!(ctx.window, Some(128_000));
        // 96k/128k = 0.75 -> above the smart-compact trigger band (0.60-0.70);
        // against the stale 200k it was 0.48 and never fired.
        assert_eq!(ctx.percent(), Some(75));
    }

    #[test]
    fn known_model_wins_over_differing_config_window() {
        // A TOML context_window=200_000 must NOT override the real 128k of a
        // swapped gpt-4o (the #255 fix). config_window is fallback-only.
        let ctx = ContextWindow::resolve(1_000, "openai", "gpt-4o", 200_000);
        assert_eq!(ctx.window, Some(128_000));
    }

    #[test]
    fn fraction_and_percent_basic() {
        let ctx = ContextWindow {
            used_tokens: 64_000,
            window: Some(128_000),
        };
        assert_eq!(ctx.fraction(), Some(0.5));
        assert_eq!(ctx.percent(), Some(50));
    }

    #[test]
    fn fraction_overflow_exceeds_one_not_clamped() {
        // 250k against gpt-4o's 128k -> > 100% shown, not hidden.
        let ctx = ContextWindow {
            used_tokens: 250_000,
            window: Some(128_000),
        };
        assert!(ctx.fraction().unwrap() > 1.0);
        assert_eq!(ctx.percent(), Some(195));
    }

    #[test]
    fn fraction_unknown_window_is_none() {
        let ctx = ContextWindow {
            used_tokens: 1_000,
            window: None,
        };
        assert_eq!(ctx.fraction(), None);
        assert_eq!(ctx.percent(), None);
    }

    #[test]
    fn fraction_zero_tokens() {
        let ctx = ContextWindow {
            used_tokens: 0,
            window: Some(128_000),
        };
        assert_eq!(ctx.fraction(), Some(0.0));
        assert_eq!(ctx.percent(), Some(0));
    }

    #[test]
    fn fraction_zero_window_no_div_by_zero() {
        // Defensive: even if a zero window reaches the struct, no panic.
        let ctx = ContextWindow {
            used_tokens: 1_000,
            window: Some(0),
        };
        assert_eq!(ctx.fraction(), None);
    }

    #[test]
    fn input_ceiling_known_fires_on_gpt4o_where_200k_would_not() {
        let cfg = crate::compact::CompactConfig::default();
        let ctx = ContextWindow {
            used_tokens: 110_000,
            window: Some(128_000),
        };
        let ceiling = ctx.input_ceiling(&cfg);
        // 128_000 is far above the #1179 scaling crossover (60_000), so the
        // reserves apply in full and this is the number it always was.
        assert_eq!(ceiling, Some(105_000));
        // 110_000 >= 105_000 -> the #255 guard fires; the old 200k-based
        // ceiling (177_000) would have let it through (false negative).
        assert!(ctx.used_tokens >= ceiling.unwrap());
    }

    #[test]
    fn input_ceiling_unknown_is_none() {
        let cfg = crate::compact::CompactConfig::default();
        let ctx = ContextWindow {
            used_tokens: 110_000,
            window: None,
        };
        assert_eq!(ctx.input_ceiling(&cfg), None);
    }

    /// #1179 — this used to assert `Some(0)`, i.e. that a window smaller than
    /// the absolute reserves produced a ceiling of zero "without underflowing".
    /// Zero is not a safe answer on this path: `used >= ceiling` is true of
    /// every turn, including an empty one, so the #255 guard fires immediately
    /// and aborts the run. The saturating subtraction never underflowed and
    /// never helped.
    ///
    /// With the reserves scaled to the window the case cannot arise: the
    /// ceiling is at least `(1 - MAX_RESERVE_FRACTION)` of the window for any
    /// positive window, so it is positive whenever the window is.
    #[test]
    fn input_ceiling_is_positive_for_every_positive_window() {
        let cfg = crate::compact::CompactConfig::default();
        for window in [1_000u64, 4_096, 8_192, 32_768, 60_000, 128_000, 200_000] {
            let ctx = ContextWindow {
                used_tokens: 0,
                window: Some(window),
            };
            let ceiling = ctx.input_ceiling(&cfg).expect("a known window");
            assert!(
                ceiling > 0,
                "a zero ceiling fires the #255 guard on an empty turn; window {window}"
            );
            assert!(
                ceiling as f64 >= window as f64 * (1.0 - crate::compact::MAX_RESERVE_FRACTION),
                "window {window} kept only {ceiling} tokens of input budget"
            );
        }
    }
}
