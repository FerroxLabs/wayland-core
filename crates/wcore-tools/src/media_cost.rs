//! F27-C3 — per-call cost record for billable media generation.
//!
//! # Why this exists
//!
//! Media generation is billable. Before this module the engine produced **no
//! cost record at all** for a media call: the only cost sink in the product is
//! `ProviderBudgetReservation::settle(input_tokens, output_tokens, cost_usd)`,
//! which is keyed to a provider dispatch with token counts. A media tool call
//! has no reservation, no dispatch and no tokens, so it fell through every
//! accounting path. A user could spend real money generating images and see
//! nothing.
//!
//! # The measured constraint this design is built around
//!
//! Phase 27 probed a live FluxRouter account with one real transcription and
//! one real image and captured the HTTP response headers of each:
//!
//! | shape          | cost in headers            | cost in body      |
//! |----------------|----------------------------|-------------------|
//! | transcription  | `x-flux-cost-usd`          | no                |
//! | **image**      | **none**                   | **none**          |
//! | chat (control) | `x-flux-cost-usd`          | `usage.cost_usd`  |
//!
//! **For an image, that provider returns no billing figure in any channel.**
//! So a dollar amount for an image call cannot come from the provider. Any
//! code that produces one has invented it.
//!
//! This module therefore separates two things that are normally conflated:
//!
//! * **Billable units** ([`MediaUnits`]) — what was actually performed. These
//!   are always observable, always recorded, and always vary with the work.
//! * **A dollar figure** ([`MediaCostRecord::cost_usd`]) — recorded only when
//!   something actually supplied one, and always stamped with a
//!   [`PriceSource`] saying *which channel it came from*.
//!
//! A reader can therefore never mistake an operator's local estimate for the
//! provider's own number, and an absent dollar figure is reported as
//! [`PriceSource::Unpriced`] with a reason rather than as `$0.00`.
//!
//! # What this module deliberately does NOT do
//!
//! * It does not default any price. An unconfigured install records units and
//!   `unpriced`, never a guess.
//! * It does not treat a failed call as free. A provider can bill for a
//!   rejected prompt, so a failure records
//!   [`UnpricedReason::CallFailedBillingUnknown`] — not `$0.00`.
//! * It does not route through the frontier scorecard's cost observable, which
//!   is recorded elsewhere in this programme as degenerate (invariant across
//!   conforming harnesses).

use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What a media call is billed *on*. Recorded explicitly rather than inferred
/// from which fields happen to be populated.
///
/// Inference was tried and rejected: a transcription whose provider declined to
/// report a duration has no populated unit field at all, and would be
/// indistinguishable from a token-billed vision call. Both would then land in
/// whichever bucket the inference happened to default to, and a summary that
/// silently files one billing basis under another is the same category of lie
/// as a `$0.00` — it reads as a measurement and is not one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingBasis {
    /// Billed per artifact produced — image generation.
    Artifacts,
    /// Billed per second of audio processed — transcription.
    Duration,
    /// Billed per token — the multimodal chat calls behind `vision_analyze`
    /// and, nine at a time, behind `video_analyze`.
    Tokens,
    /// Billed per character of input text — speech synthesis.
    Characters,
}

impl Default for BillingBasis {
    /// Artifacts, matching the only shape that existed when this type was
    /// introduced, so a record deserialized from an older host reads the way it
    /// did when it was written.
    fn default() -> Self {
        Self::Artifacts
    }
}

/// Billable units actually performed by one media call.
///
/// Every field here is observable without the provider pricing anything, and
/// every field varies with the work requested — which is the property that
/// makes this record a measurement rather than a constant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaUnits {
    /// What this call is billed on. See [`BillingBasis`].
    #[serde(default)]
    pub basis: BillingBasis,
    /// Number of media artifacts produced (images returned, clips rendered).
    /// Always known — it is the count of artifacts the caller received.
    ///
    /// **Stays 0 for every non-[`BillingBasis::Artifacts`] shape**, including
    /// a vision call that *consumed* an image. A vision call produces no
    /// artifact, and setting `images: 1` for one would let a per-artifact rate
    /// card price it as though it had generated an image — a wrong figure,
    /// which is worse than the `$0.00` this module already refuses.
    pub images: u32,
    /// Pixel dimensions, when the surface actually knows them.
    ///
    /// `None` is a real case, not a placeholder: the `wayland-core image`
    /// subcommand lets the size be omitted, in which case the provider picks
    /// and the response does not say. **A zero sentinel was rejected here** —
    /// `0x0` reads as a measurement and would flow into a megapixel total as
    /// if the call had produced nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Seconds of audio/video billed, when the shape has a duration and the
    /// provider reports one. `None` for still images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billed_seconds: Option<f64>,
    /// Prompt tokens the provider reported for a token-billed call. `None`
    /// means the provider did not report a count — never `0`, which would
    /// claim it processed an empty prompt on a call it charged for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u32>,
    /// Completion tokens the provider reported for a token-billed call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u32>,
    /// Characters of input text submitted to a character-billed call
    /// (speech synthesis). Counted locally from the text we sent, so unlike
    /// the token counts this is always known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billed_characters: Option<u32>,
}

impl MediaUnits {
    /// Shared zero value. Every constructor below starts here and sets only
    /// the fields its billing basis actually has, so a new field can never
    /// silently default to a value that reads as a measurement.
    fn empty(basis: BillingBasis) -> Self {
        Self {
            basis,
            images: 0,
            width: None,
            height: None,
            billed_seconds: None,
            input_tokens: None,
            output_tokens: None,
            billed_characters: None,
        }
    }

    /// One still image at the given pixel dimensions.
    pub fn one_image(width: u32, height: u32) -> Self {
        Self {
            images: 1,
            width: Some(width),
            height: Some(height),
            ..Self::empty(BillingBasis::Artifacts)
        }
    }

    /// `n` artifacts whose pixel dimensions this surface does not know.
    /// The count is still recorded, because the count is still billable.
    pub fn images_of_unknown_size(images: u32) -> Self {
        Self {
            images,
            ..Self::empty(BillingBasis::Artifacts)
        }
    }

    /// `n` artifacts at a known size.
    pub fn images_at(images: u32, width: u32, height: u32) -> Self {
        Self {
            images,
            width: Some(width),
            height: Some(height),
            ..Self::empty(BillingBasis::Artifacts)
        }
    }

