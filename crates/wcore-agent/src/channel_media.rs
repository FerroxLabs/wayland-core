//! Inbound media enrichment for channel turns.
//!
//! A channel message can carry typed [`Attachment`]s (an image, a voice
//! note, …). By default the dispatcher only *summarises* them into the
//! prompt — the model is told "an image arrived" but never sees the content.
//! For conversational channels the agent also has no vision / transcription
//! tool to reach for, so the media is effectively invisible.
//!
//! This module closes that gap: before the turn prompt is built, the
//! [`ChannelMediaEnricher`] resolves each attachment to *derived text* — a
//! transcript for audio, a description for images — and writes it into
//! [`Attachment::transcribed`], which `build_turn_prompt` already prefers
//! over the bare URL.
//!
//! ## Tokens stay in the connector (auth-aware fetch)
//!
//! The enricher does NOT hold any channel credentials. It fetches the raw
//! bytes through a [`MediaByteSource`] — in production [`ManagerMediaSource`],
//! which routes to the originating connector via
//! [`ChannelManager::fetch_media_on`](wcore_channels::ChannelManager::fetch_media_on).
//! Each connector downloads its OWN media with its OWN token and platform
//! protocol (Slack bearer on `url_private`, WhatsApp's id→url→bytes, Matrix's
//! `mxc://`→authenticated endpoint, Telegram/Discord plain GET). The bytes
//! then go to the host-wired vision / transcription backend.
//!
//! ## Fail-soft and bounded
//!
//! Every step is best-effort: a connector that can't fetch (default
//! `Rejected`), a fetch error/timeout, an oversize payload, an unsupported
//! format, a missing backend, or a backend error all log and write an honest
//! degraded notice into [`Attachment::transcribed`] (#660) — the model is told
//! it cannot see/hear the content and why, never left to answer blind from a
//! bare URL. A media problem never fails the turn. Both the fetch and the
//! analyze step are wall-clock bounded.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use wcore_channels::{Attachment, ChannelManager, MediaBounds, MediaKind};
use wcore_tools::media_intake::admit_bytes;
use wcore_tools::transcription_tools::{
    TranscriptionBackend, TranscriptionOutcome, audio_intake_policy,
};
use wcore_tools::vision_tools::{VisionBackend, VisionOutcome, image_intake_policy};

/// Max characters of derived text injected per attachment, to protect the
/// turn's prompt budget. Longer transcripts/descriptions are truncated.
const MAX_DERIVED_CHARS: usize = 2_000;

/// Wall-clock cap on the connector media download.
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

/// Wall-clock cap on the vision/transcription model call.
const ANALYZE_TIMEOUT: Duration = Duration::from_secs(45);

/// Vision prompt for eager image enrichment — terse on purpose.
const IMAGE_DESCRIBE_PROMPT: &str =
    "Concisely describe this image for a chat assistant, and quote any visible text verbatim.";

// Honest degraded-mode notices (#660). When inbound media cannot be turned into
// text, the model must be told WHY — otherwise the prompt carries only a bare
// URL it cannot fetch and it answers blind (confidently describing an image it
// never saw). These strings are written into `Attachment::transcribed`, which
// `build_turn_prompt` surfaces into the turn.
const IMAGE_NO_VISION_NOTICE: &str = "[Inbound image received but NOT analyzed: no vision backend is configured, so the \
     assistant cannot see this image. Do not guess its contents. To enable image \
     understanding, set ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY, or \
     FLUX_API_KEY, or configure an OpenAI-wire provider (FluxRouter / OpenAI).]";
const AUDIO_NO_TRANSCRIPTION_NOTICE: &str = "[Inbound audio received but NOT transcribed: no transcription backend is configured, so \
     the assistant cannot hear this audio. To enable transcription, set GROQ_API_KEY or \
     OPENAI_API_KEY.]";
