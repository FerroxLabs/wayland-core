//! Media normalisation — platform attachment shapes onto the host's media
//! kinds, with a DECLARED bound and EXPLICIT degradation.
//!
//! # The rule this module exists to enforce: never drop silently
//!
//! Every path through [`normalize`] produces a [`MediaDisposition`] naming what
//! happened. An attachment that is too large, of an unsupported kind, or
//! carrying a content type the host does not classify is DEGRADED — the
//! attachment survives with a reason attached — and it is never removed from
//! the message without a record. A silent drop is indistinguishable from a
//! platform that never sent the media, and it makes an agent answer a question
//! about a picture it was never shown.
//!
//! # The bound is declared per adapter, not hardcoded here
//!
//! Platform caps differ by an order of magnitude, so [`MediaBounds`] is what an
//! adapter declares and this module enforces. [`MediaBounds::DEFAULT_MAX_BYTES`]
//! is the fallback for an adapter that declares nothing — deliberately finite,
//! because an unbounded default is how a host ends up fetching a multi-gigabyte
//! "attachment" a hostile sender pointed it at.

use serde::{Deserialize, Serialize};

use crate::event::{Attachment, MediaKind};

/// Declared intake bounds for one adapter's inbound media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaBounds {
    /// Largest attachment this adapter will normalise for fetch, in bytes.
    pub max_bytes: u64,
    /// Largest number of attachments carried on one inbound message.
    pub max_attachments: usize,
}

impl MediaBounds {
    /// Fallback cap for an adapter that declares nothing: 25 MiB. Finite by
    /// construction — an unbounded default is a fetch a hostile sender chooses
    /// the size of.
    pub const DEFAULT_MAX_BYTES: u64 = 25 * 1024 * 1024;
    /// Fallback attachment count cap.
    pub const DEFAULT_MAX_ATTACHMENTS: usize = 10;
}

impl Default for MediaBounds {
    fn default() -> Self {
        Self {
            max_bytes: Self::DEFAULT_MAX_BYTES,
            max_attachments: Self::DEFAULT_MAX_ATTACHMENTS,
        }
    }
}

/// What normalisation did to one attachment. Every variant is a record; there
/// is no "dropped" variant because dropping is not permitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MediaDisposition {
    /// Classified into a known kind and within bounds. Fetchable.
    Accepted { kind: MediaKind },
    /// Retained and visible, but NOT fetchable, for the stated reason. The
    /// attachment's URL and content type survive so a human can still act on
    /// it; only the automatic fetch is withheld.
    Degraded { kind: MediaKind, reason: String },
}

impl MediaDisposition {
    /// Whether the host may fetch this attachment's bytes.
    pub fn is_fetchable(&self) -> bool {
        matches!(self, MediaDisposition::Accepted { .. })
    }