    /// A token-billed multimodal call — the shape behind `vision_analyze`, and
    /// behind every one of the nine provider calls a single `video_analyze`
    /// fans out into.
    ///
    /// `images` stays 0 deliberately: the call *consumed* an image and
    /// *produced* text. See the field comment on [`Self::images`].
    pub fn tokens(input_tokens: Option<u32>, output_tokens: Option<u32>) -> Self {
        Self {
            input_tokens,
            output_tokens,
            ..Self::empty(BillingBasis::Tokens)
        }
    }

    /// A character-billed call — speech synthesis, which OpenAI and ElevenLabs
    /// both price per character of input text.
    pub fn text_characters(characters: u32) -> Self {
        Self {
            billed_characters: Some(characters),
            ..Self::empty(BillingBasis::Characters)
        }
    }

    /// A duration-billed call that produces **no image artifact** —
    /// transcription and speech synthesis, which providers bill per second of
    /// audio rather than per artifact.
    ///
    /// `images: 0` is the honest count here, not a placeholder: a transcription
    /// genuinely produces zero images. That makes `images` unusable as a
    /// pricing multiplier for this shape, which is why
    /// [`MediaCostRecord::for_success`] refuses to apply a per-image rate card
    /// to it rather than multiplying by zero and reporting `$0.00`.
    pub fn audio_seconds(seconds: f64) -> Self {
        Self {
            billed_seconds: Some(seconds),
            ..Self::empty(BillingBasis::Duration)
        }
    }

    /// A duration-billed call whose duration the provider did not report.
    ///
    /// Distinct from [`Self::audio_seconds`] with `0.0`, which would claim the
    /// provider processed zero seconds of audio. This says "we do not know",
    /// matching the `Option` discipline the rest of the module uses.
    pub fn audio_of_unknown_duration() -> Self {
        Self::empty(BillingBasis::Duration)
    }

    /// True when this call is billed per second of audio.
    ///
    /// Reads the declared [`BillingBasis`] rather than inferring from
    /// `billed_seconds`, because a provider that declines to report a duration
    /// does not thereby turn a transcription into an image call — and if it
    /// did, the call would land in `calls_of_unknown_size`, a figure that is
    /// supposed to mean "we produced pixels but could not measure them".
    ///
    /// This previously keyed on `images == 0`, which was correct while audio
    /// was the only zero-artifact shape. It is not any more: vision is
    /// token-billed and speech synthesis is character-billed, and both also
    /// have `images == 0`.
    pub fn is_duration_billed(&self) -> bool {
        self.basis == BillingBasis::Duration
    }

    /// True when this call is billed per token — the vision / `video_analyze`
    /// shape.
    pub fn is_token_billed(&self) -> bool {
        self.basis == BillingBasis::Tokens
    }

    /// True when this call is billed per character of input text — speech
    /// synthesis.
    pub fn is_character_billed(&self) -> bool {
        self.basis == BillingBasis::Characters
    }

    /// Total tokens the provider reported, or `None` when it reported neither
    /// count. `Some` when either is present, so a provider that reports only
    /// one of the two still contributes a real figure.
    pub fn total_tokens(&self) -> Option<u32> {
        match (self.input_tokens, self.output_tokens) {
            (None, None) => None,
            (i, o) => Some(i.unwrap_or(0).saturating_add(o.unwrap_or(0))),
        }
    }

    /// Total megapixels produced, or `None` when the dimensions are unknown.
    /// This is the unit most image providers price on, and it separates
    /// `landscape` (1536x1024) from `square` (1024x1024) — i.e. it changes
    /// when the requested work changes.
    ///
    /// Returns `Option` rather than `0.0` so an unknown size can never be
    /// summed into a session total as if it were free.
    pub fn megapixels(&self) -> Option<f64> {
        let (w, h) = (self.width?, self.height?);
        Some((f64::from(w) * f64::from(h) * f64::from(self.images)) / 1_000_000.0)
    }
}

/// Where a dollar figure came from. **Never infer this** — it is the field
/// that stops an operator's local estimate being read as the provider's own
/// number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PriceSource {
    /// The provider returned the cost in the named HTTP response header.
    ProviderHeader { header: String },
    /// The provider returned the cost at the named JSON path in the body.
    ProviderBody { field: String },
    /// Priced locally from an operator-configured rate card. **This is an
    /// estimate the operator supplied, not the provider's own figure.**
    LocalRateCard { entry: String },
    /// No dollar figure is available. Carries the reason so a reader can tell
    /// "nobody reports a price for this" from "this call blew up".
    Unpriced { reason: UnpricedReason },
}

impl PriceSource {
    /// True when the figure came from the provider itself rather than from a
    /// local estimate. Hosts should render these two differently.
    pub fn is_provider_reported(&self) -> bool {
        matches!(
            self,
            PriceSource::ProviderHeader { .. } | PriceSource::ProviderBody { .. }
        )
    }
}

/// Why a call carries no dollar figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnpricedReason {
    /// The provider returned no cost in any channel (measured true of
    /// FluxRouter image generation) and no local rate-card entry matched.
    /// The call still happened and was still billable.
    ProviderReportsNoCost,
    /// The call did not return media. **Whether the provider billed for it is
    /// unknown** — a rejected prompt can still be charged — so this is
    /// explicitly not `$0.00`.
    CallFailedBillingUnknown,
}

/// Outcome of the media call the record describes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MediaOutcome {
    Ok,
    /// `category` is the backend's stable error class (e.g. `prompt_rejected`,
    /// `insufficient_credits`) so failures are comparable across presentations.
    Failed {
        category: String,
    },
}

/// A cost figure a backend actually observed on the wire. Backends construct
/// this **only** when the provider genuinely returned a number; they must not
/// synthesise one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportedCost {
    pub usd: f64,
    pub source: PriceSource,
}

impl ReportedCost {
    /// Cost read from an HTTP response header.
    pub fn from_header(header: impl Into<String>, usd: f64) -> Self {
        Self {
            usd,
            source: PriceSource::ProviderHeader {
                header: header.into(),
            },
        }
    }

    /// Cost read from a JSON field in the response body.
    pub fn from_body(field: impl Into<String>, usd: f64) -> Self {
        Self {
            usd,
            source: PriceSource::ProviderBody {
                field: field.into(),
            },
        }
    }
}

