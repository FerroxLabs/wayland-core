//! Windows OCR backend — `Windows.Media.Ocr` (WinRT).
//!
//! Ships as the default Windows OCR backend with no feature flag
//! because the OCR engine is part of the OS (Win10+, requires the
//! language pack — we use `TryCreateFromUserProfileLanguages`, which
//! gracefully returns `None` when no recognisable language is
//! installed).
//!
//! Pipeline:
//!   1. Wrap PNG bytes in an `InMemoryRandomAccessStream` via
//!      `DataWriter::WriteBytes` + `StoreAsync`.
//!   2. `BitmapDecoder::CreateAsync` → `GetSoftwareBitmapAsync` to
//!      produce a `SoftwareBitmap` the engine can consume.
//!   3. `OcrEngine::RecognizeAsync(&bitmap)` → blocking `.get()` on
//!      the returned `IAsyncOperation`.
//!   4. Iterate `Lines` × `Words`, emit one `TextRegion` per word.
//!
//! Coordinate system note: `OcrWord::BoundingRect` returns a
//! `Foundation::Rect` already in **pixel coords with top-left origin**
//! — no flip needed, only float→u32 rounding (`floor`/`ceil`) to
//! match the redact pipeline's inclusive integer-bbox contract.

use windows::Graphics::Imaging::BitmapDecoder;
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

use super::{BoundingBox, OcrBackend, OcrError, TextRegion};

/// Windows.Media.Ocr backend. The expensive `OcrEngine` is created
/// lazily inside `extract_text_regions`, not cached on the struct —
/// `Box<dyn OcrBackend>` must stay `Send + Sync` and `OcrEngine` is a
/// COM object whose Send/Sync story is opaque to us.
pub struct WindowsMediaOcr;

impl WindowsMediaOcr {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WindowsMediaOcr {
    fn default() -> Self {
        Self::new()
    }
}

impl OcrBackend for WindowsMediaOcr {
    fn extract_text_regions(&self, png_bytes: &[u8]) -> Result<Vec<TextRegion>, OcrError> {
        // Empty buffer guard — the WinRT calls would fail later, but
        // surfacing the early bail-out makes the error message clearer.
        if png_bytes.is_empty() {
            return Ok(Vec::new());
        }

        // Step 1: PNG bytes -> InMemoryRandomAccessStream.
        let stream = InMemoryRandomAccessStream::new()
            .map_err(|e| OcrError::new(format!("windows_ocr: InMemoryRandomAccessStream: {e}")))?;
        let writer = DataWriter::CreateDataWriter(&stream)
            .map_err(|e| OcrError::new(format!("windows_ocr: CreateDataWriter: {e}")))?;
        writer
            .WriteBytes(png_bytes)
            .map_err(|e| OcrError::new(format!("windows_ocr: WriteBytes: {e}")))?;
        writer
            .StoreAsync()
            .map_err(|e| OcrError::new(format!("windows_ocr: StoreAsync: {e}")))?
            .get()
            .map_err(|e| OcrError::new(format!("windows_ocr: StoreAsync await: {e}")))?;
        // `DetachStream` releases the DataWriter's lock on the stream
        // so the BitmapDecoder can seek to position 0 and read.
        let _ = writer
            .DetachStream()
            .map_err(|e| OcrError::new(format!("windows_ocr: DetachStream: {e}")))?;
        stream
            .Seek(0)
            .map_err(|e| OcrError::new(format!("windows_ocr: Seek: {e}")))?;

        // Step 2: stream -> BitmapDecoder -> SoftwareBitmap.
        let decoder = BitmapDecoder::CreateAsync(&stream)
            .map_err(|e| OcrError::new(format!("windows_ocr: BitmapDecoder::CreateAsync: {e}")))?
            .get()
            .map_err(|e| OcrError::new(format!("windows_ocr: BitmapDecoder await: {e}")))?;
        let bitmap = decoder
            .GetSoftwareBitmapAsync()
            .map_err(|e| OcrError::new(format!("windows_ocr: GetSoftwareBitmapAsync: {e}")))?
            .get()
            .map_err(|e| OcrError::new(format!("windows_ocr: SoftwareBitmap await: {e}")))?;

        // Step 3: build the OCR engine from the user-profile languages.
        // Returns Ok(null-equivalent) on systems with no recognisable
        // language — handled below as "no OCR available, no regions".
        let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| {
            OcrError::new(format!(
                "windows_ocr: TryCreateFromUserProfileLanguages: {e}"
            ))
        })?;