    /// The reason, for a degraded attachment.
    pub fn reason(&self) -> Option<&str> {
        match self {
            MediaDisposition::Degraded { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

/// A platform's raw attachment description, before normalisation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawAttachment {
    /// Platform URL or reference.
    pub url: String,
    /// MIME type the platform reported, if any.
    pub content_type: Option<String>,
    /// Size the platform reported, if any. `None` means the platform did not
    /// say — which is NOT the same as small (see [`normalize`]).
    pub size_bytes: Option<u64>,
    /// Platform-supplied filename, used only to classify when there is no
    /// content type.
    pub filename: Option<String>,
}

/// Classify a MIME type onto the host's coarse media kinds.
///
/// An unrecognised type maps to [`MediaKind::Other`], which is a real kind and
/// not a failure — the failure would be discarding it.
pub fn classify(content_type: Option<&str>, filename: Option<&str>) -> MediaKind {
    if let Some(ct) = content_type {
        let ct = ct
            .split(';')
            .next()
            .unwrap_or(ct)
            .trim()
            .to_ascii_lowercase();
        if let Some(top) = ct.split('/').next() {
            match top {
                "image" => return MediaKind::Image,
                "video" => return MediaKind::Video,
                "audio" => return MediaKind::Audio,
                _ => {}
            }
        }
        if ct == "application/pdf" || ct.starts_with("text/") {
            return MediaKind::Document;
        }
        if !ct.is_empty() {
            return MediaKind::Other;
        }
    }
    // No usable content type — fall back to the extension. Platforms that
    // report neither leave this at `Other`, which is honest.
    if let Some(name) = filename {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        return match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "bmp" => MediaKind::Image,
            "mp4" | "mov" | "webm" | "mkv" | "avi" => MediaKind::Video,
            "mp3" | "m4a" | "ogg" | "oga" | "opus" | "wav" | "flac" => MediaKind::Audio,
            "pdf" | "txt" | "md" | "csv" | "doc" | "docx" => MediaKind::Document,
            _ => MediaKind::Other,
        };
    }
    MediaKind::Other
}

/// Normalise one platform attachment against `bounds`.
///
/// Returns the host-shaped [`Attachment`] and the [`MediaDisposition`] naming
/// what was decided. The attachment is ALWAYS returned — degradation withholds
/// the fetch, never the record.
///
/// An attachment whose size the platform did not report is ACCEPTED but the
/// caller is expected to enforce `bounds.max_bytes` at fetch time; an unreported
/// size is not evidence of a small file, so it is not treated as one. That is
/// stated here rather than left to a call site to remember.
pub fn normalize(raw: &RawAttachment, bounds: MediaBounds) -> (Attachment, MediaDisposition) {
    let kind = classify(raw.content_type.as_deref(), raw.filename.as_deref());
    let attachment = Attachment {
        url: raw.url.clone(),
        path: None,
        content_type: raw.content_type.clone(),
        kind,
        transcribed: None,
    };

    if raw.url.trim().is_empty() {
        return (
            attachment,
            MediaDisposition::Degraded {
                kind,
                reason: "platform supplied no media reference".to_string(),
            },
        );
    }

    if let Some(size) = raw.size_bytes
        && size > bounds.max_bytes
    {
        return (
            attachment,
            MediaDisposition::Degraded {
                kind,
                reason: format!(
                    "attachment is {size} bytes, over this adapter's declared {} byte bound",
                    bounds.max_bytes
                ),
            },
        );
    }

    (attachment, MediaDisposition::Accepted { kind })
}

/// Normalise a whole platform attachment list against `bounds`.
///
/// Attachments beyond `bounds.max_attachments` are DEGRADED with a reason
/// rather than truncated away, because a truncated list is a message the agent
/// answers with no idea it was incomplete.
pub fn normalize_all(
    raws: &[RawAttachment],
    bounds: MediaBounds,
) -> Vec<(Attachment, MediaDisposition)> {
    raws.iter()
        .enumerate()
        .map(|(i, raw)| {
            let (attachment, disposition) = normalize(raw, bounds);
            if i >= bounds.max_attachments {
                let kind = attachment.kind;
                return (
                    attachment,
                    MediaDisposition::Degraded {
                        kind,
                        reason: format!(
                            "message carried more than this adapter's declared {} attachment bound",
                            bounds.max_attachments
                        ),
                    },
                );
            }
            (attachment, disposition)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(url: &str, ct: Option<&str>, size: Option<u64>) -> RawAttachment {
        RawAttachment {
            url: url.to_string(),
            content_type: ct.map(str::to_string),
            size_bytes: size,
            filename: None,
        }
    }

    #[test]
    fn classifies_the_top_level_mime_families() {
        assert_eq!(classify(Some("image/png"), None), MediaKind::Image);
        assert_eq!(classify(Some("video/mp4"), None), MediaKind::Video);
        assert_eq!(
            classify(Some("audio/ogg; codecs=opus"), None),
            MediaKind::Audio
        );
        assert_eq!(classify(Some("application/pdf"), None), MediaKind::Document);
        assert_eq!(classify(Some("text/plain"), None), MediaKind::Document);
    }

    #[test]
    fn an_unrecognised_type_is_other_and_is_still_a_kind() {
        // `Other` is a classification, not a discard. If this ever starts
        // returning an error or an Option the caller will learn to drop it.
        assert_eq!(
            classify(Some("application/x-vendor-blob"), None),
            MediaKind::Other
        );
    }

    #[test]
    fn falls_back_to_the_extension_only_when_there_is_no_content_type() {
        assert_eq!(classify(None, Some("holiday.JPG")), MediaKind::Image);
        assert_eq!(classify(None, Some("note.pdf")), MediaKind::Document);
        assert_eq!(classify(None, Some("thing.unknownext")), MediaKind::Other);
        // A present content type WINS over a misleading extension: platforms
        // let a sender name a file anything.
        assert_eq!(
            classify(Some("image/png"), Some("invoice.pdf")),
            MediaKind::Image
        );
    }

    #[test]
    fn an_oversized_attachment_degrades_explicitly_and_is_still_returned() {
        let bounds = MediaBounds {
            max_bytes: 1024,
            max_attachments: 10,
        };
        let (att, disp) = normalize(
            &raw("https://x/y.png", Some("image/png"), Some(4096)),
            bounds,
        );
        assert!(!disp.is_fetchable(), "over the bound: not fetchable");
        assert!(
            disp.reason().is_some_and(|r| r.contains("4096")),
            "the reason must name the measurement, got {:?}",
            disp.reason()
        );
        assert_eq!(
            att.url, "https://x/y.png",
            "degradation withholds the fetch, never the record — a dropped \
             attachment is indistinguishable from one never sent"
        );
        assert_eq!(att.kind, MediaKind::Image);
    }

    #[test]
    fn an_unreported_size_is_accepted_and_is_not_treated_as_small() {
        // The dangerous reading is "no size means zero". Assert the accepted
        // path so a future change that starts inferring a size from silence
        // has to change this test on purpose.
        let bounds = MediaBounds {
            max_bytes: 1,
            max_attachments: 10,
        };
        let (_, disp) = normalize(&raw("https://x/y.png", Some("image/png"), None), bounds);
        assert!(
            disp.is_fetchable(),
            "an unreported size cannot be compared against the bound here; \
             the fetch path enforces it"
        );
    }

    #[test]
    fn an_empty_reference_degrades_rather_than_producing_a_fetchable_nothing() {
        let (_, disp) = normalize(
            &raw("", Some("image/png"), Some(10)),
            MediaBounds::default(),
        );
        assert!(!disp.is_fetchable());
        assert!(
            disp.reason()
                .is_some_and(|r| r.contains("no media reference"))
        );
    }

    #[test]
    fn attachments_past_the_count_bound_degrade_instead_of_vanishing() {
        let bounds = MediaBounds {
            max_bytes: u64::MAX,
            max_attachments: 2,
        };
        let raws: Vec<RawAttachment> = (0..5)
            .map(|i| raw(&format!("https://x/{i}.png"), Some("image/png"), Some(1)))
            .collect();
        let out = normalize_all(&raws, bounds);
        assert_eq!(out.len(), 5, "nothing is truncated away");
        assert!(out[0].1.is_fetchable());
        assert!(out[1].1.is_fetchable());
        for (i, item) in out.iter().enumerate().skip(2) {
            assert!(
                !item.1.is_fetchable(),
                "attachment {i} is past the declared bound and must be degraded"
            );
            assert!(
                item.1
                    .reason()
                    .is_some_and(|r| r.contains("attachment bound"))
            );
        }
    }

    #[test]
    fn the_default_bound_is_finite() {
        // An unbounded default is a fetch whose size a hostile sender picks.
        assert!(MediaBounds::default().max_bytes < u64::MAX);
        assert!(MediaBounds::default().max_attachments < usize::MAX);
    }
}