/// The record produced by exactly one billable media call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaCostRecord {
    /// Tool that made the call, e.g. `image_generate`.
    pub tool: String,
    /// Which backend served it. This is what distinguishes the built-in,
    /// MCP-served and combined presentations of the same capability.
    pub backend_id: String,
    /// Model or endpoint identifier the backend reported.
    pub model: String,
    pub units: MediaUnits,
    pub outcome: MediaOutcome,
    /// `None` whenever nothing supplied a figure. Never defaulted to `0.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub price_source: PriceSource,
}

impl MediaCostRecord {
    /// Assemble a record for a call that produced media.
    ///
    /// Resolution order, highest authority first:
    /// 1. a figure the provider actually returned (`reported`);
    /// 2. an operator-configured rate-card entry matching `backend_id`;
    /// 3. unpriced, with [`UnpricedReason::ProviderReportsNoCost`].
    pub fn for_success(
        tool: impl Into<String>,
        backend_id: impl Into<String>,
        model: impl Into<String>,
        units: MediaUnits,
        reported: Option<ReportedCost>,
        rate_card: &MediaRateCard,
    ) -> Self {
        let backend_id = backend_id.into();
        let (cost_usd, price_source) = match reported {
            Some(r) => (Some(r.usd), r.source),
            // A per-artifact rate card can only price a call that produced
            // artifacts. Applying it to any other basis — duration
            // (transcription), tokens (vision, and the nine calls behind
            // `video_analyze`) or characters (speech synthesis) — multiplies
            // the rate by zero artifacts and records **$0.00 for a call that
            // cost real money**, which is precisely the lie this module exists
            // to prevent. Such a call falls through to `unpriced`, keeping its
            // own units.
            //
            // Keyed on the declared basis rather than on `images == 0` so that
            // a future artifact-billed shape which legitimately produced zero
            // artifacts is not silently reclassified.
            None if units.basis != BillingBasis::Artifacts || units.images == 0 => (
                None,
                PriceSource::Unpriced {
                    reason: UnpricedReason::ProviderReportsNoCost,
                },
            ),
            None => match rate_card.lookup(&backend_id) {
                Some((entry, usd_per_image)) => (
                    Some(usd_per_image * f64::from(units.images)),
                    PriceSource::LocalRateCard {
                        entry: entry.to_string(),
                    },
                ),
                None => (
                    None,
                    PriceSource::Unpriced {
                        reason: UnpricedReason::ProviderReportsNoCost,
                    },
                ),
            },
        };
        Self {
            tool: tool.into(),
            backend_id,
            model: model.into(),
            units,
            outcome: MediaOutcome::Ok,
            cost_usd,
            price_source,
        }
    }

    /// Assemble a record for a call that failed.
    ///
    /// A failure is **not** recorded as `$0.00`: providers do bill for
    /// rejected prompts. The units are the units that were *requested*, which
    /// is what the operator would be charged for if the provider did bill.
    pub fn for_failure(
        tool: impl Into<String>,
        backend_id: impl Into<String>,
        model: impl Into<String>,
        units: MediaUnits,
        category: impl Into<String>,
    ) -> Self {
        Self {
            tool: tool.into(),
            backend_id: backend_id.into(),
            model: model.into(),
            units,
            outcome: MediaOutcome::Failed {
                category: category.into(),
            },
            cost_usd: None,
            price_source: PriceSource::Unpriced {
                reason: UnpricedReason::CallFailedBillingUnknown,
            },
        }
    }

    /// Override the outcome while keeping the resolved pricing.
    ///
    /// Used for the case that matters most to a user's wallet: the provider
    /// completed the work and billed for it, and the product then rejected the
    /// result locally (oversized payload, unsafe URL). The units were
    /// performed, so the price stands; only the outcome changes.
    pub fn with_outcome(mut self, outcome: MediaOutcome) -> Self {
        self.outcome = outcome;
        self
    }

    /// Stable JSON shape surfaced to the model in the tool result and, via
    /// `ToolResult.output`, to a protocol host. Uses the derived
    /// `Serialize`, so the wire shape and the type cannot drift apart.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }

    /// One-line operator-facing rendering. Says "unpriced" out loud rather
    /// than printing a zero.
    pub fn summary_line(&self) -> String {
        let price = match (&self.cost_usd, &self.price_source) {
            (Some(usd), PriceSource::LocalRateCard { entry }) => {
                format!("${usd:.6} (local estimate from rate-card entry `{entry}`)")
            }
            (Some(usd), PriceSource::ProviderHeader { header }) => {
                format!("${usd:.6} (provider, header `{header}`)")
            }
            (Some(usd), PriceSource::ProviderBody { field }) => {
                format!("${usd:.6} (provider, body `{field}`)")
            }
            (_, PriceSource::Unpriced { reason }) => match reason {
                UnpricedReason::ProviderReportsNoCost => {
                    "unpriced — this provider reports no cost for this call, and no local \
                     rate-card entry matched. The call was still billable."
                        .to_string()
                }
                UnpricedReason::CallFailedBillingUnknown => {
                    "unpriced — the call failed and it is unknown whether the provider \
                     billed for it. Not treated as $0."
                        .to_string()
                }
            },
            (None, _) => "unpriced".to_string(),
        };
        // Render the units this call was actually billed on. Printing
        // "0 image(s)" for a vision or speech call would read as "nothing was
        // produced", i.e. as free — the same misreading `$0.00` produces.
        let work = match self.units.basis {
            BillingBasis::Artifacts => {
                let size = match (self.units.width, self.units.height, self.units.megapixels()) {
                    (Some(w), Some(h), Some(mp)) => format!("{w}x{h} = {mp:.3} MP"),
                    _ => "size not reported by this surface".to_string(),
                };
                format!("{} image(s) {size}", self.units.images)
            }
            BillingBasis::Duration => match self.units.billed_seconds {
                Some(s) => format!("{s:.3}s of audio"),
                None => "audio of a duration this provider did not report".to_string(),
            },
            BillingBasis::Tokens => match (self.units.input_tokens, self.units.output_tokens) {
                (None, None) => "tokens this provider did not report".to_string(),
                (i, o) => format!(
                    "{} in / {} out tokens",
                    i.map_or_else(|| "?".to_string(), |v| v.to_string()),
                    o.map_or_else(|| "?".to_string(), |v| v.to_string())
                ),
            },
            BillingBasis::Characters => match self.units.billed_characters {
                Some(c) => format!("{c} characters of input text"),
                None => "text of a length this surface did not record".to_string(),
            },
        };
        format!(
            "{} via {} ({}): {} — {}",
            self.tool, self.backend_id, self.model, work, price
        )
    }
}

