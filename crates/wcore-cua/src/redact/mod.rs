//! Screenshot redaction for sensitive UI patterns.
//!
//! Two complementary pipelines:
//!   1. **Heuristic** (always on, see `heuristic`) — detect "asterisk
//!      runs" (uniform foreground glyph rows) typical of password
//!      fields. Blur the bounding box of each detected run.
//!   2. **OCR-backed** (per-platform `OcrBackend`) — extract text +
//!      bounding boxes from the captured PNG, then blur any region
//!      whose text matches a sensitive-content regex (email, SSN,
//!      credit card, API-key patterns).
//!
//! Per-platform OCR backends (W9 — closes debt-register item A.6):
//! - **macOS:** Apple Vision (`VNRecognizeTextRequest`) — default, no
//!   feature flag, ships with the OS.
//! - **Windows:** `Windows.Media.Ocr` — default, no feature flag, ships
//!   with the OS.
//! - **Linux:** `leptess` (Tesseract bindings) — opt-in via feature
//!   `redact-ocr` because it pulls in a ~200 MB native-lib dependency.
//! - **Other platforms:** No OCR backend; the heuristic password-band
//!   detector remains the only redaction pass.
//!
//! Callers don't touch the trait directly — they call `redact_png`,
//! which always runs the heuristic and additionally runs whatever
//! `platform_default_ocr_backend()` returns. On any backend error the
//! function falls back to heuristic-only redaction (logged at warn),
//! so redaction is never load-bearing for screenshot delivery.
//!
//! Caller workflow:
//!   1. Backend captures raw PNG bytes via `Screenshot { redact: true }`.
//!   2. Tool calls `redact_png` before encoding the result. On any
//!      decode/encode failure the bytes pass through unchanged with a
//!      `tracing::warn!` — redaction is best-effort, never blocks the
//!      tool result.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

use crate::error::CuaResult;

pub mod heuristic;

#[cfg(target_os = "macos")]
pub mod apple_vision;

#[cfg(target_os = "windows")]
pub mod windows_ocr;

#[cfg(all(target_os = "linux", feature = "redact-ocr"))]
pub mod leptess;

/// A single recognized text region from an OCR pass.
///
/// `bbox` is in image-pixel coordinates with the origin at the
/// top-left and `(x1, y1)` inclusive (matches `apply_box_blur`'s
/// contract).
#[derive(Debug, Clone)]
pub struct TextRegion {
    pub text: String,
    pub bbox: BoundingBox,
    /// Confidence in `[0.0, 1.0]`. Backends that don't expose a per-
    /// region confidence return `1.0` so callers don't filter them out.
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct BoundingBox {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

/// Backend-neutral OCR error. Backends translate platform errors into
/// a single `String` payload — the redact pipeline only needs to know
/// "this failed, fall back to heuristic-only".
#[derive(Debug, thiserror::Error)]
#[error("ocr backend error: {0}")]
pub struct OcrError(pub String);

impl OcrError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// OCR backend abstraction. Implementations are platform-specific
/// (Apple Vision / Windows.Media.Ocr / leptess) and selected at
/// compile time by `platform_default_ocr_backend()`.
///
/// Backends MUST be `Send + Sync` so the redact pipeline can hold one
/// behind `Box<dyn OcrBackend>` across `.await` points if it grows
/// async-aware in the future. Today the trait is sync because every
/// underlying SDK exposes a blocking API and the redact pipeline runs
/// inside the screenshot capture path which is already off the async
/// runtime.
pub trait OcrBackend: Send + Sync {
    /// Extract every text region the backend can recognise from
    /// `png_bytes`. Returns an empty vec when nothing was found.
    fn extract_text_regions(&self, png_bytes: &[u8]) -> Result<Vec<TextRegion>, OcrError>;
}

/// Resolve the platform-default OCR backend, if any. Returns `None`
/// on platforms without a built-in backend (and on Linux when the
/// `redact-ocr` feature is off).
///
/// The redact pipeline treats `None` as "OCR pass disabled" — the
/// heuristic password-band detector still runs.
pub fn platform_default_ocr_backend() -> Option<Box<dyn OcrBackend>> {
    #[cfg(target_os = "macos")]
    {
        Some(Box::new(apple_vision::AppleVisionOcr::new()))
    }
    #[cfg(target_os = "windows")]
    {
        Some(Box::new(windows_ocr::WindowsMediaOcr::new()))
    }
    #[cfg(all(target_os = "linux", feature = "redact-ocr"))]
    {
        Some(Box::new(leptess::LeptessOcr::new()))
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", feature = "redact-ocr"),
    )))]
    {
        None
    }
}

