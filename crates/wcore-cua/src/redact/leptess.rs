//! Linux OCR backend — Tesseract via `leptess` (Leptonica + Tesseract
//! C bindings). Gated `#[cfg(all(target_os = "linux", feature =
//! "redact-ocr"))]` because Tesseract is a ~200 MB native-lib
//! dependency that we don't want to force on every Linux build.
//!
//! It returns recognized words for precise filtering and always retains the
//! full-page OCR result as conservative fallback evidence. Sensitive-pattern
//! filtering remains in the caller (`redact::filter_sensitive_regions`).

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
                let Some(region_bbox) =
                    inclusive_bbox(geometry.x, geometry.y, geometry.w, geometry.h)
                else {
                    continue;
                };
                lt.set_rectangle_from_box(&bbox);
                let Ok(word_text) = lt.get_utf8_text() else {
                    continue;
                };
                let word_text = word_text.trim().to_owned();
                if word_text.is_empty() {
                    continue;
                }
                regions.push(TextRegion {
                    text: word_text,
                    bbox: region_bbox,
                    confidence: (lt.mean_text_conf() as f32 / 100.0).clamp(0.0, 1.0),
                });
            }
        }

        preserve_full_page_evidence(regions, text, image_dimensions)
    }
}

fn inclusive_bbox(x: i32, y: i32, width: i32, height: i32) -> Option<BoundingBox> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let x1 = x.saturating_add(width.saturating_sub(1));
    let y1 = y.saturating_add(height.saturating_sub(1));
    if x1 < 0 || y1 < 0 {
        return None;
    }
    Some(BoundingBox {
        x0: x.max(0) as u32,
        y0: y.max(0) as u32,
        x1: x1 as u32,
        y1: y1 as u32,
    })
}

fn preserve_full_page_evidence(
    mut regions: Vec<TextRegion>,
    text: String,
    image_dimensions: Option<(u32, u32)>,
) -> Result<Vec<TextRegion>, OcrError> {
    let (width, height) = image_dimensions
        .filter(|(width, height)| *width > 0 && *height > 0)
        .ok_or_else(|| OcrError::new("loaded OCR image has no valid dimensions"))?;
    regions.push(TextRegion {
        text,
        bbox: BoundingBox {
            x0: 0,
            y0: 0,
            x1: width - 1,
            y1: height - 1,
        },
        confidence: 1.0,
    });
    Ok(regions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(text: &str, x0: u32, x1: u32) -> TextRegion {
        TextRegion {
            text: text.to_owned(),
            bbox: BoundingBox {
                x0,
                y0: 3,
                x1,
                y1: 8,
            },
            confidence: 0.9,
        }
    }

    #[test]
    fn full_page_evidence_survives_word_fragmentation() {
        let words = vec![
            region("4111", 1, 4),
            region("1111", 6, 9),
            region("1111", 11, 14),
            region("1111", 16, 19),
        ];

        let regions =
            preserve_full_page_evidence(words, "4111 1111 1111 1111".to_owned(), Some((24, 12)))
                .unwrap();

        assert_eq!(regions.len(), 5);
        let page = regions.last().unwrap();
        assert_eq!(page.text, "4111 1111 1111 1111");
        assert_eq!((page.bbox.x0, page.bbox.y0), (0, 0));
        assert_eq!((page.bbox.x1, page.bbox.y1), (23, 11));
        assert_eq!(
            super::super::filter_sensitive_regions(&regions, 24, 12),
            vec![(0, 0, 23, 11)]
        );
    }

    #[test]
    fn full_page_evidence_survives_partial_word_ocr() {
        let regions = preserve_full_page_evidence(
            vec![region("account", 1, 7)],
            "account 4111 1111 1111 1111".to_owned(),
            Some((40, 12)),
        )
        .unwrap();

        assert_eq!(regions.len(), 2);
        assert_eq!(regions.last().unwrap().text, "account 4111 1111 1111 1111");
        assert_eq!(
            super::super::filter_sensitive_regions(&regions, 40, 12),
            vec![(0, 0, 39, 11)]
        );
    }

    #[test]
    fn full_page_evidence_finds_sensitive_candidates_in_context() {
        let contextual_api_key =
            ["header ", "sk-", "abcdef0123456789", "ABCDEFGHIJK footer"].concat();
        for text in [
            "Account sean@example.com balance",
            contextual_api_key.as_str(),
            "Invoice 2026 card 4111 1111 1111 1111",
        ] {
            let regions =
                preserve_full_page_evidence(Vec::new(), text.to_owned(), Some((80, 24))).unwrap();

            assert_eq!(
                super::super::filter_sensitive_regions(&regions, 80, 24),
                vec![(0, 0, 79, 23)],
                "full-page OCR evidence did not redact contextual secret: {text}"
            );
        }
    }

    #[test]
    fn invalid_image_dimensions_fail_extraction() {
        assert!(preserve_full_page_evidence(Vec::new(), "secret".to_owned(), None).is_err());
        assert!(
            preserve_full_page_evidence(Vec::new(), "secret".to_owned(), Some((0, 8))).is_err()
        );
    }

    #[test]
    fn leptonica_geometry_maps_to_inclusive_bbox() {
        let bbox = inclusive_bbox(10, 20, 3, 4).unwrap();
        assert_eq!((bbox.x0, bbox.y0), (10, 20));
        assert_eq!((bbox.x1, bbox.y1), (12, 23));
    }

    #[test]
    fn invalid_or_off_image_geometry_is_rejected_or_clipped() {
        assert!(inclusive_bbox(0, 0, 0, 4).is_none());
        assert!(inclusive_bbox(-5, 0, 3, 4).is_none());
        let clipped = inclusive_bbox(-2, -1, 4, 3).unwrap();
        assert_eq!((clipped.x0, clipped.y0), (0, 0));
        assert_eq!((clipped.x1, clipped.y1), (1, 1));
    }
}