/// Operator-configured price list, keyed by backend identifier, in USD per
/// media artifact. **Empty by default** — an unconfigured install prices
/// nothing and says so, rather than inventing a figure.
///
/// Matching is exact first, then longest-prefix, so an operator can write
/// `"OpenAI"` to cover every OpenAI model or `"OpenAI gpt-image-1"` to price
/// one exactly.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaRateCard {
    usd_per_image: BTreeMap<String, f64>,
}

impl MediaRateCard {
    pub fn new(usd_per_image: BTreeMap<String, f64>) -> Self {
        Self { usd_per_image }
    }

    pub fn is_empty(&self) -> bool {
        self.usd_per_image.is_empty()
    }

    /// Resolve a price for `backend_id`. Returns the matching entry key
    /// alongside the price so the record can name which rule priced it.
    pub fn lookup(&self, backend_id: &str) -> Option<(&str, f64)> {
        if let Some((k, v)) = self.usd_per_image.get_key_value(backend_id) {
            return Some((k.as_str(), *v));
        }
        self.usd_per_image
            .iter()
            .filter(|(k, _)| backend_id.starts_with(k.as_str()))
            .max_by_key(|(k, _)| k.len())
            .map(|(k, v)| (k.as_str(), *v))
    }
}

/// Rolled-up view of one session's media spend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaCostSummary {
    /// Every billable media call, priced or not.
    pub calls: usize,
    /// Calls carrying a dollar figure from any source.
    pub priced_calls: usize,
    /// Calls with no dollar figure. These are **not** zero-cost.
    pub unpriced_calls: usize,
    /// Sum of the figures that exist. Meaningless without `unpriced_calls`,
    /// which is why both are always reported together.
    pub total_usd: f64,
    /// Total media artifacts produced across the session.
    pub images: u32,
    /// Total megapixels produced across the session, counting only the calls
    /// whose dimensions were known.
    pub megapixels: f64,
    /// Calls whose dimensions the surface did not report. Reported alongside
    /// `megapixels` for the same reason `unpriced_calls` sits alongside
    /// `total_usd`: a total that silently omits them is misleading.
    ///
    /// Counts **image** calls only. A transcription has no dimensions to
    /// report and never will, so folding it in here would inflate a figure
    /// that is supposed to mean "we produced pixels but could not measure
    /// them" with calls that produced no pixels at all.
    pub calls_of_unknown_size: usize,
    /// Total seconds of audio billed across the session, for the
    /// duration-billed shapes (transcription, speech synthesis). Sits
    /// alongside `images`/`megapixels` rather than being folded into them:
    /// seconds and pixels are different billable units and summing them
    /// would produce a number that means nothing.
    pub billed_seconds: f64,
    /// Number of duration-billed calls. Reported so `billed_seconds` can be
    /// read the same way `total_usd` is read against `unpriced_calls`.
    pub duration_billed_calls: usize,
    /// Prompt tokens across the token-billed calls (vision, and every call
    /// behind `video_analyze`). Counts only what providers actually reported.
    pub input_tokens: u64,
    /// Completion tokens across the token-billed calls.
    pub output_tokens: u64,
    /// Number of token-billed calls. **This is the figure that makes
    /// `video_analyze` legible**: one tool call fans out to nine provider
    /// calls, so a session with a single video analysis shows nine here.
    pub token_billed_calls: usize,
    /// Characters of input text across the character-billed calls (speech
    /// synthesis).
    pub billed_characters: u64,
    /// Number of character-billed calls.
    pub character_billed_calls: usize,
    /// Token-billed calls for which the provider reported no token counts.
    /// Sits alongside the token totals for the same reason `unpriced_calls`
    /// sits alongside `total_usd`: a total that silently omits them reads as
    /// though those calls did no work.
    pub calls_of_unknown_tokens: usize,
}

/// Session-scoped accumulation of [`MediaCostRecord`]s.
#[derive(Debug, Default)]
pub struct MediaCostLedger {
    records: Mutex<Vec<MediaCostRecord>>,
}

impl MediaCostLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn record(&self, record: MediaCostRecord) {
        self.records.lock().push(record);
    }

    pub fn snapshot(&self) -> Vec<MediaCostRecord> {
        self.records.lock().clone()
    }

    pub fn summary(&self) -> MediaCostSummary {
        let records = self.records.lock();
        let mut summary = MediaCostSummary {
            calls: records.len(),
            priced_calls: 0,
            unpriced_calls: 0,
            total_usd: 0.0,
            images: 0,
            megapixels: 0.0,
            calls_of_unknown_size: 0,
            billed_seconds: 0.0,
            duration_billed_calls: 0,
            input_tokens: 0,
            output_tokens: 0,
            token_billed_calls: 0,
            billed_characters: 0,
            character_billed_calls: 0,
            calls_of_unknown_tokens: 0,
        };
        for r in records.iter() {
            match r.cost_usd {
                Some(usd) => {
                    summary.priced_calls += 1;
                    summary.total_usd += usd;
                }
                None => summary.unpriced_calls += 1,
            }
            summary.images += r.units.images;
            // Bucket on the DECLARED basis. Seconds, tokens, characters and
            // pixels are four different billable units; summing any two of
            // them produces a number that means nothing, and filing one under
            // another produces a number that means something false.
            match r.units.basis {
                BillingBasis::Artifacts => match r.units.megapixels() {
                    Some(mp) => summary.megapixels += mp,
                    // An image call whose size the surface did not report.
                    None => summary.calls_of_unknown_size += 1,
                },
                BillingBasis::Duration => {
                    summary.duration_billed_calls += 1;
                    summary.billed_seconds += r.units.billed_seconds.unwrap_or(0.0);
                }
                BillingBasis::Tokens => {
                    summary.token_billed_calls += 1;
                    match r.units.total_tokens() {
                        Some(_) => {
                            summary.input_tokens += u64::from(r.units.input_tokens.unwrap_or(0));
                            summary.output_tokens += u64::from(r.units.output_tokens.unwrap_or(0));
                        }
                        None => summary.calls_of_unknown_tokens += 1,
                    }
                }
                BillingBasis::Characters => {
                    summary.character_billed_calls += 1;
                    summary.billed_characters += u64::from(r.units.billed_characters.unwrap_or(0));
                }
            }
        }
        summary
    }
}

