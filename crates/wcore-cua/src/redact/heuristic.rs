//! Heuristic password-field detector.
//!
//! No OCR — scans for horizontal rows whose luminance histogram is
//! dominated by a single mid-band value, which is the signature of
//! "asterisk runs" in password fields. The output is a list of
//! `(x0, y0, x1, y1)` bounding boxes spanning the full image width;
//! the redact pipeline blurs each box.
//!
//! Intentionally cautious: favors false negatives over false
//! positives because the heuristic is the always-on baseline — over-
//! redacting normal UI would be worse than missing a password field
//! that the per-platform OCR pass also catches.

use image::{Rgba, RgbaImage};

/// Heuristic scan for password-field-like horizontal runs. Returns the
/// bounding boxes `(x0, y0, x1, y1)` (inclusive). The current heuristic
/// is intentionally cautious — it favors false negatives over false
/// positives.
pub fn detect_password_field_runs(img: &RgbaImage) -> Vec<(u32, u32, u32, u32)> {
    let (w, h) = img.dimensions();
    let mut runs = Vec::new();
    if w < 40 || h < 12 {
        return runs;
    }

    let min_band_height = 8u32;
    let mut band_start: Option<u32> = None;

    for y in 0..h {
        if row_looks_like_password_field(img, y) {
            band_start.get_or_insert(y);
        } else if let Some(start) = band_start.take()
            && y - start >= min_band_height
        {
            runs.push((0, start, w.saturating_sub(1), y.saturating_sub(1)));
        }
    }
    if let Some(start) = band_start
        && h - start >= min_band_height
    {
        runs.push((0, start, w.saturating_sub(1), h.saturating_sub(1)));
    }
    runs
}

fn row_looks_like_password_field(img: &RgbaImage, y: u32) -> bool {
    let (w, _) = img.dimensions();
    if w == 0 {
        return false;
    }
    let mut hist = [0u32; 256];
    let mut total = 0u32;
    for x in 0..w {
        let Rgba([r, g, b, _]) = *img.get_pixel(x, y);
        let l = ((u32::from(r) + u32::from(g) + u32::from(b)) / 3) as usize;
        hist[l] += 1;
        total += 1;
    }
    if total == 0 {
        return false;
    }
    let mut best_count = 0u32;
    for (i, &c) in hist.iter().enumerate() {
        if !(16..=240).contains(&i) {
            continue;
        }
        if c > best_count {
            best_count = c;
        }
    }
    best_count * 100 / total >= 30
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_a_band_in_synthetic_password_fixture() {
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
        let runs = detect_password_field_runs(&img);
        assert!(!runs.is_empty(), "expected at least one detected band");
    }

    #[test]
    fn returns_empty_for_blank_image() {
        let img = RgbaImage::from_pixel(64, 32, Rgba([255, 255, 255, 255]));
        let runs = detect_password_field_runs(&img);
        assert!(runs.is_empty());
    }

    #[test]
    fn rejects_undersized_images() {
        let img = RgbaImage::from_pixel(10, 10, Rgba([128, 128, 128, 255]));
        let runs = detect_password_field_runs(&img);
        assert!(runs.is_empty());
    }
}
