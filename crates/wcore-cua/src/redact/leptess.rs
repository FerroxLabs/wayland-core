//! Linux OCR backend — Tesseract via `leptess` (Leptonica + Tesseract
//! C bindings). Gated `#[cfg(all(target_os = "linux", feature =
//! "redact-ocr"))]` because Tesseract is a ~200 MB native-lib
//! dependency that we don't want to force on every Linux build.
//!
//! Behavior preserved from the pre-W9 `redact.rs` `ocr_sensitive_regions`
//! function — same call sequence, same fallback semantics (full-image
//! blur when per-word bboxes aren't available). The only change is the
//! shape: it now implements the `OcrBackend` trait instead of being a
//! free function, and sensitive-pattern filtering moves to the caller
//! (`redact::filter_sensitive_regions`), so this backend just returns
//! every recognized word.

use std::io::Write;

use ::leptess::LepTess;

use super::{BoundingBox, OcrBackend, OcrError, TextRegion};

pub struct LeptessOcr;

impl LeptessOcr {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LeptessOcr {
    fn default() -> Self {
        Self::new()
    }
}

impl OcrBackend for LeptessOcr {
    fn extract_text_regions(&self, png_bytes: &[u8]) -> Result<Vec<TextRegion>, OcrError> {
        // Tesseract data dir is discovered via the standard
        // `TESSDATA_PREFIX` env var or the system default
        // `/usr/share/tessdata`. If neither is present `LepTess::new`
        // fails and the redact pipeline falls back to heuristic-only.
        let mut lt =
            LepTess::new(None, "eng").map_err(|e| OcrError::new(format!("LepTess::new: {e}")))?;

        // leptess takes a path; write the PNG to a tempfile that auto-
        // cleans when this function returns.
        let mut tmp = tempfile::Builder::new()
            .prefix("wcore-cua-ocr-")
            .suffix(".png")
            .tempfile()
            .map_err(|e| OcrError::new(format!("tempfile: {e}")))?;
        tmp.write_all(png_bytes)
            .map_err(|e| OcrError::new(format!("write: {e}")))?;
        tmp.flush()
            .map_err(|e| OcrError::new(format!("flush: {e}")))?;
        lt.set_image(tmp.path())
            .map_err(|e| OcrError::new(format!("set_image: {e}")))?;

        let text = lt
            .get_utf8_text()
            .map_err(|e| OcrError::new(format!("get_utf8_text: {e}")))?;
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let image_dimensions = lt.get_image_dimensions();

        let mut regions: Vec<TextRegion> = Vec::new();
        if let Some(boxes) =
            lt.get_component_boxes(::leptess::capi::TessPageIteratorLevel_RIL_WORD, true)
        {
            for bbox in &boxes {
                let geometry = bbox.get_geometry();
                if geometry.w <= 0 || geometry.h <= 0 {
                    continue;
                }
                lt.set_rectangle_from_box(&bbox);
                let Ok(word_text) = lt.get_utf8_text() else {
                    continue;
                };
                let word_text = word_text.trim().to_owned();
                if word_text.is_empty() {
                    continue;
                }
                let x0 = geometry.x.max(0) as u32;
                let y0 = geometry.y.max(0) as u32;
                let x1 = geometry.x.saturating_add(geometry.w).max(0) as u32;
                let y1 = geometry.y.saturating_add(geometry.h).max(0) as u32;
                regions.push(TextRegion {
                    text: word_text,
                    bbox: BoundingBox { x0, y0, x1, y1 },
                    confidence: (lt.mean_text_conf() as f32 / 100.0).clamp(0.0, 1.0),
                });
            }
        }

        // Fallback: if the iterator path didn't yield words (some
        // leptess versions don't surface per-word text on the iterator),
        // emit one region covering the full image with the full-page
        // text. The caller's `is_sensitive` filter decides whether to
        // blur it.
        if regions.is_empty()
            && let Some((w, h)) = image_dimensions
        {
            regions.push(TextRegion {
                text,
                bbox: BoundingBox {
                    x0: 0,
                    y0: 0,
                    x1: w.saturating_sub(1),
                    y1: h.saturating_sub(1),
                },
                confidence: 1.0,
            });
        }
        Ok(regions)
    }
}