/// The two things every billable media backend needs in order to account for
/// itself: somewhere to accumulate, and the operator's price list.
///
/// Bundled because they were being threaded separately and one of them kept
/// getting dropped: at the time this was added, `bootstrap.rs` bound a rate
/// card to image generation and to nothing else, so an operator who had
/// configured `[tools.media_pricing]` had it silently ignored for
/// transcription. A single value that carries both makes "wired for cost" one
/// decision per backend instead of two independent ones.
///
/// [`Default`] is the unconfigured install: no ledger, empty price list —
/// which records units and `unpriced`, never a guess.
#[derive(Debug, Clone, Default)]
pub struct MediaAccounting {
    pub ledger: Option<Arc<MediaCostLedger>>,
    pub rate_card: MediaRateCard,
}

impl MediaAccounting {
    /// A session ledger with the operator's price list.
    pub fn new(ledger: Arc<MediaCostLedger>, rate_card: MediaRateCard) -> Self {
        Self {
            ledger: Some(ledger),
            rate_card,
        }
    }

    /// Record one billable call: emit the structured log line every backend
    /// shares, then accumulate if a ledger is bound. Returns the record so a
    /// caller can surface it without a ledger.
    ///
    /// Centralised so a backend cannot accidentally log without recording, or
    /// record without logging — the two halves drifted apart across backends
    /// before this existed.
    pub fn account(&self, record: MediaCostRecord) -> MediaCostRecord {
        tracing::info!(
            target: "wcore::media_cost",
            tool = %record.tool,
            backend_id = %record.backend_id,
            model = %record.model,
            basis = ?record.units.basis,
            cost_usd = ?record.cost_usd,
            "media call accounted: {}",
            record.summary_line()
        );
        if let Some(ledger) = &self.ledger {
            ledger.record(record.clone());
        }
        record
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(pairs: &[(&str, f64)]) -> MediaRateCard {
        MediaRateCard::new(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), *v))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    /// The central property: the record is not a constant. Change the work,
    /// the record changes. This is the assertion the whole module exists for.
    #[test]
    fn record_varies_with_the_work_done() {
        let empty = MediaRateCard::default();
        let landscape = MediaCostRecord::for_success(
            "image_generate",
            "OpenAI gpt-image-1",
            "gpt-image-1",
            MediaUnits::one_image(1536, 1024),
            None,
            &empty,
        );
        let square = MediaCostRecord::for_success(
            "image_generate",
            "OpenAI gpt-image-1",
            "gpt-image-1",
            MediaUnits::one_image(1024, 1024),
            None,
            &empty,
        );
        assert_ne!(
            landscape.units, square.units,
            "aspect change must change the recorded units"
        );
        assert_ne!(
            landscape.units.megapixels(),
            square.units.megapixels(),
            "aspect change must change the recorded megapixels"
        );
        assert!(
            (landscape.units.megapixels().expect("known size") - 1.572_864).abs() < 1e-9,
            "landscape megapixels drifted: {:?}",
            landscape.units.megapixels()
        );

        // Backend change must be visible — this is what distinguishes the
        // built-in presentation from an MCP-served one.
        let other_backend = MediaCostRecord::for_success(
            "image_generate",
            "mcp:media-fixture/mcp_image_generate",
            "fixture-v1",
            MediaUnits::one_image(1536, 1024),
            None,
            &empty,
        );
        assert_ne!(landscape.backend_id, other_backend.backend_id);
    }

    /// A record must never claim a dollar figure nobody supplied. On the
    /// measured FluxRouter image path this is the real production shape.
    #[test]
    fn unreported_cost_is_unpriced_not_zero() {
        let r = MediaCostRecord::for_success(
            "image_generate",
            "OpenAI gpt-image-1",
            "gpt-image-1",
            MediaUnits::one_image(1536, 1024),
            None,
            &MediaRateCard::default(),
        );
        assert_eq!(r.cost_usd, None, "must not synthesise a price");
        assert_eq!(
            r.price_source,
            PriceSource::Unpriced {
                reason: UnpricedReason::ProviderReportsNoCost
            }
        );
        let rendered = r.summary_line();
        assert!(
            rendered.contains("unpriced"),
            "summary must say unpriced: {rendered}"
        );
        assert!(
            !rendered.contains("$0.00"),
            "an unpriced call must never render as $0.00: {rendered}"
        );
    }

    /// Known-negative for the previous test: with a rate card configured the
    /// SAME call does produce a figure. Without this the test above would
    /// pass on an implementation that can never price anything at all.
    #[test]
    fn rate_card_prices_the_same_call_that_was_otherwise_unpriced() {
        let rc = card(&[("OpenAI gpt-image-1", 0.08)]);
        let one = MediaCostRecord::for_success(
            "image_generate",
            "OpenAI gpt-image-1",
            "gpt-image-1",
            MediaUnits::one_image(1536, 1024),
            None,
            &rc,
        );
        assert_eq!(one.cost_usd, Some(0.08));
        assert_eq!(
            one.price_source,
            PriceSource::LocalRateCard {
                entry: "OpenAI gpt-image-1".to_string()
            }
        );
        assert!(
            !one.price_source.is_provider_reported(),
            "a local estimate must never be labelled provider-reported"
        );

        // ...and the priced figure varies with the number of artifacts.
        let two = MediaCostRecord::for_success(
            "image_generate",
            "OpenAI gpt-image-1",
            "gpt-image-1",
            MediaUnits::images_at(2, 1536, 1024),
            None,
            &rc,
        );
        assert_eq!(two.cost_usd, Some(0.16));
        assert_ne!(one.cost_usd, two.cost_usd);
    }

    /// A provider-reported figure outranks the local rate card, and is
    /// labelled as provider-reported so a host can render it differently.
    #[test]
    fn provider_reported_cost_outranks_rate_card() {
        let rc = card(&[("flux", 0.08)]);
        let r = MediaCostRecord::for_success(
            "image_generate",
            "flux-router",
            "flux-1",
            MediaUnits::one_image(1024, 1024),
            Some(ReportedCost::from_header("x-flux-cost-usd", 0.016_67)),
            &rc,
        );
        assert_eq!(r.cost_usd, Some(0.016_67));
        assert!(r.price_source.is_provider_reported());
        assert_eq!(
            r.price_source,
            PriceSource::ProviderHeader {
                header: "x-flux-cost-usd".to_string()
            }
        );
    }

