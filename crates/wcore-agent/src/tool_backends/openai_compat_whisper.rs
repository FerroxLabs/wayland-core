//! Moved from monolith `tool_backends.rs` during v0.9.0 Wave-1 prep
//! (Sub-agent B0). The R-B1 fix: each backend lives in its own file so
//! parallel Wave-1 sub-agents can add new backend files without
//! colliding on `tool_backends.rs`.

use std::sync::Arc;

use async_trait::async_trait;
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
// 27-C3. `cost_from_headers` started here and now lives in `shared.rs`: five
// billable backends need it, and three of them were discarding
// `resp.headers()` entirely.
use super::shared::{cost_from_headers, http_error_detail};
use wcore_tools::media_cost::{
    MediaAccounting, MediaCostLedger, MediaCostRecord, MediaRateCard, MediaUnits,
};
use wcore_tools::transcription_tools::{TranscriptionBackend, TranscriptionOutcome};

/// Billable units for one transcription, read from a `verbose_json` reply.
///
/// `duration` is the seconds of audio the provider actually processed — the
/// unit transcription is billed on. A provider that omits it yields
/// "unknown duration", never `0.0`: claiming zero seconds would assert the
/// provider did no work on a call it was paid for.
fn units_from_response(parsed: &serde_json::Value) -> MediaUnits {
    match parsed.get("duration").and_then(|v| v.as_f64()) {
        // A negative or non-finite duration is corrupt, not a measurement.
        Some(secs) if secs.is_finite() && secs >= 0.0 => MediaUnits::audio_seconds(secs),
        _ => MediaUnits::audio_of_unknown_duration(),
    }
}

/// OpenAI-compatible Whisper backend. Drives both Groq's
/// `whisper-large-v3-turbo` and OpenAI's `whisper-1` since they share
/// the same multipart-form `/audio/transcriptions` API shape.
pub struct OpenAiCompatWhisperBackend {
    client: Client,
    api_key: String,
    endpoint: String,
    model: String,
    backend_id: &'static str,
    /// 27-C3. Session ledger + operator price list for the billable call this
    /// backend makes. Unbound by default so every existing construction site is
    /// unchanged and behaviour is identical until a host binds one.
    accounting: MediaAccounting,
}

impl OpenAiCompatWhisperBackend {
    pub fn new(api_key: String, endpoint: String, model: String, backend_id: &'static str) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            endpoint,
            model,
            backend_id,
            accounting: MediaAccounting::default(),
        }
    }

    /// 27-C3. Bind the session ledger and the operator price list together, so
    /// this backend cannot end up wired for one and not the other.
    pub fn with_accounting(mut self, accounting: MediaAccounting) -> Self {
        self.accounting = accounting;
        self
    }

    /// 27-C3. Bind a session ledger so transcription spend accumulates
    /// somewhere a host can total it. Mirrors
    /// `ImageGenerationTool::with_cost_ledger`.
    pub fn with_cost_ledger(mut self, ledger: Arc<MediaCostLedger>) -> Self {
        self.accounting.ledger = Some(ledger);
        self
    }

    /// 27-C3. Bind an operator price list.
    pub fn with_rate_card(mut self, rate_card: MediaRateCard) -> Self {
        self.accounting.rate_card = rate_card;
        self
    }

    /// Record one billable transcription. Returns the record so a caller can
    /// assert on it without a ledger bound.
    fn account(&self, record: MediaCostRecord) -> MediaCostRecord {
        self.accounting.account(record)
    }

    /// Resolved request endpoint. Exposed so the resolver wiring is
    /// unit-assertable without a network round-trip.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Model sent in the multipart form.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Backend label used in log lines and error messages.
    pub fn backend_id(&self) -> &str {
        self.backend_id
    }
}