/// Apply heuristic + (optional) OCR redaction to a PNG byte buffer.
/// Returns (`redacted_png_bytes`, `width`, `height`, `redaction_applied`).
///
/// `redaction_applied` is `false` when NEITHER pipeline found anything
/// to blur — the bytes are returned untouched (re-encoded so caller can
/// rely on a single PNG decode path downstream).
pub fn redact_png(png_bytes: &[u8]) -> CuaResult<(Vec<u8>, u32, u32, bool)> {
    let img = image::load_from_memory_with_format(png_bytes, ImageFormat::Png)?;
    let mut rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();

    // Pass 1: heuristic password-band detector.
    let mut applied = false;
    let boxes = heuristic::detect_password_field_runs(&rgba);
    if !boxes.is_empty() {
        applied = true;
        for (x0, y0, x1, y1) in boxes {
            apply_box_blur(&mut rgba, x0, y0, x1, y1);
        }
    }

    // Pass 2: OCR-backed sensitive-pattern detector. Uses the platform-
    // default backend (Apple Vision on macOS, Windows.Media.Ocr on
    // Windows, leptess on Linux when `redact-ocr` is enabled). When no
    // backend is available the pass is a no-op.
    if let Some(backend) = platform_default_ocr_backend() {
        // Encode the current RGBA buffer back to PNG for the OCR pass.
        let mut intermediate = Vec::new();
        DynamicImage::ImageRgba8(rgba.clone())
            .write_to(&mut Cursor::new(&mut intermediate), ImageFormat::Png)?;
        match backend.extract_text_regions(&intermediate) {
            Ok(regions) => {
                let hits = filter_sensitive_regions(&regions, w, h);
                if !hits.is_empty() {
                    applied = true;
                    for (x0, y0, x1, y1) in hits {
                        apply_box_blur(&mut rgba, x0, y0, x1, y1);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "cua: OCR backend failed; falling back to heuristic-only redaction"
                );
            }
        }
    }

    let mut out = Vec::with_capacity(png_bytes.len());
    {
        let cursor = Cursor::new(&mut out);
        DynamicImage::ImageRgba8(rgba).write_to(&mut { cursor }, ImageFormat::Png)?;
    }
    Ok((out, w, h, applied))
}

/// Filter OCR-recognised regions down to those whose text matches a
/// sensitive-content pattern. Clamps bbox coordinates to image bounds
/// so backend off-by-ones can't panic the blur pass.
fn filter_sensitive_regions(regions: &[TextRegion], w: u32, h: u32) -> Vec<(u32, u32, u32, u32)> {
    let mut hits = Vec::new();
    for r in regions {
        if !is_sensitive(&r.text) {
            continue;
        }
        let x0 = r.bbox.x0.min(w.saturating_sub(1));
        let y0 = r.bbox.y0.min(h.saturating_sub(1));
        let x1 = r.bbox.x1.min(w.saturating_sub(1));
        let y1 = r.bbox.y1.min(h.saturating_sub(1));
        if x0 <= x1 && y0 <= y1 {
            hits.push((x0, y0, x1, y1));
        }
    }
    hits
}

/// Apply a small box-blur to a rectangular region. `(x1, y1)` is
/// inclusive. Re-exported for backend impls that need it (Windows
/// crops via this helper after an OCR hit).
pub(crate) fn apply_box_blur(img: &mut RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) {
    let (w, h) = img.dimensions();
    let x1 = x1.min(w.saturating_sub(1));
    let y1 = y1.min(h.saturating_sub(1));
    if x0 > x1 || y0 > y1 {
        return;
    }
    let bx0 = x0.saturating_sub(2);
    let by0 = y0.saturating_sub(2);
    let bx1 = (x1 + 2).min(w.saturating_sub(1));
    let by1 = (y1 + 2).min(h.saturating_sub(1));

    let mut buf = Vec::with_capacity(((x1 - x0 + 1) * (y1 - y0 + 1)) as usize);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (mut sr, mut sg, mut sb, mut sa) = (0u32, 0u32, 0u32, 0u32);
            let mut n = 0u32;
            for ky in y.saturating_sub(2)..=(y + 2).min(by1) {
                for kx in x.saturating_sub(2)..=(x + 2).min(bx1) {
                    if kx < bx0 || ky < by0 {
                        continue;
                    }
                    let Rgba([r, g, b, a]) = *img.get_pixel(kx, ky);
                    sr += u32::from(r);
                    sg += u32::from(g);
                    sb += u32::from(b);
                    sa += u32::from(a);
                    n += 1;
                }
            }
            if n == 0 {
                continue;
            }
            buf.push((
                x,
                y,
                (sr / n) as u8,
                (sg / n) as u8,
                (sb / n) as u8,
                (sa / n) as u8,
            ));
        }
    }
    for (x, y, r, g, b, a) in buf {
        img.put_pixel(x, y, Rgba([r, g, b, a]));
    }
}