    /// A failure is never $0. Providers bill for rejected prompts.
    #[test]
    fn failure_is_unpriced_with_billing_unknown_not_zero() {
        let r = MediaCostRecord::for_failure(
            "image_generate",
            "OpenAI gpt-image-1",
            "gpt-image-1",
            MediaUnits::one_image(1024, 1024),
            "prompt_rejected",
        );
        assert_eq!(r.cost_usd, None);
        assert_eq!(
            r.price_source,
            PriceSource::Unpriced {
                reason: UnpricedReason::CallFailedBillingUnknown
            }
        );
        assert_eq!(
            r.outcome,
            MediaOutcome::Failed {
                category: "prompt_rejected".to_string()
            }
        );
        assert!(r.summary_line().contains("unknown whether the provider"));
    }

    /// Longest-prefix matching lets an operator price a family and override
    /// one member of it.
    #[test]
    fn rate_card_prefers_the_most_specific_entry() {
        let rc = card(&[("OpenAI", 0.02), ("OpenAI gpt-image-1", 0.08)]);
        assert_eq!(
            rc.lookup("OpenAI gpt-image-1"),
            Some(("OpenAI gpt-image-1", 0.08))
        );
        assert_eq!(rc.lookup("OpenAI dall-e-3"), Some(("OpenAI", 0.02)));
        assert_eq!(rc.lookup("FAL FLUX schnell"), None);
    }

    /// The ledger must keep priced and unpriced calls distinguishable. A
    /// total that silently absorbs unpriced calls is the same lie as $0.00.
    #[test]
    fn ledger_reports_unpriced_calls_alongside_the_total() {
        let ledger = MediaCostLedger::new();
        let rc = card(&[("priced-backend", 0.05)]);
        ledger.record(MediaCostRecord::for_success(
            "image_generate",
            "priced-backend",
            "m",
            MediaUnits::one_image(1024, 1024),
            None,
            &rc,
        ));
        ledger.record(MediaCostRecord::for_success(
            "image_generate",
            "silent-backend",
            "m",
            MediaUnits::one_image(1536, 1024),
            None,
            &rc,
        ));
        let s = ledger.summary();
        assert_eq!(s.calls, 2);
        assert_eq!(s.priced_calls, 1);
        assert_eq!(s.unpriced_calls, 1);
        assert!((s.total_usd - 0.05).abs() < 1e-12);
        assert_eq!(s.images, 2);
        assert!(
            (s.megapixels - (1.048_576 + 1.572_864)).abs() < 1e-9,
            "megapixels: {}",
            s.megapixels
        );
        assert_eq!(s.calls_of_unknown_size, 0);

        // A call whose size the surface does not know must NOT be folded into
        // the megapixel total as if it were zero-sized.
        ledger.record(MediaCostRecord::for_success(
            "image_generate",
            "silent-backend",
            "m",
            MediaUnits::images_of_unknown_size(3),
            None,
            &rc,
        ));
        let s2 = ledger.summary();
        assert_eq!(s2.calls_of_unknown_size, 1);
        assert_eq!(
            s2.images, 5,
            "the artifact COUNT is still known and billable"
        );
        assert!(
            (s2.megapixels - s.megapixels).abs() < 1e-12,
            "an unknown-size call must not move the megapixel total"
        );
    }

    /// **The $0.00 trap.** A per-image rate card must never price a
    /// duration-billed call, because `usd_per_image * 0 images` is `0.0` and
    /// would record a real, billable transcription as free — the exact lie
    /// this module was written to prevent, arriving through the pricing path
    /// instead of the reporting path.
    #[test]
    fn rate_card_never_prices_a_duration_billed_call_as_zero() {
        // A rate card that DOES match this backend by name.
        let rc = card(&[("flux-router", 0.08)]);
        let audio = MediaCostRecord::for_success(
            "transcribe_audio",
            "flux-router",
            "whisper-1",
            MediaUnits::audio_seconds(12.5),
            None,
            &rc,
        );
        assert_eq!(
            audio.cost_usd, None,
            "a per-image card must not price an audio call"
        );
        assert_eq!(
            audio.price_source,
            PriceSource::Unpriced {
                reason: UnpricedReason::ProviderReportsNoCost
            }
        );
        let rendered = audio.summary_line();
        assert!(
            !rendered.contains("$0.00"),
            "a billable audio call must never render as $0.00: {rendered}"
        );
        // The units survive being unpriced — that is the whole point.
        assert_eq!(audio.units.billed_seconds, Some(12.5));

        // KNOWN-POSITIVE (can this path still price anything?): the SAME rate
        // card, same backend id, applied to an IMAGE call, does produce a
        // figure. Without this the assertion above would pass on an
        // implementation whose rate card had simply stopped working.
        let image = MediaCostRecord::for_success(
            "image_generate",
            "flux-router",
            "flux-1",
            MediaUnits::one_image(1024, 1024),
            None,
            &rc,
        );
        assert_eq!(
            image.cost_usd,
            Some(0.08),
            "the same card must still price an image call"
        );
    }

    /// A provider-reported figure must still reach a duration-billed call —
    /// the refusal above is specific to the per-image rate card, not a blanket
    /// "audio is never priced". FluxRouter really does return
    /// `x-flux-cost-usd` on transcription, so this is the production shape.
    #[test]
    fn duration_billed_call_still_takes_a_provider_reported_figure() {
        let rc = card(&[("flux-router", 0.08)]);
        let r = MediaCostRecord::for_success(
            "transcribe_audio",
            "flux-router",
            "whisper-large-v3",
            MediaUnits::audio_seconds(30.0),
            Some(ReportedCost::from_header("x-flux-cost-usd", 0.0031)),
            &rc,
        );
        assert_eq!(r.cost_usd, Some(0.0031));
        assert!(
            r.price_source.is_provider_reported(),
            "a header figure must be labelled provider-reported, not rate-card"
        );
        assert_ne!(
            r.price_source,
            PriceSource::LocalRateCard {
                entry: "flux-router".to_string()
            },
            "the matching rate-card entry must not shadow the provider's own number"
        );
    }