const IMAGE_ANALYSIS_FAILED_NOTICE: &str = "[Inbound image could not be analyzed (it may be too large, an unsupported format, or the \
     vision backend errored/timed out). The assistant has NOT seen its contents; do not guess.]";
const AUDIO_ANALYSIS_FAILED_NOTICE: &str = "[Inbound audio could not be transcribed (it may be too large, an unsupported format, or the \
     transcription backend errored/timed out). The assistant has NOT heard its contents.]";

/// Source of inbound media bytes, fetched WITH the originating connector's
/// own credentials. Abstracts [`ChannelManager`] so the enricher is unit
/// testable without a live channel.
#[async_trait]
pub trait MediaByteSource: Send + Sync {
    /// Fetch the bytes of `attachment` as received on `channel`.
    async fn fetch(&self, channel: &str, attachment: &Attachment) -> Result<Vec<u8>, String>;

    /// The bounds `channel` declares. Defaulted so a test source need not
    /// model them; the production source resolves the real per-adapter
    /// declaration.
    ///
    /// This exists so `max_attachments` has an enforcement point at all. The
    /// byte bound is enforced inside `fetch` (connector-side and again at
    /// `ChannelManager::fetch_media_on`), but a per-message attachment COUNT
    /// can only be applied where the whole list is in scope, which is
    /// [`ChannelMediaEnricher::enrich`] — not the per-attachment fetch.
    async fn bounds(&self, _channel: &str) -> MediaBounds {
        MediaBounds::default()
    }
}

/// Production [`MediaByteSource`]: routes through the [`ChannelManager`] so
/// each connector fetches its own media with its own token. Credentials
/// never leave the connector boundary.
pub struct ManagerMediaSource {
    manager: Arc<tokio::sync::RwLock<ChannelManager>>,
}