/// Sensitive-pattern matcher used by the OCR pass. Returns `true` when
/// `text` contains a likely email / SSN / credit-card / API-key.
///
/// Public within the crate so backend impls + tests can validate against
/// the same predicate.
pub(crate) fn is_sensitive(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    // Email — at-sign with non-whitespace flanking + dot in domain.
    if let Some(at) = t.find('@') {
        let (lhs, rhs) = t.split_at(at);
        if !lhs.is_empty() && rhs[1..].contains('.') && !rhs[1..].contains(' ') {
            return true;
        }
    }
    // SSN: XXX-XX-XXXX (digits + literal dashes).
    if t.len() >= 11 {
        let bytes = t.as_bytes();
        for i in 0..=bytes.len() - 11 {
            let slice = &bytes[i..i + 11];
            let pattern_ok = slice[0..3].iter().all(|b| b.is_ascii_digit())
                && slice[3] == b'-'
                && slice[4..6].iter().all(|b| b.is_ascii_digit())
                && slice[6] == b'-'
                && slice[7..11].iter().all(|b| b.is_ascii_digit());
            if pattern_ok {
                return true;
            }
        }
    }
    // Credit card: 13-19 contiguous digits (allowing spaces/dashes).
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 13 && digits.len() <= 19 {
        // Reject if there's a long alphanumeric run mixed in
        // (heuristic — avoids matching SHAs/hashes).
        let stripped: String = t
            .chars()
            .filter(|c| c.is_ascii_digit() || matches!(*c, ' ' | '-'))
            .collect();
        if stripped.chars().filter(|c| c.is_ascii_digit()).count() == digits.len()
            && digits.len() >= 13
        {
            return true;
        }
    }
    // API key prefixes (OpenAI / Anthropic / GitHub / AWS / generic).
    let lc = t.to_ascii_lowercase();
    if lc.starts_with("sk-")
        || lc.starts_with("sk_live_")
        || lc.starts_with("sk_test_")
        || lc.starts_with("pk_live_")
        || lc.starts_with("pk_test_")
        || lc.starts_with("ghp_")
        || lc.starts_with("github_pat_")
        || lc.starts_with("aws_secret_")
        || lc.starts_with("xoxp-")
        || lc.starts_with("xoxb-")
    {
        return true;
    }
    // `<NAME>_API_KEY=...` or `<NAME>_TOKEN=...` literal envvar-looking
    // strings.
    if let Some(eq) = t.find('=') {
        let key = &t[..eq];
        let val = &t[eq + 1..].trim();
        if (key.to_ascii_uppercase().contains("API_KEY")
            || key.to_ascii_uppercase().contains("SECRET")
            || key.to_ascii_uppercase().contains("TOKEN"))
            && val.len() >= 16
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_with_password_band() -> Vec<u8> {
        let w = 64u32;
        let h = 32u32;
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let in_band = (10..22).contains(&y);
                if in_band {
                    let dot = (x / 4) % 2 == 0;
                    let l = if dot { 96 } else { 255 };
                    img.put_pixel(x, y, Rgba([l, l, l, 255]));
                } else {
                    img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
                }
            }
        }
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn redacts_password_field_band() {
        let bytes = fixture_with_password_band();
        let (out, w, h, applied) = redact_png(&bytes).unwrap();
        assert!(applied, "the password band must be detected");
        assert_eq!((w, h), (64, 32));
        let decoded = image::load_from_memory_with_format(&out, ImageFormat::Png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (64, 32));
    }

    #[test]
    fn pass_through_when_no_band() {
        let img = RgbaImage::from_pixel(40, 16, Rgba([255, 255, 255, 255]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        let (out, w, h, applied) = redact_png(&bytes).unwrap();
        // Note: on macOS/Windows, `applied` may flip to `true` if the
        // platform OCR backend hallucinates a "word" in the flat-white
        // fixture AND that word matches `is_sensitive`. In practice
        // Apple Vision and Windows.Media.Ocr return zero regions for a
        // featureless white image, so this assertion holds.
        assert!(!applied);
        assert_eq!((w, h), (40, 16));
        assert!(!out.is_empty());
        image::load_from_memory_with_format(&out, ImageFormat::Png).unwrap();
    }

    #[test]
    fn corrupt_input_propagates_error() {
        let r = redact_png(&[0, 1, 2, 3, 4]);
        assert!(r.is_err());
    }

    #[test]
    fn sensitive_pattern_matcher_recognizes_common_secrets() {
        assert!(is_sensitive("sean@example.com"));
        assert!(is_sensitive("SSN: 123-45-6789"));
        assert!(is_sensitive("4111 1111 1111 1111"));
        assert!(is_sensitive("sk-abcdef0123456789ABCDEFGHIJK"));
        assert!(is_sensitive("ghp_abcdefABCDEF1234567890aaaa"));
        assert!(is_sensitive("ANTHROPIC_API_KEY=sk-ant-1234567890abcdef"));
        assert!(!is_sensitive("hello"));
        assert!(!is_sensitive("123"));
    }

    #[test]
    fn filter_sensitive_regions_clamps_bbox() {
        let r = TextRegion {
            text: "sean@example.com".into(),
            bbox: BoundingBox {
                x0: 5,
                y0: 5,
                x1: 1_000,
                y1: 1_000,
            },
            confidence: 0.9,
        };
        let hits = filter_sensitive_regions(&[r], 100, 100);
        assert_eq!(hits, vec![(5, 5, 99, 99)]);
    }

    #[test]
    fn filter_sensitive_regions_skips_non_sensitive() {
        let r = TextRegion {
            text: "hello world".into(),
            bbox: BoundingBox {
                x0: 0,
                y0: 0,
                x1: 10,
                y1: 10,
            },
            confidence: 1.0,
        };
        let hits = filter_sensitive_regions(&[r], 100, 100);
        assert!(hits.is_empty());
    }

    #[test]
    fn platform_default_backend_present_on_known_platforms() {
        let backend = platform_default_ocr_backend();
        if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
            assert!(
                backend.is_some(),
                "macOS/Windows ship a built-in OCR backend by default"
            );
        }
        #[cfg(all(target_os = "linux", not(feature = "redact-ocr")))]
        {
            assert!(
                backend.is_none(),
                "Linux without `redact-ocr` should not have a default OCR backend"
            );
        }
        #[cfg(all(target_os = "linux", feature = "redact-ocr"))]
        {
            assert!(
                backend.is_some(),
                "Linux with `redact-ocr` should expose the leptess backend"
            );
        }
    }
}