    /// Seconds and pixels are different billable units. Rolling audio into the
    /// image-shaped fields would make both meaningless.
    #[test]
    fn summary_keeps_audio_and_image_units_separate() {
        let ledger = MediaCostLedger::new();
        let empty = MediaRateCard::default();
        ledger.record(MediaCostRecord::for_success(
            "image_generate",
            "b",
            "m",
            MediaUnits::one_image(1024, 1024),
            None,
            &empty,
        ));
        ledger.record(MediaCostRecord::for_success(
            "transcribe_audio",
            "b",
            "m",
            MediaUnits::audio_seconds(42.0),
            None,
            &empty,
        ));

        let s = ledger.summary();
        assert_eq!(s.calls, 2);
        assert_eq!(s.images, 1, "the audio call produced no image");
        assert!(
            (s.billed_seconds - 42.0).abs() < 1e-9,
            "billed_seconds: {}",
            s.billed_seconds
        );
        assert_eq!(s.duration_billed_calls, 1);
        // The regression this guards: an audio call has no dimensions and
        // never will, so it must NOT be counted as an image call whose size
        // went unreported.
        assert_eq!(
            s.calls_of_unknown_size, 0,
            "an audio call is not an image of unknown size"
        );
        assert!(
            (s.megapixels - 1.048_576).abs() < 1e-9,
            "audio must not move the megapixel total: {}",
            s.megapixels
        );

        // KNOWN-NEGATIVE: an IMAGE call of genuinely unknown size MUST still
        // raise `calls_of_unknown_size`. Without this, the assertion above
        // would pass on an implementation that had simply stopped counting.
        ledger.record(MediaCostRecord::for_success(
            "image_generate",
            "b",
            "m",
            MediaUnits::images_of_unknown_size(2),
            None,
            &empty,
        ));
        let s2 = ledger.summary();
        assert_eq!(
            s2.calls_of_unknown_size, 1,
            "an unsized IMAGE call must still be counted"
        );
        assert_eq!(s2.duration_billed_calls, 1, "and it is not duration-billed");
    }

    /// **The `$0.00` trap, token edition.** A vision call has `images == 0`
    /// and is billed on tokens. A per-artifact rate card that happens to match
    /// its backend id must not price it — and must not price it as *zero*,
    /// which is how `usd_per_image * 0` would land.
    ///
    /// This is the `video_analyze` exposure in miniature: at the default frame
    /// count one tool call is nine of these, so a wrong figure here is wrong
    /// nine times over.
    #[test]
    fn rate_card_never_prices_a_token_billed_call_as_zero() {
        let rc = card(&[("anthropic", 0.08)]);
        let vision = MediaCostRecord::for_success(
            "vision_analyze",
            "anthropic",
            "claude-sonnet-4-6",
            MediaUnits::tokens(Some(1_842), Some(310)),
            None,
            &rc,
        );
        assert_eq!(
            vision.cost_usd, None,
            "a per-artifact card must not price a token-billed call"
        );
        assert_eq!(
            vision.price_source,
            PriceSource::Unpriced {
                reason: UnpricedReason::ProviderReportsNoCost
            }
        );
        let rendered = vision.summary_line();
        assert!(
            !rendered.contains("$0.00"),
            "a billable vision call must never render as $0.00: {rendered}"
        );
        // It must also not claim it produced nothing — "0 image(s)" reads as
        // free just as surely as "$0.00" does.
        assert!(
            !rendered.contains("0 image(s)"),
            "a vision call must not be described in artifacts: {rendered}"
        );
        assert!(
            rendered.contains("1842 in / 310 out tokens"),
            "the units actually billed must be rendered: {rendered}"
        );
        // The units survive being unpriced — that is the whole point.
        assert_eq!(vision.units.total_tokens(), Some(2_152));

        // KNOWN-POSITIVE (can this path still price anything?): the SAME card,
        // same backend id, applied to an ARTIFACT call, does produce a figure.
        // Without this the assertion above would pass on an implementation
        // whose rate card had simply stopped working.
        let image = MediaCostRecord::for_success(
            "image_generate",
            "anthropic",
            "some-image-model",
            MediaUnits::one_image(1024, 1024),
            None,
            &rc,
        );
        assert_eq!(
            image.cost_usd,
            Some(0.08),
            "the same card must still price an artifact call"
        );
    }

    /// A character-billed speech call gets the same protection, and is
    /// likewise described in the units it was billed on.
    #[test]
    fn rate_card_never_prices_a_character_billed_call_as_zero() {
        let rc = card(&[("openai", 0.04)]);
        let speech = MediaCostRecord::for_success(
            "text_to_speech",
            "openai",
            "tts-1",
            MediaUnits::text_characters(2_048),
            None,
            &rc,
        );
        assert_eq!(speech.cost_usd, None);
        let rendered = speech.summary_line();
        assert!(!rendered.contains("$0.00"), "{rendered}");
        assert!(
            rendered.contains("2048 characters of input text"),
            "{rendered}"
        );
        assert_eq!(speech.units.billed_characters, Some(2_048));

        // KNOWN-POSITIVE: the same card still prices an artifact call.
        let image = MediaCostRecord::for_success(
            "image_generate",
            "openai",
            "gpt-image-1",
            MediaUnits::one_image(512, 512),
            None,
            &rc,
        );
        assert_eq!(image.cost_usd, Some(0.04));
    }

    /// A provider-reported figure must still reach a token-billed call. This
    /// is the production shape for vision served over FluxRouter, which the
    /// module header records as returning `x-flux-cost-usd` on chat.
    #[test]
    fn token_billed_call_still_takes_a_provider_reported_figure() {
        let rc = card(&[("flux-router", 0.08)]);
        let r = MediaCostRecord::for_success(
            "vision_analyze",
            "flux-router",
            "gpt-4o",
            MediaUnits::tokens(Some(1_500), Some(240)),
            Some(ReportedCost::from_header("x-flux-cost-usd", 0.004_21)),
            &rc,
        );
        assert_eq!(r.cost_usd, Some(0.004_21));
        assert!(
            r.price_source.is_provider_reported(),
            "a header figure must be labelled provider-reported, not rate-card"
        );
        assert_ne!(
            r.price_source,
            PriceSource::LocalRateCard {
                entry: "flux-router".to_string()
            },
            "the matching rate-card entry must not shadow the provider's own number"
        );
    }

