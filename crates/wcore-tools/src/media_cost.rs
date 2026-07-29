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

/// Billable units actually performed by one media call.
///
/// Every field here is observable without the provider pricing anything, and
/// every field varies with the work requested — which is the property that
/// makes this record a measurement rather than a constant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaUnits {
    /// Number of media artifacts produced (images returned, clips rendered).
    pub images: u32,
    pub width: u32,
    pub height: u32,
    /// Seconds of audio/video billed, when the shape has a duration and the
    /// provider reports one. `None` for still images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billed_seconds: Option<f64>,
}

impl MediaUnits {
    /// One still image at the given pixel dimensions.
    pub fn one_image(width: u32, height: u32) -> Self {
        Self {
            images: 1,
            width,
            height,
            billed_seconds: None,
        }
    }

    /// Total megapixels produced. This is the unit most image providers price
    /// on, and it separates `landscape` (1536x1024) from `square` (1024x1024)
    /// — i.e. it changes when the requested work changes.
    pub fn megapixels(&self) -> f64 {
        (f64::from(self.width) * f64::from(self.height) * f64::from(self.images)) / 1_000_000.0
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
    Failed { category: String },
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
        format!(
            "{} via {} ({}): {} image(s) {}x{} = {:.3} MP — {}",
            self.tool,
            self.backend_id,
            self.model,
            self.units.images,
            self.units.width,
            self.units.height,
            self.units.megapixels(),
            price
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
    /// Total megapixels produced across the session.
    pub megapixels: f64,
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
            summary.megapixels += r.units.megapixels();
        }
        summary
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
            (landscape.units.megapixels() - 1.572_864).abs() < 1e-9,
            "landscape megapixels drifted: {}",
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
            MediaUnits {
                images: 2,
                width: 1536,
                height: 1024,
                billed_seconds: None,
            },
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
        assert_eq!(rc.lookup("OpenAI gpt-image-1"), Some(("OpenAI gpt-image-1", 0.08)));
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