        let result = engine
            .RecognizeAsync(&bitmap)
            .map_err(|e| OcrError::new(format!("windows_ocr: RecognizeAsync: {e}")))?
            .get()
            .map_err(|e| OcrError::new(format!("windows_ocr: Recognize await: {e}")))?;

        // Step 4: walk Lines × Words. WinRT `IVectorView::Size` +
        // `GetAt` is the iteration shape exposed by the windows crate.
        let lines = result
            .Lines()
            .map_err(|e| OcrError::new(format!("windows_ocr: Lines: {e}")))?;
        let line_count = lines
            .Size()
            .map_err(|e| OcrError::new(format!("windows_ocr: Lines.Size: {e}")))?;
        let mut out: Vec<TextRegion> = Vec::new();
        for i in 0..line_count {
            let line = lines
                .GetAt(i)
                .map_err(|e| OcrError::new(format!("windows_ocr: Lines.GetAt({i}): {e}")))?;
            let words = line
                .Words()
                .map_err(|e| OcrError::new(format!("windows_ocr: line.Words: {e}")))?;
            let word_count = words
                .Size()
                .map_err(|e| OcrError::new(format!("windows_ocr: Words.Size: {e}")))?;
            for j in 0..word_count {
                let word = words
                    .GetAt(j)
                    .map_err(|e| OcrError::new(format!("windows_ocr: Words.GetAt({j}): {e}")))?;
                let rect = word
                    .BoundingRect()
                    .map_err(|e| OcrError::new(format!("windows_ocr: word.BoundingRect: {e}")))?;
                let text = word
                    .Text()
                    .map_err(|e| OcrError::new(format!("windows_ocr: word.Text: {e}")))?
                    .to_string_lossy();
                let (x0, y0, x1, y1) = rect_to_inclusive_bbox(rect);
                out.push(TextRegion {
                    text,
                    bbox: BoundingBox { x0, y0, x1, y1 },
                    // Windows.Media.Ocr doesn't expose per-word
                    // confidence; emit 1.0 so the redact pipeline
                    // doesn't filter the region out.
                    confidence: 1.0,
                });
            }
        }
        Ok(out)
    }
}

/// Convert a `Foundation::Rect` (float top-left origin pixel coords +
/// width/height) into the redact pipeline's inclusive integer bbox.
/// Negative dims are clamped to zero; floor on the upper-left and ceil
/// on the lower-right widen the box by at most one pixel to be safe.
fn rect_to_inclusive_bbox(rect: windows::Foundation::Rect) -> (u32, u32, u32, u32) {
    let x0 = rect.X.max(0.0).floor() as u32;
    let y0 = rect.Y.max(0.0).floor() as u32;
    let x1 = (rect.X + rect.Width).max(0.0).ceil() as u32;
    let y1 = (rect.Y + rect.Height).max(0.0).ceil() as u32;
    // Defensive: ensure x1 >= x0 (a malformed rect shouldn't crash the
    // blur pass). saturating_sub(1) keeps the result a valid inclusive
    // bbox when the source has width/height == 0.
    let x1 = x1.max(x0);
    let y1 = y1.max(y0);
    (x0, y0, x1, y1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_constructs() {
        let _b = WindowsMediaOcr::new();
    }

    #[test]
    fn rect_to_inclusive_bbox_floors_and_ceils() {
        let r = windows::Foundation::Rect {
            X: 10.4,
            Y: 20.7,
            Width: 30.2,
            Height: 40.9,
        };
        let (x0, y0, x1, y1) = rect_to_inclusive_bbox(r);
        assert_eq!(x0, 10);
        assert_eq!(y0, 20);
        assert_eq!(x1, 41); // 10.4 + 30.2 = 40.6, ceil -> 41
        assert_eq!(y1, 62); // 20.7 + 40.9 = 61.6, ceil -> 62
    }

    #[test]
    fn rect_to_inclusive_bbox_clamps_negative_origin() {
        let r = windows::Foundation::Rect {
            X: -1.0,
            Y: -1.0,
            Width: 10.0,
            Height: 5.0,
        };
        let (x0, y0, x1, y1) = rect_to_inclusive_bbox(r);
        assert_eq!((x0, y0), (0, 0));
        assert_eq!(x1, 9); // (-1.0 + 10.0).max(0) = 9
        assert_eq!(y1, 4);
    }
}