    /// Four billing bases, four buckets, no cross-contamination. Pixels,
    /// seconds, tokens and characters are not summable with one another and a
    /// call filed under the wrong basis is a false measurement.
    #[test]
    fn summary_keeps_all_four_billing_bases_separate() {
        let ledger = MediaCostLedger::new();
        let empty = MediaRateCard::default();
        for units in [
            MediaUnits::one_image(1024, 1024),
            MediaUnits::audio_seconds(42.0),
            MediaUnits::tokens(Some(100), Some(20)),
            MediaUnits::text_characters(500),
        ] {
            ledger.record(MediaCostRecord::for_success(
                "t", "b", "m", units, None, &empty,
            ));
        }

        let s = ledger.summary();
        assert_eq!(s.calls, 4);
        // Each basis lands in exactly one bucket...
        assert_eq!(s.images, 1, "only the artifact call produced an image");
        assert_eq!(s.duration_billed_calls, 1);
        assert_eq!(s.token_billed_calls, 1);
        assert_eq!(s.character_billed_calls, 1);
        // ...and contributes to no other bucket's total.
        assert!(
            (s.billed_seconds - 42.0).abs() < 1e-9,
            "{}",
            s.billed_seconds
        );
        assert_eq!(s.input_tokens, 100);
        assert_eq!(s.output_tokens, 20);
        assert_eq!(s.billed_characters, 500);
        assert!((s.megapixels - 1.048_576).abs() < 1e-9, "{}", s.megapixels);
        // The regression this guards hardest: before the basis field existed,
        // `is_duration_billed()` was `images == 0`, so the vision call and the
        // speech call would BOTH have been counted as duration-billed and
        // added 0.0 seconds each — inflating a seconds-denominated call count
        // with calls that have no seconds.
        assert_eq!(
            s.duration_billed_calls, 1,
            "token and character calls must not be counted as duration-billed"
        );
        // And neither is an image of unmeasured size.
        assert_eq!(
            s.calls_of_unknown_size, 0,
            "no non-artifact call is an image of unknown size"
        );

        // KNOWN-NEGATIVE: a genuinely unsized ARTIFACT call MUST still raise
        // `calls_of_unknown_size`, and a token call whose provider reported no
        // counts MUST still raise `calls_of_unknown_tokens`. Without these the
        // assertions above would pass on an implementation that had simply
        // stopped counting.
        ledger.record(MediaCostRecord::for_success(
            "t",
            "b",
            "m",
            MediaUnits::images_of_unknown_size(2),
            None,
            &empty,
        ));
        ledger.record(MediaCostRecord::for_success(
            "t",
            "b",
            "m",
            MediaUnits::tokens(None, None),
            None,
            &empty,
        ));
        let s2 = ledger.summary();
        assert_eq!(s2.calls_of_unknown_size, 1, "unsized artifact call counted");
        assert_eq!(
            s2.calls_of_unknown_tokens, 1,
            "token call with no reported counts must be visible, not silently zero"
        );
        assert_eq!(s2.token_billed_calls, 2);
        assert_eq!(
            s2.input_tokens, 100,
            "an unreported token count must not be summed as 0 into the total"
        );
    }

    /// **The `video_analyze` exposure, stated as an assertion.** One tool call
    /// at the default frame count is nine billable provider calls. The ledger
    /// must show nine, because a user who sees one has been told this cost
    /// roughly a ninth of what it did.
    #[test]
    fn video_analyze_fan_out_is_nine_visible_token_billed_calls() {
        const DEFAULT_FRAMES: usize = 8;
        let ledger = MediaCostLedger::new();
        let empty = MediaRateCard::default();
        // 8 per-frame vision calls...
        for i in 0..DEFAULT_FRAMES {
            ledger.record(MediaCostRecord::for_success(
                "video_analyze",
                "flux-router",
                "gpt-4o",
                MediaUnits::tokens(Some(1_000 + i as u32), Some(50)),
                Some(ReportedCost::from_header("x-flux-cost-usd", 0.002)),
                &empty,
            ));
        }
        // ...plus the synthesis pass.
        ledger.record(MediaCostRecord::for_success(
            "video_analyze",
            "flux-router",
            "gpt-4o",
            MediaUnits::tokens(Some(4_000), Some(600)),
            Some(ReportedCost::from_header("x-flux-cost-usd", 0.011)),
            &empty,
        ));

        let s = ledger.summary();
        assert_eq!(
            s.calls, 9,
            "one video_analyze must be visible as nine billable calls"
        );
        assert_eq!(s.token_billed_calls, 9);
        assert_eq!(s.priced_calls, 9, "every call carried a provider figure");
        assert_eq!(s.unpriced_calls, 0);
        // 8 * 0.002 + 0.011
        assert!(
            (s.total_usd - 0.027).abs() < 1e-9,
            "total must be the SUM of all nine, not one: {}",
            s.total_usd
        );
        // The synthesis pass is materially more expensive than a frame, so a
        // total that had counted only one call would be visibly wrong either
        // way — this pins that the fan-out is summed, not sampled.
        assert!(
            s.total_usd > 0.011,
            "total must exceed the single most expensive call: {}",
            s.total_usd
        );
        assert_eq!(s.images, 0, "video_analyze produces no artifacts");
    }

    /// The JSON the model and the host see must carry the price source, not
    /// just a number. Serialization is derived, so this pins the wire shape.
    #[test]
    fn json_shape_carries_the_price_source() {
        let r = MediaCostRecord::for_success(
            "image_generate",
            "OpenAI gpt-image-1",
            "gpt-image-1",
            MediaUnits::one_image(1536, 1024),
            None,
            &MediaRateCard::default(),
        );
        let v = r.to_json();
        assert_eq!(v["tool"], "image_generate");
        assert_eq!(v["price_source"]["kind"], "unpriced");
        assert_eq!(v["price_source"]["reason"], "provider_reports_no_cost");
        assert_eq!(v["units"]["width"], 1536);
        assert!(
            v.get("cost_usd").is_none(),
            "absent cost must be absent, not null-or-zero: {v}"
        );
        // Round-trips, so a host decoding it gets the same record back.
        let back: MediaCostRecord = serde_json::from_value(v).expect("record must round-trip");
        assert_eq!(back, r);
    }
}