impl ManagerMediaSource {
    pub fn new(manager: Arc<tokio::sync::RwLock<ChannelManager>>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl MediaByteSource for ManagerMediaSource {
    async fn fetch(&self, channel: &str, attachment: &Attachment) -> Result<Vec<u8>, String> {
        let guard = self.manager.read().await;
        guard
            .fetch_media_on(channel, attachment)
            .await
            .map_err(|e| e.to_string())
    }

    async fn bounds(&self, channel: &str) -> MediaBounds {
        let guard = self.manager.read().await;
        // An unknown channel falls back to the trait default rather than to
        // "unbounded": a missing declaration is not permission to fetch
        // anything, which is the reasoning `MediaBounds::DEFAULT_MAX_BYTES`
        // already carries.
        guard
            .media_bounds_on(channel)
            .await
            .unwrap_or_else(MediaBounds::default)
    }
}

/// Resolves inbound attachments to derived text (audio→transcript,
/// image→description) using the host-wired vision / transcription backends
/// and a [`MediaByteSource`] for the auth-aware download.
///
/// Construct via [`ChannelMediaEnricher::new`]. When neither backend is
/// configured the enricher is *inert* ([`Self::is_inert`]) for deriving text,
/// but is still installed so it can emit honest "cannot see/hear this" degraded
/// notices (#660) rather than letting inbound media be answered blind.
pub struct ChannelMediaEnricher {
    vision: Option<Arc<dyn VisionBackend>>,
    transcription: Option<Arc<dyn TranscriptionBackend>>,
    source: Arc<dyn MediaByteSource>,
    fetch_timeout: Duration,
    analyze_timeout: Duration,
}

impl ChannelMediaEnricher {
    /// Build an enricher from the optional backends (the same the agent's
    /// `vision_analyze` / `transcribe_audio` tools use) and a byte source.
    pub fn new(
        vision: Option<Arc<dyn VisionBackend>>,
        transcription: Option<Arc<dyn TranscriptionBackend>>,
        source: Arc<dyn MediaByteSource>,
    ) -> Self {
        Self {
            vision,
            transcription,
            source,
            fetch_timeout: FETCH_TIMEOUT,
            analyze_timeout: ANALYZE_TIMEOUT,
        }
    }

    /// `true` when no backend is wired — the enricher derives no text and only
    /// emits honest degraded notices for inbound media (#660).
    pub fn is_inert(&self) -> bool {
        self.vision.is_none() && self.transcription.is_none()
    }

    /// Enrich each attachment in place. Best-effort and fail-soft.
    ///
    /// When an image/audio attachment cannot be turned into text — no backend
    /// configured, or fetch/analysis failed — an honest degraded notice is
    /// written to [`Attachment::transcribed`] instead of silently leaving a
    /// bare URL (#660). Non-media kinds are left untouched.
    ///
    /// # The declared attachment-count bound is applied here
    ///
    /// Attachments past the channel's declared `max_attachments` are NOT
    /// fetched or analysed, and get a notice saying so. They are never removed
    /// from the list — a truncated list is a message the agent answers with no
    /// idea it was incomplete, which is the same reasoning
    /// [`normalize_all`](wcore_channels::media::normalize_all) carries. This is
    /// the only place the count bound can be applied, because it is the only
    /// place the whole list is in scope.
    pub async fn enrich(&self, attachments: &mut [Attachment], channel: &str) {
        let max_attachments = self.source.bounds(channel).await.max_attachments;
        let total = attachments.len();
        for (idx, att) in attachments.iter_mut().enumerate() {
            // Never overwrite a connector-supplied transcript.
            if att.transcribed.is_some() {
                continue;
            }
            // Past the declared count bound: record why, keep the record.
            if idx >= max_attachments {
                att.transcribed = Some(format!(
                    "[Inbound attachment {} of {total} was NOT analyzed: it is past this \
                     channel's declared bound of {max_attachments} attachments per message. The \
                     assistant has NOT seen its contents; do not guess.]",
                    idx + 1
                ));
                continue;
            }
            // No backend for this media kind → record why, don't drop it blind.
            match att.kind {
                MediaKind::Image if self.vision.is_none() => {
                    att.transcribed = Some(IMAGE_NO_VISION_NOTICE.to_string());
                    continue;
                }
                MediaKind::Audio if self.transcription.is_none() => {
                    att.transcribed = Some(AUDIO_NO_TRANSCRIPTION_NOTICE.to_string());
                    continue;
                }
                // A matching backend is configured — proceed to fetch+analyze.
                MediaKind::Image | MediaKind::Audio => {}
                // Non-media kind (file, …) — nothing to derive, leave as-is.
                _ => continue,
            }

            // Fetch the bytes via the originating connector (auth lives
            // there), bounded so a slow media host can't stall the turn.
            let att_for_fetch = att.clone();
            let bytes = match tokio::time::timeout(
                self.fetch_timeout,
                self.source.fetch(channel, &att_for_fetch),
            )
            .await
            {
                Ok(Ok(b)) => Some(b),
                Ok(Err(e)) => {
                    tracing::debug!(
                        target: "wcore_agent::channel_media",
                        channel,
                        kind = ?att.kind,
                        error = %e,
                        "inbound media fetch failed"
                    );
                    None
                }
                Err(_) => {
                    tracing::warn!(
                        target: "wcore_agent::channel_media",
                        channel,
                        kind = ?att.kind,
                        timeout_secs = self.fetch_timeout.as_secs(),
                        "inbound media fetch timed out"
                    );
                    None
                }
            };

            let derived = match (att.kind, bytes.as_deref()) {
                (MediaKind::Image, Some(b)) => self.describe_image(b, channel).await,
                (MediaKind::Audio, Some(b)) => self.transcribe_audio(b, channel).await,
                _ => None,
            };
            match derived {
                Some(text) => {
                    let (text, truncated) = truncate(text, MAX_DERIVED_CHARS);
                    tracing::info!(
                        target: "wcore_agent::channel_media",
                        channel,
                        kind = ?att.kind,
                        chars = text.len(),
                        truncated,
                        "inbound media enriched"
                    );
                    att.transcribed = Some(text);
                }
                // Backend WAS configured but fetch/analysis produced nothing —
                // an honest "could not analyze" notice, never a silent drop.
                None => {
                    att.transcribed = Some(
                        match att.kind {
                            MediaKind::Image => IMAGE_ANALYSIS_FAILED_NOTICE,
                            _ => AUDIO_ANALYSIS_FAILED_NOTICE,
                        }
                        .to_string(),
                    );
                }
            }
        }
    }

    async fn describe_image(&self, bytes: &[u8], channel: &str) -> Option<String> {
        let backend = self.vision.as_ref()?;
        // Connector-supplied bytes face the SAME caps and the SAME format
        // decision a composer path faces, through the shared chokepoint — so a
        // channel cannot introduce a class the composer would have refused.
        let mime = match admit_bytes(bytes, &image_intake_policy()) {
            Ok(kind) => kind.as_str(),
            Err(reason) => {
                tracing::debug!(
                    target: "wcore_agent::channel_media",
                    channel,
                    bytes = bytes.len(),
                    %reason,
                    "inbound image refused by media intake; skipping"
                );
                return None;
            }
        };
        match tokio::time::timeout(
            self.analyze_timeout,
            backend.analyze(mime, bytes, IMAGE_DESCRIBE_PROMPT),
        )
        .await
        {
            Ok(VisionOutcome::Ok { analysis }) => non_empty(analysis),
            Ok(VisionOutcome::Err { message }) => {
                tracing::debug!(target: "wcore_agent::channel_media", channel, error = %message, "vision backend error");
                None
            }
            Err(_) => {
                tracing::warn!(target: "wcore_agent::channel_media", channel, "vision analyze timed out");
                None
            }
        }
    }

    async fn transcribe_audio(&self, bytes: &[u8], channel: &str) -> Option<String> {
        let backend = self.transcription.as_ref()?;
        // Same shared chokepoint, this surface's audio policy.
        let mime = match admit_bytes(bytes, &audio_intake_policy()) {
            Ok(kind) => kind.as_str(),
            Err(reason) => {
                tracing::debug!(
                    target: "wcore_agent::channel_media",
                    channel,
                    bytes = bytes.len(),
                    %reason,
                    "inbound audio refused by media intake; skipping"
                );
                return None;
            }
        };
        match tokio::time::timeout(self.analyze_timeout, backend.transcribe(mime, bytes, None))
            .await
        {
            Ok(TranscriptionOutcome::Ok { transcript, .. }) => non_empty(transcript),
            Ok(TranscriptionOutcome::Err { message }) => {
                tracing::debug!(target: "wcore_agent::channel_media", channel, error = %message, "transcription backend error");
                None
            }
            Err(_) => {
                tracing::warn!(target: "wcore_agent::channel_media", channel, "transcription timed out");
                None
            }
        }
    }
}

/// `Some(trimmed)` when non-empty, else `None`.
fn non_empty(text: String) -> Option<String> {
    let t = text.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Truncate to at most `max` characters (char-boundary safe), appending a
/// marker when cut. Returns `(text, was_truncated)`.
fn truncate(text: String, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text, false);
    }
    let cut: String = text.chars().take(max).collect();
    (format!("{cut}… [truncated]"), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_tools::transcription_tools::CapturingTranscriptionBackend;
    use wcore_tools::vision_tools::CapturingVisionBackend;

    /// Minimal valid PNG header — passes `detect_image_mime` + min-size.
    fn png_bytes() -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend_from_slice(&[0u8; 64]);
        v
    }

    /// Minimal valid OGG header — passes `detect_audio_mime` + min-size.
    fn ogg_bytes() -> Vec<u8> {
        let mut v = b"OggS".to_vec();
        v.extend_from_slice(&[0u8; 64]);
        v
    }

    /// Test byte source returning a fixed result, ignoring channel/attachment.
    struct StaticSource(Result<Vec<u8>, String>);

    #[async_trait]
    impl MediaByteSource for StaticSource {
        async fn fetch(&self, _channel: &str, _att: &Attachment) -> Result<Vec<u8>, String> {
            self.0.clone()
        }
    }

    fn image_att() -> Attachment {
        Attachment {
            url: "mxc://ex.org/abc".into(),
            content_type: Some("image/png".into()),
            kind: MediaKind::Image,
            ..Default::default()
        }
    }

    fn audio_att() -> Attachment {
        Attachment {
            url: "media-id-123".into(),
            content_type: Some("audio/ogg".into()),
            kind: MediaKind::Audio,
            ..Default::default()
        }
    }

    fn vision_enricher(canned: &str, source: StaticSource) -> ChannelMediaEnricher {
        ChannelMediaEnricher::new(
            Some(Arc::new(CapturingVisionBackend::new(canned))),
            None,
            Arc::new(source),
        )
    }

    fn audio_enricher(canned: &str, source: StaticSource) -> ChannelMediaEnricher {
        ChannelMediaEnricher::new(
            None,
            Some(Arc::new(CapturingTranscriptionBackend::new(canned))),
            Arc::new(source),
        )
    }

    #[tokio::test]
    async fn enriches_image_into_description() {
        let enricher = vision_enricher("a red bicycle", StaticSource(Ok(png_bytes())));
        let mut atts = vec![image_att()];
        enricher.enrich(&mut atts, "slack").await;
        assert_eq!(atts[0].transcribed.as_deref(), Some("a red bicycle"));
    }

    #[tokio::test]
    async fn enriches_audio_into_transcript() {
        let enricher = audio_enricher("meet at noon", StaticSource(Ok(ogg_bytes())));
        let mut atts = vec![audio_att()];
        enricher.enrich(&mut atts, "whatsapp").await;
        assert_eq!(atts[0].transcribed.as_deref(), Some("meet at noon"));
    }

    #[tokio::test]
    async fn fetch_error_yields_analysis_failed_notice() {
        // #660: a fetch failure (backend present) must surface an honest notice,
        // not silently drop the image to a bare URL the model answers blind.
        let enricher = vision_enricher("never", StaticSource(Err("401 unauthorized".into())));
        let mut atts = vec![image_att()];
        enricher.enrich(&mut atts, "slack").await;
        assert_eq!(
            atts[0].transcribed.as_deref(),
            Some(IMAGE_ANALYSIS_FAILED_NOTICE)
        );
    }

    #[tokio::test]
    async fn preserves_connector_supplied_transcript() {
        let enricher = audio_enricher("model text", StaticSource(Ok(ogg_bytes())));
        let mut atts = vec![Attachment {
            kind: MediaKind::Audio,
            transcribed: Some("connector transcript".into()),
            ..Default::default()
        }];
        enricher.enrich(&mut atts, "whatsapp").await;
        assert_eq!(atts[0].transcribed.as_deref(), Some("connector transcript"));
    }

    #[tokio::test]
    async fn unsupported_kind_is_skipped_without_fetch() {
        // Document kind + a source that would panic-loudly is never called
        // because the kind is filtered before fetch.
        let enricher = vision_enricher("never", StaticSource(Ok(png_bytes())));
        let mut atts = vec![Attachment {
            url: "x".into(),
            kind: MediaKind::Document,
            ..Default::default()
        }];
        enricher.enrich(&mut atts, "slack").await;
        assert!(atts[0].transcribed.is_none());
    }

    #[tokio::test]
    async fn image_without_vision_backend_yields_notice() {
        // #660: an image with no vision backend must get an honest "cannot see,
        // set a key" notice instead of being silently dropped.
        let enricher = audio_enricher("audio only", StaticSource(Ok(png_bytes())));
        let mut atts = vec![image_att()];
        enricher.enrich(&mut atts, "slack").await;
        assert_eq!(atts[0].transcribed.as_deref(), Some(IMAGE_NO_VISION_NOTICE));
    }

    #[tokio::test]
    async fn audio_without_transcription_backend_yields_notice() {
        // #660: audio with no transcription backend gets the honest notice.
        let enricher = vision_enricher("vision only", StaticSource(Ok(ogg_bytes())));
        let mut atts = vec![audio_att()];
        enricher.enrich(&mut atts, "whatsapp").await;
        assert_eq!(
            atts[0].transcribed.as_deref(),
            Some(AUDIO_NO_TRANSCRIPTION_NOTICE)
        );
    }

    #[tokio::test]
    async fn non_media_bytes_yield_analysis_failed_notice() {
        // Source returns an HTML error page; the mime sniff fails. Backend was
        // present, so #660 surfaces an honest "could not analyze" notice.
        let html = b"<!DOCTYPE html><html>nope</html>".to_vec();
        let enricher = vision_enricher("never", StaticSource(Ok(html)));
        let mut atts = vec![image_att()];
        enricher.enrich(&mut atts, "slack").await;
        assert_eq!(
            atts[0].transcribed.as_deref(),
            Some(IMAGE_ANALYSIS_FAILED_NOTICE)
        );
    }

    #[tokio::test]
    async fn oversize_payload_yields_analysis_failed_notice() {
        let mut huge = b"\x89PNG\r\n\x1a\n".to_vec();
        huge.resize(VISION_MAX_BYTES + 1024, 0u8);
        let enricher = vision_enricher("never", StaticSource(Ok(huge)));
        let mut atts = vec![image_att()];
        enricher.enrich(&mut atts, "slack").await;
        assert_eq!(
            atts[0].transcribed.as_deref(),
            Some(IMAGE_ANALYSIS_FAILED_NOTICE)
        );
    }

    #[tokio::test]
    async fn long_derived_text_is_truncated() {
        let huge = "x".repeat(MAX_DERIVED_CHARS + 500);
        let enricher = vision_enricher(&huge, StaticSource(Ok(png_bytes())));
        let mut atts = vec![image_att()];
        enricher.enrich(&mut atts, "slack").await;
        let got = atts[0].transcribed.as_deref().unwrap();
        assert!(got.ends_with("… [truncated]"));
    }

    #[tokio::test]
    async fn inert_enricher_emits_degraded_notice_not_noop() {
        // #660: even with NO backend wired the enricher must not silently drop
        // inbound media — it emits the honest no-vision notice so the model
        // never answers an unseen image blind.
        let enricher =
            ChannelMediaEnricher::new(None, None, Arc::new(StaticSource(Ok(png_bytes()))));
        assert!(enricher.is_inert());
        let mut atts = vec![image_att()];
        enricher.enrich(&mut atts, "slack").await;
        assert_eq!(atts[0].transcribed.as_deref(), Some(IMAGE_NO_VISION_NOTICE));
    }

    #[test]
    fn truncate_below_cap_is_unchanged() {
        let (t, cut) = truncate("short".to_string(), 100);
        assert_eq!(t, "short");
        assert!(!cut);
    }

    // -----------------------------------------------------------------------
    // The declared attachment-COUNT bound.
    //
    // `max_attachments` was the more thoroughly dead half of `MediaBounds`:
    // `max_bytes` at least had hardcoded counterparts being enforced elsewhere,
    // whereas `max_attachments` was enforced NOWHERE in the workspace — its
    // only non-definition use was inside `media::normalize_all`, which has no
    // production caller. These tests carry the same three assertions as the
    // byte-bound suite, including the one that fails against the pre-fix code.
    // -----------------------------------------------------------------------

    /// A source that declares a count bound and counts how often it is asked
    /// for bytes. The fetch counter is the instrument: it distinguishes "the
    /// attachment was skipped" from "the attachment was fetched and then
    /// relabelled", which a `transcribed` assertion alone cannot.
    struct CountingSource {
        max_attachments: usize,
        fetches: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl MediaByteSource for CountingSource {
        async fn fetch(&self, _channel: &str, _att: &Attachment) -> Result<Vec<u8>, String> {
            self.fetches
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(png_bytes())
        }
        async fn bounds(&self, _channel: &str) -> MediaBounds {
            MediaBounds {
                max_bytes: MediaBounds::DEFAULT_MAX_BYTES,
                max_attachments: self.max_attachments,
            }
        }
    }

    /// ASSERTIONS 1+2+3 in one case, because the third is a statement about the
    /// same run as the first two.
    #[tokio::test]
    async fn attachments_past_the_declared_count_bound_are_not_fetched_and_say_so() {
        let fetches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let enricher = ChannelMediaEnricher::new(
            Some(Arc::new(CapturingVisionBackend::new("a red bicycle"))),
            None,
            Arc::new(CountingSource {
                max_attachments: 2,
                fetches: Arc::clone(&fetches),
            }),
        );

        let mut atts = vec![image_att(), image_att(), image_att()];
        enricher.enrich(&mut atts, "slack").await;

        // (1) KNOWN-POSITIVE — the first two are genuinely enriched. Without
        // this the case would pass on an enricher that refused everything.
        assert_eq!(atts[0].transcribed.as_deref(), Some("a red bicycle"));
        assert_eq!(atts[1].transcribed.as_deref(), Some("a red bicycle"));

        // (2) KNOWN-NEGATIVE — the third is past the bound, is NOT analysed,
        // and the notice names both its position and the bound.
        let third = atts[2].transcribed.as_deref().unwrap_or_default();
        assert!(
            third.contains("NOT analyzed") && third.contains("declared bound of 2"),
            "the past-bound attachment must carry an honest notice naming the \
             bound, got: {third}"
        );
        assert_ne!(
            atts[2].transcribed.as_deref(),
            Some("a red bicycle"),
            "the past-bound attachment must not carry a description — that \
             would mean it was analysed after all"
        );

        // (3) THE OLD SHAPE WOULD HAVE MISSED IT. Pre-fix, `enrich` never
        // consulted any bound and fetched every attachment, so this counter
        // read 3. It is the only assertion here that the pre-fix code fails:
        // assertions (1) and (2) are about content, and a `transcribed` string
        // could in principle be produced without the fetch ever being skipped.
        assert_eq!(
            fetches.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "exactly the in-bound attachments may be fetched; a 3 here is the \
             pre-fix behaviour, where the declared count bound was enforced \
             nowhere in the workspace"
        );

        // Nothing is removed. A truncated list is a message the agent answers
        // with no idea it was incomplete.
        assert_eq!(atts.len(), 3, "the list must never be truncated");
    }

    /// The count bound is a BOUND, not a blanket refusal: a message inside it
    /// is fetched in full. This is the control that keeps the case above from
    /// passing on an enricher that simply stopped fetching.
    #[tokio::test]
    async fn a_message_within_the_declared_count_bound_is_fully_enriched() {
        let fetches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let enricher = ChannelMediaEnricher::new(
            Some(Arc::new(CapturingVisionBackend::new("a red bicycle"))),
            None,
            Arc::new(CountingSource {
                max_attachments: 5,
                fetches: Arc::clone(&fetches),
            }),
        );

        let mut atts = vec![image_att(), image_att(), image_att()];
        enricher.enrich(&mut atts, "slack").await;

        assert_eq!(
            fetches.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "all three are inside a bound of 5 and must all be fetched"
        );
        for (i, a) in atts.iter().enumerate() {
            assert_eq!(
                a.transcribed.as_deref(),
                Some("a red bicycle"),
                "attachment {i} is within the bound and must be enriched"
            );
        }
    }
}