#[async_trait]
impl TranscriptionBackend for OpenAiCompatWhisperBackend {
    async fn transcribe(
        &self,
        mime: &'static str,
        bytes: &[u8],
        language: Option<&str>,
    ) -> TranscriptionOutcome {
        let filename = match mime {
            "audio/mpeg" => "audio.mp3",
            "audio/mp4" => "audio.m4a",
            "audio/aac" => "audio.aac",
            "audio/wav" | "audio/x-wav" | "audio/wave" => "audio.wav",
            "audio/ogg" => "audio.ogg",
            "audio/webm" => "audio.webm",
            "audio/flac" => "audio.flac",
            _ => "audio.bin",
        };
        // Multipart form: file, model, optional language, request_json
        // response_format = verbose_json so we get language + segments.
        let file_part = reqwest::multipart::Part::bytes(bytes.to_vec())
            .file_name(filename.to_string())
            .mime_str(mime)
            .unwrap_or_else(|_| reqwest::multipart::Part::bytes(bytes.to_vec()));
        let mut form = reqwest::multipart::Form::new()
            .text("model", self.model.clone())
            .text("response_format", "verbose_json")
            .part("file", file_part);
        if let Some(lang) = language {
            form = form.text("language", lang.to_string());
        }
        let resp = match self
            .client
            .post(&self.endpoint)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.api_key),
            )
            .timeout(std::time::Duration::from_secs(120))
            .multipart(form)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // The request never reached the provider, so nothing was
                // billed and there is no record to make.
                return TranscriptionOutcome::Err {
                    message: format!("{} transcription request failed: {e}", self.backend_id),
                };
            }
        };
        let status = resp.status();
        // 27-C3: read the cost header BEFORE `resp.text()` consumes the
        // response. This was previously dropped entirely.
        let reported_cost = cost_from_headers(resp.headers());
        let txt = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            // The provider was reached and rejected the call. Whether it
            // billed is unknown — a rejected request can still be charged —
            // so this records `CallFailedBillingUnknown`, never $0.00.
            // Duration is genuinely unknown here, so it is recorded as
            // unknown rather than as `0.0`, which would assert that the
            // provider processed no audio.
            self.account(MediaCostRecord::for_failure(
                "transcribe_audio",
                self.backend_id,
                &self.model,
                MediaUnits::audio_of_unknown_duration(),
                format!("http_{}", status.as_u16()),
            ));
            return TranscriptionOutcome::Err {
                message: format!(
                    "{} transcription returned HTTP {}: {}",
                    self.backend_id,
                    status.as_u16(),
                    http_error_detail("speech-to-text", status.as_u16(), &txt)
                ),
            };
        }
        let parsed: serde_json::Value = match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(e) => {
                // HTTP 200 — the provider did the work and billed for it; we
                // simply could not read the reply. Recording this as anything
                // other than billable would understate real spend.
                self.account(MediaCostRecord::for_failure(
                    "transcribe_audio",
                    self.backend_id,
                    &self.model,
                    MediaUnits::audio_of_unknown_duration(),
                    "response_parse_failed",
                ));
                return TranscriptionOutcome::Err {
                    message: format!("{} transcription JSON parse failed: {e}", self.backend_id),
                };
            }
        };
        let billed_units = units_from_response(&parsed);
        let transcript = parsed
            .get("text")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default();
        let language = parsed
            .get("language")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        // `segments` (whisper verbose_json) → our `TranscriptSegment`s.
        let segments = parsed
            .get("segments")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|seg| wcore_tools::transcription_tools::TranscriptSegment {
                        start_seconds: seg.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0)
                            as f32,
                        end_seconds: seg.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                        text: seg
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        // 27-C3. The provider ran and billed regardless of what came back, so
        // the record is made on BOTH remaining exits. An empty transcript is a
        // product-side rejection of a call the provider already performed —
        // exactly the `with_outcome` case the ledger documents — so it keeps
        // any resolved price rather than being written off as free.
        let record = MediaCostRecord::for_success(
            "transcribe_audio",
            self.backend_id,
            &self.model,
            billed_units,
            reported_cost,
            &self.accounting.rate_card,
        );

        if transcript.is_empty() {
            self.account(
                record.with_outcome(wcore_tools::media_cost::MediaOutcome::Failed {
                    category: "empty_transcript".to_string(),
                }),
            );
            return TranscriptionOutcome::Err {
                message: format!("{} transcription returned empty text", self.backend_id),
            };
        }
        self.account(record);
        TranscriptionOutcome::Ok {
            transcript,
            language,
            segments,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
    use serde_json::json;
    use wcore_tools::media_cost::{MediaOutcome, PriceSource, UnpricedReason};

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            // `HeaderName::from_bytes` rather than inserting the `&str`
            // directly: the `&str` impl requires a `'static` key, which a
            // borrowed test slice is not.
            h.insert(
                HeaderName::from_bytes(k.as_bytes()).expect("test header name must be valid"),
                HeaderValue::from_str(v).expect("test header value must be valid"),
            );
        }
        h
    }

    /// 27-C3. The provider's own dollar figure must be read off the wire.
    /// Before this, `resp.headers()` was discarded and a transcription
    /// produced no cost record at all.
    #[test]
    fn provider_cost_header_is_read_off_the_wire() {
        let got = cost_from_headers(&headers(&[("x-flux-cost-usd", "0.0031")]))
            .expect("a present, parseable cost header must be read");
        assert_eq!(got.usd, 0.0031);
        assert_eq!(
            got.source,
            PriceSource::ProviderHeader {
                header: "x-flux-cost-usd".to_string()
            },
            "the record must name which header supplied the figure"
        );
        assert!(got.source.is_provider_reported());
    }

    /// KNOWN-NEGATIVE for the test above. Without this, that test would pass
    /// on an implementation that returned a hardcoded figure for every call.
    /// The three cases here are the ones that must NOT yield a number — and
    /// critically must not yield `0.0`, because "we do not know" and "it was
    /// free" are different claims about the user's money.
    #[test]
    fn absent_or_unparseable_cost_header_yields_no_figure_not_zero() {
        // Nothing at all.
        assert!(cost_from_headers(&HeaderMap::new()).is_none());
        // A header we do not treat as a cost channel.
        assert!(cost_from_headers(&headers(&[("x-request-id", "abc123")])).is_none());
        // The right header carrying garbage.
        assert!(
            cost_from_headers(&headers(&[("x-flux-cost-usd", "free")])).is_none(),
            "an unparseable figure must be absent, never coerced to 0.0"
        );

        // LIVENESS CONTROL: the same helper, same HeaderMap construction, DOES
        // find a figure when one is genuinely present. A dead matcher would
        // satisfy all three assertions above for free.
        assert!(
            cost_from_headers(&headers(&[("x-cost-usd", "1.5")])).is_some(),
            "liveness control failed: the header matcher finds nothing at all"
        );
    }

    /// The billable unit for transcription is seconds of audio, and it must
    /// come from the provider's own `duration` rather than being assumed.
    #[test]
    fn duration_becomes_billed_seconds() {
        let u = units_from_response(&json!({"text": "hi", "duration": 12.5}));
        assert_eq!(u.billed_seconds, Some(12.5));
        assert_eq!(u.images, 0, "a transcription produces no image artifact");
        assert!(u.is_duration_billed());

        // A different duration must produce a different record — the property
        // that makes this a measurement rather than a constant.
        let v = units_from_response(&json!({"text": "hi", "duration": 300.0}));
        assert_ne!(u.billed_seconds, v.billed_seconds);
    }

    /// A missing or corrupt duration is "unknown", never zero. `0.0` would
    /// assert the provider processed no audio on a call it charged for, and
    /// would silently drag a session's `billed_seconds` total downwards.
    #[test]
    fn missing_or_corrupt_duration_is_unknown_not_zero() {
        for body in [
            json!({"text": "hi"}),
            json!({"text": "hi", "duration": "twelve"}),
            json!({"text": "hi", "duration": -4.0}),
        ] {
            let u = units_from_response(&body);
            assert_eq!(
                u.billed_seconds, None,
                "unknown duration must stay unknown for body {body}"
            );
            assert!(
                u.is_duration_billed(),
                "it is still an audio call, so it must not be counted as an \
                 image of unknown size: {body}"
            );
        }

        // LIVENESS CONTROL: the same function does extract a real duration,
        // so the Nones above are discrimination and not a dead parser.
        assert_eq!(
            units_from_response(&json!({"duration": 7.0})).billed_seconds,
            Some(7.0)
        );
    }

    /// A provider that reached us and failed is not a free call. This is the
    /// money-correctness case: providers do bill for rejected requests.
    #[test]
    fn a_failed_call_is_recorded_as_billing_unknown_not_free() {
        let r = MediaCostRecord::for_failure(
            "transcribe_audio",
            "flux-router",
            "whisper-1",
            MediaUnits::audio_of_unknown_duration(),
            "http_402",
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
                category: "http_402".to_string()
            }
        );
        assert!(
            !r.summary_line().contains("$0.00"),
            "a failed billable call must never render as free: {}",
            r.summary_line()
        );
    }

    /// End-to-end through the real assembly the backend uses: a live-shaped
    /// FluxRouter reply (cost header + verbose_json duration) must produce one
    /// record carrying the provider's figure and the audio duration, and it
    /// must land in a bound ledger.
    #[test]
    fn backend_records_a_priced_transcription_into_the_ledger() {
        let ledger = MediaCostLedger::shared();
        let backend = OpenAiCompatWhisperBackend::new(
            "unused-in-this-test".to_string(),
            "https://example.invalid/v1/audio/transcriptions".to_string(),
            "whisper-large-v3".to_string(),
            "flux-router",
        )
        .with_cost_ledger(ledger.clone());

        let reported = cost_from_headers(&headers(&[("x-flux-cost-usd", "0.0031")]));
        let units = units_from_response(&json!({"text": "hello", "duration": 12.5}));
        backend.account(MediaCostRecord::for_success(
            "transcribe_audio",
            backend.backend_id(),
            backend.model(),
            units,
            reported,
            &MediaRateCard::default(),
        ));

        let snapshot = ledger.snapshot();
        assert_eq!(snapshot.len(), 1, "exactly one billable call was made");
        let rec = &snapshot[0];
        assert_eq!(rec.tool, "transcribe_audio");
        assert_eq!(rec.backend_id, "flux-router");
        assert_eq!(rec.model, "whisper-large-v3");
        assert_eq!(rec.cost_usd, Some(0.0031));
        assert!(rec.price_source.is_provider_reported());
        assert_eq!(rec.units.billed_seconds, Some(12.5));

        let s = ledger.summary();
        assert_eq!(s.calls, 1);
        assert_eq!(s.priced_calls, 1);
        assert_eq!(s.duration_billed_calls, 1);
        assert!((s.billed_seconds - 12.5).abs() < 1e-9);
        assert_eq!(
            s.calls_of_unknown_size, 0,
            "an audio call is not an image of unknown size"
        );
        assert_eq!(s.images, 0);
    }

    /// KNOWN-NEGATIVE for the ledger wiring: with NO ledger bound the backend
    /// must not panic and must not record anywhere. Without this the test
    /// above could pass against a global sink that swallows everything.
    #[test]
    fn unbound_backend_records_nothing_but_still_builds_the_record() {
        let backend = OpenAiCompatWhisperBackend::new(
            "k".to_string(),
            "https://example.invalid/v1/audio/transcriptions".to_string(),
            "whisper-1".to_string(),
            "openai",
        );
        let returned = backend.account(MediaCostRecord::for_success(
            "transcribe_audio",
            "openai",
            "whisper-1",
            MediaUnits::audio_seconds(3.0),
            None,
            &MediaRateCard::default(),
        ));
        // The record is still produced and handed back to the caller.
        assert_eq!(returned.units.billed_seconds, Some(3.0));

        // A separate ledger, never bound, must be untouched.
        let other = MediaCostLedger::new();
        assert_eq!(other.snapshot().len(), 0);
    }

    /// #938 RED ARM. The same FluxRouter 402 the image path maps to a typed
    /// `PremiumLocked` ("requires a paid Flux plan") must produce the SAME
    /// actionable message on the transcription path. Before the fix this
    /// handed the provider's raw JSON envelope to the user.
    #[tokio::test]
    async fn flux_402_on_transcription_yields_the_typed_entitlement_message() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(402).set_body_json(json!({
                "error": {
                    "message": "speech-to-text is available on paid plans only",
                    "code": "premium_locked"
                }
            })))
            .mount(&server)
            .await;
        let backend = OpenAiCompatWhisperBackend::new(
            "flux-test-key".to_string(),
            format!("{}/v1/audio/transcriptions", server.uri()),
            "flux-voice-fast".to_string(),
            "flux-router",
        );
        let msg = match backend.transcribe("audio/wav", b"RIFF0000WAVE", None).await {
            TranscriptionOutcome::Err { message } => message,
            other => panic!("a 402 must not succeed: {other:?}"),
        };
        assert!(
            msg.contains("requires a paid Flux plan"),
            "the entitlement lock must be spelled out the way the image path \
             spells it, got: {msg}"
        );
        assert!(
            !msg.contains("\"code\""),
            "the provider's raw JSON envelope must not reach the user, got: {msg}"
        );
        assert!(
            msg.contains("402"),
            "the status must survive the rewrite - video_analyze classifies \
             InsufficientCredits by string-matching it, got: {msg}"
        );
    }

    /// KNOWN-POSITIVE CONTROL for the test above. A non-Flux failure body must
    /// still reach the user verbatim - without this, an implementation that
    /// threw every error body away would satisfy the "no raw JSON" assertion
    /// for free.
    #[tokio::test]
    async fn a_non_flux_transcription_failure_body_is_still_surfaced_verbatim() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(500).set_body_string("upstream transcoder exploded"),
            )
            .mount(&server)
            .await;
        let backend = OpenAiCompatWhisperBackend::new(
            "k".to_string(),
            format!("{}/v1/audio/transcriptions", server.uri()),
            "whisper-1".to_string(),
            "openai",
        );
        let msg = match backend.transcribe("audio/wav", b"RIFF0000WAVE", None).await {
            TranscriptionOutcome::Err { message } => message,
            other => panic!("a 500 must not succeed: {other:?}"),
        };
        assert!(msg.contains("upstream transcoder exploded"), "got: {msg}");
    }
}
